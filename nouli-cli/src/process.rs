//! Bounded local subprocesses: nonblocking pipes, one deadline, and group cleanup.
use nix::{
    fcntl::{FcntlArg, OFlag, fcntl},
    sys::{
        resource::{Resource, setrlimit},
        signal::{Signal, killpg},
    },
    unistd::Pid,
};
use std::{
    io::{self, Read, Write},
    os::{fd::AsFd, unix::process::CommandExt},
    process::{Child, ChildStdin, ChildStdout, Command, Stdio},
    thread,
    time::{Duration, Instant},
};
use thiserror::Error;
use tokio_util::sync::CancellationToken;

pub(crate) const MEMORY: u64 = 512 * 1024 * 1024;
const POLL: Duration = Duration::from_millis(10);

/// Parser limits also apply to direct invocations of the private worker.
#[derive(Clone, Copy)]
pub(crate) enum Limits {
    Parser,
    Office { max_file_bytes: u64 },
}
impl Limits {
    pub(crate) fn apply(self) -> io::Result<()> {
        for (kind, limit) in [(Resource::RLIMIT_CORE, 0), (Resource::RLIMIT_CPU, 8)] {
            setrlimit(kind, limit, limit).map_err(io::Error::from)?;
        }
        if let Self::Office { max_file_bytes } = self {
            setrlimit(Resource::RLIMIT_FSIZE, max_file_bytes, max_file_bytes)
                .map_err(io::Error::from)?;
        }
        #[cfg(not(target_os = "macos"))]
        {
            setrlimit(Resource::RLIMIT_DATA, MEMORY, MEMORY).map_err(io::Error::from)?;
            setrlimit(Resource::RLIMIT_AS, MEMORY, MEMORY).map_err(io::Error::from)?;
        }
        Ok(())
    }

    fn over_memory(self, pid: u32, remaining: Duration) -> bool {
        match self {
            Self::Parser => over_memory(pid),
            Self::Office { .. } => group_over_memory(pid, remaining),
        }
    }
}

