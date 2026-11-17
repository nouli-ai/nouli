//! Real `ConPTY` sessions. Reader/writer threads are owned and joined on cleanup;
//! the reader keeps draining after capture overflow so closing `ConPTY` is safe.
use super::fixture;
use portable_pty::{Child, CommandBuilder, ExitStatus, MasterPty, PtySize, native_pty_system};
use std::{
    io::{self, Read, Write},
    process::Command,
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};
use thiserror::Error;

const TIMEOUT: Duration = Duration::from_secs(10);
const MAX_OUTPUT: usize = 1024 * 1024;

#[derive(Debug, Error)]
pub enum Error {
    #[error("ConPTY I/O failed: {0}")]
    Io(#[from] io::Error),
    #[error("ConPTY operation failed: {0}")]
    Native(String),
    #[error("timed out waiting for {0}")]
    Timeout(&'static str),
    #[error("timed out waiting for {expected}; captured terminal bytes: {output:?}")]
    ScreenTimeout {
        expected: &'static str,
        output: String,
    },
    #[error(
        "terminal closed before {expected}; child status: {status:?}; captured terminal bytes: {output:?}"
    )]
    Closed {
        expected: &'static str,
        status: Option<ExitStatus>,
        output: String,
    },
    #[error("terminal output exceeded the {MAX_OUTPUT}-byte test limit")]
    OutputLimit,
}

enum Captured {
    Bytes(Vec<u8>),
    Closed,
    Overflow,
}
enum WriteRequest {
    Bytes(Vec<u8>, mpsc::Sender<io::Result<()>>),
    Close,
}
pub struct Terminal {
    child: Box<dyn Child + Send + Sync>,
    master: Option<Box<dyn MasterPty + Send>>,
    input: Option<mpsc::Sender<WriteRequest>>,
    captures: mpsc::Receiver<Captured>,
    reader: Option<thread::JoinHandle<()>>,
    writer: Option<thread::JoinHandle<()>>,
    output: Vec<u8>,
    closed: bool,
}

