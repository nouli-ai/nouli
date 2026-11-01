use serde_json::Value;
use std::{
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread::JoinHandle,
    time::Duration,
};
pub struct Server {
    pub url: String,
    pub calls: Arc<Mutex<Vec<Value>>>,
    pub authorizations: Arc<Mutex<Vec<Option<String>>>>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}
impl Server {
    pub fn new(response: &'static str) -> Self {
        Self::with_handler(move |_| (200, response.to_owned()))
    }
    pub fn with_handler(handler: impl Fn(&Value) -> (u16, String) + Send + 'static) -> Self {
        Self::bind("127.0.0.1:0", handler).unwrap()
    }
    /// A server on a chosen address, for a ledger that names its provider's address.
    pub fn bind(
        address: &str,
        handler: impl Fn(&Value) -> (u16, String) + Send + 'static,
    ) -> std::io::Result<Self> {
        let listener = TcpListener::bind(address)?;
        let address = listener.local_addr().unwrap();
        let calls = Arc::new(Mutex::new(vec![]));
        let captured = calls.clone();
        let authorizations = Arc::new(Mutex::new(vec![]));
        let captured_authorizations = authorizations.clone();
        let stop = Arc::new(AtomicBool::new(false));
        let stopping = stop.clone();
        let thread = std::thread::spawn(move || {
            loop {
                let (mut stream, _) = listener.accept().unwrap();
                if stopping.load(Ordering::SeqCst) {
                    break;
                }
                stream
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                let mut bytes = vec![];
                let mut buffer = [0u8; 4096];
                let end = loop {
                    let count = stream.read(&mut buffer).unwrap();
                    assert!(count > 0);
                    bytes.extend_from_slice(&buffer[..count]);
                    if let Some(end) = bytes.windows(4).position(|part| part == b"\r\n\r\n") {
                        break end + 4;
                    }
                    assert!(bytes.len() < 64 * 1024);
                };
                let header = std::str::from_utf8(&bytes[..end]).unwrap();
                let path = header.split(' ').nth(1).unwrap_or("").to_owned();
                captured_authorizations
                    .lock()
                    .unwrap()
                    .push(header.lines().find_map(|line| {
                        line.split_once(':')
                            .filter(|(name, _)| name.eq_ignore_ascii_case("authorization"))
                            .map(|(_, value)| value.trim().to_owned())
                    }));
                let length = header
                    .lines()
                    .find_map(|line| {
                        line.split_once(':')
                            .filter(|(name, _)| name.eq_ignore_ascii_case("content-length"))
                            .map(|(_, value)| value.trim().parse::<usize>().unwrap())
                    })
                    .unwrap_or(0);
                assert!(length < 16 * 1024 * 1024);
                while bytes.len() < end + length {
                    let count = stream.read(&mut buffer).unwrap();
                    assert!(count > 0);
                    bytes.extend_from_slice(&buffer[..count]);
                }
                // A GET has no body, so the handler gets its path instead.
                let body: Value = if length == 0 {
                    serde_json::json!({ "get": path })
                } else {
                    serde_json::from_slice(&bytes[end..end + length]).unwrap()
                };
                captured.lock().unwrap().push(body.clone());
                let (status, response) = handler(&body);
                if let Err(error) = write!(
                    stream,
                    "HTTP/1.1 {status} Fixture\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    response.len(),
                    response
                ) {
                    assert!(matches!(
                        error.kind(),
                        std::io::ErrorKind::BrokenPipe | std::io::ErrorKind::ConnectionReset
                    ));
                }
            }
        });
        Ok(Self {
            url: format!("http://{address}"),
            calls,
            authorizations,
            stop,
            thread: Some(thread),
        })
    }
    pub fn count(&self) -> usize {
        self.calls.lock().unwrap().len()
    }
}
impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        let _ = TcpStream::connect(self.url.trim_start_matches("http://"));
        if let Some(thread) = self.thread.take() {
            let result = thread.join();
            if !std::thread::panicking() {
                result.unwrap();
            }
        }
    }
}
