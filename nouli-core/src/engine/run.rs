//! Shared attempt execution, reduction and retries for saved and ephemeral work.
use super::{
    Engine, EngineError,
    execution::{DecisionProvider, RetryClock},
};
use crate::{
    config::{
        ConfigError,
        resolve::{ResolvedSettings, SettingsSnapshot},
    },
    context::{
        builder::{BuildError, ContextBuilder, ContextInput, ContextPlan, RequiredItem},
        compact::Staged,
    },
    decision::TurnRequest,
    history::{EvaluationManifest, EvaluationRequest, FinalizedTurn, SourcePoint},
    identity::Id,
    limits::{LimitError, ResolvedLimits, count::TokenCounter},
    outcome::{ProviderIdentity, TurnOutcome},
    protocol::ErrorReport,
    provider::ProviderError,
    storage::{
        replay::RequestOwner,
        turns::{AttemptProgress, AttemptStatus},
        worker::LedgerWorker,
    },
    transport::{TransportError, retry::RetryPolicy},
};
use std::{future::Future, sync::Arc, time::Duration};
use tokio_util::sync::CancellationToken;
pub(super) struct RunInput<'a> {
    pub context_base: Option<&'a crate::history::base::ContextBase>,
    pub runtime_only: bool,
    pub request: &'a EvaluationRequest,
    pub sources: &'a [SourcePoint],
    pub history: &'a [FinalizedTurn],
    pub settings: SettingsSnapshot,
    pub session: Id,
}
/// A recording policy whose attempt identity and completed result cannot be mixed
/// between durable and ephemeral execution.
pub(super) trait Recorder: Sync {
    type Attempt: Send;
    type Output: Send;

    fn begin(
        &self,
        manifest: EvaluationManifest,
    ) -> impl Future<Output = Result<Self::Attempt, EngineError>> + Send;
    fn fail(
        &self,
        attempt: Self::Attempt,
        progress: AttemptProgress,
    ) -> impl Future<Output = Result<(), EngineError>> + Send;
    fn succeed(
        &self,
        attempt: Self::Attempt,
        outcome: TurnOutcome,
        manifest: EvaluationManifest,
    ) -> impl Future<Output = Result<Self::Output, EngineError>> + Send;
}

pub(super) enum DurableRecorder<'a> {
    Evaluation {
        worker: &'a LedgerWorker,
        owner: Arc<RequestOwner>,
    },
    Workflow {
        worker: &'a LedgerWorker,
        owner: Arc<crate::storage::workflows::WorkflowOwner>,
        intent: Arc<crate::storage::workflows::nodes::NodeIntent>,
    },
}
impl Recorder for DurableRecorder<'_> {
    type Attempt = Id;
    type Output = FinalizedTurn;

    async fn begin(&self, manifest: EvaluationManifest) -> Result<Id, EngineError> {
        match self {
            Self::Evaluation { worker, owner } => {
                let writer = owner.clone();
                worker
                    .call(move |ledger| {
                        let attempt = writer.prepare(ledger, manifest)?;
                        ledger.record_attempt_progress(
                            writer.guard(),
                            attempt.id,
                            &AttemptProgress {
                                state: AttemptStatus::Running,
                                error: None,
                            },
                        )?;
                        Ok::<_, EngineError>(attempt.id)
                    })
                    .await?
            }
            Self::Workflow {
                worker,
                owner,
                intent,
            } => {
                let writer = owner.clone();
                let intent = intent.clone();
                Ok(worker
                    .call(move |ledger| ledger.prepare_workflow_attempt(&writer, &intent, manifest))
                    .await??
                    .id)
            }
        }
    }
    async fn fail(&self, attempt: Id, progress: AttemptProgress) -> Result<(), EngineError> {
        match self {
            Self::Evaluation { worker, owner } => {
                let writer = owner.clone();
                worker
                    .call(move |ledger| {
                        ledger.record_attempt_progress(writer.guard(), attempt, &progress)
                    })
                    .await??;
            }
            Self::Workflow { worker, owner, .. } => {
                let writer = owner.clone();
                worker
                    .call(move |ledger| ledger.fail_workflow_attempt(&writer, attempt, &progress))
                    .await??;
            }
        }
        Ok(())
    }
    async fn succeed(
        &self,
        attempt: Id,
        outcome: TurnOutcome,
        _manifest: EvaluationManifest,
    ) -> Result<FinalizedTurn, EngineError> {
        match self {
            Self::Evaluation { worker, owner } => {
                let writer = owner.clone();
                Ok(worker
                    .call(move |ledger| ledger.finalize_attempt(writer.guard(), attempt, outcome))
                    .await??)
            }
            Self::Workflow { worker, owner, .. } => {
                let writer = owner.clone();
                Ok(worker
                    .call(move |ledger| ledger.finalize_workflow_attempt(&writer, attempt, outcome))
                    .await??)
            }
        }
    }
}

