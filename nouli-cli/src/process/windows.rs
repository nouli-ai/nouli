//! Suspended children enter an owned job before execution. The sole stdout
//! reader peeks before reading; terminating the job releases any blocked stdin
//! writer, which is always joined before this boundary returns.
use super::{Limits, RunError, job::Job};
use std::{
    fs,
    io::{self, Read, Write},
    mem::{size_of, zeroed},
    os::windows::{
        fs::MetadataExt,
        io::{AsRawHandle, FromRawHandle, OwnedHandle},
        process::CommandExt,
    },
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    ptr,
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};
use tokio_util::sync::CancellationToken;
use windows_sys::Win32::{
    Foundation::{ERROR_BROKEN_PIPE, INVALID_HANDLE_VALUE},
    System::{
        Diagnostics::ToolHelp::{
            CreateToolhelp32Snapshot, TH32CS_SNAPTHREAD, THREADENTRY32, Thread32First, Thread32Next,
        },
        Pipes::PeekNamedPipe,
        Threading::{
            CREATE_NO_WINDOW, CREATE_SUSPENDED, OpenThread, ResumeThread, THREAD_SUSPEND_RESUME,
        },
    },
};

fn resume(child: &Child) -> io::Result<()> {
    // Rust 1.99 does not expose the initial thread handle. A suspended, newly
    // created process has its initial thread; find/open it before any code runs.
    let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0) };
    if snapshot == INVALID_HANDLE_VALUE {
        return Err(io::Error::last_os_error());
    }
    let snapshot = unsafe { OwnedHandle::from_raw_handle(snapshot) };
    let mut entry: THREADENTRY32 = unsafe { zeroed() };
    entry.dwSize = u32::try_from(size_of::<THREADENTRY32>()).expect("thread info fits u32");
    let mut available = unsafe { Thread32First(snapshot.as_raw_handle(), &raw mut entry) };
    while available != 0 {
        if entry.th32OwnerProcessID == child.id() {
            let handle = unsafe { OpenThread(THREAD_SUSPEND_RESUME, 0, entry.th32ThreadID) };
            if handle.is_null() {
                return Err(io::Error::last_os_error());
            }
            let thread = unsafe { OwnedHandle::from_raw_handle(handle) };
            if unsafe { ResumeThread(thread.as_raw_handle()) } == u32::MAX {
                return Err(io::Error::last_os_error());
            }
            return Ok(());
        }
        available = unsafe { Thread32Next(snapshot.as_raw_handle(), &raw mut entry) };
    }
    Err(io::Error::other("suspended child thread is unavailable"))
}

fn office_directory(command: &Command, limits: Option<Limits>) -> Option<(PathBuf, u64)> {
    let Some(Limits::Office { max_file_bytes }) = limits else {
        return None;
    };
    let args: Vec<_> = command.get_args().collect();
    args.windows(2)
        .find(|args| args[0] == "--outdir")
        .map(|args| (PathBuf::from(args[1]), max_file_bytes))
}

fn files_within_limit(root: &Path, limit: u64) -> io::Result<bool> {
    let mut pending = vec![root.to_owned()];
    let mut count = 0;
    while let Some(directory) = pending.pop() {
        let entries = match fs::read_dir(directory) {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error),
        };
        for entry in entries {
            let entry = entry?;
            count += 1;
            if count > 4096 {
                return Ok(false);
            }
            let metadata = match fs::symlink_metadata(entry.path()) {
                Ok(metadata) => metadata,
                Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
                Err(error) => return Err(error),
            };
            if metadata.file_attributes()
                & windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT
                != 0
            {
                return Ok(false);
            }
            if metadata.is_dir() {
                pending.push(entry.path());
            } else if metadata.len() > limit {
                return Ok(false);
            }
        }
    }
    Ok(true)
}

