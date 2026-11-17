//! Deterministic subprocess behavior for Cargo product tests. No shell is used.
#[cfg(windows)]
mod child_windows;
#[cfg(windows)]
#[path = "../../src/process/job.rs"]
#[allow(
    dead_code,
    reason = "the fixture invokes the direct-worker limiter only"
)]
mod job;
#[path = "../../src/process/limits.rs"]
#[allow(
    dead_code,
    reason = "the fixture invokes the direct-worker limiter only"
)]
mod limits;
use std::{
    io::{self, Read, Write},
    process::{Command, ExitCode, Stdio},
    thread,
    time::Duration,
};

fn run(args: &[String]) -> io::Result<()> {
    match args.first().map(String::as_str) {
        Some("interrupt") => interrupt(
            args.get(1)
                .and_then(|value| value.parse().ok())
                .ok_or_else(|| io::Error::other("missing target PID"))?,
        )?,
        Some("emit") => io::stdout().write_all(
            args.get(1)
                .map_or(b"fixture-key".as_slice(), String::as_bytes),
        )?,
        Some("empty") => {}
        Some("count") => {
            let mut file = std::fs::OpenOptions::new().append(true).create(true).open(
                args.get(1)
                    .ok_or_else(|| io::Error::other("missing counter file"))?,
            )?;
            file.write_all(b"x\n")?;
            io::stdout().write_all(args.get(2).map_or(b"key".as_slice(), String::as_bytes))?;
        }
        Some("gate") => {
            let mut file = std::fs::OpenOptions::new()
                .append(true)
                .create(true)
                .open(&args[1])?;
            file.write_all(b"x")?;
            while !std::path::Path::new(&args[2]).is_file() {
                thread::sleep(Duration::from_millis(20));
            }
            io::stdout().write_all(b"fixture-key")?;
        }
        Some("pid-sleep") => {
            std::fs::write(&args[1], std::process::id().to_string())?;
            thread::sleep(Duration::from_secs(30));
        }
        Some("fail") => {
            io::stdout().write_all(b"LEAKED_OUTPUT")?;
            io::stderr().write_all(b"LEAKED_ERROR")?;
            return Err(io::Error::other("fixture failure"));
        }
        Some("echo") => {
            let mut bytes = Vec::new();
            io::stdin().read_to_end(&mut bytes)?;
            io::stdout().write_all(&bytes)?;
        }
        Some("sleep") => thread::sleep(Duration::from_millis(
            args.get(1)
                .and_then(|value| value.parse().ok())
                .unwrap_or(30_000),
        )),
        Some("fill") => {
            let count = args
                .get(1)
                .and_then(|value| value.parse().ok())
                .unwrap_or(200_000);
            let bytes = vec![b'x'; count];
            io::stdout().write_all(&bytes)?;
            if args.get(2).is_some_and(|value| value == "wait") {
                thread::sleep(Duration::from_secs(30));
            }
        }
        Some("env") => {
            let mut names: Vec<_> = std::env::vars_os()
                .map(|(key, _)| key.to_string_lossy().into_owned())
                .collect();
            names.sort();
            io::stdout().write_all(names.join(" ").as_bytes())?;
        }
        Some("spawn") => {
            let mut child = Command::new(std::env::current_exe()?)
                .args(["sleep", "30000"])
                .stdin(Stdio::null())
                .stdout(Stdio::inherit())
                .stderr(Stdio::null())
                .spawn()?;
            std::fs::write(
                args.get(1)
                    .ok_or_else(|| io::Error::other("missing PID file"))?,
                child.id().to_string(),
            )?;
            io::stdout().write_all(b"fixture-key")?;
            io::stdout().flush()?;
            if args.get(2).is_some_and(|value| value == "wait") {
                let _ = child.wait();
            }
        }
        Some(mode @ ("memory" | "limited-memory")) => {
            if mode == "limited-memory" {
                limits::Limits::Parser.apply()?;
            }
            let mut allocations = Vec::new();
            for _ in 0..1024 {
                allocations.push(vec![42u8; 1024 * 1024]);
                std::hint::black_box(&allocations);
            }
        }
        _ => return Err(io::Error::other("unknown fixture mode")),
    }
    Ok(())
}

fn interrupt(pid: u32) -> io::Result<()> {
    #[cfg(unix)]
    {
        nix::sys::signal::kill(
            nix::unistd::Pid::from_raw(i32::try_from(pid).map_err(io::Error::other)?),
            nix::sys::signal::Signal::SIGINT,
        )
        .map_err(io::Error::from)
    }
    #[cfg(windows)]
    {
        use windows_sys::Win32::System::Console::{
            AttachConsole, CTRL_C_EVENT, FreeConsole, GenerateConsoleCtrlEvent,
            SetConsoleCtrlHandler,
        };
        // SAFETY: this short-lived fixture detaches only itself and signals the
        // target's isolated console. It cannot signal the Cargo test console.
        unsafe {
            FreeConsole();
            if AttachConsole(pid) == 0 {
                return Err(io::Error::last_os_error());
            }
            if SetConsoleCtrlHandler(None, 1) == 0 {
                return Err(io::Error::last_os_error());
            }
            if GenerateConsoleCtrlEvent(CTRL_C_EVENT, 0) == 0 {
                return Err(io::Error::last_os_error());
            }
            thread::sleep(Duration::from_millis(50));
            FreeConsole();
        }
        Ok(())
    }
}

fn main() -> ExitCode {
    let args: Vec<_> = std::env::args().skip(1).collect();
    #[cfg(windows)]
    if args.first().is_some_and(|arg| arg == "console") {
        return ExitCode::from(child_windows::console(&args[1..]).unwrap_or(2));
    }
    #[cfg(windows)]
    if args.first().is_some_and(|arg| arg == "mouse-disabled") {
        return ExitCode::from(
            if child_windows::mouse_disabled(args[1].parse().unwrap()).is_ok() {
                0
            } else {
                2
            },
        );
    }
    if run(&args).is_ok() {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(2)
    }
}
