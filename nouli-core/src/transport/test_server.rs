//! Test-only, one-request HTTP fixture server with owned task cleanup.

use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
    sync::oneshot,
    task::JoinHandle,
};

pub(crate) struct MockServer {
    pub url: String,
    request: Option<oneshot::Receiver<Vec<u8>>>,
    response: Option<oneshot::Sender<Vec<u8>>>,
    task: Option<JoinHandle<std::io::Result<()>>>,
}

impl MockServer {
    pub async fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let (request_tx, request_rx) = oneshot::channel();
        let (response_tx, response_rx) = oneshot::channel::<Vec<u8>>();
        let task = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await?;
            let mut request = Vec::new();
            loop {
                let mut buffer = [0_u8; 4096];
                let read = socket.read(&mut buffer).await?;
                if read == 0 {
                    break;
                }
                request.extend_from_slice(&buffer[..read]);
                assert!(
                    request.len() < 1024 * 1024,
                    "test request unexpectedly large"
                );
                if let Some(header_end) = request.windows(4).position(|bytes| bytes == b"\r\n\r\n")
                {
                    let headers = String::from_utf8_lossy(&request[..header_end]);
                    let length = headers
                        .lines()
                        .find_map(|line| {
                            line.to_ascii_lowercase()
                                .strip_prefix("content-length:")
                                .map(str::trim)
                                .map(str::parse::<usize>)
                        })
                        .transpose()
                        .unwrap()
                        .unwrap_or(0);
                    if request.len() >= header_end + 4 + length {
                        break;
                    }
                }
            }
            assert!(
                request_tx.send(request).is_ok(),
                "test request receiver was dropped"
            );
            if let Ok(response) = response_rx.await {
                socket.write_all(&response).await?;
            }
            socket.shutdown().await
        });
        Self {
            url,
            request: Some(request_rx),
            response: Some(response_tx),
            task: Some(task),
        }
    }

    pub async fn received(&mut self) -> Vec<u8> {
        self.request.take().unwrap().await.unwrap()
    }
    pub fn respond(&mut self, response: Vec<u8>) {
        self.response.take().unwrap().send(response).unwrap();
    }
    pub async fn finish(mut self) {
        self.response.take();
        self.task.take().unwrap().await.unwrap().unwrap();
    }
}

impl Drop for MockServer {
    fn drop(&mut self) {
        if let Some(task) = &self.task {
            task.abort();
        }
    }
}
