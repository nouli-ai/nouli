//! Store-wide request reservations with frozen intent and explicit recovery.

use rusqlite::{Connection, OptionalExtension, Transaction, params};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use super::{
    Ledger, StorageError,
    guards::{self, FileGuard, GuardError, SessionGuard},
    sessions::{Session, SessionError, insert_session, read_session},
    sql_error,
    turns::{AttemptInput, AttemptProgress, AttemptStatus, PreparedAttempt, TurnError},
};
use crate::{
    codec,
    config::resolve::SettingsSnapshot,
    history::FinalizedTurn,
    history::{
        EvaluationManifest, EvaluationRequest, HistoryError, ModelSelection, SessionSelection,
        SourcePoint, StandingChanges,
    },
    identity::{Id, SessionSelector},
    protocol::{ErrorCategory, ErrorReport},
};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FrozenRequest {
    pub request: EvaluationRequest,
    pub settings: SettingsSnapshot,
    pub sources: Vec<SourcePoint>,
    pub session: Id,
    pub frontier: Vec<Id>,
    pub revision: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecoveryKind {
    NeverDispatched,
    OutcomeUnknown,
    KnownFailure,
    Interrupted,
}

pub enum RequestDecision {
    Accepted(Box<RequestOwner>),
    Completed {
        turn: Box<FinalizedTurn>,
        frozen: Box<FrozenRequest>,
    },
    InProgress {
        session: Option<Id>,
    },
    Interrupted {
        kind: RecoveryKind,
        frozen: Box<FrozenRequest>,
    },
}

pub struct RequestOwner {
    key: String,
    frozen: FrozenRequest,
    session_guard: SessionGuard,
    _request_guard: FileGuard,
}

impl RequestOwner {
    pub fn frozen(&self) -> &FrozenRequest {
        &self.frozen
    }
    pub fn guard(&self) -> &SessionGuard {
        &self.session_guard
    }
    pub fn prepare(
        &self,
        ledger: &mut Ledger,
        manifest: EvaluationManifest,
    ) -> Result<PreparedAttempt, ReplayError> {
        let session = ledger.session(&SessionSelector::Id(self.frozen.session))?;
        if session.revision != self.frozen.revision || session.frontier != self.frozen.frontier {
            return Err(ReplayError::Conflict);
        }
        Ok(ledger.prepare_attempt(
            &self.session_guard,
            AttemptInput {
                request: ledger.effective_turn(&self.frozen)?,
                updates: self.frozen.request.updates.clone(),
                standing: self.frozen.request.standing.clone(),
                manifest,
                request_id: Some(self.key.clone()),
            },
        )?)
    }
}

#[derive(Debug, Error)]
pub enum ReplayError {
    #[error("request ID is missing or empty")]
    MissingId,
    #[error("request ID already identifies different intent")]
    IntentConflict,
    #[error("saved session head or request ownership changed")]
    Conflict,
    #[error(transparent)]
    Storage(#[from] StorageError),
    #[error(transparent)]
    Session(#[from] SessionError),
    #[error(transparent)]
    Guard(#[from] GuardError),
    #[error(transparent)]
    Turn(#[from] TurnError),
    #[error(transparent)]
    History(#[from] HistoryError),
    #[error("cannot fingerprint request intent")]
    Fingerprint,
    #[error(transparent)]
    Source(#[from] super::sources::SourceError),
}

/// Presentation/config defaults are not part of submitted intent. Resolve
/// explicit CLI provider/model overrides into request.model before this call.
pub fn fingerprint(request: &EvaluationRequest) -> Result<String, ReplayError> {
    request.validate()?;
    #[derive(Serialize)]
    struct Intent<'a> {
        turn: &'a crate::decision::TurnRequest,
        #[serde(skip_serializing_if = "StandingChanges::is_empty")]
        standing: &'a StandingChanges,
        session: &'a SessionSelection,
        updates: &'a [crate::context::ContextUpdate],
        required_records: &'a [Id],
        model: &'a ModelSelection,
    }
    codec::fingerprint(&Intent {
        turn: &request.turn,
        standing: &request.standing,
        session: &request.session,
        updates: &request.updates,
        required_records: &request.required_records,
        model: &request.model,
    })
    .map_err(|_| ReplayError::Fingerprint)
}

struct Reservation {
    session: Id,
    fingerprint: String,
    state: String,
    frozen: FrozenRequest,
    result: Option<String>,
    recovery: Option<RecoveryKind>,
}

fn reservation(connection: &Connection, key: &str) -> Result<Option<Reservation>, ReplayError> {
    let kind: Option<String> = connection
        .query_row(
            "SELECT kind FROM request_reservations WHERE request_id=?1",
            [key],
            |row| row.get(0),
        )
        .optional()
        .map_err(|error| sql_error("read request kind", error))?;
    if kind.as_deref().is_some_and(|kind| kind != "evaluation") {
        return Err(ReplayError::IntentConflict);
    }
    type ReservationRow = (
        String,
        String,
        String,
        String,
        Option<String>,
        Option<String>,
    );
    let row:Option<ReservationRow>=connection.query_row("SELECT session_id,fingerprint,state,frozen_toon,result_toon,recovery_kind FROM request_reservations WHERE request_id=?1",[key],|row|Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?,row.get(4)?,row.get(5)?))).optional().map_err(|error|sql_error("read request reservation",error))?;
    row.map(|(session, hash, state, frozen, result, recovery)| {
        let frozen: FrozenRequest = Ledger::decode_payload(&frozen)?;
        let session = Id::try_from(session).map_err(|_| StorageError::Payload)?;
        if frozen.session != session || fingerprint(&frozen.request)? != hash {
            return Err(StorageError::Payload.into());
        }
        Ok(Reservation {
            session,
            fingerprint: hash,
            state,
            frozen,
            result,
            recovery: recovery.map(|s| Ledger::decode_payload(&s)).transpose()?,
        })
    })
    .transpose()
}

fn completed(saved: Reservation) -> Result<RequestDecision, ReplayError> {
    let turn: FinalizedTurn = Ledger::decode_payload(&saved.result.ok_or(StorageError::Payload)?)?;
    turn.validate()?;
    if turn.session != saved.session {
        return Err(StorageError::Payload.into());
    }
    Ok(RequestDecision::Completed {
        turn: Box::new(turn),
        frozen: Box::new(saved.frozen),
    })
}

fn recovery_priority(kind: RecoveryKind) -> u8 {
    match kind {
        RecoveryKind::NeverDispatched => 0,
        RecoveryKind::KnownFailure => 1,
        RecoveryKind::Interrupted => 2,
        RecoveryKind::OutcomeUnknown => 3,
    }
}

fn interrupt(
    transaction: &Transaction<'_>,
    key: &str,
    saved: &Reservation,
) -> Result<RecoveryKind, ReplayError> {
    if saved.state == "interrupted" {
        return Ok(saved.recovery.unwrap_or(RecoveryKind::Interrupted));
    }
    let mut statement=transaction.prepare("SELECT a.id,a.payload_toon,s.state,(SELECT payload_toon FROM attempt_events e WHERE e.attempt_id=a.id ORDER BY sequence DESC LIMIT 1) FROM attempts a JOIN attempt_states s ON s.attempt_id=a.id WHERE a.session_id=?1 ORDER BY a.id").map_err(|error|sql_error("inspect orphan attempts",error))?;
    let rows = statement
        .query_map([saved.session.to_string()], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
            ))
        })
        .map_err(|error| sql_error("read orphan attempts", error))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| sql_error("read orphan rows", error))?;
    drop(statement);
    let mut kind = RecoveryKind::NeverDispatched;
    for (id, intent, state, progress) in rows {
        let intent: PreparedAttempt = Ledger::decode_payload(&intent)?;
        if intent.input.request_id.as_deref() != Some(key) {
            continue;
        }
        let observed = match state.as_str() {
            "running" => RecoveryKind::OutcomeUnknown,
            "failed" => {
                let progress: AttemptProgress = Ledger::decode_payload(&progress)?;
                if progress
                    .error
                    .as_ref()
                    .is_some_and(|error| error.code == "provider_outcome_unknown")
                {
                    RecoveryKind::OutcomeUnknown
                } else {
                    RecoveryKind::KnownFailure
                }
            }
            "interrupted" => {
                let progress: AttemptProgress = Ledger::decode_payload(&progress)?;
                match progress.error.as_ref().map(|error| error.code.as_str()) {
                    Some("provider_outcome_unknown") => RecoveryKind::OutcomeUnknown,
                    Some("not_dispatched") => RecoveryKind::NeverDispatched,
                    _ => RecoveryKind::Interrupted,
                }
            }
            "pending" => RecoveryKind::NeverDispatched,
            _ => return Err(StorageError::Payload.into()),
        };
        if recovery_priority(observed) > recovery_priority(kind) {
            kind = observed;
        }
        if matches!(state.as_str(), "pending" | "running") {
            let code = if state == "running" {
                "provider_outcome_unknown"
            } else {
                "not_dispatched"
            };
            let progress = AttemptProgress {
                state: AttemptStatus::Interrupted,
                error: Some(ErrorReport {
                    category: ErrorCategory::Interrupted,
                    code: code.into(),
                    message: "Request owner exited before finalization".into(),
                    retryable: false,
                }),
            };
            transaction
                .execute(
                    "UPDATE attempt_states SET state='interrupted' WHERE attempt_id=?1",
                    [&id],
                )
                .map_err(|error| sql_error("mark orphan attempt", error))?;
            transaction.execute("INSERT INTO attempt_events(attempt_id,sequence,payload_toon) SELECT ?1,COALESCE(MAX(sequence),-1)+1,?2 FROM attempt_events WHERE attempt_id=?1",params![id,Ledger::encode_payload(&progress)?]).map_err(|error|sql_error("record orphan attempt",error))?;
        }
    }
    transaction.execute("UPDATE request_reservations SET state='interrupted',recovery_kind=?2 WHERE request_id=?1 AND state='pending'",params![key,Ledger::encode_payload(&kind)?]).map_err(|error|sql_error("record interrupted request",error))?;
    Ok(kind)
}

