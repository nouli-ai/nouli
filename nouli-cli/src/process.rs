//! Owned subprocesses with one deadline and bounded resources.
#[cfg(unix)]
#[path = "process/unix.rs"]
mod unix;
#[cfg(windows)]
#[path = "process/windows.rs"]
mod windows;
#[cfg(unix)]
pub(crate) use unix::run_bounded;
#[cfg(windows)]
pub(crate) use windows::run_bounded;

use std::io;
use thiserror::Error;

#[path = "process/limits.rs"]
mod limits;
pub(crate) use limits::Limits;
#[cfg(windows)]
#[path = "process/job.rs"]
mod job;

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
