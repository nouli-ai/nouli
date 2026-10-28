//! Fixed source points and immutable finalized decision history.

pub mod order;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{
    context::{ContextError, ContextUpdate},
    decision::{ContractError, TurnRequest},
    identity::{Id, SessionName, SessionSelector},
    outcome::{ProviderIdentity, TurnOutcome},
};

/// A captured ordered frontier of finalized turns, never a moving session head.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourcePoint {
    pub session: Id,
    pub frontier: Vec<Id>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceSelection {
    pub session: SessionSelector,
    pub frontier: Option<Vec<Id>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case", deny_unknown_fields)]
pub enum SessionSelection {
    New {
        name: Option<SessionName>,
    },
    Continue {
        session: SessionSelector,
    },
    /// One source branches, multiple sources blend. Sources are captured once.
    From {
        sources: Vec<SourceSelection>,
        name: Option<SessionName>,
    },
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelSelection {
    pub provider: Option<String>,
    pub model: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EvaluationRequest {
    pub turn: TurnRequest,
    pub session: SessionSelection,
    #[serde(default)]
    pub updates: Vec<ContextUpdate>,
    #[serde(default)]
    pub required_records: Vec<Id>,
    #[serde(default)]
    pub model: ModelSelection,
    pub request_id: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Representation {
    Full,
    Compacted,
    Omitted,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecordProjection {
    pub record: Id,
    pub representation: Representation,
    pub protected: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TokenCount {
    pub tokens: u64,
    pub exact: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EvaluationManifest {
    pub sources: Vec<SourcePoint>,
    pub records: Vec<RecordProjection>,
    pub provider: ProviderIdentity,
    pub budget: u64,
    pub input_count: TokenCount,
    pub reserved_output_tokens: u64,
    /// The exact serialized model context sent to the provider.
    pub context_toon: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FinalizedTurn {
    pub id: Id,
    pub session: Id,
    /// Stable caller-source or workflow declaration order for unrelated parents.
    pub parents: Vec<Id>,
    pub request: TurnRequest,
    pub outcome: TurnOutcome,
    pub updates: Vec<ContextUpdate>,
    pub manifest: EvaluationManifest,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AttemptState {
    Pending,
    Running,
    Failed,
    Interrupted,
}

/// Attempts are separate from finalized turns and cannot appear in source heads.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AttemptRecord {
    pub id: Id,
    pub session: Id,
    pub state: AttemptState,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum HistoryEvent {
    SessionCreated {
        session: Id,
        sources: Vec<SourcePoint>,
    },
    AttemptRecorded {
        attempt: AttemptRecord,
    },
    TurnFinalized {
        session: Id,
        turn: Id,
        frontier: Vec<Id>,
    },
}

#[derive(Debug, Error, PartialEq)]
pub enum HistoryError {
    #[error("source selector does not exist")]
    UnknownSession,
    #[error("source turn is not finalized or does not exist")]
    UnknownTurn,
    #[error("source head belongs to another session")]
    ForeignHead,
    #[error("source frontier must contain distinct finalized turns")]
    InvalidFrontier,
    #[error("duplicate turn identity")]
    DuplicateTurn,
    #[error("history contains a causal cycle")]
    Cycle,
    #[error("history manifest is inconsistent")]
    InvalidManifest,
    #[error("invalid evaluation selection or request metadata")]
    InvalidSelection,
    #[error(transparent)]
    Decision(#[from] ContractError),
    #[error(transparent)]
    Context(#[from] ContextError),
}

impl EvaluationRequest {
    pub fn validate(&self) -> Result<(), HistoryError> {
        self.turn.validate()?;
        for update in &self.updates {
            update.validate()?;
        }
        if self
            .request_id
            .as_ref()
            .is_some_and(|id| id.trim().is_empty())
            || self
                .model
                .provider
                .as_ref()
                .is_some_and(|id| id.trim().is_empty())
            || self
                .model
                .model
                .as_ref()
                .is_some_and(|id| id.trim().is_empty())
            || self
                .required_records
                .iter()
                .collect::<std::collections::HashSet<_>>()
                .len()
                != self.required_records.len()
        {
            return Err(HistoryError::InvalidSelection);
        }
        if let SessionSelection::From { sources, .. } = &self.session {
            if sources.is_empty() {
                return Err(HistoryError::InvalidSelection);
            }
            for source in sources {
                if let Some(frontier) = &source.frontier
                    && (frontier.is_empty()
                        || frontier
                            .iter()
                            .collect::<std::collections::HashSet<_>>()
                            .len()
                            != frontier.len())
                {
                    return Err(HistoryError::InvalidFrontier);
                }
            }
        }
        Ok(())
    }
}

impl FinalizedTurn {
    pub fn validate(&self) -> Result<(), HistoryError> {
        self.outcome.validate(&self.request)?;
        for update in &self.updates {
            update.validate()?;
        }
        let mut ids = std::collections::HashSet::new();
        for update in &self.updates {
            if let Some(record) = update.new_record()
                && !ids.insert(record.id)
            {
                return Err(ContextError::DuplicateRecord.into());
            }
        }
        let mut projections = std::collections::HashSet::new();
        if self.manifest.provider != self.outcome.provider
            || self.manifest.budget == 0
            || self
                .manifest
                .input_count
                .tokens
                .checked_add(self.manifest.reserved_output_tokens)
                .is_none_or(|total| total > self.manifest.budget)
            || self.manifest.records.iter().any(|r| {
                !projections.insert(r.record)
                    || (r.protected && r.representation == Representation::Omitted)
            })
        {
            return Err(HistoryError::InvalidManifest);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn turn() -> FinalizedTurn {
        let fixture: serde_json::Value =
            serde_json::from_str(include_str!("../../fixtures/contracts/history.json")).unwrap();
        serde_json::from_value(fixture["turns"][0].clone()).unwrap()
    }

    #[test]
    fn rejects_inconsistent_provider_and_budget_manifests() {
        let original = turn();
        original.validate().unwrap();
        let mut changed = original.clone();
        changed.manifest.provider.model = "another".into();
        assert_eq!(changed.validate(), Err(HistoryError::InvalidManifest));
        for (budget, input, output) in [(0, 0, 0), (10, 8, 3), (u64::MAX, u64::MAX, 1)] {
            let mut changed = original.clone();
            changed.manifest.budget = budget;
            changed.manifest.input_count.tokens = input;
            changed.manifest.reserved_output_tokens = output;
            assert_eq!(changed.validate(), Err(HistoryError::InvalidManifest));
        }
    }

    #[test]
    fn rejects_duplicate_records_projections_and_omitted_protection() {
        let mut changed = turn();
        changed.updates.push(changed.updates[0].clone());
        assert_eq!(
            changed.validate(),
            Err(ContextError::DuplicateRecord.into())
        );
        let projection = RecordProjection {
            record: Id::generate(),
            representation: Representation::Full,
            protected: true,
        };
        let mut changed = turn();
        changed.manifest.records = vec![projection.clone(), projection.clone()];
        assert_eq!(changed.validate(), Err(HistoryError::InvalidManifest));
        changed.manifest.records = vec![RecordProjection {
            representation: Representation::Omitted,
            ..projection
        }];
        assert_eq!(changed.validate(), Err(HistoryError::InvalidManifest));
    }

    #[test]
    fn evaluation_metadata_rejects_empty_sources_ids_and_duplicate_requirements() {
        let mut request = EvaluationRequest {
            turn: turn().request,
            session: SessionSelection::From {
                sources: vec![],
                name: None,
            },
            updates: vec![],
            required_records: vec![],
            model: ModelSelection::default(),
            request_id: None,
        };
        assert_eq!(request.validate(), Err(HistoryError::InvalidSelection));
        request.session = SessionSelection::New { name: None };
        request.validate().unwrap();
        request.request_id = Some(" ".into());
        assert_eq!(request.validate(), Err(HistoryError::InvalidSelection));
        request.request_id = None;
        let id = Id::generate();
        request.required_records = vec![id, id];
        assert_eq!(request.validate(), Err(HistoryError::InvalidSelection));
    }
}