pub(super) fn validate_reserved_input(
    ledger: &Ledger,
    guard: &SessionGuard,
    input: &AttemptInput,
) -> Result<(), StorageError> {
    let Some(key) = &input.request_id else {
        return Ok(());
    };
    let row:Option<(String,String,String)>=ledger.connection.query_row("SELECT owner_id,state,frozen_toon FROM request_reservations WHERE request_id=?1 AND session_id=?2",params![key,guard.session.to_string()],|row|Ok((row.get(0)?,row.get(1)?,row.get(2)?))).optional().map_err(|error|sql_error("validate request owner",error))?;
    let (owner, state, frozen) = row.ok_or(StorageError::Conflict)?;
    let frozen: FrozenRequest = Ledger::decode_payload(&frozen)?;
    // The caller's questions stay unexpanded in the frozen request; the attempt
    // must carry the same expansion the engine executes.
    let effective = ledger
        .effective_turn(&frozen)
        .map_err(|_| StorageError::Conflict)?;
    if owner != guard.owner.to_string()
        || state != "pending"
        || input.request != effective
        || input.standing != frozen.request.standing
        || input.updates != frozen.request.updates
        || input.manifest.sources != frozen.sources
        || input.manifest.provider.provider != frozen.settings.profile_name
        || input.manifest.provider.model != frozen.settings.profile.model
    {
        return Err(StorageError::Conflict);
    }
    if frozen.request.required_records.iter().any(|id| {
        !input.manifest.records.iter().any(|record| {
            record.record == *id
                && record.protected
                && record.representation != crate::history::Representation::Omitted
        })
    }) {
        return Err(StorageError::Conflict);
    }
    Ok(())
}

