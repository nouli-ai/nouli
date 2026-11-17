//! Redirected stdout may be a synchronous anonymous pipe. Give each frame an
//! owned native writer and cancel/join it, including when its future is dropped.
use super::{failure, interrupted};
use nouli_core::protocol::ErrorReport;
use std::{
    fs::File,
    io::{self, Write},
    os::windows::io::{AsRawHandle, FromRawHandle},
    ptr,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::Duration,
};
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;
use windows_sys::Win32::{
    Foundation::{DUPLICATE_SAME_ACCESS, DuplicateHandle, ERROR_OPERATION_ABORTED},
    System::{
        Console::{GetConsoleMode, GetStdHandle, STD_OUTPUT_HANDLE, WriteConsoleW},
        IO::CancelSynchronousIo,
        Threading::GetCurrentProcess,
    },
};

pub(super) struct Destination(File);
impl Destination {
    pub(super) fn new() -> Result<Self, ErrorReport> {
        let mut duplicate = ptr::null_mut();
        // SAFETY: duplicate the current stdout handle without taking its ownership.
        unsafe {
            if DuplicateHandle(
                GetCurrentProcess(),
                GetStdHandle(STD_OUTPUT_HANDLE),
                GetCurrentProcess(),
                &raw mut duplicate,
                0,
                0,
                DUPLICATE_SAME_ACCESS,
            ) == 0
            {
                return Err(failure());
            }
            Ok(Self(File::from_raw_handle(duplicate)))
        }
    }
    pub(super) async fn bytes(
        &self,
        bytes: &[u8],
        cancel: &CancellationToken,
    ) -> Result<(), ErrorReport> {
        let file = self.0.try_clone().map_err(|_| failure())?;
        let bytes = bytes.to_owned();
        let stop = Arc::new(AtomicBool::new(false));
        let stopped = stop.clone();
        let (sender, mut receiver) = oneshot::channel();
        let writer = thread::spawn(move || {
            let result = write_frame(file, &bytes, &stopped);
            let _ = sender.send(result);
        });
        let mut owned = OwnedWriter {
            writer: Some(writer),
            stop,
        };
        let mut cancelled = false;
        let result = loop {
            tokio::select! {
                biased;
                result = &mut receiver => break result.map_err(|_| failure())?,
                () = cancel.cancelled(), if !cancelled => { cancelled = true; owned.stop.store(true, Ordering::Release); },
                () = tokio::time::sleep(Duration::from_millis(2)), if cancelled => owned.cancel(),
            }
        };
        owned.join();
        result.map_err(|error| {
            if cancelled
                && (error.kind() == io::ErrorKind::Interrupted
                    || error.raw_os_error() == Some(ERROR_OPERATION_ABORTED.cast_signed()))
            {
                interrupted()
            } else {
                failure()
            }
        })
    }
}

fn write_frame(mut file: File, bytes: &[u8], stop: &AtomicBool) -> io::Result<()> {
    let mut mode = 0;
    // SAFETY: the owned handle is live and mode is valid output storage.
    let console = unsafe { GetConsoleMode(file.as_raw_handle(), &raw mut mode) } != 0;
    if console {
        let text = String::from_utf8_lossy(bytes);
        let wide: Vec<u16> = text.encode_utf16().collect();
        let mut offset = 0;
        while offset < wide.len() {
            let mut written = 0;
            let mut count = (wide.len() - offset).min(16 * 1024);
            // Keep a surrogate pair together across console write boundaries.
            if offset + count < wide.len() && (0xd800..=0xdbff).contains(&wide[offset + count - 1])
            {
                count -= 1;
            }
            // SAFETY: the buffer is valid for count UTF-16 units; only this thread writes this frame.
            if unsafe {
                WriteConsoleW(
                    file.as_raw_handle(),
                    wide[offset..].as_ptr().cast(),
                    u32::try_from(count).expect("chunk fits u32"),
                    &raw mut written,
                    ptr::null(),
                )
            } == 0
            {
                return Err(io::Error::last_os_error());
            }
            if written == 0 {
                return Err(io::ErrorKind::WriteZero.into());
            }
            offset += written as usize;
            if offset < wide.len() && stop.load(Ordering::Acquire) {
                return Err(io::ErrorKind::Interrupted.into());
            }
        }
    } else {
        let mut offset = 0;
        while offset < bytes.len() {
            let count = file.write(&bytes[offset..bytes.len().min(offset + 16 * 1024)])?;
            if count == 0 {
                return Err(io::ErrorKind::WriteZero.into());
            }
            offset += count;
            if offset < bytes.len() && stop.load(Ordering::Acquire) {
                return Err(io::ErrorKind::Interrupted.into());
            }
        }
    }
    Ok(())
}

struct OwnedWriter {
    writer: Option<thread::JoinHandle<()>>,
    stop: Arc<AtomicBool>,
}
impl OwnedWriter {
    fn cancel(&self) {
        if let Some(writer) = &self.writer {
            // SAFETY: the thread handle stays owned until join. Repetition closes
            // the race between cancellation and entry into synchronous WriteFile.
            unsafe {
                CancelSynchronousIo(writer.as_raw_handle());
            }
        }
    }
    fn join(&mut self) {
        if let Some(writer) = self.writer.take() {
            let _ = writer.join();
        }
    }
}
impl Drop for OwnedWriter {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        while self
            .writer
            .as_ref()
            .is_some_and(|writer| !writer.is_finished())
        {
            self.cancel();
            thread::sleep(Duration::from_millis(2));
        }
        self.join();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        future::Future,
        task::{Context, Poll, Waker},
        time::Instant,
    };
    use windows_sys::Win32::System::Pipes::CreatePipe;

    #[tokio::test]
    async fn dropping_a_backpressured_future_cancels_and_joins_its_writer() {
        let mut read = ptr::null_mut();
        let mut write = ptr::null_mut();
        // SAFETY: successful creation transfers two independent owned handles.
        let (reader, writer) = unsafe {
            assert_ne!(
                CreatePipe(&raw mut read, &raw mut write, ptr::null(), 4096),
                0
            );
            (File::from_raw_handle(read), File::from_raw_handle(write))
        };
        let destination = Destination(writer);
        let cancel = CancellationToken::new();
        let bytes = vec![b'x'; 256 * 1024];
        let mut future = Box::pin(destination.bytes(&bytes, &cancel));
        let mut context = Context::from_waker(Waker::noop());
        assert!(matches!(future.as_mut().poll(&mut context), Poll::Pending));
        tokio::time::sleep(Duration::from_millis(50)).await;
        let started = Instant::now();
        drop(future);
        assert!(started.elapsed() < Duration::from_secs(2));
        assert!(
            !cancel.is_cancelled(),
            "cleanup must work without token cancellation"
        );
        drop(reader);
    }
}