struct Output {
    pipe: Option<std::process::ChildStdout>,
    bytes: Vec<u8>,
    eof: bool,
}
impl Output {
    fn new(pipe: Option<std::process::ChildStdout>) -> Self {
        Self {
            eof: pipe.is_none(),
            pipe,
            bytes: Vec::new(),
        }
    }
    fn drain(&mut self, max: u64) -> Result<bool, RunError> {
        let mut progressed = false;
        if !self.eof {
            let stdout = self
                .pipe
                .as_mut()
                .ok_or_else(|| io::Error::other("subprocess stdout is unavailable"))?;
            let mut available = 0;
            // SAFETY: stdout has one reader, so peeked bytes cannot be
            // consumed elsewhere before the following bounded read.
            if unsafe {
                PeekNamedPipe(
                    stdout.as_raw_handle(),
                    ptr::null_mut(),
                    0,
                    ptr::null_mut(),
                    &raw mut available,
                    ptr::null_mut(),
                )
            } == 0
            {
                if io::Error::last_os_error().raw_os_error()
                    == Some(ERROR_BROKEN_PIPE.cast_signed())
                {
                    self.eof = true;
                } else {
                    return Err(io::Error::last_os_error().into());
                }
            } else if available != 0 {
                let mut buffer = [0u8; 16 * 1024];
                let capacity = (available as usize).min(buffer.len());
                let count = stdout.read(&mut buffer[..capacity])?;
                if count == 0 {
                    self.eof = true;
                } else {
                    if self.bytes.len() as u64 + count as u64 > max {
                        return Err(RunError::OutputLimit);
                    }
                    self.bytes.extend_from_slice(&buffer[..count]);
                    progressed = true;
                }
            }
        }
        Ok(progressed)
    }
}

