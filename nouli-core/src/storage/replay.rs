//! Store-wide request reservations with frozen intent and explicit recovery.

use rusqlite::{Connection, Transaction};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use super::{
    Ledger, StorageError, freeze_settings, freeze_store,
    guards::{self, FileGuard, GuardError, SessionGuard},
    reservations::{self, ReservationKind, ReservationState},
    sessions::{self, SessionError},
    sql_error,
    turns::{self, AttemptInput, AttemptProgress, AttemptStatus, PreparedAttempt, TurnError},
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
    protocol::{NOT_DISPATCHED, OUTCOME_UNKNOWN},
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

/// Decode an owned request, validating its known frozen-settings field first.
pub(super) fn decode_frozen_request(payload: &str) -> Result<FrozenRequest, StorageError> {
    let value = codec::decode_value(payload.as_bytes(), codec::InputFormat::Toon)
        .map_err(|_| StorageError::Payload)?;
    super::validate_frozen_settings_version(value.get("settings").ok_or(StorageError::Payload)?)?;
    serde_json::from_value(value).map_err(|_| StorageError::Payload)
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
    #[must_use]
    pub fn frozen(&self) -> &FrozenRequest {
        &self.frozen
    }
    #[must_use]
    pub fn guard(&self) -> &SessionGuard {
        &self.session_guard
    }
    /// Prepares the reserved request's attempt with the frozen intent and the
    /// given manifest.
    ///
    /// # Errors
    ///
    /// Returns [`ReplayError::Session`] if the session cannot be read,
    /// [`ReplayError::Conflict`] if its head moved since the request was
    /// frozen, or [`ReplayError::Turn`] if [`Ledger::prepare_attempt`] fails.
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
                request: self.frozen.request.turn.clone(),
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
    #[error("no interrupted request has this request ID")]
    UnknownRequest,
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
///
/// # Errors
///
/// Returns [`ReplayError::History`] if the request is invalid, or
/// [`ReplayError::Fingerprint`] if its intent cannot be hashed.
pub fn fingerprint(request: &EvaluationRequest) -> Result<String, ReplayError> {
    request.validate()?;
    intent_hash(request)
}

/// The hash of a request's intent, without checking it against the rules for
/// new requests, so a request saved by an earlier release still matches its
/// stored hash. `standing` stays in the intent: 0.2.0 hashes include it.
fn intent_hash(request: &EvaluationRequest) -> Result<String, ReplayError> {
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
    state: ReservationState,
    frozen: FrozenRequest,
    result: Option<String>,
    recovery: Option<RecoveryKind>,
}

fn reservation(connection: &Connection, key: &str) -> Result<Option<Reservation>, ReplayError> {
    if reservations::matches_kind(connection, key, ReservationKind::Evaluation)?
        .is_some_and(|matches| !matches)
    {
        return Err(ReplayError::IntentConflict);
    }
    let row = reservations::evaluation(connection, key)?;
    row.map(
        |reservations::EvaluationRow {
             session,
             fingerprint: hash,
             state,
             frozen,
             result,
             recovery,
         }| {
            let frozen = decode_frozen_request(&frozen)?;
            let session = Id::try_from(session).map_err(|_| StorageError::Payload)?;
            if frozen.session != session || intent_hash(&frozen.request)? != hash {
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
        },
    )
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
    if saved.state == ReservationState::Interrupted {
        return Ok(saved.recovery.unwrap_or(RecoveryKind::Interrupted));
    }
    let mut statement=transaction.prepare("SELECT a.id,a.payload_toon,s.state,(SELECT payload_toon FROM attempt_events e WHERE e.attempt_id=a.id ORDER BY sequence DESC LIMIT 1) FROM attempts a JOIN attempt_states s ON s.attempt_id=a.id WHERE a.session_id=?1 ORDER BY a.id").map_err(|error|sql_error("inspect orphan attempts",&error))?;
    let rows = statement
        .query_map([saved.session.to_string()], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
            ))
        })
        .map_err(|error| sql_error("read orphan attempts", &error))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| sql_error("read orphan rows", &error))?;
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
                    .is_some_and(|error| error.code == OUTCOME_UNKNOWN)
                {
                    RecoveryKind::OutcomeUnknown
                } else {
                    RecoveryKind::KnownFailure
                }
            }
            "interrupted" => {
                let progress: AttemptProgress = Ledger::decode_payload(&progress)?;
                match progress.error.as_ref().map(|error| error.code.as_str()) {
                    Some(OUTCOME_UNKNOWN) => RecoveryKind::OutcomeUnknown,
                    Some(NOT_DISPATCHED) => RecoveryKind::NeverDispatched,
                    _ => RecoveryKind::Interrupted,
                }
            }
            "pending" => RecoveryKind::NeverDispatched,
            _ => return Err(StorageError::Payload.into()),
        };
        if recovery_priority(observed) > recovery_priority(kind) {
            kind = observed;
        }
        turns::interrupt_attempt(
            transaction,
            Id::try_from(id).map_err(|_| StorageError::Payload)?,
            AttemptStatus::parse(&state)?,
            "Request owner exited before finalization",
        )?;
    }
    reservations::interrupt_evaluation(transaction, key, &Ledger::encode_payload(&kind)?)?;
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
    let row = reservations::owner(&ledger.connection, key, guard.session)?;
    let (owner, state, frozen) = row.ok_or(StorageError::Conflict)?;
    let frozen = decode_frozen_request(&frozen)?;
    if owner != guard.owner.to_string()
        || state != ReservationState::Pending
        || input.request != frozen.request.turn
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
    ///
    /// # Errors
    ///
    /// - Any error from [`fingerprint`] for the request.
    /// - [`ReplayError::IntentConflict`] if the request ID already identifies
    ///   different intent or a non-evaluation request.
    /// - [`ReplayError::Storage`] or [`ReplayError::Fingerprint`] if the SQLite
    ///   query fails or the saved reservation is invalid.
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
    ///
    /// # Errors
    ///
    /// Returns [`ReplayError::IntentConflict`] if the ID identifies a
    /// non-evaluation request, or [`ReplayError::Storage`] or
    /// [`ReplayError::Fingerprint`] if the SQLite query fails or the saved
    /// reservation is invalid.
    pub fn saved_request_by_id(&self, key: &str) -> Result<Option<FrozenRequest>, ReplayError> {
        Ok(reservation(&self.connection, key)?.map(|saved| saved.frozen))
    }
    /// Reserves a request ID with frozen intent and settings, or reports the
    /// existing reservation's completed, in-progress, or interrupted state.
    ///
    /// # Errors
    ///
    /// - [`ReplayError::MissingId`] if the request ID is missing or blank.
    /// - [`ReplayError::History`] or [`ReplayError::Fingerprint`] if the
    ///   request is invalid or cannot be hashed.
    /// - [`ReplayError::IntentConflict`] if the ID already identifies
    ///   different intent.
    /// - [`ReplayError::Guard`] if the ledger is read-only or another writer
    ///   owns the session.
    /// - [`ReplayError::Session`] or [`ReplayError::Source`] if the selected
    ///   session or its sources cannot be resolved or created.
    /// - [`ReplayError::Storage`] if the settings are invalid, the store path
    ///   cannot be resolved, or the SQLite write fails.
    pub fn accept_request(
        &mut self,
        request: &EvaluationRequest,
        settings: SettingsSnapshot,
    ) -> Result<RequestDecision, ReplayError> {
        self.request_decision(request, Some(settings), false)
    }
    /// Retakes ownership of an interrupted reservation for the same request
    /// under its frozen intent.
    ///
    /// # Errors
    ///
    /// - [`ReplayError::MissingId`] if the request ID is missing or blank.
    /// - [`ReplayError::UnknownRequest`] if no reservation exists for it.
    /// - [`ReplayError::History`] or [`ReplayError::Fingerprint`] if the
    ///   request is invalid or cannot be hashed.
    /// - [`ReplayError::IntentConflict`] if the ID identifies different intent.
    /// - [`ReplayError::Guard`] if the ledger is read-only or another writer
    ///   owns the session.
    /// - [`ReplayError::Conflict`] if the session head moved since the request
    ///   was frozen.
    /// - [`ReplayError::Session`] or [`ReplayError::Storage`] if reading the
    ///   session or the SQLite write fails.
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
            .is_some_and(|saved| saved.state == ReservationState::Completed)
        {
            return completed(existing.ok_or(StorageError::Payload)?);
        }
        let Some(request_guard) =
            reservations::lock_request(self, key).map_err(|error| match error {
                reservations::LockError::Fingerprint => ReplayError::Fingerprint,
                reservations::LockError::Guard(error) => ReplayError::Guard(error),
            })?
        else {
            return Ok(RequestDecision::InProgress {
                session: existing.map(|saved| saved.session),
            });
        };
        if let Some(saved) = reservation(&self.connection, key)? {
            return self.resume_reservation(key, &hash, saved, retry, request_guard);
        }
        if retry {
            return Err(ReplayError::UnknownRequest);
        }
        let mut settings = settings.ok_or(StorageError::Payload)?;
        freeze_settings(&settings).map_err(|_| StorageError::Payload)?;
        settings.store = freeze_store(self, "freeze store path")?;
        self.reserve_new_request(request, key, &hash, settings, request_guard)
    }

    /// Settles a reservation seen under the request lock: reports it completed
    /// or interrupted, or, for an explicit retry, retakes its ownership.
    fn resume_reservation(
        &mut self,
        key: &str,
        hash: &str,
        saved: Reservation,
        retry: bool,
        request_guard: FileGuard,
    ) -> Result<RequestDecision, ReplayError> {
        if saved.fingerprint != hash {
            return Err(ReplayError::IntentConflict);
        }
        if saved.state == ReservationState::Completed {
            return completed(saved);
        }
        let kind = self.transaction::<_, ReplayError>("recover exited owner", |transaction| {
            interrupt(transaction, key, &saved)
        })?;
        if !retry {
            return Ok(RequestDecision::Interrupted {
                kind,
                frozen: Box::new(saved.frozen),
            });
        }
        self.retake_interrupted_reservation(key, saved, request_guard)
    }

    /// Reassigns an interrupted reservation to a new owner, provided the
    /// session head has not moved since the request was frozen.
    fn retake_interrupted_reservation(
        &mut self,
        key: &str,
        saved: Reservation,
        request_guard: FileGuard,
    ) -> Result<RequestDecision, ReplayError> {
        let guard = guards::acquire_session(self, saved.session)?;
        let current = self.session(&SessionSelector::Id(saved.session))?;
        if current.frontier != saved.frozen.frontier || current.revision != saved.frozen.revision {
            return Err(ReplayError::Conflict);
        }
        self.transaction::<_, StorageError>("retry original reservation", |transaction| {
            reservations::reclaim_evaluation(transaction, key, guard.owner)
        })?;
        Ok(accepted(key, saved.frozen, guard, request_guard))
    }

    /// Reserves a request ID that has never been used: resolves or creates the
    /// target session, captures its sources, and saves the frozen request.
    fn reserve_new_request(
        &mut self,
        request: &EvaluationRequest,
        key: &str,
        hash: &str,
        settings: SettingsSnapshot,
        request_guard: FileGuard,
    ) -> Result<RequestDecision, ReplayError> {
        let target = sessions::target(self, &request.session)?;
        let id = target.id;
        let guard = guards::acquire_session(self, id)?;
        let frozen =
            self.transaction::<_, ReplayError>("accept unique request", |transaction| {
                let (current, captured) = sessions::open_target::<ReplayError>(
                    transaction,
                    target,
                    &request.session,
                    ModelSelection {
                        provider: Some(settings.profile_name.clone()),
                        model: Some(settings.profile.model.clone()),
                    },
                )?;
                let frozen = FrozenRequest {
                    request: request.clone(),
                    settings,
                    sources: captured,
                    session: id,
                    frontier: current.frontier,
                    revision: current.revision,
                };
                reservations::insert_evaluation(
                    transaction,
                    frozen.session,
                    key,
                    hash,
                    guard.owner,
                    &Ledger::encode_payload(&frozen)?,
                )?;
                Ok(frozen)
            })?;
        Ok(accepted(key, frozen, guard, request_guard))
    }
}