#[derive(Debug, Error)]
pub(crate) enum RunError {
    #[error("cannot start subprocess")]
    Spawn(#[source] io::Error),
    #[error("subprocess pipe failed")]
    Io(#[from] io::Error),
    #[error("subprocess exited unsuccessfully")]
    Exit,
    #[error("subprocess timed out")]
    Timeout,
    #[error("subprocess canceled")]
    Cancelled,
    #[error("subprocess output exceeded its limit")]
    OutputLimit,
    #[error("subprocess exceeded its resource limit")]
    ResourceLimit,
}

/// The caller selects piped stdout to capture it, or null stdout for converters
/// that publish a bounded file instead. Input is finite caller-owned bytes;
/// stdin, stdout, and the child must all finish within the same deadline.
pub(crate) fn run_bounded(
    command: &mut Command,
    input: Option<&[u8]>,
    timeout: Duration,
    max_output: u64,
    cancel: Option<&CancellationToken>,
    limits: Option<Limits>,
) -> Result<Vec<u8>, RunError> {
    if cancel.is_some_and(CancellationToken::is_cancelled) {
        return Err(RunError::Cancelled);
    }
    let start = Instant::now();
    command
        .process_group(0)
        .stderr(Stdio::null())
        .stdin(if input.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        });
    if let Some(limits) = limits {
        // SAFETY: only async-signal-safe setrlimit calls run between fork/exec.
        unsafe {
            command.pre_exec(move || limits.apply());
        }
    }
    let child = command.spawn().map_err(RunError::Spawn)?;
    let mut group = ChildGroup::new(child)?;
    let result = drive(
        &mut group.child,
        input,
        start,
        timeout,
        max_output,
        cancel,
        limits,
    );
    if result.is_ok() {
        // LibreOffice launchers may finish before their helpers. Preserve the
        // previous converter cleanup even when conversion succeeds.
        if matches!(limits, Some(Limits::Office { .. })) {
            group.stop();
        }
        group.armed = false;
    }
    result
}

struct ChildGroup {
    child: Child,
    group: Pid,
    armed: bool,
}
impl ChildGroup {
    fn new(mut child: Child) -> io::Result<Self> {
        let Ok(pid) = i32::try_from(child.id()) else {
            let _ = child.kill();
            let _ = child.wait();
            return Err(io::Error::other("subprocess identifier is out of range"));
        };
        Ok(Self {
            child,
            group: Pid::from_raw(pid),
            armed: true,
        })
    }
    fn stop(&mut self) {
        let _ = killpg(self.group, Signal::SIGKILL);
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
impl Drop for ChildGroup {
    fn drop(&mut self) {
        if self.armed {
            self.stop();
        }
    }
}

fn nonblocking(fd: &impl AsFd) -> io::Result<()> {
    let flags = OFlag::from_bits_retain(fcntl(fd, FcntlArg::F_GETFL).map_err(io::Error::from)?);
    fcntl(fd, FcntlArg::F_SETFL(flags | OFlag::O_NONBLOCK)).map_err(io::Error::from)?;
    Ok(())
}

struct Pipes<'a> {
    input: &'a [u8],
    offset: usize,
    stdin: Option<ChildStdin>,
    stdout: Option<ChildStdout>,
    output: Vec<u8>,
}
impl<'a> Pipes<'a> {
    fn new(child: &mut Child, input: Option<&'a [u8]>) -> io::Result<Self> {
        let stdin = child.stdin.take();
        if input.is_some() && stdin.is_none() {
            return Err(io::Error::other("subprocess stdin is unavailable"));
        }
        let stdout = child.stdout.take();
        if let Some(stdin) = &stdin {
            nonblocking(stdin)?;
        }
        if let Some(stdout) = &stdout {
            nonblocking(stdout)?;
        }
        Ok(Self {
            input: input.unwrap_or_default(),
            offset: 0,
            stdin,
            stdout,
            output: Vec::new(),
        })
    }
    fn write(&mut self) -> io::Result<bool> {
        if self.offset == self.input.len() {
            self.stdin = None;
            return Ok(false);
        }
        let Some(stdin) = &mut self.stdin else {
            return Ok(false);
        };
        let end = self.input.len().min(self.offset.saturating_add(64 * 1024));
        match stdin.write(&self.input[self.offset..end]) {
            Ok(0) => Err(io::ErrorKind::WriteZero.into()),
            Ok(count) => {
                self.offset += count;
                Ok(true)
            }
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::Interrupted | io::ErrorKind::WouldBlock
                ) =>
            {
                Ok(false)
            }
            Err(error) => Err(error),
        }
    }
    fn read(&mut self, max_output: u64) -> Result<bool, RunError> {
        let Some(stdout) = &mut self.stdout else {
            return Ok(false);
        };
        let mut chunk = [0_u8; 4096];
        match stdout.read(&mut chunk) {
            Ok(0) => {
                self.stdout = None;
                Ok(true)
            }
            Ok(count) => {
                self.output.extend_from_slice(&chunk[..count]);
                if self.output.len() as u64 > max_output {
                    return Err(RunError::OutputLimit);
                }
                Ok(true)
            }
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::Interrupted | io::ErrorKind::WouldBlock
                ) =>
            {
                Ok(false)
            }
            Err(error) => Err(error.into()),
        }
    }
    fn complete(&self) -> bool {
        self.stdin.is_none() && self.stdout.is_none()
    }
}

fn drive(
    child: &mut Child,
    input: Option<&[u8]>,
    start: Instant,
    timeout: Duration,
    max_output: u64,
    cancel: Option<&CancellationToken>,
    limits: Option<Limits>,
) -> Result<Vec<u8>, RunError> {
    let mut pipes = Pipes::new(child, input)?;
    let mut status = None;
    let mut next_memory = None;
    loop {
        if cancel.is_some_and(CancellationToken::is_cancelled) {
            return Err(RunError::Cancelled);
        }
        if start.elapsed() >= timeout {
            return Err(RunError::Timeout);
        }
        if status.is_none() {
            status = child.try_wait()?;
            if status.is_some_and(|status| !status.success()) {
                return Err(RunError::Exit);
            }
        }
        if let Some(limits) = limits
            && status.is_none()
            && next_memory.is_none_or(|next| Instant::now() >= next)
        {
            if limits.over_memory(child.id(), timeout.saturating_sub(start.elapsed())) {
                return Err(RunError::ResourceLimit);
            }
            next_memory = Some(Instant::now() + POLL);
        }
        if start.elapsed() >= timeout {
            return Err(RunError::Timeout);
        }
        let written = pipes.write()?;
        let read = pipes.read(max_output)?;
        if status.is_some() && pipes.complete() {
            return Ok(pipes.output);
        }
        if !(written || read) {
            thread::sleep(POLL.min(timeout.saturating_sub(start.elapsed())));
        }
    }
}

