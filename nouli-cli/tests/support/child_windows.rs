//! Isolated interactive-console defaults for tests launched through OpenSSH.
use std::{
    io,
    mem::{size_of, zeroed},
    os::windows::io::{FromRawHandle, OwnedHandle},
    process::Command,
    ptr,
};
use windows_sys::Win32::System::{
    Console::{
        GetConsoleMode, GetStdHandle, STD_INPUT_HANDLE, STD_OUTPUT_HANDLE, SetConsoleCtrlHandler,
    },
    JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
        SetInformationJobObject,
    },
    Threading::GetCurrentProcess,
};

unsafe extern "system" fn handled(_: u32) -> i32 {
    1
}

fn console_modes() -> Option<(u32, u32)> {
    let mut input = 0;
    let mut output = 0;
    // SAFETY: borrowed standard handles and valid output storage.
    unsafe {
        (GetConsoleMode(GetStdHandle(STD_INPUT_HANDLE), &raw mut input) != 0
            && GetConsoleMode(GetStdHandle(STD_OUTPUT_HANDLE), &raw mut output) != 0)
            .then_some((input, output))
    }
}

pub fn console(args: &[String]) -> io::Result<u8> {
    // SAFETY: changes only this isolated fixture's handler list. The ignore
    // attribute is inherited from SSH; custom handlers are not inherited.
    unsafe {
        if SetConsoleCtrlHandler(None, 0) == 0 || SetConsoleCtrlHandler(Some(handled), 1) == 0 {
            return Err(io::Error::last_os_error());
        }
        let handle = CreateJobObjectW(ptr::null(), ptr::null());
        if handle.is_null() {
            return Err(io::Error::last_os_error());
        }
        let job = OwnedHandle::from_raw_handle(handle);
        let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = zeroed();
        info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        if SetInformationJobObject(
            handle,
            JobObjectExtendedLimitInformation,
            (&raw const info).cast(),
            u32::try_from(size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>()).unwrap(),
        ) == 0
            || AssignProcessToJobObject(handle, GetCurrentProcess()) == 0
        {
            return Err(io::Error::last_os_error());
        }
        // The wrapper itself owns this job. Retain its handle until process
        // exit so normal return preserves the child's exit code. The OS closes
        // it on exit or forced termination, killing any remaining descendants.
        std::mem::forget(job);
    }
    let modes = console_modes();
    let status = Command::new(
        args.first()
            .ok_or_else(|| io::Error::other("missing console program"))?,
    )
    .args(&args[1..])
    .status()?;
    if modes != console_modes() {
        return Err(io::Error::other("CLI did not restore console modes"));
    }
    Ok(status
        .code()
        .and_then(|code| code.try_into().ok())
        .unwrap_or(2))
}

pub fn mouse_disabled(pid: u32) -> io::Result<()> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::System::Console::{AttachConsole, ENABLE_MOUSE_INPUT, FreeConsole};
    // SAFETY: the fixture attaches only itself to the isolated test console.
    unsafe {
        FreeConsole();
        if AttachConsole(pid) == 0 {
            return Err(io::Error::last_os_error());
        }
    }
    let console = std::fs::File::open("CONIN$")?;
    let mut mode = 0;
    // SAFETY: CONIN$ resolves to the attached console even with redirected stdio.
    if unsafe { GetConsoleMode(console.as_raw_handle(), &raw mut mode) } == 0 {
        return Err(io::Error::last_os_error());
    }
    if mode & ENABLE_MOUSE_INPUT != 0 {
        return Err(io::Error::other("mouse capture is enabled"));
    }
    Ok(())
}
