use super::{failure, interrupted};
use nix::{
    errno::Errno,
    fcntl::{FcntlArg, FdFlag, OFlag, fcntl},
    sys::stat::{SFlag, fstat},
    unistd::{dup, write},
};
use nouli_core::protocol::ErrorReport;
use std::{
    os::fd::{AsRawFd, OwnedFd, RawFd},
    time::Duration,
};
use tokio::io::{Interest, unix::AsyncFd};
use tokio_util::sync::CancellationToken;

pub(super) struct Descriptor {
    fd: OwnedFd,
    flags: OFlag,
}
impl AsRawFd for Descriptor {
    fn as_raw_fd(&self) -> RawFd {
        self.fd.as_raw_fd()
    }
}
impl Drop for Descriptor {
    fn drop(&mut self) {
        // This duplicate shares stdout's file description. Restore its original
        // status flags before closing it, including on cancellation/unwind.
        if let Err(error) = fcntl(&self.fd, FcntlArg::F_SETFL(self.flags)) {
            tracing::warn!(%error,"could not restore standard output status flags");
        }
    }
}
pub(super) enum Destination {
    Ready(AsyncFd<Descriptor>),
    Direct(Descriptor),
}
impl Destination {
    pub(super) fn new() -> Result<Self, ErrorReport> {
        let fd = dup(std::io::stdout()).map_err(|_| failure())?;
        Self::from_fd(fd)
    }
    fn from_fd(fd: OwnedFd) -> Result<Self, ErrorReport> {
        fcntl(&fd, FcntlArg::F_SETFD(FdFlag::FD_CLOEXEC)).map_err(|_| failure())?;
        let flags = OFlag::from_bits_retain(fcntl(&fd, FcntlArg::F_GETFL).map_err(|_| failure())?);
        let descriptor = Descriptor { fd, flags };
        let kind = SFlag::from_bits_truncate(fstat(&descriptor.fd).map_err(|_| failure())?.st_mode);
        fcntl(&descriptor.fd, FcntlArg::F_SETFL(flags | OFlag::O_NONBLOCK))
            .map_err(|_| failure())?;
        if kind == SFlag::S_IFREG {
            return Ok(Self::Direct(descriptor));
        }
        // SAFETY: Descriptor exclusively owns a valid OwnedFd. Its AsRawFd value
        // never changes; no API exposes, replaces, or closes it. AsyncFd keeps
        // that Descriptor until deregistration, then drops/closes it normally.
        match unsafe { AsyncFd::register_with_interest(descriptor, Interest::WRITABLE) } {
            Ok(fd) => Ok(Self::Ready(fd)),
            Err(error) => {
                let (descriptor, error) = error.into_parts();
                // /dev/null is unpollable: epoll returns EPERM, Darwin kqueue
                // returns EINVAL. Character devices still use the nonblocking
                // descriptor and a cancellation-aware retry if they would block.
                if kind == SFlag::S_IFCHR
                    && matches!(error.raw_os_error(),Some(code) if code==Errno::EPERM as i32||code==Errno::EINVAL as i32)
                {
                    Ok(Self::Direct(descriptor))
                } else {
                    Err(failure())
                }
            }
        }
    }
    fn descriptor(&self) -> &Descriptor {
        match self {
            Self::Ready(fd) => fd.get_ref(),
            Self::Direct(fd) => fd,
        }
    }
    pub(super) async fn bytes(
        &self,
        bytes: &[u8],
        cancel: &CancellationToken,
    ) -> Result<(), ErrorReport> {
        let mut offset = 0;
        while offset < bytes.len() {
            // Try first even after cancellation so a readable consumer can still
            // receive a small recoverable terminal event. Cancellation wins once
            // backpressure prevents progress.
            match write(
                &self.descriptor().fd,
                &bytes[offset..bytes.len().min(offset + 16 * 1024)],
            ) {
                Ok(0) => return Err(failure()),
                Ok(count) => offset += count,
                Err(Errno::EINTR) => {}
                Err(Errno::EAGAIN) => match self {
                    Self::Ready(fd) => {
                        tokio::select! {biased;()=cancel.cancelled()=>return Err(interrupted()),ready=fd.writable()=>{ready.map_err(|_|failure())?.clear_ready();}}
                    }
                    Self::Direct(_) => {
                        tokio::select! {biased;()=cancel.cancelled()=>return Err(interrupted()),()=tokio::time::sleep(Duration::from_millis(10))=>{}}
                    }
                },
                Err(_) => return Err(failure()),
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn owned_nonblocking_descriptor_restores_shared_status_flags_when_dropped() {
        let (_reader, writer) = nix::unistd::pipe().unwrap();
        let before = fcntl(&writer, FcntlArg::F_GETFL).unwrap();
        let destination = Destination::from_fd(dup(&writer).unwrap()).unwrap();
        assert!(
            OFlag::from_bits_retain(fcntl(&writer, FcntlArg::F_GETFL).unwrap())
                .contains(OFlag::O_NONBLOCK)
        );
        drop(destination);
        assert_eq!(fcntl(&writer, FcntlArg::F_GETFL).unwrap(), before);
    }
    #[tokio::test]
    async fn actual_null_device_accepts_nonblocking_output_without_reactor_registration() {
        let file = std::fs::OpenOptions::new()
            .write(true)
            .open("/dev/null")
            .unwrap();
        let destination = Destination::from_fd(file.into()).unwrap();
        destination
            .bytes(b"fixture event\n", &CancellationToken::new())
            .await
            .unwrap();
    }
}
