//! Fixed source points and immutable finalized decision history.

pub mod base;
pub mod order;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{
    context::{ContextError, ContextUpdate},
    decision::{ContractError, Question, TurnRequest},
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

/// Standing-question changes saved by Nouli 0.2.0, which asked a session's
/// standing questions again on every turn. Nouli no longer asks a question
/// again, so new requests must leave this empty; it stays so that 0.2.0
/// sessions and requests still read.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StandingChanges {
    #[serde(default)]
    pub set: Vec<Question>,
    #[serde(default)]
    pub drop: Vec<String>,
}

impl StandingChanges {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.set.is_empty() && self.drop.is_empty()
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EvaluationRequest {
    pub turn: TurnRequest,
    pub session: SessionSelection,
    #[serde(default, skip_serializing_if = "StandingChanges::is_empty")]
    pub standing: StandingChanges,
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
    #[serde(default)]
    pub method: CountMethod,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CountMethod {
    #[default]
    ByteEstimate,
    MultimodalEstimate,
    TokenizerEstimate,
    VerifiedProtocol,
    ProviderReported,
}

impl TokenCount {
    #[must_use]
    pub fn is_consistent(&self) -> bool {
        self.exact
            == matches!(
                self.method,
                CountMethod::VerifiedProtocol | CountMethod::ProviderReported
            )
    }
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
    /// Canonical evidence used by the provider adapter. Native image payloads
    /// may be prepared from saved originals according to the selected protocol.
    pub context_toon: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FinalizedTurn {
    #[serde(default)]
    pub context_base: Option<base::ContextBase>,
    #[serde(default)]
    pub workflow_run: Option<Id>,
    #[serde(default)]
    pub runtime_only: bool,
    #[serde(default)]
    pub workflow_node: Option<String>,
    pub id: Id,
    pub session: Id,
    /// Stable caller-source or workflow declaration order for unrelated parents.
    pub parents: Vec<Id>,
    pub request: TurnRequest,
    pub outcome: TurnOutcome,
    pub updates: Vec<ContextUpdate>,
    /// Standing-question changes a 0.2.0 turn saved; empty for every other turn.
    #[serde(default, skip_serializing_if = "StandingChanges::is_empty")]
    pub standing: StandingChanges,
    pub manifest: EvaluationManifest,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum HistoryEvent {
    SessionCreated {
        session: Id,
        sources: Vec<SourcePoint>,
    },
    SessionRenamed {
        session: Id,
        previous_name: Option<SessionName>,
        name: SessionName,
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
    #[error("standing questions are no longer supported; list each question in `turn.questions`")]
    Standing,
    #[error(transparent)]
    Decision(#[from] ContractError),
    #[error(transparent)]
    Context(#[from] ContextError),
}

impl EvaluationRequest {
    /// A turn asks exactly its own questions, so it needs at least one.
    ///
    /// # Errors
    ///
    /// - [`HistoryError::Standing`] if the request uses standing questions.
    /// - [`HistoryError::Decision`] if the turn is invalid, or [`HistoryError::Context`]
    ///   if a context update is invalid.
    /// - [`HistoryError::InvalidSelection`] if the request ID, provider, or model is blank,
    ///   or required records repeat.
    /// - Any error from [`SessionSelection::validate`].
    pub fn validate(&self) -> Result<(), HistoryError> {
        // First, so a 0.2.0 request with only standing questions says why.
        if !self.standing.is_empty() {
            return Err(HistoryError::Standing);
        }
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
        self.session.validate()?;
        Ok(())
    }
}

impl SessionSelection {
    /// Check that a source selection names at least one source with a valid frontier.
    ///
    /// # Errors
    ///
    /// - [`HistoryError::InvalidSelection`] if `From` lists no sources.
    /// - [`HistoryError::InvalidFrontier`] if an explicit frontier is empty or repeats a turn.
    pub fn validate(&self) -> Result<(), HistoryError> {
        if let Self::From { sources, .. } = self {
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
    /// Check that a finalized turn is internally consistent.
    ///
    /// # Errors
    ///
    /// - [`HistoryError::InvalidManifest`] if workflow node, run, context base, session, or
    ///   runtime-only flags disagree, or the manifest provider differs from the outcome's.
    /// - Any error from [`ContextBase::validate`](base::ContextBase::validate) or
    ///   [`EvaluationManifest::validate`].
    /// - [`HistoryError::Decision`] if the outcome does not match the request.
    /// - [`HistoryError::Context`] if an update is invalid or two updates create the same record.
    pub fn validate(&self) -> Result<(), HistoryError> {
        if self
            .workflow_node
            .as_ref()
            .is_some_and(|node| node.trim().is_empty())
            || self.workflow_node.is_some() != self.workflow_run.is_some()
        {
            return Err(HistoryError::InvalidManifest);
        }
        if let Some(base) = &self.context_base {
            base.validate()?;
            if Some(base.id) != self.workflow_run
                || base.session != self.session
                || base.runtime_only != self.runtime_only
            {
                return Err(HistoryError::InvalidManifest);
            }
        } else if self.workflow_run.is_some() {
            return Err(HistoryError::InvalidManifest);
        }
        if self.runtime_only
            && self
                .workflow_node
                .as_ref()
                .is_none_or(|node| node.trim().is_empty())
        {
            return Err(HistoryError::InvalidManifest);
        }
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
        self.manifest.validate()?;
        if self.manifest.provider != self.outcome.provider {
            return Err(HistoryError::InvalidManifest);
        }
        Ok(())
    }
}

impl EvaluationManifest {
    /// Check that a manifest's provider, token counts, and record projections are consistent.
    ///
    /// # Errors
    ///
    /// - [`HistoryError::Decision`] if the provider or model name is blank.
    /// - [`HistoryError::InvalidManifest`] if the input count is inconsistent, the budget is
    ///   zero or exceeded by input plus reserved output, a record is projected twice, or a
    ///   protected record is omitted.
    pub fn validate(&self) -> Result<(), HistoryError> {
        crate::decision::nonempty(&self.provider.provider, "manifest.provider")?;
        crate::decision::nonempty(&self.provider.model, "manifest.model")?;
        let mut projections = std::collections::HashSet::new();
        if !self.input_count.is_consistent()
            || self.budget == 0
            || self
                .input_count
                .tokens
                .checked_add(self.reserved_output_tokens)
                .is_none_or(|total| total > self.budget)
            || self.records.iter().any(|r| {
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
        let mut changed = original.clone();
        changed.manifest.input_count.exact = true;
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
            standing: StandingChanges::default(),
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

    #[test]
    fn a_request_asks_its_own_questions_and_sets_no_standing_ones() {
        let question = turn().request.questions[0].clone();
        let mut request = EvaluationRequest {
            standing: StandingChanges::default(),
            turn: TurnRequest {
                questions: vec![],
                ..turn().request
            },
            session: SessionSelection::New { name: None },
            updates: vec![],
            required_records: vec![],
            model: ModelSelection::default(),
            request_id: None,
        };
        assert_eq!(
            request.validate(),
            Err(HistoryError::Decision(ContractError::Empty("questions")))
        );
        request.turn.questions = vec![question.clone()];
        request.validate().unwrap();
        // A 0.2.0 request still reads, but a new one cannot set standing questions.
        request.standing.set = vec![question];
        assert_eq!(request.validate(), Err(HistoryError::Standing));
        request.standing = StandingChanges::default();
        assert!(
            !serde_json::to_string(&request)
                .unwrap()
                .contains("standing")
        );
    }
}
