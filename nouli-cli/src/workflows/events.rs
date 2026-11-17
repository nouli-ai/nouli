//! Ordered JSONL publication with owned, cancellation-aware platform output.
#[cfg(unix)]
mod unix;
#[cfg(windows)]
mod windows;
use nouli_core::{
    codec,
    protocol::{EngineEvent, ErrorCategory, ErrorReport, stream::EventSink},
};
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;
#[cfg(unix)]
use unix::Destination;
#[cfg(windows)]
use windows::Destination;

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
