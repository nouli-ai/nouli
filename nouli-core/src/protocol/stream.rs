//! Ordered logical events, independent of diagnostic logging or process policy.
use super::{EngineEvent, ErrorCategory, ErrorReport, EventKind, Report, ResultEnvelope};
use crate::{
    decision::CONTRACT_VERSION,
    identity::Id,
    workflow::schedule::{NodeProgress, ScheduleObserver},
};
use std::{future::Future, sync::Mutex};
use thiserror::Error;

pub trait EventSink: Send + Sync + 'static {
    fn send(&self, event: EngineEvent) -> impl Future<Output = Result<(), ErrorReport>> + Send;
}
#[derive(Default)]
pub struct CollectEvents(Mutex<Vec<EngineEvent>>);
impl CollectEvents {
    pub fn snapshot(&self) -> Result<Vec<EngineEvent>, StreamError> {
        self.0
            .lock()
            .map(|events| events.clone())
            .map_err(|_| StreamError::State)
    }
}
impl EventSink for CollectEvents {
    async fn send(&self, event: EngineEvent) -> Result<(), ErrorReport> {
        self.0.lock().map_err(|_| state_report())?.push(event);
        Ok(())
    }
}
#[derive(Debug, Error)]
pub enum StreamError {
    #[error("event stream state is inconsistent")]
    State,
    #[error("event sink failed (saved run: {run:?})")]
    Sink { run: Option<Id>, error: ErrorReport },
}
fn state_report() -> ErrorReport {
    ErrorReport {
        category: ErrorCategory::Internal,
        code: "event_stream_state".into(),
        message: "Event stream state is inconsistent".into(),
        retryable: false,
    }
}
#[derive(Default)]
struct State {
    sequence: u64,
    run: Option<Id>,
    started: bool,
    terminal: bool,
    failure: Option<ErrorReport>,
}
/// Serializes event assignment and sink delivery. A terminal send is attempted
/// once even if the sink fails; callers must report I/O failure separately.
pub struct EventStream<S> {
    sink: std::sync::Arc<S>,
    nodes: Vec<String>,
    state: tokio::sync::Mutex<State>,
}
impl<S: EventSink> EventStream<S> {
    pub fn new(sink: std::sync::Arc<S>, nodes: Vec<String>) -> Self {
        Self {
            sink,
            nodes,
            state: tokio::sync::Mutex::new(State::default()),
        }
    }
    async fn emit(&self, state: &mut State, event: EventKind) -> Result<(), StreamError> {
        if state.terminal {
            return Err(StreamError::State);
        }
        let terminal = matches!(event, EventKind::Terminal { .. });
        let envelope = EngineEvent {
            version: CONTRACT_VERSION,
            sequence: state.sequence,
            run: state.run,
            event,
        };
        state.sequence = state.sequence.checked_add(1).ok_or(StreamError::State)?;
        state.terminal = terminal;
        match self.sink.send(envelope).await {
            Ok(()) => Ok(()),
            Err(error) => {
                state.failure.get_or_insert_with(|| error.clone());
                Err(StreamError::Sink {
                    run: state.run,
                    error,
                })
            }
        }
    }
    pub(crate) async fn established_run(&self) -> Option<Id> {
        self.state.lock().await.run
    }
    pub async fn finish(&self, result: ResultEnvelope) -> Result<(), StreamError> {
        let mut state = self.state.lock().await;
        let run = if state.started {
            state.run
        } else {
            match &result.result {
                Report::Run(report) => report.run,
                _ => result.recovery.as_ref().and_then(|reference| reference.run),
            }
        };
        if !super::terminal_matches(&result, run) {
            return Err(StreamError::State);
        }
        if !state.started {
            state.run = run;
            state.started = true;
            // Sink failure is retained in state; still attempt one terminal delivery.
            if let Err(error) = self.emit(&mut state, EventKind::Started).await
                && !matches!(error, StreamError::Sink { .. })
            {
                return Err(error);
            }
        }
        self.emit(
            &mut state,
            EventKind::Terminal {
                result: Box::new(result),
            },
        )
        .await?;
        if let Some(error) = &state.failure {
            return Err(StreamError::Sink {
                run: state.run,
                error: error.clone(),
            });
        }
        Ok(())
    }
}
impl<S: EventSink> ScheduleObserver for EventStream<S> {
    async fn started(&self, run: Option<Id>) -> Result<(), ErrorReport> {
        let mut state = self.state.lock().await;
        if state.started {
            return Err(state_report());
        }
        state.started = true;
        state.run = run;
        self.emit(&mut state, EventKind::Started)
            .await
            .map_err(|error| match error {
                StreamError::Sink { error, .. } => error,
                StreamError::State => state_report(),
            })
    }
    async fn changed(&self, node: usize, progress: NodeProgress) -> Result<(), ErrorReport> {
        let node = self.nodes.get(node).ok_or_else(state_report)?.clone();
        let turn = progress
            .output
            .as_ref()
            .and_then(|output| output.source.as_ref())
            .and_then(|source| source.frontier.first())
            .copied();
        let mut state = self.state.lock().await;
        if !state.started {
            return Err(state_report());
        }
        self.emit(
            &mut state,
            EventKind::NodeState {
                node,
                status: progress.status,
                turn,
            },
        )
        .await
        .map_err(|error| match error {
            StreamError::Sink { error, .. } => error,
            StreamError::State => state_report(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{protocol::validate_events, workflow::NodeStatus};
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    fn result() -> ResultEnvelope {
        ResultEnvelope::new(Report::Error(ErrorReport {
            category: ErrorCategory::Input,
            code: "invalid_input".into(),
            message: "invalid input".into(),
            retryable: false,
        }))
    }
    fn progress() -> NodeProgress {
        NodeProgress {
            status: NodeStatus::Skipped,
            output: None,
            error: None,
            ancestors: vec![],
            inputs: vec![],
        }
    }
    #[tokio::test]
    async fn concurrent_observers_serialize_delivery_and_reject_a_second_terminal() {
        let sink = Arc::new(CollectEvents::default());
        let stream = Arc::new(EventStream::new(sink.clone(), vec!["a".into(), "b".into()]));
        stream.started(None).await.unwrap();
        let first = stream.clone();
        let second = stream.clone();
        let (a, b) = tokio::join!(first.changed(0, progress()), second.changed(1, progress()));
        a.unwrap();
        b.unwrap();
        let terminal = result();
        stream.finish(terminal.clone()).await.unwrap();
        validate_events(&sink.snapshot().unwrap()).unwrap();
        assert_eq!(sink.snapshot().unwrap().len(), 4);
        assert!(stream.finish(terminal).await.is_err());
        assert_eq!(sink.snapshot().unwrap().len(), 4);
    }
    struct FailsFirst {
        calls: AtomicUsize,
        terminal: AtomicUsize,
    }
    impl EventSink for FailsFirst {
        async fn send(&self, event: EngineEvent) -> Result<(), ErrorReport> {
            if matches!(event.event, EventKind::Terminal { .. }) {
                self.terminal.fetch_add(1, Ordering::SeqCst);
            }
            if self.calls.fetch_add(1, Ordering::SeqCst) == 0 {
                Err(state_report())
            } else {
                Ok(())
            }
        }
    }
    #[tokio::test]
    async fn a_failed_sink_is_not_reported_as_a_successful_stream() {
        let sink = Arc::new(FailsFirst {
            calls: AtomicUsize::new(0),
            terminal: AtomicUsize::new(0),
        });
        let stream = EventStream::new(sink.clone(), vec![]);
        assert!(stream.started(None).await.is_err());
        assert!(matches!(
            stream.finish(result()).await,
            Err(StreamError::Sink { .. })
        ));
        assert_eq!(sink.terminal.load(Ordering::SeqCst), 1);
        assert!(stream.finish(result()).await.is_err());
        assert_eq!(sink.terminal.load(Ordering::SeqCst), 1);
    }
    #[tokio::test]
    async fn invalid_terminal_identities_never_emit_a_terminal_or_auto_started_event() {
        use crate::protocol::{InspectionReference, TurnReport};
        let sink = Arc::new(CollectEvents::default());
        let stream = EventStream::new(sink.clone(), vec![]);
        let run = Id::generate();
        stream.started(Some(run)).await.unwrap();
        let history: serde_json::Value =
            serde_json::from_str(include_str!("../../../fixtures/contracts/history.json")).unwrap();
        let turn: crate::history::FinalizedTurn =
            serde_json::from_value(history["turns"][0].clone()).unwrap();
        let mut turn_result = ResultEnvelope::new(Report::Turn(Box::new(TurnReport {
            questions: turn.request.questions,
            session: Some(turn.session),
            turn: Some(turn.id),
            outcome: turn.outcome,
            manifest: turn.manifest,
            replayed: false,
        })));
        assert!(matches!(
            stream.finish(turn_result.clone()).await,
            Err(StreamError::State)
        ));
        assert_eq!(sink.snapshot().unwrap().len(), 1);
        let mut error = result();
        error.recovery = Some(InspectionReference {
            run: Some(Id::generate()),
            session: None,
            node: None,
            turn: None,
            attempts: vec![],
        });
        assert!(matches!(
            stream.finish(error).await,
            Err(StreamError::State)
        ));
        assert_eq!(sink.snapshot().unwrap().len(), 1);
        let fixture: Vec<EngineEvent> =
            serde_json::from_str(include_str!("../../../fixtures/contracts/events.json")).unwrap();
        let EventKind::Terminal { result: mismatched } = &fixture.last().unwrap().event else {
            panic!("terminal")
        };
        assert!(matches!(
            stream.finish(*mismatched.clone()).await,
            Err(StreamError::State)
        ));
        assert_eq!(sink.snapshot().unwrap().len(), 1);
        stream.finish(result()).await.unwrap();
        validate_events(&sink.snapshot().unwrap()).unwrap();
        let fresh = Arc::new(CollectEvents::default());
        let unopened = EventStream::new(fresh.clone(), vec![]);
        turn_result.recovery = Some(InspectionReference {
            run: Some(run),
            session: None,
            node: None,
            turn: None,
            attempts: vec![],
        });
        assert!(matches!(
            unopened.finish(turn_result).await,
            Err(StreamError::State)
        ));
        assert_eq!(
            fresh.snapshot().unwrap(),
            [] as [crate::protocol::EngineEvent; 0]
        );
    }
}
