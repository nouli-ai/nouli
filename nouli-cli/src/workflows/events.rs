//! Owned nonblocking JSONL output. Backpressure never owns a blocking writer task.
use nix::{
    errno::Errno,
    fcntl::{FcntlArg, FdFlag, OFlag, fcntl},
    sys::stat::{SFlag, fstat},
    unistd::{dup, write},
};
use nouli_core::{
    codec,
    protocol::{EngineEvent, ErrorCategory, ErrorReport, stream::EventSink},
};
use std::{
    os::fd::{AsRawFd, OwnedFd, RawFd},
    time::Duration,
};
use tokio::{
    io::{Interest, unix::AsyncFd},
    sync::Mutex,
};
use tokio_util::sync::CancellationToken;

struct Descriptor {
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
enum Destination {
    Ready(AsyncFd<Descriptor>),
    Direct(Descriptor),
}
impl Destination {
    fn new() -> Result<Self, ErrorReport> {
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
    async fn bytes(&self, bytes: &[u8], cancel: &CancellationToken) -> Result<(), ErrorReport> {
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
                Err(Errno::EINTR) => continue,
                Err(Errno::EAGAIN) => match self {
                    Self::Ready(fd) => {
                        tokio::select! {biased;_=cancel.cancelled()=>return Err(interrupted()),ready=fd.writable()=>{ready.map_err(|_|failure())?.clear_ready();}}
                    }
                    Self::Direct(_) => {
                        tokio::select! {biased;_=cancel.cancelled()=>return Err(interrupted()),_=tokio::time::sleep(Duration::from_millis(10))=>{}}
                    }
                },
                Err(_) => return Err(failure()),
            }
        }
        Ok(())
    }
}
struct Writer {
    destination: Destination,
    failure: Mutex<Option<ErrorReport>>,
}
pub(super) struct Output {
    writer: Option<Writer>,
    cancel: CancellationToken,
    config_path: Option<std::path::PathBuf>,
}
impl Output {
    pub(super) fn new(stream: bool, cancel: CancellationToken) -> Result<Self, ErrorReport> {
        Ok(Self {
            writer: if stream {
                Some(Writer {
                    destination: Destination::new()?,
                    failure: Mutex::new(None),
                })
            } else {
                None
            },
            cancel,
            config_path: None,
        })
    }
    pub(super) fn with_config(mut self, path: std::path::PathBuf) -> Self {
        self.config_path = Some(path);
        self
    }
}
fn failure() -> ErrorReport {
    ErrorReport {
        category: ErrorCategory::Storage,
        code: "workflow_event_output_failed".into(),
        message: "Cannot write workflow events".into(),
        retryable: false,
    }
}
fn interrupted() -> ErrorReport {
    ErrorReport {
        category: ErrorCategory::Interrupted,
        code: "workflow_event_output_interrupted".into(),
        message: "Workflow event output interrupted under backpressure".into(),
        retryable: false,
    }
}
impl EventSink for Output {
    async fn send(&self, mut event: EngineEvent) -> Result<(), ErrorReport> {
        let Some(writer) = &self.writer else {
            return Ok(());
        };
        let mut failure_state = writer.failure.lock().await;
        if let Some(error) = &*failure_state {
            return Err(error.clone());
        }
        if let Some(path) = &self.config_path
            && let nouli_core::protocol::EventKind::Terminal { result } = &mut event.event
        {
            crate::settings::annotate_envelope(result, path);
        }
        let encoded = codec::to_value(&event)
            .map_err(|_| failure())
            .and_then(|value| serde_json::to_vec(&value).map_err(|_| failure()));
        let result = match encoded {
            Ok(mut bytes) => {
                bytes.push(b'\n');
                writer.destination.bytes(&bytes, &self.cancel).await
            }
            Err(error) => Err(error),
        };
        if let Err(error) = &result {
            *failure_state = Some(error.clone());
        }
        result
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