fn accepted(
    key: &str,
    frozen: FrozenRequest,
    session_guard: SessionGuard,
    request_guard: FileGuard,
) -> RequestDecision {
    RequestDecision::Accepted(Box::new(RequestOwner {
        key: key.into(),
        frozen,
        session_guard,
        _request_guard: request_guard,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ConnectionSettings;
    use crate::config::ModelSettings;
    use crate::config::{ExecutionSettings, OutputFormat, Protocol, ProviderProfile};
    use crate::history::SourceSelection;
    use crate::protocol::{ErrorCategory, ErrorReport};
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
            standing: StandingChanges::default(),
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
            format_version: 2,
            profile_name: "fixture".into(),
            profile: ProviderProfile {
                connection: ConnectionSettings::default(),
                limits: ModelSettings::default(),
                protocol: Protocol::SystemOne,
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
                &AttemptProgress {
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
    fn tampered_frozen_versions_keep_the_unsupported_settings_report() {
        let store = Store::new();
        let mut ledger = store.open();
        let owner = accepted(ledger.accept_request(&request(), settings(&store)).unwrap());
        let mut frozen = serde_json::to_value(owner.frozen()).unwrap();
        for version in [None, Some(serde_json::json!(0)), Some(serde_json::json!(3))] {
            let settings = frozen["settings"].as_object_mut().unwrap();
            if let Some(version) = version {
                settings.insert("format_version".into(), version);
            } else {
                settings.remove("format_version");
            }
            let payload = Ledger::encode_payload(&frozen).unwrap();
            let error = decode_frozen_request(&payload).unwrap_err();
            assert_eq!(error.report().code, "unsupported_frozen_settings");
            let snapshot_payload = Ledger::encode_payload(&frozen["settings"]).unwrap();
            let error = Ledger::decode_payload::<SettingsSnapshot>(&snapshot_payload).unwrap_err();
            assert_eq!(error.report().code, "invalid_saved_payload");
            let error = Ledger::decode_settings(&snapshot_payload).unwrap_err();
            assert_eq!(error.report().code, "unsupported_frozen_settings");
            let workflow_payload = Ledger::encode_payload(&vec![&frozen["settings"]]).unwrap();
            let error = super::super::workflows::decode_settings(&workflow_payload).unwrap_err();
            assert_eq!(error.report().code, "unsupported_frozen_settings");
        }
    }

    #[test]
    fn accepted_request_json_matches_the_string_state_export() {
        use super::super::inspection::AcceptedRequest;

        #[derive(Serialize)]
        struct PreviousAcceptedRequest<'a> {
            request_id: &'a str,
            state: &'a str,
            frozen: &'a FrozenRequest,
        }

        let store = Store::new();
        let mut ledger = store.open();
        let request = request();
        let owner = accepted(ledger.accept_request(&request, settings(&store)).unwrap());
        for (state, previous_state) in [
            (ReservationState::Pending, "pending"),
            (ReservationState::Completed, "completed"),
            (ReservationState::Interrupted, "interrupted"),
        ] {
            let current = AcceptedRequest {
                request_id: "stable-request".into(),
                state,
                frozen: owner.frozen.clone(),
            };
            let previous = PreviousAcceptedRequest {
                request_id: "stable-request",
                state: previous_state,
                frozen: owner.frozen(),
            };
            let json = serde_json::to_vec(&current).unwrap();
            assert_eq!(json, serde_json::to_vec(&previous).unwrap());
            assert_eq!(
                serde_json::from_slice::<AcceptedRequest>(&json).unwrap(),
                current
            );
        }
    }

    #[test]
    fn retrying_an_unknown_request_id_is_not_reported_as_missing() {
        let store = Store::new();
        let mut ledger = store.open();
        assert!(matches!(
            ledger.retry_interrupted_request(&request()),
            Err(ReplayError::UnknownRequest)
        ));
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
                &AttemptProgress {
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
                &AttemptProgress {
                    state: AttemptStatus::Running,
                    error: None,
                },
            )
            .unwrap();
        ledger
            .record_attempt_progress(
                owner.guard(),
                a.id,
                &AttemptProgress {
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
                &AttemptProgress {
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
            standing: StandingChanges::default(),
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
