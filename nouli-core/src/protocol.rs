//! Versioned public reports and logical event envelopes, separate from tracing.

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{
    data::Data,
    decision::CONTRACT_VERSION,
    history::{EvaluationManifest, SourcePoint},
    identity::Id,
    outcome::TurnOutcome,
    workflow::{NodeStatus, RunStatus, reference::ReferenceValue},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCategory {
    Input,
    Configuration,
    Authentication,
    Capability,
    Contract,
    Capacity,
    Transport,
    Storage,
    Conflict,
    Interrupted,
    Internal,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ErrorReport {
    pub category: ErrorCategory,
    pub code: String,
    /// Safe diagnostic text, never request/response bodies or credentials.
    pub message: String,
    pub retryable: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TurnReport {
    pub session: Option<Id>,
    pub turn: Option<Id>,
    pub outcome: TurnOutcome,
    pub manifest: EvaluationManifest,
    pub replayed: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NodeReport {
    pub node: String,
    pub status: NodeStatus,
    pub turn: Option<Id>,
    pub outcome: Option<TurnOutcome>,
    pub error: Option<ErrorReport>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectedOutput {
    pub name: String,
    pub value: ReferenceValue,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunReport {
    pub run: Id,
    pub session: Option<Id>,
    pub status: RunStatus,
    pub sources: Vec<SourcePoint>,
    pub frontier: Vec<Id>,
    pub nodes: Vec<NodeReport>,
    pub outputs: Vec<ProjectedOutput>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    content = "report",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum Report {
    Turn(Box<TurnReport>),
    Run(RunReport),
    Error(ErrorReport),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResultEnvelope {
    pub version: u32,
    pub result: Report,
}

impl ResultEnvelope {
    pub fn new(result: Report) -> Self {
        Self {
            version: CONTRACT_VERSION,
            result,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum EventKind {
    Started,
    NodeState {
        node: String,
        status: NodeStatus,
        turn: Option<Id>,
    },
    Attempt {
        node: Option<String>,
        attempt: u32,
        error: Option<ErrorCategory>,
    },
    /// Safe structural metadata only, separate from serialized model context.
    Metadata {
        data: Data,
    },
    Terminal {
        result: Box<ResultEnvelope>,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EngineEvent {
    pub version: u32,
    pub sequence: u64,
    pub run: Option<Id>,
    pub event: EventKind,
}

#[derive(Debug, Error, PartialEq)]
pub enum EventError {
    #[error(
        "event stream must use v1, consecutive sequence numbers, one run, and exactly one final terminal event"
    )]
    InvalidStream,
}

pub fn validate_events(events: &[EngineEvent]) -> Result<(), EventError> {
    if events.is_empty() {
        return Err(EventError::InvalidStream);
    }
    let run = events[0].run;
    for (index, event) in events.iter().enumerate() {
        if event.version != CONTRACT_VERSION
            || event.sequence != index as u64
            || event.run != run
            || matches!(event.event, EventKind::Terminal { .. }) != (index == events.len() - 1)
        {
            return Err(EventError::InvalidStream);
        }
        if let EventKind::Terminal { result } = &event.event {
            if result.version != CONTRACT_VERSION {
                return Err(EventError::InvalidStream);
            }
            match &result.result {
                Report::Run(report) if Some(report.run) != run => {
                    return Err(EventError::InvalidStream);
                }
                Report::Turn(_) if run.is_some() => return Err(EventError::InvalidStream),
                _ => {}
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn partial_failure_refused_sibling_and_frozen_resume_reports_decode() {
        let events: Vec<EngineEvent> =
            serde_json::from_str(include_str!("../../fixtures/contracts/events.json")).unwrap();
        validate_events(&events).unwrap();
        let EventKind::Terminal { result } = &events.last().unwrap().event else {
            panic!()
        };
        let Report::Run(report) = &result.result else {
            panic!()
        };
        assert_eq!(report.status, RunStatus::Partial);
        assert!(report.nodes.iter().any(|n| n.status == NodeStatus::Failed));
        assert!(
            report
                .nodes
                .iter()
                .any(|n| n.outcome.as_ref().is_some_and(|o| o.outcomes.len() == 4))
        );
        assert_eq!(
            events,
            serde_json::from_str::<Vec<EngineEvent>>(&serde_json::to_string(&events).unwrap())
                .unwrap()
        );
    }

    #[test]
    fn rejects_out_of_order_duplicate_or_missing_terminal_events() {
        let mut events: Vec<EngineEvent> =
            serde_json::from_str(include_str!("../../fixtures/contracts/events.json")).unwrap();
        events[1].sequence = 0;
        assert_eq!(validate_events(&events), Err(EventError::InvalidStream));
        events[1].sequence = 1;
        events.push(events.last().unwrap().clone());
        assert_eq!(validate_events(&events), Err(EventError::InvalidStream));
        events.last_mut().unwrap().sequence = 3;
        assert_eq!(validate_events(&events), Err(EventError::InvalidStream));
        events.truncate(1);
        assert_eq!(validate_events(&events), Err(EventError::InvalidStream));
    }

    #[test]
    fn terminal_report_must_belong_to_the_event_stream_run() {
        let mut events: Vec<EngineEvent> =
            serde_json::from_str(include_str!("../../fixtures/contracts/events.json")).unwrap();
        let EventKind::Terminal { result } = &mut events.last_mut().unwrap().event else {
            panic!()
        };
        let Report::Run(report) = &mut result.result else {
            panic!()
        };
        report.run = Id::generate();
        assert_eq!(validate_events(&events), Err(EventError::InvalidStream));
    }
}