impl Ledger {
    /// Inspect validated frozen settings without acquiring ownership or credentials.
    pub fn saved_request(
        &self,
        request: &EvaluationRequest,
    ) -> Result<Option<FrozenRequest>, ReplayError> {
        let Some(key) = request.request_id.as_deref() else {
            return Ok(None);
        };
        let hash = fingerprint(request)?;
        let saved = reservation(&self.connection, key)?;
        if saved
            .as_ref()
            .is_some_and(|saved| saved.fingerprint != hash)
        {
            return Err(ReplayError::IntentConflict);
        }
        Ok(saved.map(|saved| saved.frozen))
    }
    /// The frozen request saved under a request ID, whatever its intent. A
    /// caller that builds a request from the ledger's current state uses it to
    /// rebuild the same request when the ID was already used.
    pub fn saved_request_by_id(&self, key: &str) -> Result<Option<FrozenRequest>, ReplayError> {
        Ok(reservation(&self.connection, key)?.map(|saved| saved.frozen))
    }
    pub fn accept_request(
        &mut self,
        request: &EvaluationRequest,
        settings: SettingsSnapshot,
    ) -> Result<RequestDecision, ReplayError> {
        self.request_decision(request, Some(settings), false)
    }
    pub fn retry_interrupted_request(
        &mut self,
        request: &EvaluationRequest,
    ) -> Result<RequestDecision, ReplayError> {
        self.request_decision(request, None, true)
    }

