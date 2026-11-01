//! Shared attempt execution, reduction and retries for saved and ephemeral work.
use super::{
    Engine, EngineError,
    execution::{DecisionProvider, RetryClock},
};
use crate::{
    config::{ConfigError, resolve::SettingsSnapshot},
    context::{
        builder::{ContextBuilder, ContextInput, RequiredItem},
        compact::Staged,
    },
    history::{EvaluationManifest, EvaluationRequest, FinalizedTurn, SourcePoint},
    identity::Id,
    limits::{LimitError, ResolvedLimits, count::TokenCounter},
    outcome::{ProviderIdentity, TurnOutcome},
    protocol::{ErrorCategory, ErrorReport},
    provider::ProviderError,
    storage::{
        StorageError,
        replay::RequestOwner,
        turns::{AttemptProgress, AttemptStatus},
        worker::LedgerWorker,
    },
    transport::{TransportError, retry::RetryPolicy},
};
use std::{sync::Arc, time::Duration};
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
pub(super) enum Executed {
    Saved(Box<FinalizedTurn>),
    Ephemeral {
        outcome: Box<TurnOutcome>,
        manifest: Box<EvaluationManifest>,
    },
}
enum Target<'a> {
    Evaluation {
        worker: &'a LedgerWorker,
        owner: Arc<RequestOwner>,
    },
    Workflow {
        worker: &'a LedgerWorker,
        owner: Arc<crate::storage::workflows::WorkflowOwner>,
        intent: Arc<crate::storage::workflows::nodes::NodeIntent>,
    },
    Ephemeral,
}
pub(super) struct Recorder<'a> {
    target: Target<'a>,
    attempt: Option<Id>,
}
impl<'a> Recorder<'a> {
    pub fn durable(worker: &'a LedgerWorker, owner: Arc<RequestOwner>) -> Self {
        Self {
            target: Target::Evaluation { worker, owner },
            attempt: None,
        }
    }
    pub fn workflow(
        worker: &'a LedgerWorker,
        owner: Arc<crate::storage::workflows::WorkflowOwner>,
        intent: crate::storage::workflows::nodes::NodeIntent,
    ) -> Self {
        Self {
            target: Target::Workflow {
                worker,
                owner,
                intent: Arc::new(intent),
            },
            attempt: None,
        }
    }
    pub fn ephemeral() -> Self {
        Self {
            target: Target::Ephemeral,
            attempt: None,
        }
    }
    async fn begin(&mut self, manifest: EvaluationManifest) -> Result<(), EngineError> {
        self.attempt = match &self.target {
            Target::Evaluation { worker, owner } => {
                let writer = owner.clone();
                Some(
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
                        .await??,
                )
            }
            Target::Workflow {
                worker,
                owner,
                intent,
            } => {
                let writer = owner.clone();
                let intent = intent.clone();
                Some(
                    worker
                        .call(move |ledger| {
                            ledger.prepare_workflow_attempt(&writer, &intent, manifest)
                        })
                        .await??
                        .id,
                )
            }
            Target::Ephemeral => None,
        };
        Ok(())
    }
    async fn fail(&self, progress: AttemptProgress) -> Result<(), EngineError> {
        match &self.target {
            Target::Evaluation { worker, owner } => {
                let writer = owner.clone();
                let attempt = self.attempt.ok_or(StorageError::Payload)?;
                worker
                    .call(move |ledger| {
                        ledger.record_attempt_progress(writer.guard(), attempt, &progress)
                    })
                    .await??;
            }
            Target::Workflow { worker, owner, .. } => {
                let writer = owner.clone();
                let attempt = self.attempt.ok_or(StorageError::Payload)?;
                worker
                    .call(move |ledger| ledger.fail_workflow_attempt(&writer, attempt, &progress))
                    .await??;
            }
            Target::Ephemeral => {}
        }
        Ok(())
    }
    async fn succeed(
        &self,
        outcome: TurnOutcome,
        mut manifest: EvaluationManifest,
    ) -> Result<Executed, EngineError> {
        match &self.target {
            Target::Evaluation { worker, owner } => {
                let writer = owner.clone();
                let attempt = self.attempt.ok_or(StorageError::Payload)?;
                Ok(Executed::Saved(Box::new(
                    worker
                        .call(move |ledger| {
                            ledger.finalize_attempt(writer.guard(), attempt, outcome)
                        })
                        .await??,
                )))
            }
            Target::Workflow { worker, owner, .. } => {
                let writer = owner.clone();
                let attempt = self.attempt.ok_or(StorageError::Payload)?;
                Ok(Executed::Saved(Box::new(
                    worker
                        .call(move |ledger| {
                            ledger.finalize_workflow_attempt(&writer, attempt, outcome)
                        })
                        .await??,
                )))
            }
            Target::Ephemeral => {
                manifest.provider = outcome.provider.clone();
                manifest.validate()?;
                Ok(Executed::Ephemeral {
                    outcome: Box::new(outcome),
                    manifest: Box::new(manifest),
                })
            }
        }
    }
}
impl<P: DecisionProvider, C: TokenCounter + Sync, T: RetryClock> Engine<P, C, T> {
    #[expect(
        clippy::too_many_lines,
        reason = "pending refactor into smaller functions"
    )]
    pub(super) async fn execute(
        &self,
        input: RunInput<'_>,
        mut recorder: Recorder<'_>,
        lookup: impl Fn(&crate::config::auth::CredentialSource) -> Option<String>,
        cancel: &CancellationToken,
    ) -> Result<Executed, EngineError> {
        let required: Vec<_> = input
            .request
            .required_records
            .iter()
            .map(|id| RequiredItem {
                record: *id,
                pointer: None,
            })
            .collect();
        let mut limits = ResolvedLimits::for_profile(&input.settings.profile)?;
        // Pending updates are labeled pending, never assigned a persisted turn ID.
        let context_turn = Id::generate();
        let build = |limits: &ResolvedLimits| {
            ContextBuilder::build(
                ContextInput {
                    context_base: input.context_base,
                    runtime_only: input.runtime_only,
                    history: input.history,
                    sources: input.sources,
                    request: &input.request.turn,
                    updates: &input.request.updates,
                    required: &required,
                    workflow_base: &[],
                    session: input.session,
                    turn: context_turn,
                    provider: ProviderIdentity {
                        provider: input.settings.profile_name.clone(),
                        model: input.settings.profile.model.clone(),
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
        let mut settings = input.settings.clone().with_credentials(lookup)?;
        let policy = RetryPolicy::new(&settings.snapshot.execution).map_err(ProviderError::from)?;
        let timeout = Duration::from_secs(settings.snapshot.execution.attempt_timeout_seconds);
        if tokio::time::Instant::now().checked_add(timeout).is_none() {
            return Err(ConfigError::Invalid("attempt timeout").into());
        }
        for number in 1..=settings.snapshot.execution.max_attempts {
            if cancel.is_cancelled() {
                return Err(EngineError::Canceled);
            }
            recorder.begin(plan.manifest.clone()).await?;
            let dispatched = tokio::select! {biased;
                ()=cancel.cancelled()=>Err(ProviderError::Transport(TransportError::Canceled)),
                result=tokio::time::timeout(timeout,self.provider.attempt(&settings,&input.request.turn,&plan.manifest.context_toon,&self.counter,cancel))=>result.unwrap_or(Err(ProviderError::Transport(TransportError::Timeout))),
            };
            let error = match dispatched {
                Ok(outcome) => {
                    if let Err(error) = outcome.validate(&input.request.turn) {
                        ProviderError::Contract(error)
                    } else {
                        return recorder.succeed(outcome, plan.manifest).await;
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
            let progress = AttemptProgress {
                state: if canceled {
                    AttemptStatus::Interrupted
                } else {
                    AttemptStatus::Failed
                },
                error: Some(error_report(&error, delay.is_some())),
            };
            recorder.fail(progress).await?;
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
        ProviderError::Profile => (ErrorCategory::Configuration, "invalid_provider_profile"),
        _ => (ErrorCategory::Contract, "provider_contract"),
    };
    ErrorReport {
        category,
        code: code.into(),
        message: error.to_string(),
        retryable,
    }
}
