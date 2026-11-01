//! Accepted attempt intent and atomically published immutable finalized turns.

use std::collections::{HashMap, HashSet};

use rusqlite::{OptionalExtension, Transaction, params};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use super::{Ledger, StorageError, guards::SessionGuard, sessions::SessionError, sql_error};
use crate::{
    context::ContextUpdate,
    decision::TurnRequest,
    history::{
        EvaluationManifest, FinalizedTurn, HistoryError, HistoryEvent, SourcePoint,
        StandingChanges, order::canonical_history,
    },
    identity::{Id, SessionSelector},
    outcome::TurnOutcome,
    protocol::ErrorReport,
};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AttemptInput {
    pub request: TurnRequest,
    pub updates: Vec<ContextUpdate>,
    /// Standing-question changes a 0.2.0 attempt saved; empty for new attempts.
    #[serde(default, skip_serializing_if = "StandingChanges::is_empty")]
    pub standing: StandingChanges,
    pub manifest: EvaluationManifest,
    pub request_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PreparedAttempt {
    #[serde(default)]
    pub workflow: Option<super::workflows::nodes::NodeAttemptScope>,
    pub id: Id,
    #[serde(default)]
    pub owner: Option<Id>,
    pub session: Id,
    pub expected_frontier: Vec<Id>,
    pub expected_revision: i64,
    pub input: AttemptInput,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AttemptStatus {
    Pending,
    Running,
    Completed,
    Failed,
    Interrupted,
}

impl AttemptStatus {
    fn name(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Running => "running",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Interrupted => "interrupted",
        }
    }
    fn parse(value: &str) -> Result<Self, StorageError> {
        match value {
            "pending" => Ok(Self::Pending),
            "running" => Ok(Self::Running),
            "completed" => Ok(Self::Completed),
            "failed" => Ok(Self::Failed),
            "interrupted" => Ok(Self::Interrupted),
            _ => Err(StorageError::Payload),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AttemptProgress {
    pub state: AttemptStatus,
    pub error: Option<ErrorReport>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AttemptSnapshot {
    pub intent: PreparedAttempt,
    pub progress: AttemptProgress,
    pub finalized_turn: Option<Id>,
}

#[derive(Debug, Error)]
pub enum TurnError {
    #[error(transparent)]
    Projection(#[from] crate::context::projection::ProjectionError),
    #[error("turn does not exist")]
    UnknownTurn,
    #[error("attempt does not exist")]
    UnknownAttempt,
    #[error("attempt transition is invalid")]
    AttemptState,
    #[error("session head or attempt changed concurrently")]
    Conflict,
    #[error(transparent)]
    History(#[from] HistoryError),
    #[error(transparent)]
    Session(#[from] SessionError),
    #[error(transparent)]
    Storage(#[from] StorageError),
}

fn write_error(error: StorageError) -> TurnError {
    match error {
        StorageError::Conflict => TurnError::Conflict,
        error => TurnError::Storage(error),
    }
}

pub(super) fn progress_event(
    transaction: &Transaction<'_>,
    attempt: Id,
    progress: &AttemptProgress,
) -> Result<(), StorageError> {
    let payload = Ledger::encode_payload(progress)?;
    transaction.execute("INSERT INTO attempt_events(attempt_id,sequence,payload_toon) SELECT ?1,COALESCE(MAX(sequence),-1)+1,?2 FROM attempt_events WHERE attempt_id=?1",params![attempt.to_string(),payload]).map_err(|error|sql_error("append attempt event",error))?;
    Ok(())
}

impl Ledger {
    pub fn prepare_attempt(
        &mut self,
        guard: &SessionGuard,
        input: AttemptInput,
    ) -> Result<PreparedAttempt, TurnError> {
        let session = guard.session();
        guard.validate(self, session).map_err(write_error)?;
        input.request.validate().map_err(HistoryError::from)?;
        input.manifest.validate()?;
        for update in &input.updates {
            update.validate().map_err(HistoryError::from)?;
        }
        let mut record_ids = HashSet::new();
        if input
            .updates
            .iter()
            .filter_map(ContextUpdate::new_record)
            .any(|record| !record_ids.insert(record.id))
        {
            return Err(
                HistoryError::Context(crate::context::ContextError::DuplicateRecord).into(),
            );
        }
        if input
            .request_id
            .as_ref()
            .is_some_and(|id| id.trim().is_empty())
        {
            return Err(HistoryError::InvalidSelection.into());
        }
        let session = self.session(&SessionSelector::Id(session))?;
        super::replay::validate_reserved_input(self, guard, &input).map_err(write_error)?;
        let prepared = PreparedAttempt {
            workflow: None,
            id: Id::generate(),
            owner: Some(guard.owner),
            session: session.id,
            expected_frontier: session.frontier,
            expected_revision: session.revision,
            input,
        };
        self.context_at(
            &self.context_sources(prepared.session)?,
            &prepared.input.updates,
            prepared.session,
            prepared.id,
        )?;
        let payload = Self::encode_payload(&prepared)?;
        self.transaction::<_, StorageError>("prepare attempt", |transaction| {
            transaction
                .execute(
                    "INSERT INTO attempts(id,session_id,payload_toon) VALUES(?1,?2,?3)",
                    params![
                        prepared.id.to_string(),
                        prepared.session.to_string(),
                        payload
                    ],
                )
                .map_err(|error| sql_error("insert accepted intent", error))?;
            transaction
                .execute(
                    "INSERT INTO attempt_states(attempt_id,state) VALUES(?1,'pending')",
                    [prepared.id.to_string()],
                )
                .map_err(|error| sql_error("insert pending state", error))?;
            progress_event(
                transaction,
                prepared.id,
                &AttemptProgress {
                    state: AttemptStatus::Pending,
                    error: None,
                },
            )
        })?;
        Ok(prepared)
    }

    pub fn attempt(&self, id: Id) -> Result<AttemptSnapshot, TurnError> {
        let row:Option<(String,String,Option<String>,String)>=self.connection.query_row("SELECT a.payload_toon,s.state,s.finalized_turn,(SELECT payload_toon FROM attempt_events e WHERE e.attempt_id=a.id ORDER BY sequence DESC LIMIT 1) FROM attempts a JOIN attempt_states s ON s.attempt_id=a.id WHERE a.id=?1",[id.to_string()],|row|Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?))).optional().map_err(|error|sql_error("read attempt",error))?;
        let (intent, state, turn, progress) = row.ok_or(TurnError::UnknownAttempt)?;
        let intent: PreparedAttempt = Self::decode_payload(&intent)?;
        let progress: AttemptProgress = Self::decode_payload(&progress)?;
        if intent.id != id || progress.state != AttemptStatus::parse(&state)? {
            return Err(StorageError::Payload.into());
        }
        Ok(AttemptSnapshot {
            intent,
            progress,
            finalized_turn: turn
                .map(Id::try_from)
                .transpose()
                .map_err(|_| StorageError::Payload)?,
        })
    }

    pub fn list_attempts(&self, session: Id) -> Result<Vec<AttemptSnapshot>, TurnError> {
        self.session(&SessionSelector::Id(session))?;
        let mut statement = self
            .connection
            .prepare("SELECT id FROM attempts WHERE session_id=?1 ORDER BY rowid")
            .map_err(|error| sql_error("list attempts", error))?;
        let ids = statement
            .query_map([session.to_string()], |row| row.get::<_, String>(0))
            .map_err(|error| sql_error("list attempts", error))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| sql_error("read attempt ids", error))?;
        ids.into_iter()
            .map(|id| self.attempt(Id::try_from(id).map_err(|_| StorageError::Payload)?))
            .collect()
    }

    pub fn record_attempt_progress(
        &mut self,
        guard: &SessionGuard,
        id: Id,
        progress: AttemptProgress,
    ) -> Result<(), TurnError> {
        let snapshot = self.attempt(id)?;
        guard
            .validate(self, snapshot.intent.session)
            .map_err(write_error)?;
        if snapshot.intent.owner != Some(guard.owner) {
            return Err(TurnError::Conflict);
        }
        let previous = snapshot.progress.state;
        if !matches!(
            (previous, progress.state),
            (
                AttemptStatus::Pending,
                AttemptStatus::Running | AttemptStatus::Failed | AttemptStatus::Interrupted
            ) | (
                AttemptStatus::Running,
                AttemptStatus::Failed | AttemptStatus::Interrupted
            )
        ) {
            return Err(TurnError::AttemptState);
        }
        self.transaction::<_, StorageError>("record attempt state", |transaction| {
            let changed = transaction
                .execute(
                    "UPDATE attempt_states SET state=?2 WHERE attempt_id=?1 AND state=?3",
                    params![id.to_string(), progress.state.name(), previous.name()],
                )
                .map_err(|error| sql_error("advance attempt state", error))?;
            if changed != 1 {
                return Err(StorageError::Conflict);
            }
            progress_event(transaction, id, &progress)
        })
        .map_err(write_error)
    }

    pub fn finalize_attempt(
        &mut self,
        guard: &SessionGuard,
        id: Id,
        outcome: TurnOutcome,
    ) -> Result<FinalizedTurn, TurnError> {
        let snapshot = self.attempt(id)?;
        guard
            .validate(self, snapshot.intent.session)
            .map_err(write_error)?;
        if snapshot.intent.owner != Some(guard.owner) {
            return Err(TurnError::Conflict);
        }
        if !matches!(
            snapshot.progress.state,
            AttemptStatus::Pending | AttemptStatus::Running
        ) {
            return Err(TurnError::AttemptState);
        }
        let prepared = snapshot.intent;
        if prepared.workflow.is_some() {
            return Err(TurnError::Conflict);
        }
        let current = self.session(&SessionSelector::Id(prepared.session))?;
        if current.revision != prepared.expected_revision
            || current.frontier != prepared.expected_frontier
        {
            return Err(TurnError::Conflict);
        }
        self.context_at(
            &self.context_sources(prepared.session)?,
            &prepared.input.updates,
            prepared.session,
            prepared.id,
        )?;
        let mut manifest = prepared.input.manifest;
        manifest.provider = outcome.provider.clone();
        let parents = if prepared.expected_frontier.is_empty() {
            let mut seen = HashSet::new();
            self.session_sources(prepared.session)?
                .into_iter()
                .flat_map(|source| source.frontier)
                .filter(|id| seen.insert(*id))
                .collect()
        } else {
            prepared.expected_frontier
        };
        let turn = FinalizedTurn {
            context_base: None,
            workflow_run: None,
            runtime_only: false,
            workflow_node: None,
            id: Id::generate(),
            session: prepared.session,
            parents,
            request: prepared.input.request,
            outcome,
            updates: prepared.input.updates,
            standing: prepared.input.standing,
            manifest,
        };
        turn.validate()?;
        let payload = Self::encode_payload(&turn)?;
        let frontier = Self::encode_payload(&vec![turn.id])?;
        let event = Self::encode_payload(&HistoryEvent::TurnFinalized {
            session: turn.session,
            turn: turn.id,
            frontier: vec![turn.id],
        })?;
        self.transaction::<_, StorageError>("finalize turn",|transaction| {
            let current:i64=transaction.query_row("SELECT revision FROM sessions WHERE id=?1",[turn.session.to_string()],|row|row.get(0)).map_err(|error|sql_error("check expected head",error))?;
            if current!=prepared.expected_revision {return Err(StorageError::Conflict);}
            super::publication::insert_turn(transaction,&turn,id,prepared.expected_revision)?;
            let changed=transaction.execute("UPDATE sessions SET frontier_toon=?2,revision=revision+1 WHERE id=?1 AND revision=?3 AND revision<9223372036854775807",params![turn.session.to_string(),frontier,prepared.expected_revision]).map_err(|error|sql_error("publish session head",error))?;
            if changed!=1 {return Err(StorageError::Conflict);}
            if let Some(key)=&prepared.input.request_id {
                let changed=transaction.execute("UPDATE request_reservations SET state='completed',result_toon=?2 WHERE request_id=?1 AND owner_id=?3 AND session_id=?4 AND state='pending'",params![key,payload,guard.owner.to_string(),turn.session.to_string()]).map_err(|error|sql_error("complete request reservation",error))?;
                if changed!=1 {return Err(StorageError::Conflict);}
            }
            transaction.execute("INSERT INTO session_events(session_id,sequence,payload_toon) SELECT ?1,COALESCE(MAX(sequence),-1)+1,?2 FROM session_events WHERE session_id=?1",params![turn.session.to_string(),event]).map_err(|error|sql_error("record finalized head",error))?;
            Ok(())
        }).map_err(write_error)?;
        Ok(turn)
    }

    pub fn turn(&self, id: Id) -> Result<FinalizedTurn, TurnError> {
        let payload: Option<String> = self
            .connection
            .query_row(
                "SELECT payload_toon FROM turns WHERE id=?1",
                [id.to_string()],
                |row| row.get(0),
            )
            .optional()
            .map_err(|error| sql_error("read turn", error))?;
        let turn: FinalizedTurn = Self::decode_payload(&payload.ok_or(TurnError::UnknownTurn)?)?;
        if turn.id != id {
            return Err(StorageError::Payload.into());
        }
        turn.validate()?;
        Ok(turn)
    }

    pub fn manifest(&self, id: Id) -> Result<EvaluationManifest, TurnError> {
        let payload: Option<String> = self
            .connection
            .query_row(
                "SELECT payload_toon FROM manifests WHERE turn_id=?1",
                [id.to_string()],
                |row| row.get(0),
            )
            .optional()
            .map_err(|error| sql_error("read manifest", error))?;
        let manifest: EvaluationManifest =
            Self::decode_payload(&payload.ok_or(TurnError::UnknownTurn)?)?;
        manifest.validate()?;
        Ok(manifest)
    }

    pub fn ancestry(&self, sources: &[SourcePoint]) -> Result<Vec<FinalizedTurn>, TurnError> {
        let mut pending: Vec<_> = sources
            .iter()
            .flat_map(|s| s.frontier.iter().copied())
            .collect();
        let mut seen = HashSet::new();
        let mut turns = Vec::new();
        while let Some(id) = pending.pop() {
            if !seen.insert(id) {
                continue;
            }
            let turn = self.turn(id)?;
            pending.extend(turn.parents.iter().copied());
            turns.push(turn);
        }
        let ordered: Vec<_> = canonical_history(&turns, sources)?
            .iter()
            .map(|turn| turn.id)
            .collect();
        let mut turns: HashMap<_, _> = turns.into_iter().map(|turn| (turn.id, turn)).collect();
        ordered
            .into_iter()
            .map(|id| {
                turns
                    .remove(&id)
                    .ok_or_else(|| StorageError::Payload.into())
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        context::{ContextKind, ContextRecord},
        data::Data,
        history::ModelSelection,
        protocol::ErrorCategory,
        storage::OpenMode,
    };

    struct Store(std::path::PathBuf);
    impl Store {
        fn new() -> Self {
            Self(std::env::temp_dir().join(format!("nouli-turns-{}", Id::generate())))
        }
        fn open(&self) -> Ledger {
            Ledger::open(&self.0, OpenMode::ReadWriteCreate).unwrap()
        }
    }
    impl Drop for Store {
        fn drop(&mut self) {
            if self.0.exists() {
                std::fs::remove_dir_all(&self.0).unwrap();
            }
        }
    }

    fn input() -> AttemptInput {
        let fixture: serde_json::Value =
            serde_json::from_str(include_str!("../../../fixtures/contracts/history.json")).unwrap();
        let turn: FinalizedTurn = serde_json::from_value(fixture["turns"][0].clone()).unwrap();
        AttemptInput {
            standing: StandingChanges::default(),
            request: turn.request,
            updates: vec![ContextUpdate::Append {
                record: ContextRecord {
                    id: Id::generate(),
                    key: format!("commitment-{}", Id::generate()),
                    value: Data::new(serde_json::Value::Null).unwrap(),
                    kind: ContextKind::Commitment,
                },
            }],
            manifest: turn.manifest,
            request_id: None,
        }
    }
    fn outcome() -> TurnOutcome {
        serde_json::from_str(include_str!(
            "../../../fixtures/contracts/decision-outcome.json"
        ))
        .unwrap()
    }

    #[test]
    fn reopened_success_retains_full_contracts_parent_identity_and_manifests() {
        let store = Store::new();
        let mut ledger = store.open();
        let session = ledger
            .create_session(None, ModelSelection::default())
            .unwrap();
        let guard = ledger.session_guard(session.id).unwrap();
        let prepared = ledger.prepare_attempt(&guard, input()).unwrap();
        assert_eq!(ledger.attempt(prepared.id).unwrap().intent, prepared);
        assert!(
            ledger
                .session(&SessionSelector::Id(session.id))
                .unwrap()
                .frontier
                .is_empty()
        );
        let first = ledger
            .finalize_attempt(&guard, prepared.id, outcome())
            .unwrap();
        let mut next = input();
        next.manifest.sources = vec![SourcePoint {
            session: session.id,
            frontier: vec![first.id],
        }];
        let prepared_next = ledger.prepare_attempt(&guard, next).unwrap();
        let second = ledger
            .finalize_attempt(&guard, prepared_next.id, outcome())
            .unwrap();
        drop(ledger);
        let ledger = Ledger::open(&store.0, OpenMode::ReadOnlyExisting).unwrap();
        assert_eq!(
            ledger
                .session(&SessionSelector::Id(session.id))
                .unwrap()
                .frontier,
            vec![second.id]
        );
        assert_eq!(ledger.turn(first.id).unwrap(), first);
        assert_eq!(ledger.manifest(second.id).unwrap(), second.manifest);
        assert_eq!(second.parents, vec![first.id]);
        assert_eq!(
            ledger
                .ancestry(&[SourcePoint {
                    session: session.id,
                    frontier: vec![second.id]
                }])
                .unwrap(),
            vec![first.clone(), second]
        );
        assert!(matches!(
            first.outcome.outcomes[3],
            crate::outcome::QuestionOutcome::Refused { .. }
        ));
        assert_eq!(
            first.updates[0].new_record().unwrap().value.value(),
            &serde_json::Value::Null
        );
        assert_eq!(
            ledger.attempt(prepared.id).unwrap().finalized_turn,
            Some(first.id)
        );
    }

    #[test]
    fn failed_and_interrupted_attempts_preserve_finalized_head_after_reopen() {
        let store = Store::new();
        let mut ledger = store.open();
        let session = ledger
            .create_session(None, ModelSelection::default())
            .unwrap();
        let guard = ledger.session_guard(session.id).unwrap();
        let accepted = ledger.prepare_attempt(&guard, input()).unwrap();
        let saved = ledger
            .finalize_attempt(&guard, accepted.id, outcome())
            .unwrap();
        let failure = ledger.prepare_attempt(&guard, input()).unwrap();
        let progress = AttemptProgress {
            state: AttemptStatus::Failed,
            error: Some(ErrorReport {
                category: ErrorCategory::Transport,
                code: "timeout".into(),
                message: "Provider timed out".into(),
                retryable: true,
            }),
        };
        ledger
            .record_attempt_progress(&guard, failure.id, progress.clone())
            .unwrap();
        let interrupted = ledger.prepare_attempt(&guard, input()).unwrap();
        ledger
            .record_attempt_progress(
                &guard,
                interrupted.id,
                AttemptProgress {
                    state: AttemptStatus::Running,
                    error: None,
                },
            )
            .unwrap();
        ledger
            .record_attempt_progress(
                &guard,
                interrupted.id,
                AttemptProgress {
                    state: AttemptStatus::Interrupted,
                    error: None,
                },
            )
            .unwrap();
        assert!(matches!(
            ledger.finalize_attempt(&guard, interrupted.id, outcome()),
            Err(TurnError::AttemptState)
        ));
        drop(ledger);
        let ledger = Ledger::open(&store.0, OpenMode::ReadOnlyExisting).unwrap();
        assert_eq!(
            ledger
                .session(&SessionSelector::Id(session.id))
                .unwrap()
                .frontier,
            vec![saved.id]
        );
        assert_eq!(ledger.attempt(failure.id).unwrap().progress, progress);
        assert_eq!(
            ledger.attempt(interrupted.id).unwrap().progress.state,
            AttemptStatus::Interrupted
        );
        assert_eq!(ledger.list_attempts(session.id).unwrap().len(), 3);
    }

    #[test]
    fn injected_finalization_failure_rolls_back_records_context_and_head() {
        let store = Store::new();
        let mut ledger = store.open();
        let session = ledger
            .create_session(None, ModelSelection::default())
            .unwrap();
        let guard = ledger.session_guard(session.id).unwrap();
        let attempt = ledger.prepare_attempt(&guard, input()).unwrap();
        ledger.transaction::<_, StorageError>("inject failure",|transaction| {transaction.execute_batch("CREATE TRIGGER fail_publish BEFORE UPDATE ON attempt_states WHEN NEW.state='completed' BEGIN SELECT RAISE(ABORT,'injected failure'); END;").map_err(|error|sql_error("inject finalization failure",error))}).unwrap();
        assert!(
            ledger
                .finalize_attempt(&guard, attempt.id, outcome())
                .is_err()
        );
        assert!(
            ledger
                .session(&SessionSelector::Id(session.id))
                .unwrap()
                .frontier
                .is_empty()
        );
        assert_eq!(
            ledger.attempt(attempt.id).unwrap().progress.state,
            AttemptStatus::Pending
        );
        for table in ["turns", "context_records", "context_events", "manifests"] {
            let count: i64 = ledger
                .connection
                .query_row(&format!("SELECT count(*) FROM {table}"), [], |row| {
                    row.get(0)
                })
                .unwrap();
            assert_eq!(count, 0);
        }
        ledger
            .transaction::<_, StorageError>("remove failure", |transaction| {
                transaction
                    .execute_batch("DROP TRIGGER fail_publish")
                    .map_err(|error| sql_error("remove failure injection", error))
            })
            .unwrap();
        ledger
            .finalize_attempt(&guard, attempt.id, outcome())
            .unwrap();
    }

    #[test]
    fn stale_head_finalization_conflicts_without_publishing_a_second_success() {
        let store = Store::new();
        let mut ledger = store.open();
        let session = ledger
            .create_session(None, ModelSelection::default())
            .unwrap();
        let guard = ledger.session_guard(session.id).unwrap();
        let same_key_input = || {
            let mut input = input();
            if let ContextUpdate::Append { record } = &mut input.updates[0] {
                record.key = "budget".into();
            }
            input
        };
        let first = ledger.prepare_attempt(&guard, same_key_input()).unwrap();
        let second = ledger.prepare_attempt(&guard, same_key_input()).unwrap();
        let saved = ledger
            .finalize_attempt(&guard, first.id, outcome())
            .unwrap();
        assert!(matches!(
            ledger.finalize_attempt(&guard, second.id, outcome()),
            Err(TurnError::Conflict)
        ));
        assert_eq!(
            ledger
                .session(&SessionSelector::Id(session.id))
                .unwrap()
                .frontier,
            vec![saved.id]
        );
        assert_eq!(
            ledger.attempt(second.id).unwrap().progress.state,
            AttemptStatus::Pending
        );
    }
}