    fn request_decision(
        &mut self,
        request: &EvaluationRequest,
        settings: Option<SettingsSnapshot>,
        retry: bool,
    ) -> Result<RequestDecision, ReplayError> {
        let key = request
            .request_id
            .as_deref()
            .filter(|key| !key.trim().is_empty())
            .ok_or(ReplayError::MissingId)?;
        let hash = fingerprint(request)?;
        let existing = reservation(&self.connection, key)?;
        if let Some(saved) = &existing
            && saved.fingerprint != hash
        {
            return Err(ReplayError::IntentConflict);
        }
        if existing
            .as_ref()
            .is_some_and(|saved| saved.state == "completed")
        {
            return completed(existing.ok_or(StorageError::Payload)?);
        }
        let lock_key = format!(
            "request-{}",
            codec::fingerprint(&key).map_err(|_| ReplayError::Fingerprint)?
        );
        let request_guard = match guards::lock(self, &lock_key) {
            Ok(guard) => guard,
            Err(GuardError::WriterActive) => {
                return Ok(RequestDecision::InProgress {
                    session: existing.map(|saved| saved.session),
                });
            }
            Err(error) => return Err(error.into()),
        };
        if let Some(saved) = reservation(&self.connection, key)? {
            if saved.fingerprint != hash {
                return Err(ReplayError::IntentConflict);
            }
            if saved.state == "completed" {
                return completed(saved);
            }
            let kind = self
                .transaction::<_, ReplayError>("recover exited owner", |transaction| {
                    interrupt(transaction, key, &saved)
                })?;
            if !retry {
                return Ok(RequestDecision::Interrupted {
                    kind,
                    frozen: Box::new(saved.frozen),
                });
            }
            let guard = guards::acquire_session(self, saved.session)?;
            let current = self.session(&SessionSelector::Id(saved.session))?;
            if current.frontier != saved.frozen.frontier
                || current.revision != saved.frozen.revision
            {
                return Err(ReplayError::Conflict);
            }
            self.transaction::<_,StorageError>("retry original reservation",|transaction| {
                let changed=transaction.execute("UPDATE request_reservations SET owner_id=?2,state='pending',recovery_kind=NULL WHERE request_id=?1 AND state='interrupted'",params![key,guard.owner.to_string()]).map_err(|error|sql_error("reserve explicit retry",error))?;
                if changed!=1 {return Err(StorageError::Conflict);}
                Ok(())
            })?;
            return Ok(RequestDecision::Accepted(Box::new(RequestOwner {
                key: key.into(),
                frozen: saved.frozen,
                session_guard: guard,
                _request_guard: request_guard,
            })));
        }
        if retry {
            return Err(ReplayError::MissingId);
        }
        let mut settings = settings.ok_or(StorageError::Payload)?;
        settings
            .profile
            .validate()
            .map_err(|_| StorageError::Payload)?;
        settings
            .execution
            .validate()
            .map_err(|_| StorageError::Payload)?;
        settings.store = std::fs::canonicalize(self.path().parent().ok_or(StorageError::Payload)?)
            .map_err(|source| StorageError::Io {
                operation: "freeze store path",
                path: self.path().to_owned(),
                source,
            })?;
        let (id, name, new) = match &request.session {
            SessionSelection::Continue { session } => (self.session(session)?.id, None, false),
            SessionSelection::New { name } | SessionSelection::From { name, .. } => {
                (Id::generate(), name.clone(), true)
            }
        };
        let guard = guards::acquire_session(self, id)?;
        let frozen=self.transaction_with::<_,ReplayError>("accept unique request",|ledger| {
            let transaction=&ledger.connection;
            let current=if new {Session {id,name,defaults:ModelSelection {provider:Some(settings.profile_name.clone()),model:Some(settings.profile.model.clone())},frontier:vec![],revision:0}} else {read_session(transaction,&SessionSelector::Id(id))?};
            let captured=super::sources::resolve_selection(transaction,&request.session,&current)?;
            if new {insert_session(transaction,&current,&captured).map_err(|error|if current.name.is_some() && matches!(error,StorageError::Sqlite {code:Some(rusqlite::ErrorCode::ConstraintViolation),..}) {ReplayError::Session(SessionError::NameCollision)} else {ReplayError::Storage(error)})?;}
            let frozen=FrozenRequest {request:request.clone(),settings,sources:captured,session:id,frontier:current.frontier,revision:current.revision};
            transaction.execute("INSERT INTO request_reservations(session_id,request_id,fingerprint,owner_id,state,frozen_toon) VALUES(?1,?2,?3,?4,'pending',?5)",params![id.to_string(),key,hash,guard.owner.to_string(),Ledger::encode_payload(&frozen)?]).map_err(|error|sql_error("reserve unique request",error))?;
            // The turn's standing changes must apply to the session as it is now;
            // a mistake rolls back the reservation and any new session with it.
            ledger.effective_turn(&frozen)?;
            Ok(frozen)
        })?;
        Ok(RequestDecision::Accepted(Box::new(RequestOwner {
            key: key.into(),
            frozen,
            session_guard: guard,
            _request_guard: request_guard,
        })))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{ExecutionSettings, OutputFormat, Protocol, ProviderProfile};
    use crate::history::SourceSelection;
    use crate::storage::OpenMode;

    struct Store(std::path::PathBuf);
    impl Store {
        fn new() -> Self {
            Self(std::env::temp_dir().join(format!("nouli-replay-{}", Id::generate())))
        }
        fn open(&self) -> Ledger {
            Ledger::open(&self.0, OpenMode::ReadWriteCreate).unwrap()
        }
    }
    impl Drop for Store {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    fn fixture() -> FinalizedTurn {
        let data: serde_json::Value =
            serde_json::from_str(include_str!("../../../fixtures/contracts/history.json")).unwrap();
        serde_json::from_value(data["turns"][0].clone()).unwrap()
    }
    fn request() -> EvaluationRequest {
        EvaluationRequest {
            standing: Default::default(),
            turn: fixture().request,
            session: SessionSelection::New { name: None },
            updates: vec![],
            required_records: vec![],
            model: ModelSelection::default(),
            request_id: Some("stable-request".into()),
        }
    }
    fn settings(store: &Store) -> SettingsSnapshot {
        SettingsSnapshot {
            profile_name: "fixture".into(),
            profile: ProviderProfile {
                protocol: Protocol::Typesafe,
                base_url: "http://127.0.0.1:8080".into(),
                model: "offline".into(),
                api_key_env: None,
                stored_credential: false,
                context_budget_tokens: Some(4096),
                context_window_tokens: Some(8192),
                reserved_output_tokens: Some(128),
                max_attempts: None,
                attempt_timeout_seconds: None,
            },
            execution: ExecutionSettings::default(),
            output: OutputFormat::Json,
            store: store.0.clone(),
        }
    }
    fn accepted(decision: RequestDecision) -> Box<RequestOwner> {
        if let RequestDecision::Accepted(owner) = decision {
            owner
        } else {
            panic!("expected accepted request")
        }
    }
    fn finish(ledger: &mut Ledger, owner: &RequestOwner) -> FinalizedTurn {
        let mut manifest = fixture().manifest;
        manifest.sources = owner.frozen.sources.clone();
        let attempt = owner.prepare(ledger, manifest).unwrap();
        ledger
            .record_attempt_progress(
                owner.guard(),
                attempt.id,
                AttemptProgress {
                    state: AttemptStatus::Running,
                    error: None,
                },
            )
            .unwrap();
        ledger
            .finalize_attempt(owner.guard(), attempt.id, fixture().outcome)
            .unwrap()
    }
    fn count(ledger: &Ledger, table: &str) -> i64 {
        // Test-only table names are static call sites.
        ledger
            .connection
            .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                row.get(0)
            })
            .unwrap()
    }