pub(super) struct EphemeralRecorder;
impl Recorder for EphemeralRecorder {
    type Attempt = ();
    type Output = (TurnOutcome, EvaluationManifest);

    fn begin(
        &self,
        _manifest: EvaluationManifest,
    ) -> impl Future<Output = Result<(), EngineError>> + Send {
        std::future::ready(Ok(()))
    }
    fn fail(
        &self,
        (): (),
        _progress: AttemptProgress,
    ) -> impl Future<Output = Result<(), EngineError>> + Send {
        std::future::ready(Ok(()))
    }
    fn succeed(
        &self,
        (): (),
        outcome: TurnOutcome,
        mut manifest: EvaluationManifest,
    ) -> impl Future<Output = Result<Self::Output, EngineError>> + Send {
        manifest.provider = outcome.provider.clone();
        std::future::ready(
            manifest
                .validate()
                .map(|()| (outcome, manifest))
                .map_err(EngineError::from),
        )
    }
}
/// The serialized plan stays separate from its disposable image cache.
struct PlannedContext {
    plan: ContextPlan,
    images: crate::media::PreparedImages,
}
fn build_plan(
    input: ContextInput<'_>,
    limits: &ResolvedLimits,
    counter: &(impl TokenCounter + ?Sized),
) -> Result<PlannedContext, BuildError> {
    let images = crate::media::PreparedImages::default();
    let plan = ContextBuilder::build_prepared(input, limits, counter, &Staged, &images)?;
    Ok(PlannedContext { plan, images })
}
/// Owned input lets a CPU-only planning job finish safely after cancellation.
/// It contains neither ledger guards nor provider handles.
struct OwnedPlanningInput {
    context_base: Option<crate::history::base::ContextBase>,
    runtime_only: bool,
    request: EvaluationRequest,
    sources: Vec<SourcePoint>,
    history: Vec<FinalizedTurn>,
    settings: SettingsSnapshot,
    session: Id,
    context_turn: Id,
    required: Vec<RequiredItem>,
}
impl OwnedPlanningInput {
    fn build(
        &self,
        limits: &ResolvedLimits,
        counter: &dyn TokenCounter,
    ) -> Result<PlannedContext, BuildError> {
        build_plan(
            ContextInput {
                context_base: self.context_base.as_ref(),
                runtime_only: self.runtime_only,
                history: &self.history,
                sources: &self.sources,
                request: &self.request.turn,
                updates: &self.request.updates,
                required: &self.required,
                workflow_base: &[],
                session: self.session,
                turn: self.context_turn,
                provider: ProviderIdentity {
                    provider: self.settings.profile_name.clone(),
                    model: self.settings.profile.model.clone(),
                    upstream_provider: None,
                    request_id: None,
                },
            },
            limits,
            counter,
        )
    }
}
/// Builds context plans so capacity retries can rebuild under a smaller budget.
struct ContextPlanner<'a, C> {
    input: &'a RunInput<'a>,
    required: Vec<RequiredItem>,
    context_turn: Id,
    counter: &'a C,
    owned: Option<(Arc<OwnedPlanningInput>, Arc<dyn TokenCounter + Send + Sync>)>,
}
impl<'a, C: TokenCounter> ContextPlanner<'a, C> {
    fn new(input: &'a RunInput<'a>, counter: &'a C) -> Self {
        let required: Vec<_> = input
            .request
            .required_records
            .iter()
            .map(|id| RequiredItem {
                record: *id,
                pointer: None,
            })
            .collect();
        let context_turn = Id::generate();
        let owned = counter.snapshot().map(|counter| {
            (
                Arc::new(OwnedPlanningInput {
                    context_base: input.context_base.cloned(),
                    runtime_only: input.runtime_only,
                    request: input.request.clone(),
                    sources: input.sources.to_vec(),
                    history: input.history.to_vec(),
                    settings: input.settings.clone(),
                    session: input.session,
                    context_turn,
                    required: required.clone(),
                }),
                counter,
            )
        });
        Self {
            input,
            required,
            context_turn,
            counter,
            owned,
        }
    }
    fn synchronous(&self, limits: &ResolvedLimits) -> Result<PlannedContext, BuildError> {
        let input = self.input;
        build_plan(
            ContextInput {
                context_base: input.context_base,
                runtime_only: input.runtime_only,
                history: input.history,
                sources: input.sources,
                request: &input.request.turn,
                updates: &input.request.updates,
                required: &self.required,
                workflow_base: &[],
                session: input.session,
                turn: self.context_turn,
                provider: ProviderIdentity {
                    provider: input.settings.profile_name.clone(),
                    model: input.settings.profile.model.clone(),
                    upstream_provider: None,
                    request_id: None,
                },
            },
            limits,
            self.counter,
        )
    }
    async fn build(
        &self,
        limits: &ResolvedLimits,
        cancel: &CancellationToken,
    ) -> Result<PlannedContext, EngineError> {
        let Some((input, counter)) = &self.owned else {
            return Ok(self.synchronous(limits)?);
        };
        let input = input.clone();
        let counter = counter.clone();
        let limits = limits.clone();
        let mut task = tokio::task::spawn_blocking(move || input.build(&limits, &*counter));
        tokio::select! { biased;
            () = cancel.cancelled() => { task.abort(); Err(EngineError::Canceled) },
            result = &mut task => Ok(result.map_err(|_| EngineError::Internal("context planning task stopped"))??),
        }
    }
    /// Rebuild under three quarters of the input tokens plus reserved output.
    /// Only publish the reduced budget when the result is shorter.
    async fn shrink(
        &self,
        plan: &PlannedContext,
        limits: &mut ResolvedLimits,
        cancel: &CancellationToken,
    ) -> Result<Option<PlannedContext>, EngineError> {
        let effective = plan
            .plan
            .manifest
            .input_count
            .tokens
            .checked_mul(3)
            .and_then(|n| n.checked_div(4))
            .and_then(|n| n.checked_add(limits.reserved_output_tokens))
            .ok_or(LimitError::Overflow)?;
        if effective >= limits.budget || effective <= limits.reserved_output_tokens {
            return Ok(None);
        }
        let reduced = ResolvedLimits {
            budget: effective,
            ..limits.clone()
        };
        let smaller = self.build(&reduced, cancel).await?;
        if smaller.plan.manifest.context_toon.len() >= plan.plan.manifest.context_toon.len() {
            return Ok(None);
        }
        *limits = reduced;
        Ok(Some(smaller))
    }
}
impl<P: DecisionProvider, C: TokenCounter + Sync, T: RetryClock> Engine<P, C, T> {
    pub(super) async fn execute<R: Recorder>(
        &self,
        input: RunInput<'_>,
        recorder: R,
        lookup: impl Fn(&crate::config::auth::CredentialSource) -> Option<String>,
        cancel: &CancellationToken,
    ) -> Result<R::Output, EngineError> {
        let planner = ContextPlanner::new(&input, &self.counter);
        let mut limits = ResolvedLimits::for_profile(&input.settings.profile)?;
        let mut plan = planner.build(&limits, cancel).await?;
        let mut settings = input.settings.clone().with_credentials(lookup)?;
        let policy = RetryPolicy::new(&settings.snapshot.execution).map_err(ProviderError::from)?;
        let timeout = attempt_timeout(&settings)?;
        let max_attempts = settings.snapshot.execution.max_attempts;
        for number in 1..=max_attempts {
            if cancel.is_cancelled() {
                return Err(EngineError::Canceled);
            }
            let attempt = recorder.begin(plan.plan.manifest.clone()).await?;
            let error = match self
                .attempt(&settings, &input.request.turn, &plan, timeout, cancel)
                .await
            {
                Ok(outcome) => return recorder.succeed(attempt, outcome, plan.plan.manifest).await,
                Err(error) => error,
            };
            let delay = self.retry_delay(&policy, number, &error);
            recorder
                .fail(attempt, failure_progress(&error, delay.is_some()))
                .await?;
            if is_canceled(&error) {
                return Err(EngineError::Canceled);
            }
            if number < max_attempts
                && matches!(error, ProviderError::Transport(TransportError::Capacity))
            {
                let Some(smaller) = planner.shrink(&plan, &mut limits, cancel).await? else {
                    return Err(error.into());
                };
                plan = smaller;
                settings.snapshot.profile.context_budget_tokens = Some(limits.budget);
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
    /// Dispatch one provider attempt under the timeout and validate its outcome.
    async fn attempt(
        &self,
        settings: &ResolvedSettings,
        request: &TurnRequest,
        plan: &PlannedContext,
        timeout: Duration,
        cancel: &CancellationToken,
    ) -> Result<TurnOutcome, ProviderError> {
        let dispatched = tokio::select! {biased;
            ()=cancel.cancelled()=>Err(ProviderError::Transport(TransportError::Canceled)),
            result=tokio::time::timeout(timeout,self.provider.attempt_prepared(settings,request,&plan.plan.manifest.context_toon,&self.counter,cancel,&plan.images))=>result.unwrap_or(Err(ProviderError::Transport(TransportError::Timeout))),
        };
        let outcome = dispatched?;
        outcome.validate(request).map_err(ProviderError::Contract)?;
        Ok(outcome)
    }
    /// The backoff before another attempt, if the policy permits one for this error.
    fn retry_delay(
        &self,
        policy: &RetryPolicy,
        completed_attempts: u32,
        error: &ProviderError,
    ) -> Option<Duration> {
        if let ProviderError::Transport(transport) = error {
            policy.next_delay(completed_attempts, transport, self.clock.now())
        } else {
            None
        }
    }
}

/// The per-attempt timeout, rejected when the deadline would overflow the clock.
fn attempt_timeout(settings: &ResolvedSettings) -> Result<Duration, ConfigError> {
    let timeout = Duration::from_secs(settings.snapshot.execution.attempt_timeout_seconds);
    if tokio::time::Instant::now().checked_add(timeout).is_none() {
        return Err(ConfigError::Invalid("attempt timeout"));
    }
    Ok(timeout)
}

fn is_canceled(error: &ProviderError) -> bool {
    matches!(error, ProviderError::Transport(TransportError::Canceled))
}

fn failure_progress(error: &ProviderError, retryable: bool) -> AttemptProgress {
    AttemptProgress {
        state: if is_canceled(error) {
            AttemptStatus::Interrupted
        } else {
            AttemptStatus::Failed
        },
        error: Some(error_report(error, retryable)),
    }
}

/// The saved attempt error: the final report's category and code, with this attempt's
/// retry decision and the error's own safe diagnostic text, such as its HTTP status.
fn error_report(error: &ProviderError, retryable: bool) -> ErrorReport {
    ErrorReport {
        message: error.to_string(),
        retryable,
        ..super::reporting::provider(error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        codec::CodecError,
        decision::ContractError,
        protocol::{ErrorCategory, OUTCOME_UNKNOWN},
    };

    fn reducible_history() -> FinalizedTurn {
        let fixture: serde_json::Value =
            serde_json::from_str(include_str!("../../../fixtures/contracts/history.json")).unwrap();
        let mut saved: FinalizedTurn = serde_json::from_value(fixture["turns"][0].clone()).unwrap();
        saved.request.state.push(crate::data::Input::Text {
            text: "optional history ".repeat(4000),
        });
        saved
    }

    #[test]
    fn each_plan_prepares_an_image_once_across_reduction_assessment_and_wire() {
        use crate::{
            config::Protocol,
            context::{ContextKind, ContextRecord, ContextUpdate},
            data::{Data, Input},
            history::{ModelSelection, SessionSelection, StandingChanges},
            limits::count::ByteCounter,
            media::PreparedImages,
        };
        let saved = reducible_history();
        let image = crate::media::tests::picture(280, 280);
        let record = ContextRecord {
            id: Id::generate(),
            key: "image:plan".into(),
            value: Data::new(serde_json::json!({"nouli_image":image})).unwrap(),
            kind: ContextKind::Evidence,
        };
        let mut request = saved.request.clone();
        request.state = vec![Input::Text {
            text: "Inspect [photo #123]".into(),
        }];
        let profile = crate::limits::count::tests::profile(Protocol::SystemOne, "fixture");
        let mut limits = ResolvedLimits::for_profile(&profile).unwrap();
        limits.budget = 4000;
        let sources = vec![SourcePoint {
            session: saved.session,
            frontier: vec![saved.id],
        }];
        let history = vec![saved];
        let evaluation = EvaluationRequest {
            turn: request,
            updates: vec![ContextUpdate::Append { record }],
            standing: StandingChanges::default(),
            session: SessionSelection::New { name: None },
            model: ModelSelection::default(),
            required_records: vec![],
            request_id: None,
        };
        for _ in 0..2 {
            let images = PreparedImages::default();
            let plan = ContextBuilder::build_prepared(
                ContextInput {
                    context_base: None,
                    runtime_only: true,
                    history: &history,
                    sources: &sources,
                    request: &evaluation.turn,
                    updates: &evaluation.updates,
                    required: &[],
                    workflow_base: &[],
                    session: Id::generate(),
                    turn: Id::generate(),
                    provider: ProviderIdentity {
                        provider: "fixture".into(),
                        model: "fixture".into(),
                        upstream_provider: None,
                        request_id: None,
                    },
                },
                &limits,
                &ByteCounter,
                &Staged,
                &images,
            )
            .unwrap();
            assert!(
                plan.entries
                    .iter()
                    .any(|entry| entry.representation != crate::history::Representation::Full)
            );
            limits
                .assess_prepared(
                    &plan.manifest.context_toon,
                    &evaluation.turn,
                    &ByteCounter,
                    &images,
                )
                .unwrap();
            let cached = crate::provider::wire::body_prepared(
                &profile,
                &evaluation.turn,
                &plan.manifest.context_toon,
                &images,
            )
            .unwrap();
            assert_eq!(images.preparations(), 1);
            let independent = crate::provider::wire::body(
                &profile,
                &evaluation.turn,
                &plan.manifest.context_toon,
            )
            .unwrap();
            assert_eq!(
                serde_json::to_vec(&cached).unwrap(),
                serde_json::to_vec(&independent).unwrap()
            );
        }
    }

    struct UncalledProvider;
    impl DecisionProvider for UncalledProvider {
        fn attempt<C: TokenCounter + Sync>(
            &self,
            _settings: &ResolvedSettings,
            _request: &TurnRequest,
            _evidence: &str,
            _counter: &C,
            _cancel: &CancellationToken,
        ) -> impl std::future::Future<Output = Result<TurnOutcome, ProviderError>> + Send {
            std::future::ready(Err(ProviderError::Transport(TransportError::Contract)))
        }
    }
    #[tokio::test(flavor = "current_thread")]
    async fn owned_context_planning_keeps_current_thread_cancellation_responsive() {
        use crate::{
            config::{Config, resolve::RuntimeOverrides},
            engine::EvaluationInput,
            history::{ModelSelection, SessionSelection, StandingChanges},
        };
        let mut gate = crate::limits::count::tests::CounterGate::new();
        let engine = Engine {
            provider: UncalledProvider,
            counter: gate.counter.clone(),
            clock: crate::engine::execution::TokioClock,
        };
        let config =
            Config::parse(include_str!("../../../fixtures/config/providers.toml")).unwrap();
        let request = EvaluationRequest {
            turn: serde_json::from_str(include_str!(
                "../../../fixtures/contracts/decision-request.json"
            ))
            .unwrap(),
            session: SessionSelection::New { name: None },
            standing: StandingChanges::default(),
            updates: vec![],
            required_records: vec![],
            model: ModelSelection::default(),
            request_id: None,
        };
        let cancel = CancellationToken::new();
        let stopping = cancel.clone();
        let task = tokio::spawn(async move {
            engine
                .evaluate_ephemeral(
                    None,
                    EvaluationInput {
                        request: &request,
                        config: &config,
                        overrides: &RuntimeOverrides::default(),
                        home: std::path::Path::new("/unused"),
                        retry_interrupted: false,
                    },
                    |_| None,
                    &stopping,
                )
                .await
        });
        tokio::time::timeout(Duration::from_secs(2), gate.started.recv())
            .await
            .unwrap()
            .unwrap();
        assert!(
            gate.finished.try_recv().is_err(),
            "planning blocked the current-thread executor until the gate timed out"
        );
        cancel.cancel();
        assert!(matches!(
            tokio::time::timeout(Duration::from_secs(1), task)
                .await
                .unwrap()
                .unwrap(),
            Err(EngineError::Canceled)
        ));
        gate.release();
        tokio::time::timeout(Duration::from_secs(2), gate.finished.recv())
            .await
            .unwrap()
            .unwrap();
    }

    #[derive(Clone, Copy)]
    struct PanickingCounter;
    impl TokenCounter for PanickingCounter {
        fn count(
            &self,
            _profile: &crate::config::ProviderProfile,
            _context: &str,
            _questions: &[crate::decision::Question],
        ) -> Result<crate::limits::count::InputCounts, LimitError> {
            panic!("test blocking counter stopped")
        }
        fn snapshot(&self) -> Option<Arc<dyn TokenCounter + Send + Sync>> {
            Some(Arc::new(*self))
        }
    }
    #[tokio::test(flavor = "current_thread")]
    async fn a_failed_context_worker_is_internal_instead_of_a_storage_or_input_error() {
        use crate::{
            config::{Config, resolve::RuntimeOverrides},
            engine::EvaluationInput,
            history::{ModelSelection, SessionSelection, StandingChanges},
        };
        let engine = Engine {
            provider: UncalledProvider,
            counter: PanickingCounter,
            clock: crate::engine::execution::TokioClock,
        };
        let config =
            Config::parse(include_str!("../../../fixtures/config/providers.toml")).unwrap();
        let request = EvaluationRequest {
            turn: serde_json::from_str(include_str!(
                "../../../fixtures/contracts/decision-request.json"
            ))
            .unwrap(),
            session: SessionSelection::New { name: None },
            standing: StandingChanges::default(),
            updates: vec![],
            required_records: vec![],
            model: ModelSelection::default(),
            request_id: None,
        };
        let error = engine
            .evaluate_ephemeral(
                None,
                EvaluationInput {
                    request: &request,
                    config: &config,
                    overrides: &RuntimeOverrides::default(),
                    home: std::path::Path::new("/unused"),
                    retry_interrupted: false,
                },
                |_| None,
                &CancellationToken::new(),
            )
            .await
            .unwrap_err();
        assert!(matches!(
            error,
            EngineError::Internal("context planning task stopped")
        ));
        assert_eq!(
            error.report().category,
            crate::protocol::ErrorCategory::Internal
        );
        assert_eq!(error.report().code, "engine_internal");
    }

    fn every_provider_error() -> Vec<ProviderError> {
        let transport = [
            TransportError::Settings,
            TransportError::Endpoint,
            TransportError::Build,
            TransportError::Canceled,
            TransportError::Timeout,
            TransportError::Connect,
            TransportError::Body,
            TransportError::Truncated,
            TransportError::Oversize { limit: 1 },
            TransportError::Authentication { status: 401 },
            TransportError::RateLimited { retry_after: None },
            TransportError::Capacity,
            TransportError::Capability,
            TransportError::Contract,
            TransportError::Status {
                status: 500,
                retry_after: None,
            },
        ];
        let limit = [
            LimitError::InvalidBudget,
            LimitError::Modality,
            LimitError::Count,
            LimitError::RequestBytes,
            LimitError::TotalCapacity,
            LimitError::QuestionCapacity,
            LimitError::Overflow,
            LimitError::Counter,
            LimitError::Profile,
            LimitError::Contract(ContractError::Empty("questions")),
            LimitError::Codec(CodecError::Syntax),
            LimitError::Codec(CodecError::ImageReferenceConflict),
        ];
        [
            ProviderError::Protocol,
            ProviderError::Profile,
            ProviderError::Wire,
            ProviderError::Codec(CodecError::Syntax),
            ProviderError::Contract(ContractError::Empty("questions")),
        ]
        .into_iter()
        .chain(transport.into_iter().map(ProviderError::Transport))
        .chain(limit.into_iter().map(ProviderError::Limit))
        .collect()
    }

    #[test]
    fn saved_attempt_errors_match_the_final_report_for_every_provider_error() {
        for (error, same) in every_provider_error()
            .into_iter()
            .zip(every_provider_error())
        {
            let report = EngineError::Provider(same).report();
            for retryable in [false, true] {
                let attempt = failure_progress(&error, retryable).error.unwrap();
                assert_eq!(
                    (attempt.category, attempt.code.as_str()),
                    (report.category, report.code.as_str()),
                    "{error:?}"
                );
                assert_eq!(attempt.retryable, retryable);
                assert_eq!(attempt.message, error.to_string());
            }
        }
    }

    /// Recovery reads this code back from saved attempts (ADR 0001), so a call that
    /// may have reached the provider must keep it.
    #[test]
    fn canceled_and_timed_out_attempts_keep_the_outcome_unknown_code() {
        for (error, state, category) in [
            (
                TransportError::Canceled,
                AttemptStatus::Interrupted,
                ErrorCategory::Interrupted,
            ),
            (
                TransportError::Timeout,
                AttemptStatus::Failed,
                ErrorCategory::Transport,
            ),
        ] {
            let progress = failure_progress(&ProviderError::Transport(error), false);
            assert_eq!(progress.state, state);
            let report = progress.error.unwrap();
            assert_eq!(report.code, OUTCOME_UNKNOWN);
            assert_eq!(report.category, category);
        }
    }
}
