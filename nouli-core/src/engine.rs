//! Reusable durable evaluation, with bounded attempts outside transactions.
pub mod execution;
use crate::{
    config::{
        Config, ConfigError,
        resolve::{RuntimeOverrides, select_model},
    },
    context::{
        builder::{BuildError, ContextBuilder, ContextInput, RequiredItem},
        compact::Staged,
    },
    history::{EvaluationRequest, FinalizedTurn, HistoryError, ModelSelection, SessionSelection},
    identity::Id,
    limits::{LimitError, ResolvedLimits, count::TokenCounter},
    outcome::ProviderIdentity,
    protocol::{ErrorCategory, ErrorReport},
    provider::ProviderError,
    storage::{
        StorageError,
        replay::{RecoveryKind, ReplayError, RequestDecision, RequestOwner},
        sessions::SessionError,
        turns::{AttemptProgress, AttemptStatus, TurnError},
        worker::{LedgerWorker, WorkerError},
    },
    transport::{TransportError, retry::RetryPolicy},
};
use execution::{DecisionProvider, RetryClock};
use serde::{Deserialize, Serialize};
use std::{path::Path, sync::Arc, time::Duration};
use thiserror::Error;
use tokio_util::sync::CancellationToken;

pub struct EvaluationInput<'a> {
    pub request: &'a EvaluationRequest,
    pub config: &'a Config,
    pub overrides: &'a RuntimeOverrides,
    pub home: &'a Path,
    pub retry_interrupted: bool,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EvaluationResult {
    pub request_id: String,
    pub turn: FinalizedTurn,
    pub replayed: bool,
}
#[derive(Debug, Error)]
pub enum EngineError {
    #[error(transparent)]
    Worker(#[from] WorkerError),
    #[error(transparent)]
    Config(#[from] ConfigError),
    #[error(transparent)]
    History(#[from] HistoryError),
    #[error(transparent)]
    Session(#[from] SessionError),
    #[error(transparent)]
    Storage(#[from] StorageError),
    #[error(transparent)]
    Replay(#[from] ReplayError),
    #[error(transparent)]
    Turn(#[from] TurnError),
    #[error(transparent)]
    Build(#[from] BuildError),
    #[error(transparent)]
    Limit(#[from] LimitError),
    #[error(transparent)]
    Provider(#[from] ProviderError),
    #[error("another live owner is evaluating this request")]
    InProgress { session: Option<Id> },
    #[error("request requires explicit interrupted retry ({kind:?})")]
    Interrupted { kind: RecoveryKind },
    #[error("evaluation canceled")]
    Canceled,
}

pub struct Engine<P, C, T> {
    pub provider: P,
    pub counter: C,
    pub clock: T,
}
impl<P: DecisionProvider, C: TokenCounter + Sync, T: RetryClock> Engine<P, C, T> {
    pub async fn evaluate(
        &self,
        ledger: &LedgerWorker,
        input: EvaluationInput<'_>,
        lookup: impl Fn(&str) -> Option<String>,
        cancel: &CancellationToken,
    ) -> Result<EvaluationResult, EngineError> {
        if cancel.is_cancelled() {
            return Err(EngineError::Canceled);
        }
        let mut request = input.request.clone();
        request.model = select_model(&[&input.overrides.selection, &request.model]);
        request.validate()?;
        let key = request
            .request_id
            .get_or_insert_with(|| Id::generate().to_string())
            .clone();
        let query = request.clone();
        let saved = ledger
            .call(move |ledger| ledger.saved_request(&query))
            .await??;
        let decision = if input.retry_interrupted {
            let query = request.clone();
            ledger
                .call(move |ledger| ledger.retry_interrupted_request(&query))
                .await??
        } else {
            let settings = if let Some(saved) = saved {
                saved.settings
            } else {
                let defaults = match &request.session {
                    SessionSelection::Continue { session } => {
                        let selector = session.clone();
                        ledger
                            .call(move |ledger| ledger.session(&selector))
                            .await??
                            .defaults
                    }
                    _ => ModelSelection::default(),
                };
                input.config.resolve_snapshot(
                    input.overrides,
                    &request.model,
                    &defaults,
                    input.home,
                )?
            };
            let query = request.clone();
            ledger
                .call(move |ledger| ledger.accept_request(&query, settings))
                .await??
        };
        let owner = match decision {
            RequestDecision::Completed { turn, .. } => {
                return Ok(EvaluationResult {
                    request_id: key,
                    turn: *turn,
                    replayed: true,
                });
            }
            RequestDecision::InProgress { session } => {
                return Err(EngineError::InProgress { session });
            }
            RequestDecision::Interrupted { kind, .. } => {
                return Err(EngineError::Interrupted { kind });
            }
            RequestDecision::Accepted(owner) => owner,
        };
        let owner: Arc<RequestOwner> = Arc::from(owner);
        let frozen = owner.frozen();
        let sources = frozen.sources.clone();
        let history = ledger
            .call(move |ledger| ledger.ancestry(&sources))
            .await??;
        let required: Vec<_> = frozen
            .request
            .required_records
            .iter()
            .map(|id| RequiredItem {
                record: *id,
                pointer: None,
            })
            .collect();
        let mut limits = ResolvedLimits::for_profile(&frozen.settings.profile)?;
        // Pending updates are labeled pending, never assigned a persisted turn ID.
        let context_turn = Id::generate();
        let build = |limits: &ResolvedLimits| {
            ContextBuilder::build(
                ContextInput {
                    history: &history,
                    sources: &frozen.sources,
                    request: &frozen.request.turn,
                    updates: &frozen.request.updates,
                    required: &required,
                    workflow_base: &[],
                    session: frozen.session,
                    turn: context_turn,
                    provider: ProviderIdentity {
                        provider: frozen.settings.profile_name.clone(),
                        model: frozen.settings.profile.model.clone(),
                        upstream_provider: None,
                        request_id: None,
                    },
                },
                limits,
                &self.counter,
                &Staged,
            )
        };
        let mut plan = build(&limits)?;
        let mut settings = frozen.settings.clone().with_credentials(lookup)?;
        let policy = RetryPolicy::new(&settings.snapshot.execution).map_err(ProviderError::from)?;
        let timeout = Duration::from_secs(settings.snapshot.execution.attempt_timeout_seconds);
        if tokio::time::Instant::now().checked_add(timeout).is_none() {
            return Err(ConfigError::Invalid("attempt timeout").into());
        }
        for number in 1..=settings.snapshot.execution.max_attempts {
            if cancel.is_cancelled() {
                return Err(EngineError::Canceled);
            }
            let writer = owner.clone();
            let manifest = plan.manifest.clone();
            let attempt = ledger
                .call(move |ledger| {
                    let attempt = writer.prepare(ledger, manifest)?;
                    ledger.record_attempt_progress(
                        writer.guard(),
                        attempt.id,
                        AttemptProgress {
                            state: AttemptStatus::Running,
                            error: None,
                        },
                    )?;
                    Ok::<_, EngineError>(attempt.id)
                })
                .await??;
            let dispatched = tokio::select! {biased;
                _=cancel.cancelled()=>Err(ProviderError::Transport(TransportError::Canceled)),
                result=tokio::time::timeout(timeout,self.provider.attempt(&settings,&frozen.request.turn,&plan.manifest.context_toon,&self.counter,cancel))=>result.unwrap_or(Err(ProviderError::Transport(TransportError::Timeout))),
            };
            let error = match dispatched {
                Ok(outcome) => {
                    if let Err(error) = outcome.validate(&frozen.request.turn) {
                        ProviderError::Contract(error)
                    } else {
                        let writer = owner.clone();
                        let turn = ledger
                            .call(move |ledger| {
                                ledger.finalize_attempt(writer.guard(), attempt, outcome)
                            })
                            .await??;
                        return Ok(EvaluationResult {
                            request_id: key,
                            turn,
                            replayed: false,
                        });
                    }
                }
                Err(error) => error,
            };
            let canceled = matches!(error, ProviderError::Transport(TransportError::Canceled));
            let delay = if let ProviderError::Transport(transport) = &error {
                policy.next_delay(number, transport, self.clock.now())
            } else {
                None
            };
            let writer = owner.clone();
            let progress = AttemptProgress {
                state: if canceled {
                    AttemptStatus::Interrupted
                } else {
                    AttemptStatus::Failed
                },
                error: Some(error_report(&error, delay.is_some())),
            };
            ledger
                .call(move |ledger| {
                    ledger.record_attempt_progress(writer.guard(), attempt, progress)
                })
                .await??;
            if canceled {
                return Err(EngineError::Canceled);
            }
            if number < settings.snapshot.execution.max_attempts
                && matches!(error, ProviderError::Transport(TransportError::Capacity))
            {
                let effective = plan
                    .manifest
                    .input_count
                    .tokens
                    .checked_mul(3)
                    .and_then(|n| n.checked_div(4))
                    .and_then(|n| n.checked_add(limits.reserved_output_tokens))
                    .ok_or(LimitError::Overflow)?;
                if effective >= limits.budget || effective <= limits.reserved_output_tokens {
                    return Err(error.into());
                }
                limits.budget = effective;
                let smaller = build(&limits)?;
                if smaller.manifest.context_toon.len() >= plan.manifest.context_toon.len() {
                    return Err(error.into());
                }
                plan = smaller;
                settings.snapshot.profile.context_budget_tokens = Some(effective);
                continue;
            }
            let Some(delay) = delay else {
                return Err(error.into());
            };
            if !self.clock.wait(delay, cancel).await {
                return Err(EngineError::Canceled);
            }
        }
        Err(EngineError::Provider(ProviderError::Transport(
            TransportError::Settings,
        )))
    }
}

fn error_report(error: &ProviderError, retryable: bool) -> ErrorReport {
    let (category, code) = match error {
        ProviderError::Transport(TransportError::Canceled | TransportError::Timeout) => {
            (ErrorCategory::Interrupted, "provider_outcome_unknown")
        }
        ProviderError::Transport(TransportError::Capacity) | ProviderError::Limit(_) => {
            (ErrorCategory::Capacity, "capacity")
        }
        ProviderError::Transport(TransportError::Authentication { .. }) => {
            (ErrorCategory::Authentication, "authentication")
        }
        ProviderError::Transport(_) => (ErrorCategory::Transport, "provider_transport"),
        _ => (ErrorCategory::Contract, "provider_contract"),
    };
    ErrorReport {
        category,
        code: code.into(),
        message: error.to_string(),
        retryable,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        config::{
            ExecutionSettings, OutputFormat, Protocol, ProviderProfile, resolve::ResolvedSettings,
        },
        context::{ContextKind, ContextRecord, ContextUpdate, PinTarget},
        data::{Data, Input},
        decision::TurnRequest,
        history::SourceSelection,
        identity::SessionSelector,
        limits::count::ByteCounter,
        outcome::{QuestionOutcome, TurnOutcome},
        storage::{Ledger, OpenMode},
    };
    use std::{
        collections::{BTreeMap, VecDeque},
        path::PathBuf,
        sync::Mutex,
        time::SystemTime,
    };
    struct TestLedger {
        worker: LedgerWorker,
    }
    impl TestLedger {
        fn list_sessions(&self) -> Result<Vec<crate::storage::sessions::Session>, SessionError> {
            self.worker.inspect(|ledger| ledger.list_sessions())
        }
        fn list_attempts(
            &self,
            session: Id,
        ) -> Result<Vec<crate::storage::turns::AttemptSnapshot>, TurnError> {
            self.worker.inspect(|ledger| ledger.list_attempts(session))
        }
        fn saved_request(
            &self,
            request: &EvaluationRequest,
        ) -> Result<Option<crate::storage::replay::FrozenRequest>, ReplayError> {
            self.worker.inspect(|ledger| ledger.saved_request(request))
        }
        fn session(
            &self,
            selector: &SessionSelector,
        ) -> Result<crate::storage::sessions::Session, SessionError> {
            self.worker.inspect(|ledger| ledger.session(selector))
        }
        fn session_guard(
            &self,
            session: Id,
        ) -> Result<crate::storage::guards::SessionGuard, crate::storage::guards::GuardError>
        {
            self.worker.inspect(|ledger| ledger.session_guard(session))
        }
    }
    struct Store(PathBuf);
    impl Store {
        fn new() -> Self {
            Self(std::env::temp_dir().join(format!("nouli-engine-{}", Id::generate())))
        }
        fn open(&self) -> TestLedger {
            TestLedger {
                worker: Ledger::open(&self.0, OpenMode::ReadWriteCreate)
                    .unwrap()
                    .into(),
            }
        }
    }
    impl Drop for Store {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    fn request() -> EvaluationRequest {
        EvaluationRequest {
            turn: serde_json::from_str(include_str!(
                "../../fixtures/contracts/decision-request.json"
            ))
            .unwrap(),
            session: SessionSelection::New { name: None },
            updates: vec![],
            required_records: vec![],
            model: ModelSelection::default(),
            request_id: Some(Id::generate().to_string()),
        }
    }
    fn outcome() -> TurnOutcome {
        serde_json::from_str(include_str!(
            "../../fixtures/contracts/decision-outcome.json"
        ))
        .unwrap()
    }
    fn config() -> Config {
        Config {
            default_provider: Some("fixture".into()),
            output: OutputFormat::Json,
            store: None,
            execution: ExecutionSettings {
                attempt_timeout_seconds: 1,
                ..ExecutionSettings::default()
            },
            providers: BTreeMap::from([(
                "fixture".into(),
                ProviderProfile {
                    protocol: Protocol::Typesafe,
                    base_url: "http://127.0.0.1:8080".into(),
                    model: "offline".into(),
                    api_key_env: None,
                    context_budget_tokens: Some(100_000),
                    context_window_tokens: Some(100_000),
                    reserved_output_tokens: Some(128),
                    max_attempts: None,
                    attempt_timeout_seconds: None,
                },
            )]),
        }
    }
    #[derive(Clone)]
    struct Call {
        request: TurnRequest,
        evidence: String,
        model: String,
    }
    #[derive(Default)]
    struct Fake {
        results: Mutex<VecDeque<Result<TurnOutcome, ProviderError>>>,
        calls: Mutex<Vec<Call>>,
        fault: Option<PathBuf>,
    }
    impl Fake {
        fn errors(errors: Vec<ProviderError>) -> Self {
            Self {
                results: Mutex::new(errors.into_iter().map(Err).collect()),
                ..Self::default()
            }
        }
    }
    impl DecisionProvider for Fake {
        async fn attempt<C: TokenCounter + Sync>(
            &self,
            settings: &ResolvedSettings,
            request: &TurnRequest,
            evidence: &str,
            _counter: &C,
            _cancel: &CancellationToken,
        ) -> Result<TurnOutcome, ProviderError> {
            self.calls.lock().unwrap().push(Call {
                request: request.clone(),
                evidence: evidence.into(),
                model: settings.snapshot.profile.model.clone(),
            });
            if let Some(path) = &self.fault {
                let mut ledger = Ledger::open(path, OpenMode::ReadWriteCreate).unwrap();
                ledger.transaction::<_,StorageError>("inject finalization failure",|tx| {tx.execute_batch("CREATE TRIGGER IF NOT EXISTS reject_engine_finalization BEFORE UPDATE OF state ON attempt_states WHEN NEW.state='completed' BEGIN SELECT RAISE(ABORT,'fault'); END;").unwrap();Ok(())}).unwrap();
            }
            match self.results.lock().unwrap().pop_front() {
                Some(result) => result,
                None => {
                    let mut outcome = outcome();
                    outcome.provider.provider = settings.snapshot.profile_name.clone();
                    outcome.provider.model = settings.snapshot.profile.model.clone();
                    Ok(outcome)
                }
            }
        }
    }
    #[derive(Default)]
    struct FastClock {
        delays: Mutex<Vec<Duration>>,
    }
    impl RetryClock for FastClock {
        fn now(&self) -> SystemTime {
            SystemTime::UNIX_EPOCH
        }
        async fn wait(&self, delay: Duration, cancel: &CancellationToken) -> bool {
            self.delays.lock().unwrap().push(delay);
            !cancel.is_cancelled()
        }
    }
    fn make_engine(provider: Fake) -> Engine<Fake, ByteCounter, FastClock> {
        Engine {
            provider,
            counter: ByteCounter,
            clock: FastClock::default(),
        }
    }
    async fn run<P: DecisionProvider, C: TokenCounter + Sync>(
        engine: &Engine<P, C, FastClock>,
        ledger: &mut TestLedger,
        config: &Config,
        request: &EvaluationRequest,
        retry: bool,
        cancel: &CancellationToken,
    ) -> Result<EvaluationResult, EngineError> {
        engine
            .evaluate(
                &ledger.worker,
                EvaluationInput {
                    request,
                    config,
                    overrides: &RuntimeOverrides::default(),
                    home: Path::new("/unused"),
                    retry_interrupted: retry,
                },
                |_| Some("TEST_TOKEN".into()),
                cancel,
            )
            .await
    }
    fn make_record(value: i64) -> ContextRecord {
        ContextRecord {
            id: Id::generate(),
            key: "budget".into(),
            value: Data::new(serde_json::json!(value)).unwrap(),
            kind: ContextKind::Commitment,
        }
    }
    #[tokio::test]
    async fn first_continued_and_branched_turns_preserve_manifest_updates_refusals_and_replay() {
        let store = Store::new();
        let mut ledger = store.open();
        let engine = make_engine(Fake::default());
        let config = config();
        let cancel = CancellationToken::new();
        let mut first = request();
        let record = make_record(8);
        first.updates.push(ContextUpdate::Pin {
            target: PinTarget::New {
                record: record.clone(),
            },
        });
        let saved = run(&engine, &mut ledger, &config, &first, false, &cancel)
            .await
            .unwrap();
        assert!(!saved.replayed);
        assert!(matches!(
            saved.turn.outcome.outcomes[3],
            QuestionOutcome::Refused { .. }
        ));
        let call = engine.provider.calls.lock().unwrap()[0].clone();
        assert_eq!(call.evidence, saved.turn.manifest.context_toon);
        assert_eq!(call.request.questions, saved.turn.request.questions);
        assert_eq!(call.model, "offline");
        let evidence: serde_json::Value =
            crate::codec::decode(call.evidence.as_bytes(), crate::codec::InputFormat::Toon)
                .unwrap();
        assert!(evidence["records"][0]["value"]["created_turn"].is_null());
        assert_eq!(
            evidence["records"][0]["value"]["revision"]["kind"],
            "pending"
        );
        let replay = run(
            &engine,
            &mut ledger,
            &Config::default(),
            &first,
            false,
            &cancel,
        )
        .await
        .unwrap();
        assert!(replay.replayed);
        assert_eq!(replay.turn, saved.turn);
        assert_eq!(engine.provider.calls.lock().unwrap().len(), 1);
        let mut next = request();
        next.session = SessionSelection::Continue {
            session: SessionSelector::Id(saved.turn.session),
        };
        next.updates = vec![ContextUpdate::Supersede {
            targets: vec![record.id],
            replacement: make_record(12),
            pinned: None,
        }];
        let continued = run(&engine, &mut ledger, &config, &next, false, &cancel)
            .await
            .unwrap();
        assert_eq!(continued.turn.parents, vec![saved.turn.id]);
        let mut branch = request();
        branch.session = SessionSelection::From {
            sources: vec![SourceSelection {
                session: SessionSelector::Id(saved.turn.session),
                frontier: Some(vec![saved.turn.id]),
            }],
            name: None,
        };
        let branch = run(&engine, &mut ledger, &config, &branch, false, &cancel)
            .await
            .unwrap();
        assert_ne!(branch.turn.session, saved.turn.session);
        assert_eq!(branch.turn.parents, vec![saved.turn.id]);
        assert_eq!(
            ledger
                .session(&SessionSelector::Id(saved.turn.session))
                .unwrap()
                .frontier,
            vec![continued.turn.id]
        );
    }
    #[tokio::test]
    async fn retries_record_each_actual_attempt_without_provider_model_fallback() {
        let store = Store::new();
        let mut ledger = store.open();
        let engine = make_engine(Fake::errors(vec![
            ProviderError::Transport(TransportError::Timeout),
            ProviderError::Transport(TransportError::RateLimited {
                retry_after: Some(crate::transport::retry::RetryAfter::Delay(
                    Duration::from_secs(4),
                )),
            }),
        ]));
        let request = request();
        let result = run(
            &engine,
            &mut ledger,
            &config(),
            &request,
            false,
            &CancellationToken::new(),
        )
        .await
        .unwrap();
        let attempts = ledger.list_attempts(result.turn.session).unwrap();
        assert_eq!(
            attempts
                .iter()
                .map(|attempt| attempt.progress.state)
                .collect::<Vec<_>>(),
            vec![
                AttemptStatus::Failed,
                AttemptStatus::Failed,
                AttemptStatus::Completed
            ]
        );
        assert_eq!(engine.provider.calls.lock().unwrap().len(), 3);
        assert_eq!(
            *engine.clock.delays.lock().unwrap(),
            vec![Duration::from_secs(1), Duration::from_secs(4)]
        );
        for call in engine.provider.calls.lock().unwrap().iter() {
            assert_eq!(call.evidence, result.turn.manifest.context_toon);
            assert_eq!(call.model, "offline");
        }
        let permanent = make_engine(Fake::errors(vec![ProviderError::Transport(
            TransportError::Authentication { status: 401 },
        )]));
        assert!(
            run(
                &permanent,
                &mut ledger,
                &config(),
                &super::tests::request(),
                false,
                &CancellationToken::new()
            )
            .await
            .is_err()
        );
        assert_eq!(permanent.provider.calls.lock().unwrap().len(), 1);
    }
    #[tokio::test]
    async fn capacity_precedes_dispatch_and_failed_finalization_never_reports_saved_success() {
        let store = Store::new();
        let mut ledger = store.open();
        let engine = make_engine(Fake::default());
        let mut huge = request();
        huge.turn.state = vec![Input::Text {
            text: "large".repeat(100_000),
        }];
        assert!(matches!(
            run(
                &engine,
                &mut ledger,
                &config(),
                &huge,
                false,
                &CancellationToken::new()
            )
            .await,
            Err(EngineError::Build(BuildError::ProtectedCapacity))
        ));
        assert!(engine.provider.calls.lock().unwrap().is_empty());
        let fault = make_engine(Fake {
            fault: Some(store.0.clone()),
            ..Fake::default()
        });
        assert!(matches!(
            run(
                &fault,
                &mut ledger,
                &config(),
                &request(),
                false,
                &CancellationToken::new()
            )
            .await,
            Err(EngineError::Turn(_))
        ));
        for session in ledger.list_sessions().unwrap() {
            assert!(session.frontier.is_empty());
        }
        let attempts = ledger
            .list_sessions()
            .unwrap()
            .into_iter()
            .map(|session| ledger.list_attempts(session.id).unwrap().len())
            .sum::<usize>();
        assert_eq!(attempts, 1);
    }
    struct Gate {
        entered: tokio::sync::Notify,
        release: tokio::sync::Notify,
    }
    impl DecisionProvider for Gate {
        async fn attempt<C: TokenCounter + Sync>(
            &self,
            settings: &ResolvedSettings,
            _request: &TurnRequest,
            _evidence: &str,
            _counter: &C,
            _cancel: &CancellationToken,
        ) -> Result<TurnOutcome, ProviderError> {
            self.entered.notify_one();
            self.release.notified().await;
            let mut outcome = outcome();
            outcome.provider.provider = settings.snapshot.profile_name.clone();
            outcome.provider.model = settings.snapshot.profile.model.clone();
            Ok(outcome)
        }
    }
    #[tokio::test]
    async fn cancellation_releases_guards_and_other_sessions_execute_during_provider_waits() {
        let store = Store::new();
        let mut ledger = store.open();
        let mut other = store.open();
        let config = config();
        let request = request();
        let cancel = CancellationToken::new();
        let blocked = Engine {
            provider: Gate {
                entered: tokio::sync::Notify::new(),
                release: tokio::sync::Notify::new(),
            },
            counter: ByteCounter,
            clock: FastClock::default(),
        };
        let independent = make_engine(Fake::default());
        let evaluate = run(&blocked, &mut ledger, &config, &request, false, &cancel);
        let control = async {
            blocked.provider.entered.notified().await;
            assert!(matches!(
                run(
                    &independent,
                    &mut other,
                    &config,
                    &request,
                    false,
                    &CancellationToken::new()
                )
                .await,
                Err(EngineError::InProgress { .. })
            ));
            let fresh = super::tests::request();
            let done = run(
                &independent,
                &mut other,
                &config,
                &fresh,
                false,
                &CancellationToken::new(),
            )
            .await
            .unwrap();
            assert!(!done.replayed);
            cancel.cancel();
        };
        let (result, ()) = tokio::join!(evaluate, control);
        assert!(matches!(result, Err(EngineError::Canceled)));
        let frozen = ledger.saved_request(&request).unwrap().unwrap();
        assert!(ledger.session_guard(frozen.session).is_ok());
        assert_eq!(
            ledger.list_attempts(frozen.session).unwrap()[0]
                .progress
                .state,
            AttemptStatus::Interrupted
        );
        assert!(matches!(
            run(
                &independent,
                &mut ledger,
                &config,
                &request,
                false,
                &CancellationToken::new()
            )
            .await,
            Err(EngineError::Interrupted {
                kind: RecoveryKind::OutcomeUnknown
            })
        ));
        let retried = run(
            &independent,
            &mut ledger,
            &config,
            &request,
            true,
            &CancellationToken::new(),
        )
        .await
        .unwrap();
        assert_eq!(retried.turn.session, frozen.session);
        assert_eq!(ledger.list_attempts(frozen.session).unwrap().len(), 2);
    }
    struct Synthetic;
    impl TokenCounter for Synthetic {
        fn count(
            &self,
            _protocol: Protocol,
            _model: &str,
            evidence: &str,
            questions: &[crate::decision::Question],
        ) -> Result<crate::limits::count::InputCounts, LimitError> {
            let evidence: serde_json::Value =
                crate::codec::decode(evidence.as_bytes(), crate::codec::InputFormat::Toon)?;
            let mut tokens = 100;
            for record in evidence["records"].as_array().unwrap() {
                tokens += if record["kind"] == "context" {
                    10
                } else if record["representation"] == "compacted" {
                    100
                } else {
                    200
                };
            }
            let count = crate::history::TokenCount {
                tokens,
                exact: false,
                method: crate::history::CountMethod::TokenizerEstimate,
            };
            Ok(crate::limits::count::InputCounts {
                total: count.clone(),
                per_question: vec![count; questions.len()],
            })
        }
    }
    #[tokio::test]
    async fn native_capacity_replans_only_when_evidence_shrinks_and_remains_attempt_bounded() {
        let store = Store::new();
        let mut ledger = store.open();
        let config = config();
        let cancel = CancellationToken::new();
        let seed = run(
            &make_engine(Fake::default()),
            &mut ledger,
            &config,
            &request(),
            false,
            &cancel,
        )
        .await
        .unwrap();
        let mut continuation = request();
        continuation.session = SessionSelection::Continue {
            session: SessionSelector::Id(seed.turn.session),
        };
        let engine = Engine {
            provider: Fake::errors(vec![ProviderError::Transport(TransportError::Capacity)]),
            counter: Synthetic,
            clock: FastClock::default(),
        };
        let result = run(&engine, &mut ledger, &config, &continuation, false, &cancel)
            .await
            .unwrap();
        let calls = engine.provider.calls.lock().unwrap().clone();
        assert_eq!(calls.len(), 2);
        assert!(calls[1].evidence.len() < calls[0].evidence.len());
        assert_eq!(result.turn.manifest.budget, 353);
        assert!(
            result
                .turn
                .manifest
                .records
                .iter()
                .any(|record| record.representation == crate::history::Representation::Compacted)
        );
        assert_eq!(ledger.list_attempts(seed.turn.session).unwrap().len(), 3);
        let fresh = Engine {
            provider: Fake::errors(vec![ProviderError::Transport(TransportError::Capacity)]),
            counter: Synthetic,
            clock: FastClock::default(),
        };
        assert!(matches!(
            run(&fresh, &mut ledger, &config, &request(), false, &cancel).await,
            Err(EngineError::Build(BuildError::ProtectedCapacity))
        ));
        assert_eq!(fresh.provider.calls.lock().unwrap().len(), 1);
    }
    #[tokio::test]
    async fn blending_reports_source_conflict_before_dispatch_and_accepts_explicit_resolution() {
        let store = Store::new();
        let mut ledger = store.open();
        let engine = make_engine(Fake::default());
        let config = config();
        let cancel = CancellationToken::new();
        let mut root = request();
        let original = make_record(8);
        root.updates = vec![ContextUpdate::Pin {
            target: PinTarget::New {
                record: original.clone(),
            },
        }];
        let root = run(&engine, &mut ledger, &config, &root, false, &cancel)
            .await
            .unwrap();
        let mut branches = Vec::new();
        let mut variants = Vec::new();
        for value in [6, 20] {
            let mut request = request();
            request.session = SessionSelection::From {
                sources: vec![SourceSelection {
                    session: SessionSelector::Id(root.turn.session),
                    frontier: None,
                }],
                name: None,
            };
            let replacement = make_record(value);
            variants.push(replacement.id);
            request.updates = vec![ContextUpdate::Supersede {
                targets: vec![original.id],
                replacement,
                pinned: None,
            }];
            let branch = run(&engine, &mut ledger, &config, &request, false, &cancel)
                .await
                .unwrap();
            branches.push(SourceSelection {
                session: SessionSelector::Id(branch.turn.session),
                frontier: None,
            });
        }
        let mut blend = request();
        blend.session = SessionSelection::From {
            sources: branches,
            name: None,
        };
        assert!(matches!(
            run(&engine, &mut ledger, &config, &blend, false, &cancel).await,
            Err(EngineError::Build(BuildError::Projection(
                crate::context::projection::ProjectionError::Conflicts { .. }
            )))
        ));
        assert_eq!(engine.provider.calls.lock().unwrap().len(), 3);
        blend.request_id = Some(Id::generate().to_string());
        blend.updates = vec![ContextUpdate::Supersede {
            targets: variants,
            replacement: make_record(12),
            pinned: None,
        }];
        let result = run(&engine, &mut ledger, &config, &blend, false, &cancel)
            .await
            .unwrap();
        assert_eq!(result.turn.parents.len(), 2);
        assert_eq!(engine.provider.calls.lock().unwrap().len(), 4);
    }
    #[tokio::test(start_paused = true)]
    async fn owned_timeout_is_bounded_and_preserves_unknown_outcome_evidence() {
        let store = Store::new();
        let mut ledger = store.open();
        let mut config = config();
        config.execution.max_attempts = 1;
        let request = request();
        let cancel = CancellationToken::new();
        let engine = Engine {
            provider: Gate {
                entered: tokio::sync::Notify::new(),
                release: tokio::sync::Notify::new(),
            },
            counter: ByteCounter,
            clock: FastClock::default(),
        };
        assert!(matches!(
            run(&engine, &mut ledger, &config, &request, false, &cancel).await,
            Err(EngineError::Provider(ProviderError::Transport(
                TransportError::Timeout
            )))
        ));
        let frozen = ledger.saved_request(&request).unwrap().unwrap();
        let attempts = ledger.list_attempts(frozen.session).unwrap();
        assert_eq!(attempts.len(), 1);
        assert_eq!(
            attempts[0].progress.error.as_ref().unwrap().code,
            "provider_outcome_unknown"
        );
        assert!(matches!(
            run(
                &make_engine(Fake::default()),
                &mut ledger,
                &config,
                &request,
                false,
                &cancel
            )
            .await,
            Err(EngineError::Interrupted {
                kind: RecoveryKind::OutcomeUnknown
            })
        ));
    }
    #[tokio::test(flavor = "current_thread")]
    async fn contended_sqlite_worker_does_not_block_runtime_timers() {
        let store = Store::new();
        let mut ledger = store.open();
        let (ready_tx, ready_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let root = store.0.clone();
        let holder = std::thread::spawn(move || {
            let mut holder = Ledger::open(&root, OpenMode::ReadWriteCreate).unwrap();
            holder
                .transaction::<_, StorageError>("hold contended writer", |_transaction| {
                    ready_tx.send(()).unwrap();
                    release_rx.recv_timeout(Duration::from_secs(5)).unwrap();
                    Ok(())
                })
                .unwrap();
        });
        ready_rx.await.unwrap();
        let start = std::time::Instant::now();
        let engine = make_engine(Fake::default());
        let config = config();
        let request = request();
        let cancel = CancellationToken::new();
        let evaluate = run(&engine, &mut ledger, &config, &request, false, &cancel);
        let timer = async {
            tokio::time::sleep(Duration::from_millis(10)).await;
            let elapsed = start.elapsed();
            release_tx.send(()).unwrap();
            elapsed
        };
        let (result, elapsed) = tokio::join!(evaluate, timer);
        tokio::task::spawn_blocking(move || holder.join().unwrap())
            .await
            .unwrap();
        assert!(
            elapsed < Duration::from_secs(1),
            "unrelated timer must run while SQLite waits"
        );
        assert!(result.is_ok());
    }
}