fn validate_program(program: &std::ffi::OsStr) -> Result<(), RunError> {
    if Path::new(program).extension().is_some_and(|extension| {
        extension.eq_ignore_ascii_case("bat") || extension.eq_ignore_ascii_case("cmd")
    }) {
        return Err(RunError::Spawn(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invoke an executable directly",
        )));
    }
    Ok(())
}

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
    validate_program(command.get_program())?;
    let started = Instant::now();
    let directory = office_directory(command, limits);
    let job = Job::new(limits, true)?;
    let office = matches!(limits, Some(Limits::Office { .. }));
    command
        .creation_flags(CREATE_SUSPENDED | CREATE_NO_WINDOW)
        .stderr(Stdio::null())
        .stdout(if office {
            Stdio::null()
        } else {
            Stdio::piped()
        })
        .stdin(if input.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        });
    let mut child = command.spawn().map_err(RunError::Spawn)?;
    if let Err(error) = job
        .assign(child.as_raw_handle())
        .and_then(|()| resume(&child))
    {
        let _ = child.kill();
        let _ = child.wait();
        return Err(error.into());
    }
    let (sent, received) = mpsc::sync_channel(1);
    let writer = if let Some(input) = input {
        let mut stdin = child
            .stdin
            .take()
            .ok_or_else(|| io::Error::other("subprocess stdin is unavailable"))?;
        let input = input.to_owned();
        Some(thread::spawn(move || {
            let result = stdin.write_all(&input);
            drop(stdin);
            let _ = sent.send(result);
        }))
    } else {
        None
    };
    let result = (|| {
        let mut output = Output::new(child.stdout.take());
        let mut input_complete = writer.is_none();
        loop {
            let mut progressed = false;
            if cancel.is_some_and(CancellationToken::is_cancelled) {
                return Err(RunError::Cancelled);
            }
            if started.elapsed() >= timeout {
                return Err(RunError::Timeout);
            }
            if let Some((directory, limit)) = &directory
                && !files_within_limit(directory, *limit)?
            {
                return Err(RunError::ResourceLimit);
            }
            if !input_complete {
                match received.try_recv() {
                    Ok(result) => {
                        result?;
                        input_complete = true;
                        progressed = true;
                    }
                    Err(mpsc::TryRecvError::Empty) => {}
                    Err(mpsc::TryRecvError::Disconnected) => {
                        return Err(io::Error::other("subprocess input writer failed").into());
                    }
                }
            }
            progressed |= output.drain(max_output)?;
            if let Some(status) = child.try_wait()? {
                if !status.success() {
                    return Err(RunError::Exit);
                }
                if output.eof && input_complete {
                    return Ok(output.bytes);
                }
            }
            if !progressed {
                thread::sleep(Duration::from_millis(10));
            }
        }
    })();
    job.stop();
    let _ = child.wait();
    if let Some(writer) = writer {
        let _ = writer.join();
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_child;

    fn run(
        mode: &str,
        input: Option<&[u8]>,
        timeout: Duration,
        max: u64,
        limits: Option<Limits>,
    ) -> Result<Vec<u8>, RunError> {
        run_bounded(
            &mut test_child::command(mode),
            input,
            timeout,
            max,
            None,
            limits,
        )
    }

    #[test]
    fn duplex_pipes_drain_without_deadlock_or_throttling() {
        let bytes = vec![b'x'; 4 * 1024 * 1024];
        assert_eq!(
            run(
                "echo",
                Some(&bytes),
                Duration::from_secs(10),
                bytes.len() as u64,
                None
            )
            .unwrap(),
            bytes
        );
    }

    #[test]
    fn blocked_input_and_oversized_output_are_stopped() {
        let bytes = vec![b'x'; 256 * 1024];
        assert!(matches!(
            run(
                "sleep",
                Some(&bytes),
                Duration::from_millis(500),
                1024,
                None
            ),
            Err(RunError::Timeout)
        ));
        assert!(matches!(
            run("fill", None, Duration::from_secs(5), 64 * 1024, None),
            Err(RunError::OutputLimit)
        ));
    }

    #[test]
    fn cancellation_joins_the_input_writer_and_stops_children() {
        let cancel = CancellationToken::new();
        let trigger = cancel.clone();
        let stopper = thread::spawn(move || {
            thread::sleep(Duration::from_millis(200));
            trigger.cancel();
        });
        let input = vec![b'x'; 256 * 1024];
        assert!(matches!(
            run_bounded(
                &mut test_child::command("sleep"),
                Some(&input),
                Duration::from_secs(10),
                1024,
                Some(&cancel),
                None
            ),
            Err(RunError::Cancelled)
        ));
        stopper.join().unwrap();
    }

    #[test]
    fn exited_launchers_cannot_leave_a_descendant_holding_stdout() {
        let directory = std::env::temp_dir().join(format!(
            "nouli-job-{}",
            nouli_core::identity::Id::generate()
        ));
        fs::create_dir_all(&directory).unwrap();
        let pidfile = directory.join("pid");
        let mut command = test_child::command("spawn");
        command.arg(&pidfile).arg("exit");
        assert!(matches!(
            run_bounded(&mut command, None, Duration::from_secs(5), 1024, None, None),
            Err(RunError::Timeout)
        ));
        let pid = fs::read_to_string(pidfile).unwrap().parse().unwrap();
        assert!(!test_child::running(pid));
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn office_success_also_cleans_up_helper_processes() {
        let directory = std::env::temp_dir().join(format!(
            "nouli-office-job-{}",
            nouli_core::identity::Id::generate()
        ));
        fs::create_dir_all(&directory).unwrap();
        let pidfile = directory.join("pid");
        let mut command = test_child::command("spawn");
        command.arg(&pidfile).arg("exit");
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
        let pid = fs::read_to_string(pidfile).unwrap().parse().unwrap();
        assert!(!test_child::running(pid));
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn memory_limits_apply_to_spawned_and_direct_parser_workers() {
        for (mode, limits) in [("memory", Some(Limits::Parser)), ("limited-memory", None)] {
            assert!(
                run(mode, None, Duration::from_secs(10), 1024, limits).is_err(),
                "resource limit was not enforced for {mode}"
            );
        }
    }

    #[test]
    fn credentials_are_direct_arguments_in_a_scrubbed_environment() {
        let executable = test_child::executable().to_string_lossy().into_owned();
        let run = |mode: &str, argument: &str| {
            crate::settings::auth::run_command(
                &[executable.clone(), mode.into(), argument.into()],
                Duration::from_secs(10),
            )
        };
        assert_eq!(run("emit", "$HOME;x").as_deref(), Some("$HOME;x"));
        assert!(run("fail", "").is_none());
        assert!(run("empty", "").is_none());
        for name in run("env", "").unwrap().split_whitespace() {
            assert!(
                [
                    "path",
                    "home",
                    "systemroot",
                    "userprofile",
                    "localappdata",
                    "temp",
                    "tmp",
                    "processor_architecture",
                    "processor_architew6432"
                ]
                .contains(&name.to_ascii_lowercase().as_str()),
                "environment variable leaked: {name}"
            );
        }
    }
}