    #[test]
    fn identical_live_and_completed_replays_do_not_create_sessions_or_attempts() {
        let store = Store::new();
        let mut a = store.open();
        let mut b = store.open();
        let request = request();
        let owner = accepted(a.accept_request(&request, settings(&store)).unwrap());
        assert!(matches!(
            b.accept_request(&request, settings(&store)).unwrap(),
            RequestDecision::InProgress { .. }
        ));
        assert_eq!(count(&b, "sessions"), 1);
        assert_eq!(count(&b, "attempts"), 0);
        let mut changed = request.clone();
        changed.turn.questions[0].prompt.push('?');
        assert!(matches!(
            b.accept_request(&changed, settings(&store)),
            Err(ReplayError::IntentConflict)
        ));
        let turn = finish(&mut a, &owner);
        drop(owner);
        let mut changed_settings = settings(&store);
        changed_settings.output = OutputFormat::Text;
        changed_settings.profile.model = "changed-default".into();
        match b.accept_request(&request, changed_settings).unwrap() {
            RequestDecision::Completed {
                turn: saved,
                frozen,
            } => {
                assert_eq!(*saved, turn);
                assert_eq!(frozen.settings.profile.model, "offline");
            }
            _ => panic!("expected completed replay"),
        }
        assert_eq!(count(&b, "sessions"), 1);
        assert_eq!(count(&b, "attempts"), 1);
        let json: EvaluationRequest = codec::decode(
            &serde_json::to_vec(&request).unwrap(),
            codec::InputFormat::Json,
        )
        .unwrap();
        let toon: EvaluationRequest = codec::decode(
            codec::encode_toon(&request).unwrap().as_bytes(),
            codec::InputFormat::Toon,
        )
        .unwrap();
        assert_eq!(fingerprint(&json).unwrap(), fingerprint(&toon).unwrap());
    }

