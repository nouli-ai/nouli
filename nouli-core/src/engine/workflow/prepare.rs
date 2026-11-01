//! Resolve settings, freeze ephemeral inputs, and claim saved workflow ownership.
use super::{WorkflowEngineError, WorkflowInput, WorkflowResult, from_stored};
use crate::{
    codec,
    config::{
        ConfigError,
        resolve::{SettingsSnapshot, select_model},
    },
    data::Data,
    engine::{
        Engine,
        ephemeral::{ephemeral_sources, read_only_sources},
        session_defaults,
    },
    history::{FinalizedTurn, ModelSelection, SourcePoint, base::ContextBase},
    identity::Id,
    storage::{
        worker::LedgerWorker,
        workflows::{
            FrozenWorkflow, WorkflowDecision, WorkflowDefinition, WorkflowError, WorkflowOwner,
        },
    },
    workflow::{
        RunMode, WorkflowRequest,
        compile::CompiledWorkflow,
        schedule::{ScheduleError, ScheduleObserver},
    },
};
use std::sync::Arc;

pub(super) fn resolve_settings(
    input: &WorkflowInput<'_>,
    request: &WorkflowRequest,
    session: &ModelSelection,
) -> Result<(Vec<SettingsSnapshot>, ModelSelection), ConfigError> {
    let configured = ModelSelection {
        provider: input.config.default_provider.clone(),
        model: input.config.default_model.clone(),
    };
    let defaults = select_model(&[
        &input.overrides.selection,
        &request.workflow.model,
        session,
        &configured,
    ]);
    let settings = request
        .workflow
        .nodes
        .iter()
        .map(|node| {
            let mut overrides = input.overrides.clone();
            overrides.selection = node.model.clone();
            input
                .config
                .resolve_snapshot(&overrides, &request.workflow.model, session, input.home)
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok((settings, defaults))
}
/// The request as evaluation runs it, with the caller's model override applied to the
/// workflow's model.
pub(super) fn effective_request(input: &WorkflowInput<'_>) -> WorkflowRequest {
    let mut request = input.request.clone();
    request.workflow.model = select_model(&[&input.overrides.selection, &request.workflow.model]);
    request
}
/// Node settings for an ephemeral run, which never continues a session.
fn ephemeral_settings(
    input: &WorkflowInput<'_>,
    request: &WorkflowRequest,
) -> Result<(Vec<SettingsSnapshot>, ModelSelection), ConfigError> {
    resolve_settings(input, request, &ModelSelection::default())
}
impl<P, C, T> Engine<P, C, T> {
    /// The settings, in node order, that evaluating `input` will use for each node,
    /// without evaluating it: for a durable run, those frozen by an earlier acceptance
    /// of the same request ID, or settings resolved against the continued session's
    /// model defaults. Callers use it to prepare what those settings need, such as
    /// credentials, ahead of evaluation.
    ///
    /// # Errors
    ///
    /// - [`WorkflowEngineError::Store`] if a durable run has no ledger worker.
    /// - [`WorkflowEngineError::Worker`] or [`WorkflowEngineError::Repository`] if the
    ///   ledger cannot look up the saved run or continued session.
    /// - [`WorkflowEngineError::Config`] if node settings cannot be resolved.
    pub async fn planned_workflow_settings(
        &self,
        worker: Option<&LedgerWorker>,
        input: &WorkflowInput<'_>,
    ) -> Result<Vec<SettingsSnapshot>, WorkflowEngineError> {
        let request = effective_request(input);
        let (settings, _) = if request.workflow.mode == RunMode::Ephemeral {
            ephemeral_settings(input, &request)?
        } else {
            durable_settings(worker.ok_or(WorkflowEngineError::Store)?, input, &request).await?
        };
        Ok(settings)
    }
}
/// Settings frozen by an earlier acceptance of the same request, or freshly resolved
/// against the continued session's model defaults.
pub(super) async fn durable_settings(
    worker: &LedgerWorker,
    input: &WorkflowInput<'_>,
    request: &WorkflowRequest,
) -> Result<(Vec<SettingsSnapshot>, ModelSelection), WorkflowEngineError> {
    let selected = request.clone();
    let saved = worker
        .call(move |ledger| ledger.saved_workflow(&selected))
        .await??;
    if let Some(saved) = saved {
        return Ok((saved.settings, saved.definition.defaults));
    }
    let session = session_defaults(worker, &request.session)
        .await?
        .map_err(WorkflowError::from)?;
    Ok(resolve_settings(input, request, &session)?)
}
/// An ephemeral workflow frozen in memory, with the read-only history it starts from.
pub(super) struct EphemeralWorkflow {
    pub(super) frozen: Arc<FrozenWorkflow>,
    pub(super) sources: Vec<SourcePoint>,
    pub(super) history: Vec<FinalizedTurn>,
}
/// Validate ephemeral options, read any selected sources, and freeze the definition
/// under fresh run and session IDs that are never saved.
pub(super) async fn freeze_ephemeral(
    worker: Option<&LedgerWorker>,
    input: &WorkflowInput<'_>,
    request: &WorkflowRequest,
    graph: &CompiledWorkflow,
) -> Result<EphemeralWorkflow, WorkflowEngineError> {
    let selections = ephemeral_sources(&request.session, request.request_id.as_deref(), false)?;
    let (sources, history) = if let Some(selections) = selections {
        worker
            .ok_or(WorkflowEngineError::Store)?
            .call(move |ledger| read_only_sources(ledger, &selections))
            .await??
    } else {
        (vec![], vec![])
    };
    let (settings, defaults) = ephemeral_settings(input, request)?;
    let run = Id::generate();
    let session = Id::generate();
    let base = ContextBase {
        id: run,
        session,
        sources: sources.clone(),
        state: request.workflow.state.clone(),
        updates: request.updates.clone(),
        runtime_only: true,
    };
    let frozen = FrozenWorkflow {
        definition: WorkflowDefinition {
            run,
            session,
            request: request.clone(),
            base,
            defaults,
            initial_frontier: vec![],
            initial_revision: 0,
            plan: Data::new(codec::to_value(graph).map_err(|_| WorkflowError::Snapshot)?)
                .map_err(|_| WorkflowError::Snapshot)?,
        },
        settings,
    };
    Ok(EphemeralWorkflow {
        frozen: Arc::new(frozen),
        sources,
        history,
    })
}
/// The node concurrency frozen with the run's settings.
pub(super) fn max_parallel(frozen: &FrozenWorkflow) -> Result<usize, WorkflowError> {
    Ok(frozen
        .settings
        .first()
        .ok_or(WorkflowError::Snapshot)?
        .execution
        .max_parallel)
}
/// The ledger's answer to a request to own a workflow run.
pub(super) enum Claim {
    /// The run already finished; its saved result is replayed.
    Replayed(Box<WorkflowResult>),
    /// This caller now owns the run and must execute it.
    Owned(Arc<WorkflowOwner>),
}
/// Take an accepted run's ownership, or report a replayed run's start to `observer`;
/// reject a run owned elsewhere or needing recovery.
pub(super) async fn owner_or_replay<O: ScheduleObserver>(
    decision: WorkflowDecision,
    observer: &O,
) -> Result<Claim, WorkflowEngineError> {
    match decision {
        WorkflowDecision::Saved(saved) => {
            observer
                .started(Some(saved.frozen.definition.run))
                .await
                .map_err(ScheduleError::Observer)?;
            Ok(Claim::Replayed(Box::new(from_stored(*saved, true)?)))
        }
        WorkflowDecision::InProgress => Err(WorkflowEngineError::InProgress),
        WorkflowDecision::Existing { run, status } => {
            Err(WorkflowEngineError::Existing { run, status })
        }
        WorkflowDecision::Accepted(owner) => Ok(Claim::Owned(Arc::<WorkflowOwner>::from(owner))),
    }
}