impl Terminal {
    #[allow(
        clippy::needless_pass_by_value,
        reason = "matches the Unix harness API, which consumes Command"
    )]
    pub fn spawn(command: Command, columns: u16, rows: u16) -> Result<Self, Error> {
        let pair = native_pty_system()
            .openpty(PtySize {
                rows,
                cols: columns,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|error| Error::Native(error.to_string()))?;
        let builder = command_builder(&command);
        let child = pair
            .slave
            .spawn_command(builder)
            .map_err(|error| Error::Native(error.to_string()))?;
        drop(pair.slave);
        let mut reader = pair
            .master
            .try_clone_reader()
            .map_err(|error| Error::Native(error.to_string()))?;
        let mut writer = pair
            .master
            .take_writer()
            .map_err(|error| Error::Native(error.to_string()))?;
        let (input, requests) = mpsc::channel::<WriteRequest>();
        let writer = thread::spawn(move || {
            while let Ok(WriteRequest::Bytes(bytes, done)) = requests.recv() {
                let result = writer.write_all(&bytes);
                let failed = result.is_err();
                let _ = done.send(result);
                if failed {
                    break;
                }
            }
        });
        let responses = input.clone();
        let (capture, captures) = mpsc::channel();
        let reader = thread::spawn(move || {
            let mut query = 0usize;
            let mut total = 0usize;
            let mut overflow = false;
            let mut buffer = [0u8; 16 * 1024];
            loop {
                match reader.read(&mut buffer) {
                    Ok(0) => break,
                    Ok(count) => {
                        // portable-pty requests cursor inheritance. Answer DSR
                        // across chunk boundaries before ConPTY can start drawing.
                        for byte in &buffer[..count] {
                            const REQUEST: &[u8] = b"\x1b[6n";
                            query = if *byte == REQUEST[query] {
                                query + 1
                            } else {
                                usize::from(*byte == REQUEST[0])
                            };
                            if query == REQUEST.len() {
                                let (done, _) = mpsc::channel();
                                let _ = responses
                                    .send(WriteRequest::Bytes(b"\x1b[1;1R".to_vec(), done));
                                query = 0;
                            }
                        }
                        total = total.saturating_add(count);
                        if total > MAX_OUTPUT {
                            if !overflow {
                                let _ = capture.send(Captured::Overflow);
                                overflow = true;
                            }
                        } else {
                            let _ = capture.send(Captured::Bytes(buffer[..count].to_vec()));
                        }
                    }
                    Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                    Err(_) => break,
                }
            }
            let _ = capture.send(Captured::Closed);
        });
        Ok(Self {
            child,
            master: Some(pair.master),
            input: Some(input),
            captures,
            reader: Some(reader),
            writer: Some(writer),
            output: Vec::new(),
            closed: false,
        })
    }
    pub fn mark(&self) -> usize {
        self.output.len()
    }
    pub fn assert_mouse_disabled(&self) {
        use std::os::windows::process::CommandExt;
        let mut command = fixture::command("mouse-disabled");
        command
            .arg(
                self.child
                    .process_id()
                    .expect("live ConPTY child")
                    .to_string(),
            )
            .creation_flags(windows_sys::Win32::System::Threading::CREATE_NO_WINDOW);
        assert!(
            command.status().unwrap().success(),
            "native console mouse capture must be disabled"
        );
    }
    pub fn output_since(&self, mark: usize) -> &[u8] {
        &self.output[mark..]
    }
    fn diagnostic(&self, mark: usize) -> String {
        let bytes = self.output_since(mark);
        String::from_utf8_lossy(&bytes[bytes.len().saturating_sub(2048)..]).into_owned()
    }
    pub fn send(&mut self, bytes: &[u8]) -> Result<(), Error> {
        let (sent, done) = mpsc::channel();
        self.input
            .as_ref()
            .ok_or_else(|| io::Error::from(io::ErrorKind::BrokenPipe))?
            .send(WriteRequest::Bytes(bytes.to_vec(), sent))
            .map_err(|_| io::Error::from(io::ErrorKind::BrokenPipe))?;
        done.recv_timeout(TIMEOUT)
            .map_err(|_| Error::Timeout("terminal input to be written"))??;
        Ok(())
    }
    pub fn pump(&mut self, duration: Duration) -> Result<(), Error> {
        let deadline = Instant::now() + duration;
        while !self.closed && Instant::now() < deadline {
            match self
                .captures
                .recv_timeout(deadline.saturating_duration_since(Instant::now()))
            {
                Ok(Captured::Bytes(bytes)) => self.output.extend_from_slice(&bytes),
                Ok(Captured::Closed) | Err(mpsc::RecvTimeoutError::Disconnected) => {
                    self.closed = true;
                }
                Ok(Captured::Overflow) => return Err(Error::OutputLimit),
                Err(mpsc::RecvTimeoutError::Timeout) => break,
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
                .any(|window| window == bytes)
            {
                return Ok(());
            }
            if self.closed || self.child.try_wait()?.is_some() {
                self.pump(Duration::from_millis(50))?;
                if self
                    .output_since(mark)
                    .windows(bytes.len())
                    .any(|window| window == bytes)
                {
                    return Ok(());
                }
                return Err(Error::Closed {
                    expected,
                    status: self.child.try_wait()?,
                    output: self.diagnostic(mark),
                });
            }
            if Instant::now() >= deadline {
                return Err(Error::ScreenTimeout {
                    expected,
                    output: self.diagnostic(mark),
                });
            }
            self.pump(Duration::from_millis(50))?;
        }
    }
    pub fn resize(&mut self, columns: u16, rows: u16) -> Result<(), Error> {
        self.master
            .as_ref()
            .ok_or_else(|| io::Error::from(io::ErrorKind::BrokenPipe))?
            .resize(PtySize {
                rows,
                cols: columns,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|error| Error::Native(error.to_string()))
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
            self.pump(Duration::from_millis(50))?;
            if self.closed {
                thread::sleep(Duration::from_millis(10));
            }
        }
    }
}
impl Drop for Terminal {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        // Keep the responder alive while ClosePseudoConsole drains output.
        drop(self.master.take());
        if let Some(input) = self.input.take() {
            let _ = input.send(WriteRequest::Close);
        }
        if let Some(writer) = self.writer.take() {
            let _ = writer.join();
        }
        if let Some(reader) = self.reader.take() {
            let _ = reader.join();
        }
    }
}

fn command_builder(command: &Command) -> CommandBuilder {
    let mut builder = CommandBuilder::new(fixture::executable());
    builder.arg("console");
    builder.arg(command.get_program());
    builder.args(command.get_args());
    builder.env_clear();
    if let Some(root) = std::env::var_os("SystemRoot") {
        builder.env("SystemRoot", root);
    }
    for (key, value) in command.get_envs() {
        if let Some(value) = value {
            builder.env(key, value);
        } else {
            builder.env_remove(key);
        }
    }
    if let Some(directory) = command.get_current_dir() {
        builder.cwd(directory);
    }
    builder
}