    #[test]
    fn a_standing_change_that_no_longer_applies_leaves_no_reservation_or_session() {
        let store = Store::new();
        let mut ledger = store.open();
        let mut seed = request();
        seed.request_id = Some("seed".into());
        let owner = accepted(ledger.accept_request(&seed, settings(&store)).unwrap());
        finish(&mut ledger, &owner);
        let source = owner.frozen.session;
        drop(owner);
        // A drop of a question the session does not have, as when another
        // writer removed it between the preview and the acceptance.
        let mut conflicting = request();
        conflicting.standing.drop = vec!["not-asked".into()];
        for selection in [
            SessionSelection::New { name: None },
            SessionSelection::From {
                name: None,
                sources: vec![SourceSelection {
                    session: SessionSelector::Id(source),
                    frontier: None,
                }],
            },
            SessionSelection::Continue {
                session: SessionSelector::Id(source),
            },
        ] {
            conflicting.session = selection;
            let result = ledger
                .accept_request(&conflicting, settings(&store))
                .map(|_| ());
            assert!(matches!(result, Err(ReplayError::Turn(_))), "{result:?}");
            assert_eq!(count(&ledger, "sessions"), 1);
            assert_eq!(count(&ledger, "request_reservations"), 1);
            assert!(
                ledger
                    .saved_request_by_id("stable-request")
                    .unwrap()
                    .is_none()
            );
        }
        // The ledger is not left inside a transaction.
        let mut valid = request();
        valid.request_id = Some("next".into());
        valid.session = SessionSelection::Continue {
            session: SessionSelector::Id(source),
        };
        accepted(ledger.accept_request(&valid, settings(&store)).unwrap());
        assert_eq!(count(&ledger, "request_reservations"), 2);
    }

    #[test]
    fn exited_owner_distinguishes_not_dispatched_and_unknown_then_explicitly_retries() {
        let store = Store::new();
        let mut ledger = store.open();
        let request = request();
        let owner = accepted(ledger.accept_request(&request, settings(&store)).unwrap());
        let frozen = owner.frozen.clone();
        drop(owner);
        assert!(matches!(
            ledger.accept_request(&request, settings(&store)).unwrap(),
            RequestDecision::Interrupted {
                kind: RecoveryKind::NeverDispatched,
                ..
            }
        ));
        let owner = accepted(ledger.retry_interrupted_request(&request).unwrap());
        assert_eq!(owner.frozen, frozen);
        let pending = owner.prepare(&mut ledger, fixture().manifest).unwrap();
        ledger
            .record_attempt_progress(
                owner.guard(),
                pending.id,
                AttemptProgress {
                    state: AttemptStatus::Running,
                    error: None,
                },
            )
            .unwrap();
        drop(owner);
        assert!(matches!(
            ledger.accept_request(&request, settings(&store)).unwrap(),
            RequestDecision::Interrupted {
                kind: RecoveryKind::OutcomeUnknown,
                ..
            }
        ));
        assert_eq!(
            ledger.attempt(pending.id).unwrap().progress.state,
            AttemptStatus::Interrupted
        );
        let owner = accepted(ledger.retry_interrupted_request(&request).unwrap());
        let turn = finish(&mut ledger, &owner);
        assert_eq!(turn.session, frozen.session);
        assert_eq!(count(&ledger, "sessions"), 1);
        assert_eq!(count(&ledger, "attempts"), 2);
    }

