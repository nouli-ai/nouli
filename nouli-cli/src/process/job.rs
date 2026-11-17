//! Native job configuration shared by supervised children and direct workers.
use super::limits::{Limits, MEMORY};
use std::{
    io,
    mem::zeroed,
    os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle},
    ptr,
};
use windows_sys::Win32::{
    Foundation::HANDLE,
    System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_DIE_ON_UNHANDLED_EXCEPTION,
        JOB_OBJECT_LIMIT_JOB_MEMORY, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        JOB_OBJECT_LIMIT_PROCESS_MEMORY, JOB_OBJECT_LIMIT_PROCESS_TIME,
        JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
        SetInformationJobObject, TerminateJobObject,
    },
};
pub(super) struct Job(OwnedHandle);
impl Job {
    pub(super) fn new(limits: Option<Limits>, kill_on_close: bool) -> io::Result<Self> {
        // SAFETY: returned handle is checked and owned exactly once.
        let handle = unsafe { CreateJobObjectW(ptr::null(), ptr::null()) };
        if handle.is_null() {
            return Err(io::Error::last_os_error());
        }
        let job = Self(unsafe { OwnedHandle::from_raw_handle(handle) });
        let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { zeroed() };
        if kill_on_close {
            info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        }
        if limits.is_some() {
            info.BasicLimitInformation.LimitFlags |= JOB_OBJECT_LIMIT_PROCESS_TIME
                | JOB_OBJECT_LIMIT_PROCESS_MEMORY
                | JOB_OBJECT_LIMIT_JOB_MEMORY
                | JOB_OBJECT_LIMIT_DIE_ON_UNHANDLED_EXCEPTION;
            info.BasicLimitInformation.PerProcessUserTimeLimit = 8 * 10_000_000;
            info.ProcessMemoryLimit =
                usize::try_from(MEMORY).expect("memory limit fits Windows usize");
            info.JobMemoryLimit = info.ProcessMemoryLimit;
        }
        // SAFETY: the initialized information buffer has the exact advertised size.
        if unsafe {
            SetInformationJobObject(
                handle,
                JobObjectExtendedLimitInformation,
                ptr::addr_of!(info).cast(),
                u32::try_from(size_of_val(&info)).expect("job info fits u32"),
            )
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
        Ok(job)
    }
    pub(super) fn assign(&self, process: HANDLE) -> io::Result<()> {
        // SAFETY: both handles are live; assignment happens while the child is suspended.
        if unsafe { AssignProcessToJobObject(self.0.as_raw_handle(), process) } == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }
    pub(super) fn stop(&self) {
        // SAFETY: this job owns only subprocesses created by this boundary.
        unsafe {
            TerminateJobObject(self.0.as_raw_handle(), 1);
        }
    }
}