#[cfg(target_os = "macos")]
fn over_memory(pid: u32) -> bool {
    use nix::libc;
    let mut info = std::mem::MaybeUninit::<libc::proc_taskinfo>::uninit();
    let (Ok(pid), Ok(size)) = (
        i32::try_from(pid),
        i32::try_from(std::mem::size_of::<libc::proc_taskinfo>()),
    ) else {
        return false;
    };
    // SAFETY: proc_pidinfo initializes exactly this task-info struct on success.
    let copied = unsafe {
        libc::proc_pidinfo(
            pid,
            libc::PROC_PIDTASKINFO,
            0,
            info.as_mut_ptr().cast(),
            size,
        )
    };
    copied == size && unsafe { info.assume_init() }.pti_resident_size > MEMORY
}
#[cfg(not(target_os = "macos"))]
fn over_memory(_: u32) -> bool {
    false
} // RLIMIT_AS bounds mappings on Linux.

fn group_over_memory(pid: u32, remaining: Duration) -> bool {
    #[cfg(target_os = "macos")]
    {
        // Retain aggregate RSS monitoring for launcher children. The fixed ps
        // helper also uses the common runner, so it cannot outlive the deadline.
        let mut command = Command::new("/bin/ps");
        command
            .args(["-g", &pid.to_string(), "-o", "rss="])
            .stdout(Stdio::piped());
        let output = match run_bounded(&mut command, None, remaining, 64 * 1024, None, None) {
            Ok(output) => output,
            // ps reports a nonzero status when the launcher group has just exited.
            Err(RunError::Exit) => return false,
            Err(_) => return true,
        };
        String::from_utf8_lossy(&output)
            .split_whitespace()
            .filter_map(|rss| rss.parse::<u64>().ok())
            .sum::<u64>()
            > MEMORY / 1024
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (pid, remaining);
        false
    } // Descendants inherit RLIMIT_AS on Linux.
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::auth::{run_command, run_command_until};
    use nouli_core::identity::Id;
    use std::{fs, path::PathBuf};

    struct Directory(PathBuf);
    impl Directory {
        fn new() -> Self {
            Self(std::env::temp_dir().join(format!("nouli-process-{}", Id::generate())))
        }
    }
    impl Drop for Directory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn command_sources_run_without_a_shell_and_failures_yield_nothing() {
        let run = |words: &[&str], timeout| {
            let argv: Vec<String> = words.iter().map(|word| (*word).to_owned()).collect();
            run_command(&argv, timeout)
        };
        let second = Duration::from_secs(5);
        assert_eq!(
            run(&["printf", "  key-value\n"], second).as_deref(),
            Some("key-value")
        );
        // Arguments are not interpreted by a shell.
        assert_eq!(
            run(&["printf", "%s", "$HOME;x"], second).as_deref(),
            Some("$HOME;x")
        );
        assert_eq!(run(&["false"], second), None);
        assert_eq!(run(&["true"], second), None);
        assert_eq!(run(&["no-such-program-anywhere"], second), None);
        assert_eq!(run(&[], second), None);
        // Output from a failing command is never used.
        assert_eq!(run(&["sh", "-c", "echo SENTINEL; exit 1"], second), None);
        let started = Instant::now();
        assert_eq!(run(&["sleep", "30"], Duration::from_millis(200)), None);
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    #[test]
    fn a_grandchild_holding_the_output_open_cannot_outlast_the_timeout() {
        let directory = Directory::new();
        fs::create_dir_all(&directory.0).unwrap();
        let pids = directory.0.join("pid");
        let script = format!("sleep 30 & echo $! > {}; echo k", pids.display());
        let started = Instant::now();
        let argv = ["sh".to_owned(), "-c".into(), script];
        assert_eq!(run_command(&argv, Duration::from_millis(500)), None);
        assert!(started.elapsed() < Duration::from_secs(5));
        let pid: i32 = fs::read_to_string(&pids).unwrap().trim().parse().unwrap();
        // The timed-out command's descendants are gone, not orphaned.
        let gone = (0..100).any(|_| {
            thread::sleep(Duration::from_millis(50));
            nix::sys::signal::kill(Pid::from_raw(pid), None).is_err()
        });
        assert!(gone, "the grandchild is still running");
        // A background job that keeps the output open is not waited for either.
        let argv = ["sh".to_owned(), "-c".into(), "echo k; sleep 30 &".into()];
        let started = Instant::now();
        assert_eq!(run_command(&argv, Duration::from_millis(200)), None);
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn oversized_output_fails_closed_without_waiting_for_the_timeout() {
        let argv = [
            "sh".to_owned(),
            "-c".into(),
            "head -c 200000 /dev/zero | tr '\\0' a; sleep 30".into(),
        ];
        let started = Instant::now();
        assert_eq!(run_command(&argv, Duration::from_secs(20)), None);
        assert!(started.elapsed() < Duration::from_secs(5));
        // Output just within the cap is accepted.
        let argv = [
            "sh".to_owned(),
            "-c".into(),
            "head -c 60000 /dev/zero | tr '\\0' a".into(),
        ];
        assert_eq!(
            run_command(&argv, Duration::from_secs(5)).map(|key| key.len()),
            Some(60000)
        );
    }

    #[test]
    fn a_canceled_command_stops_early() {
        let cancel = CancellationToken::new();
        let stopper = cancel.clone();
        let handle = thread::spawn(move || {
            thread::sleep(Duration::from_millis(100));
            stopper.cancel();
        });
        let started = Instant::now();
        let argv = ["sleep".to_owned(), "30".into()];
        assert_eq!(
            run_command_until(&argv, Duration::from_secs(20), &cancel),
            None
        );
        assert!(started.elapsed() < Duration::from_secs(5));
        handle.join().unwrap();
    }
    #[test]
    fn a_command_sees_only_path_and_home() {
        let seen = run_command(
            &[
                "sh".into(),
                "-c".into(),
                "env | cut -d= -f1 | sort | tr '\\n' ' '".into(),
            ],
            Duration::from_secs(5),
        )
        .unwrap();
        for name in seen.split_whitespace() {
            assert!(
                matches!(name, "PATH" | "HOME" | "PWD" | "SHLVL" | "_" | "OLDPWD"),
                "{name} leaked into the command"
            );
        }
    }
    #[test]
    fn input_and_output_are_drained_together_without_pipe_deadlock() {
        let input = vec![b'x'; 256 * 1024];
        let mut command = Command::new("cat");
        command.stdout(Stdio::piped());
        let output = run_bounded(
            &mut command,
            Some(&input),
            Duration::from_secs(5),
            input.len() as u64,
            None,
            None,
        )
        .unwrap();
        assert_eq!(output, input);
    }

    #[test]
    fn a_child_that_never_reads_stdin_cannot_outlast_the_deadline() {
        let input = vec![b'x'; 256 * 1024];
        let mut command = Command::new("sleep");
        command.arg("30").stdout(Stdio::piped());
        let start = Instant::now();
        assert!(matches!(
            run_bounded(
                &mut command,
                Some(&input),
                Duration::from_millis(200),
                1024,
                None,
                None
            ),
            Err(RunError::Timeout)
        ));
        assert!(start.elapsed() < Duration::from_secs(5));
    }

    #[test]
    fn parser_limits_are_installed_in_the_child_before_execution() {
        let mut command = Command::new("sh");
        command
            .args(["-c", "ulimit -c; ulimit -t"])
            .stdout(Stdio::piped());
        let output = run_bounded(
            &mut command,
            None,
            Duration::from_secs(5),
            1024,
            None,
            Some(Limits::Parser),
        )
        .unwrap();
        assert_eq!(output, b"0\n8\n");
    }
    #[test]
    fn successful_office_launchers_also_stop_their_helper_group() {
        let directory = Directory::new();
        fs::create_dir_all(&directory.0).unwrap();
        let pids = directory.0.join("pid");
        let mut command = Command::new("sh");
        command
            .args(["-c", "sleep 30 & echo $! > \"$1\"", "--"])
            .arg(&pids)
            .stdout(Stdio::null());
        run_bounded(
            &mut command,
            None,
            Duration::from_secs(5),
            0,
            None,
            Some(Limits::Office {
                max_file_bytes: 1024,
            }),
        )
        .unwrap();
        let pid: i32 = fs::read_to_string(&pids).unwrap().trim().parse().unwrap();
        let gone = (0..100).any(|_| {
            thread::sleep(Duration::from_millis(50));
            nix::sys::signal::kill(Pid::from_raw(pid), None).is_err()
        });
        assert!(
            gone,
            "the converter's helper is still running after success"
        );
    }
}