    #[test]
    fn pending_followup_cannot_erase_prior_dispatch_or_unknown_evidence() {
        let store = Store::new();
        let mut ledger = store.open();
        let request = request();
        let owner = accepted(ledger.accept_request(&request, settings(&store)).unwrap());
        let a = owner.prepare(&mut ledger, fixture().manifest).unwrap();
        ledger
            .record_attempt_progress(
                owner.guard(),
                a.id,
                AttemptProgress {
                    state: AttemptStatus::Running,
                    error: None,
                },
            )
            .unwrap();
        ledger
            .record_attempt_progress(
                owner.guard(),
                a.id,
                AttemptProgress {
                    state: AttemptStatus::Failed,
                    error: Some(ErrorReport {
                        category: ErrorCategory::Transport,
                        code: "failure".into(),
                        message: "failed".into(),
                        retryable: true,
                    }),
                },
            )
            .unwrap();
        owner.prepare(&mut ledger, fixture().manifest).unwrap();
        drop(owner);
        assert!(matches!(
            ledger.accept_request(&request, settings(&store)).unwrap(),
            RequestDecision::Interrupted {
                kind: RecoveryKind::KnownFailure,
                ..
            }
        ));
        let owner = accepted(ledger.retry_interrupted_request(&request).unwrap());
        let b = owner.prepare(&mut ledger, fixture().manifest).unwrap();
        ledger
            .record_attempt_progress(
                owner.guard(),
                b.id,
                AttemptProgress {
                    state: AttemptStatus::Running,
                    error: None,
                },
            )
            .unwrap();
        owner.prepare(&mut ledger, fixture().manifest).unwrap();
        drop(owner);
        assert!(matches!(
            ledger.accept_request(&request, settings(&store)).unwrap(),
            RequestDecision::Interrupted {
                kind: RecoveryKind::OutcomeUnknown,
                ..
            }
        ));
        let owner = accepted(ledger.retry_interrupted_request(&request).unwrap());
        owner.prepare(&mut ledger, fixture().manifest).unwrap();
        drop(owner);
        assert!(matches!(
            ledger.accept_request(&request, settings(&store)).unwrap(),
            RequestDecision::Interrupted {
                kind: RecoveryKind::OutcomeUnknown,
                ..
            }
        ));
    }

    #[test]
    fn completed_replay_retains_source_snapshot_after_source_advances() {
        let store = Store::new();
        let mut ledger = store.open();
        let source = ledger
            .create_session(None, ModelSelection::default())
            .unwrap();
        let guard = ledger.session_guard(source.id).unwrap();
        let input = || AttemptInput {
            standing: Default::default(),
            request: fixture().request,
            updates: vec![],
            manifest: fixture().manifest,
            request_id: None,
        };
        let attempt = ledger.prepare_attempt(&guard, input()).unwrap();
        let first = ledger
            .finalize_attempt(&guard, attempt.id, fixture().outcome)
            .unwrap();
        let mut request = request();
        request.session = SessionSelection::From {
            sources: vec![SourceSelection {
                session: SessionSelector::Id(source.id),
                frontier: None,
            }],
            name: None,
        };
        let owner = accepted(ledger.accept_request(&request, settings(&store)).unwrap());
        let derived = finish(&mut ledger, &owner);
        drop(owner);
        assert_eq!(derived.parents, vec![first.id]);
        let attempt = ledger.prepare_attempt(&guard, input()).unwrap();
        let advanced = ledger
            .finalize_attempt(&guard, attempt.id, fixture().outcome)
            .unwrap();
        assert_ne!(first.id, advanced.id);
        match ledger.accept_request(&request, settings(&store)).unwrap() {
            RequestDecision::Completed { turn, frozen } => {
                assert_eq!(*turn, derived);
                assert_eq!(frozen.sources[0].frontier, vec![first.id]);
            }
            _ => panic!("expected completed replay"),
        }
    }
}
