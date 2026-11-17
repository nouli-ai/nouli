//! Cargo builds the fixture as an example next to each test profile's deps.
use std::{path::PathBuf, process::Command};

pub fn executable() -> PathBuf {
    let current = std::env::current_exe().expect("test executable path");
    let profile = current
        .parent()
        .and_then(std::path::Path::parent)
        .expect("Cargo test profile");
    profile
        .join("examples")
        .join(format!("nouli-test-child{}", std::env::consts::EXE_SUFFIX))
}

pub fn command(mode: &str) -> Command {
    let executable = executable();
    assert!(
        executable.is_file(),
        "Cargo must build the child fixture: {}",
        executable.display()
    );
    let mut command = Command::new(executable);
    command.arg(mode);
    command
}

pub fn cli_executable() -> PathBuf {
    executable()
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join(format!("nouli{}", std::env::consts::EXE_SUFFIX))
}

pub fn running(pid: u32) -> bool {
    #[cfg(unix)]
    {
        nix::sys::signal::kill(
            nix::unistd::Pid::from_raw(i32::try_from(pid).unwrap()),
            None,
        )
        .is_ok()
    }
    #[cfg(windows)]
    {
        use windows_sys::Win32::{Foundation::*, System::Threading::*};
        // SAFETY: query only the fixture PID, closing the handle after the query.
        unsafe {
            let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
            if process.is_null() {
                return false;
            }
            let mut code = 0;
            let ok = GetExitCodeProcess(process, &raw mut code);
            CloseHandle(process);
            ok != 0 && code == 259
        }
    }
}
