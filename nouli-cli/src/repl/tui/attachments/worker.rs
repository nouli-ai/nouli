//! Parser isolation for sparse spreadsheets, compressed images and XML inputs.
use super::{Error, MAX_BYTES, documents};
use std::{
    io::{Read, Write},
    process::{Command, ExitCode, Stdio},
    thread,
    time::{Duration, Instant},
};

const MEMORY: u64 = 512 * 1024 * 1024;
const FLAG: &str = "--nouli-file-worker";

pub(crate) fn maybe_worker() -> Option<ExitCode> {
    let mut args = std::env::args();
    if args.nth(1).as_deref() != Some(FLAG) {
        return None;
    }
    let result = (|| {
        use nix::sys::resource::{Resource, setrlimit};
        setrlimit(Resource::RLIMIT_CORE, 0, 0).ok()?;
        setrlimit(Resource::RLIMIT_CPU, 8, 8).ok()?;
        #[cfg(not(target_os = "macos"))]
        {
            setrlimit(Resource::RLIMIT_DATA, MEMORY, MEMORY).ok()?;
            setrlimit(Resource::RLIMIT_AS, MEMORY, MEMORY).ok()?;
        }
        let extension = args.next()?;
        if !matches!(
            extension.as_str(),
            "svg" | "docx" | "pptx" | "xls" | "xlsx" | "xlsm" | "xlsb" | "ods"
        ) {
            return None;
        }
        let mut bytes = Vec::new();
        std::io::stdin()
            .take(MAX_BYTES + 1)
            .read_to_end(&mut bytes)
            .ok()?;
        if bytes.len() as u64 > MAX_BYTES {
            return None;
        }
        let output = if extension == "svg" {
            documents::svg(&bytes, "file").ok()?
        } else {
            documents::text(bytes, &extension, "file")
                .ok()?
                .into_bytes()
        };
        if output.len() as u64 > MAX_BYTES {
            return None;
        }
        std::io::stdout().write_all(&output).ok()
    })();
    Some(if result.is_some() {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(2)
    })
}

#[cfg(target_os = "macos")]
fn over_memory(pid: u32) -> bool {
    use nix::libc;
    let mut info = std::mem::MaybeUninit::<libc::proc_taskinfo>::uninit();
    let size = std::mem::size_of::<libc::proc_taskinfo>();
    // SAFETY: proc_pidinfo initializes exactly this task-info struct on success.
    let copied = unsafe {
        libc::proc_pidinfo(
            pid as i32,
            libc::PROC_PIDTASKINFO,
            0,
            info.as_mut_ptr().cast(),
            size as i32,
        )
    };
    copied == size as i32 && unsafe { info.assume_init() }.pti_resident_size > MEMORY
}

#[cfg(not(target_os = "macos"))]
fn over_memory(_: u32) -> bool {
    false
} // RLIMIT_AS bounds mappings on Linux.

pub(super) fn extract(bytes: Vec<u8>, extension: &str, name: &str) -> Result<Vec<u8>, Error> {
    let fail = || Error::Unsupported(name.into());
    let mut child = Command::new(std::env::current_exe().map_err(|_| fail())?)
        .args([FLAG, extension])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| fail())?;
    let mut input = child.stdin.take().expect("piped stdin");
    let output = child.stdout.take().expect("piped stdout");
    let writer = thread::spawn(move || input.write_all(&bytes));
    let reader = thread::spawn(move || {
        let mut bytes = Vec::new();
        output
            .take(MAX_BYTES + 1)
            .read_to_end(&mut bytes)
            .map(|_| bytes)
    });
    let start = Instant::now();
    let success = loop {
        if over_memory(child.id()) || start.elapsed() >= Duration::from_secs(10) {
            let _ = child.kill();
            let _ = child.wait();
            break false;
        }
        match child.try_wait() {
            Ok(Some(status)) => break status.success(),
            Ok(None) => thread::sleep(Duration::from_millis(10)),
            Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                break false;
            }
        }
    };
    let written = writer.join().is_ok_and(|result| result.is_ok());
    let bytes = reader.join().ok().and_then(Result::ok).ok_or_else(fail)?;
    if !success || !written || bytes.len() as u64 > MAX_BYTES {
        return Err(fail());
    }
    Ok(bytes)
}

pub(super) fn group_over_memory(pid: u32) -> bool {
    #[cfg(target_os = "macos")]
    {
        // ps -g selects the complete process group, including launcher children.
        let Ok(output) = Command::new("/bin/ps")
            .args(["-g", &pid.to_string(), "-o", "rss="])
            .output()
        else {
            return true;
        };
        String::from_utf8_lossy(&output.stdout)
            .split_whitespace()
            .filter_map(|rss| rss.parse::<u64>().ok())
            .sum::<u64>()
            > MEMORY / 1024
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = pid;
        false
    } // Descendants inherit RLIMIT_AS on Linux.
}
