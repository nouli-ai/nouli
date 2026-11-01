//! Bounded POSIX PTY interaction with an owned child process.
use nix::{
    errno::Errno,
    fcntl::{FcntlArg, OFlag, fcntl},
    libc,
    poll::{PollFd, PollFlags, PollTimeout, poll},
    pty::{Winsize, openpty},
    sys::signal::{Signal, kill},
    unistd::{Pid, setsid},
};
use std::{
    fs::File,
    io::{self, Read, Write},
    os::{fd::AsFd, fd::AsRawFd, unix::process::CommandExt},
    process::{Child, Command, ExitStatus, Stdio},
    time::{Duration, Instant},
};
use thiserror::Error;

const TIMEOUT: Duration = Duration::from_secs(10);
const MAX_OUTPUT: usize = 1024 * 1024;

nix::ioctl_write_int_bad!(set_controlling_terminal, libc::TIOCSCTTY);
nix::ioctl_write_ptr_bad!(set_window_size, libc::TIOCSWINSZ, Winsize);

#[derive(Debug, Error)]
pub enum Error {
    #[error("PTY I/O failed: {0}")]
    Io(#[from] io::Error),
    #[error("PTY system operation failed: {0}")]
    System(#[from] Errno),
    #[error("timed out waiting for {0}")]
    Timeout(&'static str),
    #[error("terminal closed before {expected}; child status: {status:?}")]
    Closed {
        expected: &'static str,
        status: Option<ExitStatus>,
    },
    #[error("terminal output exceeded the {MAX_OUTPUT}-byte test limit")]
    OutputLimit,
}

pub struct Terminal {
    child: Child,
    master: File,
    output: Vec<u8>,
    closed: bool,
}

impl Terminal {
    pub fn spawn(mut command: Command, columns: u16, rows: u16) -> Result<Self, Error> {
        let size = Winsize {
            ws_row: rows,
            ws_col: columns,
            ws_xpixel: 0,
            ws_ypixel: 0,
        };
        let pair = openpty(Some(&size), None)?;
        let master = File::from(pair.master);
        let slave = File::from(pair.slave);
        fcntl(&master, FcntlArg::F_SETFL(OFlag::O_NONBLOCK))?;
        for file in [&master, &slave] {
            fcntl(file, FcntlArg::F_SETFD(nix::fcntl::FdFlag::FD_CLOEXEC))?;
        }
        command
            .stdin(Stdio::from(slave.try_clone()?))
            .stdout(Stdio::from(slave.try_clone()?))
            .stderr(Stdio::from(slave.try_clone()?));
        // SAFETY: only async-signal-safe setsid/ioctl operations run after fork.
        // Command has attached the live slave to stdin before this callback.
        unsafe {
            command.pre_exec(|| {
                setsid().map_err(io::Error::from)?;
                set_controlling_terminal(libc::STDIN_FILENO, 0).map_err(io::Error::from)?;
                Ok(())
            });
        }
        let child = command.spawn()?;
        Ok(Self {
            child,
            master,
            output: Vec::new(),
            closed: false,
        })
    }

    pub fn mark(&self) -> usize {
        self.output.len()
    }

    pub fn output_since(&self, mark: usize) -> &[u8] {
        &self.output[mark..]
    }

    pub fn send(&mut self, mut bytes: &[u8]) -> Result<(), Error> {
        let deadline = Instant::now() + TIMEOUT;
        while !bytes.is_empty() {
            if Instant::now() >= deadline {
                return Err(Error::Timeout("terminal input to be written"));
            }
            match self.master.write(bytes) {
                Ok(0) => return Err(io::Error::from(io::ErrorKind::WriteZero).into()),
                Ok(count) => bytes = &bytes[count..],
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                    let mut fds = [PollFd::new(self.master.as_fd(), PollFlags::POLLOUT)];
                    match poll(&mut fds, 50u16) {
                        Ok(_) | Err(Errno::EINTR) => {}
                        Err(error) => return Err(error.into()),
                    }
                }
                Err(error) => return Err(error.into()),
            }
        }
        Ok(())
    }

    pub fn pump(&mut self, duration: Duration) -> Result<(), Error> {
        let deadline = Instant::now() + duration;
        while !self.closed && Instant::now() < deadline {
            let remaining = deadline.saturating_duration_since(Instant::now());
            let timeout = PollTimeout::try_from(remaining.min(Duration::from_millis(50)))
                .expect("poll interval is bounded to 50 milliseconds");
            let mut fds = [PollFd::new(self.master.as_fd(), PollFlags::POLLIN)];
            match poll(&mut fds, timeout) {
                Ok(0) | Err(Errno::EINTR) => continue,
                Ok(_) => {}
                Err(error) => return Err(error.into()),
            }
            let mut buffer = [0; 65536];
            match self.master.read(&mut buffer) {
                Ok(0) => self.closed = true,
                Ok(count) => {
                    if self.output.len() + count > MAX_OUTPUT {
                        return Err(Error::OutputLimit);
                    }
                    self.output.extend_from_slice(&buffer[..count]);
                }
                Err(error) if error.raw_os_error() == Some(libc::EIO) => self.closed = true,
                Err(error)
                    if matches!(
                        error.kind(),
                        io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
                    ) => {}
                Err(error) => return Err(error.into()),
            }
        }
        Ok(())
    }

    pub fn wait_for(
        &mut self,
        bytes: &[u8],
        mark: usize,
        expected: &'static str,
    ) -> Result<(), Error> {
        let deadline = Instant::now() + TIMEOUT;
        loop {
            if self
                .output_since(mark)
                .windows(bytes.len())
                .any(|w| w == bytes)
            {
                return Ok(());
            }
            if self.closed || self.child.try_wait()?.is_some() {
                // Drain bytes buffered immediately before process exit.
                self.pump(Duration::from_millis(50))?;
                if self
                    .output_since(mark)
                    .windows(bytes.len())
                    .any(|w| w == bytes)
                {
                    return Ok(());
                }
                return Err(Error::Closed {
                    expected,
                    status: self.child.try_wait()?,
                });
            }
            if Instant::now() >= deadline {
                return Err(Error::Timeout(expected));
            }
            self.pump(Duration::from_millis(50))?;
        }
    }

    pub fn resize(&mut self, columns: u16, rows: u16) -> Result<(), Error> {
        let size = Winsize {
            ws_row: rows,
            ws_col: columns,
            ws_xpixel: 0,
            ws_ypixel: 0,
        };
        // SAFETY: the master FD is live and size points to a valid winsize for
        // the duration of this TIOCSWINSZ call; the kernel does not retain it.
        unsafe { set_window_size(self.master.as_raw_fd(), &size) }?;
        kill(Pid::from_raw(self.child.id() as i32), Signal::SIGWINCH)?;
        Ok(())
    }

    pub fn wait_for_exit(&mut self) -> Result<ExitStatus, Error> {
        let deadline = Instant::now() + TIMEOUT;
        loop {
            if let Some(status) = self.child.try_wait()? {
                self.pump(Duration::from_millis(50))?;
                return Ok(status);
            }
            if Instant::now() >= deadline {
                return Err(Error::Timeout("CLI exit"));
            }
            if self.closed {
                // Closing the PTY can precede process exit. Avoid spinning
                // while allowing the child its bounded shutdown interval.
                std::thread::sleep(Duration::from_millis(50));
            } else {
                self.pump(Duration::from_millis(50))?;
            }
        }
    }
}

impl Drop for Terminal {
    fn drop(&mut self) {
        // Cleanup must also run during assertion unwinding and readiness errors.
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn early_child_exit_is_reported_without_a_readiness_timeout() {
        let mut command = Command::new(env!("CARGO_BIN_EXE_nouli"));
        command.env_clear().arg("--version");
        let mut terminal = Terminal::spawn(command, 80, 24).unwrap();
        let error = terminal.wait_for(b"Ready", 0, "TUI readiness").unwrap_err();
        assert!(matches!(error, Error::Closed { .. }));
    }
}
