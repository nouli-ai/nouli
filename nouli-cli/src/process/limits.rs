pub(super) const MEMORY: u64 = 512 * 1024 * 1024;
#[derive(Clone, Copy)]
pub(crate) enum Limits {
    Parser,
    Office { max_file_bytes: u64 },
}

use std::io;

#[cfg(unix)]
impl Limits {
    pub(crate) fn apply(self) -> io::Result<()> {
        use nix::sys::resource::{Resource, setrlimit};
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
}

#[cfg(windows)]
impl Limits {
    pub(crate) fn apply(self) -> io::Result<()> {
        let job = super::job::Job::new(Some(self), false)?;
        // SAFETY: the current-process pseudo handle is valid for job assignment.
        job.assign(unsafe { windows_sys::Win32::System::Threading::GetCurrentProcess() })?;
        // A job without KILL_ON_JOB_CLOSE retains its limits while associated
        // processes live, even after the last userspace job handle is closed.
        Ok(())
    }
}
