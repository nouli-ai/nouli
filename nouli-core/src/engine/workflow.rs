//! Frozen durable and ephemeral workflows over the shared decision engine.
pub mod reporting;
use super::{
    Engine, EngineError,
    ephemeral::read_only_sources,
    execution::{DecisionProvider, RetryClock},
    run::{Executed, Recorder, RunInput},
};
use crate::history::StandingChanges;
use crate::{
    codec,
    config::{
        Config, ConfigError,
        resolve::{RuntimeOverrides, SettingsSnapshot, select_model},
    },
    data::Data,
    history::{
        EvaluationRequest, FinalizedTurn, ModelSelection, SessionSelection, SourcePoint,
        base::ContextBase,
    },
    identity::Id,
    limits::count::TokenCounter,
    protocol::{ErrorCategory, ErrorReport},
    storage::{
        worker::{LedgerWorker, WorkerError},
        workflows::{
            FrozenWorkflow, StoredRun, WorkflowDecision, WorkflowDefinition, WorkflowError,
            WorkflowOwner,
            nodes::{node_output, request_for},
        },
    },
    workflow::{
        NodeStatus, RunMode, RunStatus, WorkflowRequest,
        compile::{self, CompileError, CompiledWorkflow},
        schedule::{
            self, AncestorResult, NodeCall, NodeEvaluation, NodeExecutor, NodeOutput, NodeProgress,
            ScheduleError, ScheduleObserver, ScheduleResult,
        },
    },
};
use serde::{Deserialize, Serialize};
use std::{collections::HashSet, path::Path, sync::Arc};
use thiserror::Error;
use tokio_util::sync::CancellationToken;

pub type CredentialLookup =
    Arc<dyn Fn(&crate::config::auth::CredentialSource) -> Option<String> + Send + Sync>;
pub struct WorkflowInput<'a> {
    pub request: &'a WorkflowRequest,
    pub config: &'a Config,
    pub overrides: &'a RuntimeOverrides,
    pub home: &'a Path,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WorkflowResult {
    pub request_id: Option<String>,
    pub run: Option<Id>,
    pub session: Option<Id>,
    pub persisted: bool,
    pub replayed: bool,
    pub sources: Vec<SourcePoint>,
    pub frontier: Option<Vec<Id>>,
    pub result: ScheduleResult,
}
#[derive(Debug, Error)]
pub enum WorkflowEngineError {
    #[error("workflow {run} interrupted: {error}")]
    Execution {
        run: Id,
        #[source]
        error: Box<ScheduleError>,
    },
    #[error(transparent)]
    Engine(#[from] EngineError),
    #[error(transparent)]
    Repository(#[from] WorkflowError),
    #[error(transparent)]
    Worker(#[from] WorkerError),
    #[error(transparent)]
    Config(#[from] ConfigError),
    #[error(transparent)]
    Compile(#[from] CompileError),
    #[error(transparent)]
    Schedule(#[from] ScheduleError),
    #[error("durable workflow needs a writable ledger")]
    Store,
    #[error("another live owner is executing this workflow")]
    InProgress,
    #[error("workflow requires explicit recovery ({status:?})")]
    Existing { run: Id, status: RunStatus },
}
fn report(error: impl Into<EngineError>) -> ErrorReport {
    error.into().report()
}

fn from_stored(stored: StoredRun, replayed: bool) -> Result<WorkflowResult, WorkflowEngineError> {
    let definition = stored.frozen.definition;
    Ok(WorkflowResult {
        request_id: definition.request.request_id,
        run: Some(definition.run),
        session: Some(definition.session),
        persisted: true,
        replayed,
        sources: definition.base.sources,
        frontier: Some(stored.frontier),
        result: stored.result.ok_or(WorkflowError::Snapshot)?,
    })
}
enum Target {
    Durable {
        worker: LedgerWorker,
        owner: Arc<WorkflowOwner>,
    },
    Ephemeral {
        history: Vec<FinalizedTurn>,
        ids: Vec<Id>,
    },
}
struct Runner<P, C, T, O> {
    observer: Arc<O>,
    engine: Arc<Engine<P, C, T>>,
    frozen: Arc<FrozenWorkflow>,
    graph: Arc<CompiledWorkflow>,
    target: Target,
    lookup: CredentialLookup,
}
/// What one node evaluates against and where its attempts are recorded.
struct NodeContext<'a> {
    sources: Vec<SourcePoint>,
    history: Vec<FinalizedTurn>,
    recorder: Recorder<'a>,
}
impl<
    P: DecisionProvider + Send + 'static,
    C: TokenCounter + Sync + Send + 'static,
    T: RetryClock + Send + 'static,
    O: ScheduleObserver,
> ScheduleObserver for Runner<P, C, T, O>
{
    async fn changed(&self, node: usize, progress: NodeProgress) -> Result<(), ErrorReport> {
        let progress = if let Target::Durable { worker, owner } = &self.target {
            let owner = owner.clone();
            worker
                .call(move |ledger| {
                    ledger.record_workflow_progress(&owner, node, &progress)?;
                    ledger.workflow_node_progress(&owner, node)
                })
                .await
                .map_err(report)?
                .map_err(report)?
        } else {
            progress
        };
        self.observer.changed(node, progress).await
    }
}
impl<
    P: DecisionProvider + Send + 'static,
    C: TokenCounter + Sync + Send + 'static,
    T: RetryClock + Send + 'static,
    O: ScheduleObserver,
> NodeExecutor for Runner<P, C, T, O>
{
    async fn execute(
        &self,
        call: NodeCall,
        cancel: CancellationToken,
    ) -> Result<NodeOutput, ErrorReport> {
        let definition = &self.frozen.definition;
        let node = call.node;
        let request = request_for(&self.frozen, node, &call.inputs).map_err(report)?;
        let NodeContext {
            sources,
            history,
            recorder,
        } = match &self.target {
            Target::Durable { worker, owner } => {
                self.durable_context(worker, owner, &call, &request).await?
            }
            Target::Ephemeral { history, ids } => self.ephemeral_context(history, ids, &call)?,
        };
        let executed = self
            .engine
            .execute(
                RunInput {
                    context_base: Some(&definition.base),
                    runtime_only: definition.base.runtime_only,
                    request: &request,
                    sources: &sources,
                    history: &history,
                    settings: self.frozen.settings[node].clone(),
                    session: definition.session,
                },
                recorder,
                |name| (self.lookup)(name),
                &cancel,
            )
            .await
            .map_err(report)?;
        Ok(node_output_of(executed, request))
    }
}
impl<
    P: DecisionProvider + Send + 'static,
    C: TokenCounter + Sync + Send + 'static,
    T: RetryClock + Send + 'static,
    O: ScheduleObserver,
> Runner<P, C, T, O>
{
    /// Accept the node in the ledger and read the saved history of its parents' turns.
    async fn durable_context<'a>(
        &self,
        worker: &'a LedgerWorker,
        owner: &Arc<WorkflowOwner>,
        call: &NodeCall,
        request: &EvaluationRequest,
    ) -> Result<NodeContext<'a>, ErrorReport> {
        let node = call.node;
        let sources = if call.parents.is_empty() {
            self.frozen.definition.base.sources.clone()
        } else {
            saved_parent_sources(call).map_err(report)?
        };
        let writer = owner.clone();
        let accepted = request.clone();
        let selected = sources.clone();
        let intent = worker
            .call(move |ledger| ledger.accept_workflow_node(&writer, node, &accepted, &selected))
            .await
            .map_err(report)?
            .map_err(report)?;
        let selected = sources.clone();
        let history = worker
            .call(move |ledger| ledger.ancestry(&selected))
            .await
            .map_err(report)?
            .map_err(report)?;
        Ok(NodeContext {
            sources,
            history,
            recorder: Recorder::workflow(worker, owner.clone(), intent),
        })
    }
    /// Extend the base history with in-memory turns for every completed ancestor.
    fn ephemeral_context(
        &self,
        history: &[FinalizedTurn],
        ids: &[Id],
        call: &NodeCall,
    ) -> Result<NodeContext<'static>, ErrorReport> {
        let definition = &self.frozen.definition;
        let mut selected = history.to_vec();
        let active: HashSet<_> = call
            .ancestors
            .iter()
            .map(|ancestor| ancestor.node)
            .collect();
        for ancestor in &call.ancestors {
            selected.push(self.ephemeral_turn(ancestor, &active, ids)?);
        }
        let sources = if call.parents.is_empty() {
            definition.base.sources.clone()
        } else {
            call.parents
                .iter()
                .map(|parent| SourcePoint {
                    session: definition.session,
                    frontier: vec![ids[*parent]],
                })
                .collect()
        };
        Ok(NodeContext {
            sources,
            history: selected,
            recorder: Recorder::ephemeral(),
        })
    }
    /// The unsaved turn an ephemeral ancestor contributes, parented on its active
    /// dependencies or, for a root, on the frontier of the workflow's base sources.
    fn ephemeral_turn(
        &self,
        ancestor: &AncestorResult,
        active: &HashSet<usize>,
        ids: &[Id],
    ) -> Result<FinalizedTurn, ErrorReport> {
        let definition = &self.frozen.definition;
        let evaluation = ancestor
            .output
            .evaluation
            .as_ref()
            .ok_or_else(|| report(WorkflowError::Snapshot))?;
        let parents: Vec<_> = self.graph.nodes()[ancestor.node]
            .dependencies
            .iter()
            .filter(|parent| active.contains(parent))
            .map(|parent| ids[*parent])
            .collect();
        let parents = if parents.is_empty() {
            definition
                .base
                .sources
                .iter()
                .flat_map(|source| source.frontier.iter().copied())
                .collect()
        } else {
            parents
        };
        Ok(FinalizedTurn {
            context_base: Some(definition.base.clone()),
            workflow_run: Some(definition.run),
            runtime_only: true,
            workflow_node: Some(ancestor.id.clone()),
            id: ids[ancestor.node],
            session: definition.session,
            parents,
            request: evaluation.request.clone(),
            outcome: ancestor.output.outcome.clone(),
            updates: evaluation.updates.clone(),
            standing: StandingChanges::default(),
            manifest: evaluation.manifest.clone(),
        })
    }
}
/// The saved source each parent node published, in parent order.
fn saved_parent_sources(call: &NodeCall) -> Result<Vec<SourcePoint>, WorkflowError> {
    call.parents
        .iter()
        .map(|parent| {
            call.ancestors
                .iter()
                .find(|ancestor| ancestor.node == *parent)
                .and_then(|ancestor| ancestor.output.source.clone())
                .ok_or(WorkflowError::Snapshot)
        })
        .collect()
}
fn node_output_of(executed: Executed, request: EvaluationRequest) -> NodeOutput {
    match executed {
        Executed::Saved(turn) => node_output(&turn),
        Executed::Ephemeral { outcome, manifest } => NodeOutput {
            outcome: *outcome,
            source: None,
            evaluation: Some(NodeEvaluation {
                request: request.turn,
                updates: request.updates,
                manifest: *manifest,
            }),
        },
    }
}
fn reconciled(
    graph: &CompiledWorkflow,
    mut result: ScheduleResult,
    stored: &StoredRun,
) -> Result<ScheduleResult, WorkflowEngineError> {
    for (node, saved) in result.nodes.iter_mut().zip(&stored.nodes) {
        if saved.status == NodeStatus::Completed {
            *node = saved.clone();
        }
    }
    result.outputs =
        schedule::project_outputs(graph, &result.nodes).map_err(ScheduleError::from)?;
    Ok(result)
}
fn resolve_settings(
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
/// Settings frozen by an earlier acceptance of the same request, or freshly resolved
/// against the continued session's model defaults.
async fn durable_settings(
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
    let session = if let SessionSelection::Continue { session } = &request.session {
        let session = session.clone();
        worker
            .call(move |ledger| ledger.session(&session))
            .await?
            .map_err(WorkflowError::from)?
            .defaults
    } else {
        ModelSelection::default()
    };
    Ok(resolve_settings(input, request, &session)?)
}
/// An ephemeral workflow frozen in memory, with the read-only history it starts from.
struct EphemeralWorkflow {
    frozen: Arc<FrozenWorkflow>,
    sources: Vec<SourcePoint>,
    history: Vec<FinalizedTurn>,
}
/// Validate ephemeral options, read any selected sources, and freeze the definition
/// under fresh run and session IDs that are never saved.
async fn freeze_ephemeral(
    worker: Option<&LedgerWorker>,
    input: &WorkflowInput<'_>,
    request: &WorkflowRequest,
    graph: &CompiledWorkflow,
) -> Result<EphemeralWorkflow, WorkflowEngineError> {
    if request.request_id.is_some()
        || !matches!(
            request.session,
            SessionSelection::New { name: None } | SessionSelection::From { name: None, .. }
        )
    {
        return Err(EngineError::EphemeralOptions.into());
    }
    let (sources, history) = if let SessionSelection::From { sources, .. } = &request.session {
        let selections = sources.clone();
        worker
            .ok_or(WorkflowEngineError::Store)?
            .call(move |ledger| read_only_sources(ledger, &selections))
            .await??
    } else {
        (vec![], vec![])
    };
    let (settings, defaults) = resolve_settings(input, request, &ModelSelection::default())?;
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
fn max_parallel(frozen: &FrozenWorkflow) -> Result<usize, WorkflowError> {
    Ok(frozen
        .settings
        .first()
        .ok_or(WorkflowError::Snapshot)?
        .execution
        .max_parallel)
}
/// The ledger's answer to a request to own a workflow run.
enum Claim {
    /// The run already finished; its saved result is replayed.
    Replayed(Box<WorkflowResult>),
    /// This caller now owns the run and must execute it.
    Owned(Arc<WorkflowOwner>),
}
/// Take an accepted run's ownership, or report a replayed run's start to `observer`;
/// reject a run owned elsewhere or needing recovery.
async fn owner_or_replay<O: ScheduleObserver>(
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
impl<
    P: DecisionProvider + Send + 'static,
    C: TokenCounter + Sync + Send + 'static,
    T: RetryClock + Send + 'static,
> Engine<P, C, T>
{
    /// Evaluate a workflow without observing its progress.
    ///
    /// # Errors
    ///
    /// See [`Self::evaluate_workflow_observed`]:
    ///
    /// - [`WorkflowEngineError::Engine`] wrapping [`EngineError::Canceled`] if `cancel` has
    ///   already fired, or [`EngineError::EphemeralOptions`] /
    ///   [`EngineError::EphemeralStore`] for an invalid ephemeral run.
    /// - [`WorkflowEngineError::Compile`] if the workflow definition does not compile, or
    ///   [`WorkflowEngineError::Repository`] if it cannot be fingerprinted.
    /// - [`WorkflowEngineError::Store`] if a durable run, or an ephemeral run with sources,
    ///   has no ledger worker.
    /// - [`WorkflowEngineError::Config`] if node settings cannot be resolved.
    /// - [`WorkflowEngineError::Schedule`] if an ephemeral run fails to schedule.
    /// - [`WorkflowEngineError::InProgress`] if another live owner is executing the run, or
    ///   [`WorkflowEngineError::Existing`] if it needs explicit recovery.
    /// - [`WorkflowEngineError::Worker`] or [`WorkflowEngineError::Repository`] if the
    ///   ledger cannot load, interrupt, or finish the run.
    /// - [`WorkflowEngineError::Schedule`] if the observer rejects the start event.
    /// - [`WorkflowEngineError::Execution`] if scheduling fails after the run starts; the
    ///   run is marked interrupted first.
    pub async fn evaluate_workflow(
        self: &Arc<Self>,
        worker: Option<&LedgerWorker>,
        input: WorkflowInput<'_>,
        lookup: CredentialLookup,
        cancel: CancellationToken,
    ) -> Result<WorkflowResult, WorkflowEngineError> {
        self.evaluate_workflow_observed(
            worker,
            input,
            lookup,
            cancel,
            Arc::new(crate::workflow::schedule::NoopObserver),
        )
        .await
    }

    /// Evaluate a workflow, reporting its start and node progress to `observer`.
    ///
    /// # Errors
    ///
    /// - [`WorkflowEngineError::Engine`] wrapping [`EngineError::Canceled`] if `cancel` has
    ///   already fired, or [`EngineError::EphemeralOptions`] /
    ///   [`EngineError::EphemeralStore`] for an invalid ephemeral run.
    /// - [`WorkflowEngineError::Compile`] if the workflow definition does not compile, or
    ///   [`WorkflowEngineError::Repository`] if it cannot be fingerprinted.
    /// - [`WorkflowEngineError::Store`] if a durable run, or an ephemeral run with sources,
    ///   has no ledger worker.
    /// - [`WorkflowEngineError::Config`] if node settings cannot be resolved.
    /// - [`WorkflowEngineError::Schedule`] if an ephemeral run fails to schedule.
    /// - [`WorkflowEngineError::InProgress`] if another live owner is executing the run, or
    ///   [`WorkflowEngineError::Existing`] if it needs explicit recovery.
    /// - [`WorkflowEngineError::Worker`] or [`WorkflowEngineError::Repository`] if the
    ///   ledger cannot load, interrupt, or finish the run.
    /// - [`WorkflowEngineError::Schedule`] if the observer rejects the start event.
    /// - [`WorkflowEngineError::Execution`] if scheduling fails after the run starts; the
    ///   run is marked interrupted first.
    pub async fn evaluate_workflow_observed<O: ScheduleObserver>(
        self: &Arc<Self>,
        worker: Option<&LedgerWorker>,
        input: WorkflowInput<'_>,
        lookup: CredentialLookup,
        cancel: CancellationToken,
        observer: Arc<O>,
    ) -> Result<WorkflowResult, WorkflowEngineError> {
        if cancel.is_cancelled() {
            return Err(EngineError::Canceled.into());
        }
        let mut request = input.request.clone();
        request.workflow.model =
            select_model(&[&input.overrides.selection, &request.workflow.model]);
        let graph = Arc::new(compile::compile(&request.workflow)?);
        crate::storage::workflows::fingerprint(&request)?;
        if request.workflow.mode == RunMode::Ephemeral {
            let prepared = freeze_ephemeral(worker, &input, &request, &graph).await?;
            return self
                .run_ephemeral_workflow(prepared, graph, lookup, cancel, observer)
                .await;
        }
        request
            .request_id
            .get_or_insert_with(|| Id::generate().to_string());
        let worker = worker.ok_or(WorkflowEngineError::Store)?;
        let (settings, defaults) = durable_settings(worker, &input, &request).await?;
        let accepted = request.clone();
        let decision = worker
            .call(move |ledger| ledger.accept_workflow(&accepted, settings, defaults))
            .await??;
        match owner_or_replay(decision, &*observer).await? {
            Claim::Replayed(result) => Ok(*result),
            Claim::Owned(owner) => {
                self.run_owned_workflow(worker, owner, graph, lookup, cancel, observer)
                    .await
            }
        }
    }

    /// Explicitly begin a new bounded retry round using the saved definition and settings.
    /// Credentials are resolved afresh by the original source names.
    ///
    /// # Errors
    ///
    /// See [`Self::resume_workflow_observed`]:
    ///
    /// - [`WorkflowEngineError::Engine`] wrapping [`EngineError::Canceled`] if `cancel` has
    ///   already fired.
    /// - [`WorkflowEngineError::Compile`] if the saved workflow definition does not compile.
    /// - [`WorkflowEngineError::InProgress`] if another live owner is executing the run, or
    ///   [`WorkflowEngineError::Existing`] if it needs explicit recovery.
    /// - [`WorkflowEngineError::Worker`] or [`WorkflowEngineError::Repository`] if the
    ///   ledger cannot load, interrupt, or finish the run.
    /// - [`WorkflowEngineError::Schedule`] if the observer rejects the start event.
    /// - [`WorkflowEngineError::Execution`] if scheduling fails after the run starts; the
    ///   run is marked interrupted first.
    pub async fn resume_workflow(
        self: &Arc<Self>,
        worker: &LedgerWorker,
        run: Id,
        lookup: CredentialLookup,
        cancel: CancellationToken,
    ) -> Result<WorkflowResult, WorkflowEngineError> {
        self.resume_workflow_observed(
            worker,
            run,
            lookup,
            cancel,
            Arc::new(schedule::NoopObserver),
        )
        .await
    }

    /// Resume a saved workflow run, reporting its start and node progress to `observer`.
    ///
    /// # Errors
    ///
    /// - [`WorkflowEngineError::Engine`] wrapping [`EngineError::Canceled`] if `cancel` has
    ///   already fired.
    /// - [`WorkflowEngineError::Compile`] if the saved workflow definition does not compile.
    /// - [`WorkflowEngineError::InProgress`] if another live owner is executing the run, or
    ///   [`WorkflowEngineError::Existing`] if it needs explicit recovery.
    /// - [`WorkflowEngineError::Worker`] or [`WorkflowEngineError::Repository`] if the
    ///   ledger cannot load, interrupt, or finish the run.
    /// - [`WorkflowEngineError::Schedule`] if the observer rejects the start event.
    /// - [`WorkflowEngineError::Execution`] if scheduling fails after the run starts; the
    ///   run is marked interrupted first.
    pub async fn resume_workflow_observed<O: ScheduleObserver>(
        self: &Arc<Self>,
        worker: &LedgerWorker,
        run: Id,
        lookup: CredentialLookup,
        cancel: CancellationToken,
        observer: Arc<O>,
    ) -> Result<WorkflowResult, WorkflowEngineError> {
        if cancel.is_cancelled() {
            return Err(EngineError::Canceled.into());
        }
        let decision = worker
            .call(move |ledger| ledger.resume_workflow(run))
            .await??;
        let owner = match owner_or_replay(decision, &*observer).await? {
            Claim::Replayed(result) => return Ok(*result),
            Claim::Owned(owner) => owner,
        };
        let graph = Arc::new(compile::compile(
            &owner.frozen().definition.request.workflow,
        )?);
        self.run_owned_workflow(worker, owner, graph, lookup, cancel, observer)
            .await
    }

    /// Schedule a frozen ephemeral workflow entirely in memory.
    async fn run_ephemeral_workflow<O: ScheduleObserver>(
        self: &Arc<Self>,
        prepared: EphemeralWorkflow,
        graph: Arc<CompiledWorkflow>,
        lookup: CredentialLookup,
        cancel: CancellationToken,
        observer: Arc<O>,
    ) -> Result<WorkflowResult, WorkflowEngineError> {
        let parallel = max_parallel(&prepared.frozen)?;
        let runner = Arc::new(Runner {
            observer: observer.clone(),
            engine: self.clone(),
            frozen: prepared.frozen,
            graph: graph.clone(),
            target: Target::Ephemeral {
                history: prepared.history,
                ids: (0..graph.nodes().len()).map(|_| Id::generate()).collect(),
            },
            lookup,
        });
        observer
            .started(None)
            .await
            .map_err(ScheduleError::Observer)?;
        let result =
            schedule::schedule_observed(graph, runner.clone(), runner, parallel, cancel).await?;
        Ok(WorkflowResult {
            request_id: None,
            run: None,
            session: None,
            persisted: false,
            replayed: false,
            sources: prepared.sources,
            frontier: None,
            result,
        })
    }

    async fn run_owned_workflow<O: ScheduleObserver>(
        self: &Arc<Self>,
        worker: &LedgerWorker,
        owner: Arc<WorkflowOwner>,
        graph: Arc<CompiledWorkflow>,
        lookup: CredentialLookup,
        cancel: CancellationToken,
        observer: Arc<O>,
    ) -> Result<WorkflowResult, WorkflowEngineError> {
        let frozen = Arc::new(owner.frozen().clone());
        let run = frozen.definition.run;
        let parallel = max_parallel(&frozen)?;
        let runner = Arc::new(Runner {
            observer: observer.clone(),
            engine: self.clone(),
            frozen,
            graph: graph.clone(),
            target: Target::Durable {
                worker: worker.clone(),
                owner: owner.clone(),
            },
            lookup,
        });
        let initial = worker
            .call(move |ledger| ledger.workflow_run(run))
            .await??
            .nodes;
        let execution = async {
            observer
                .started(Some(run))
                .await
                .map_err(ScheduleError::Observer)?;
            schedule::schedule_resumed(
                graph.clone(),
                runner.clone(),
                runner,
                parallel,
                cancel,
                initial,
            )
            .await
        }
        .await;
        let result = match execution {
            Ok(result) => result,
            Err(error) => {
                let writer = owner.clone();
                worker
                    .call(move |ledger| ledger.interrupt_workflow(&writer))
                    .await??;
                return Err(WorkflowEngineError::Execution {
                    run,
                    error: Box::new(error),
                });
            }
        };
        let stored = worker
            .call(move |ledger| ledger.workflow_run(run))
            .await??;
        let result = reconciled(&graph, result, &stored)?;
        let writer = owner.clone();
        let finished = worker
            .call(move |ledger| ledger.finish_workflow(&writer, &result))
            .await??;
        from_stored(finished, false)
    }
}

#[cfg(test)]
#[expect(
    clippy::unused_async_trait_impl,
    reason = "test fakes implement async traits without needing to await"
)]
mod tests {
    use super::*;
    use crate::config::ConnectionSettings;
    use crate::history::StandingChanges;
    use crate::{
        config::{ExecutionSettings, ModelSettings, OutputFormat, Protocol, ProviderDefinition},
        context::{ContextKind, ContextRecord, ContextUpdate, PinTarget},
        data::Input,
        decision::TurnRequest,
        engine::{EvaluationInput, execution::TokioClock},
        history::EvaluationRequest,
        identity::SessionSelector,
        limits::count::ByteCounter,
        outcome::TurnOutcome,
        provider::ProviderError,
        storage::{OpenMode, guards::GuardError, replay::ReplayError},
        workflow::WorkflowSpec,
    };
    use std::{
        collections::{BTreeMap, HashMap},
        path::PathBuf,
        sync::{
            Mutex,
            atomic::{AtomicUsize, Ordering},
        },
    };
    struct Store(PathBuf);
    impl Store {
        fn new() -> Self {
            Self(std::env::temp_dir().join(format!("nouli-workflow-{}", Id::generate())))
        }
    }
    impl Drop for Store {
        fn drop(&mut self) {
            if self.0.exists() {
                std::fs::remove_dir_all(&self.0).unwrap();
            }
        }
    }
    #[derive(Clone)]
    struct Call {
        node: Option<String>,
        model: String,
        context: String,
    }
    struct Fake {
        publication_fault: Mutex<Option<LedgerWorker>>,
        calls: Mutex<Vec<Call>>,
        gates: HashMap<String, tokio::sync::Semaphore>,
        started: tokio::sync::mpsc::UnboundedSender<String>,
        fail: HashSet<String>,
        transient: bool,
        active: AtomicUsize,
        peak: AtomicUsize,
    }
    struct Active<'a>(&'a AtomicUsize);
    impl Drop for Active<'_> {
        fn drop(&mut self) {
            self.0.fetch_sub(1, Ordering::SeqCst);
        }
    }
    impl DecisionProvider for Fake {
        async fn attempt<C: TokenCounter + Sync>(
            &self,
            settings: &crate::config::resolve::ResolvedSettings,
            request: &TurnRequest,
            context: &str,
            _counter: &C,
            _cancel: &CancellationToken,
        ) -> Result<TurnOutcome, ProviderError> {
            let count = self.active.fetch_add(1, Ordering::SeqCst) + 1;
            let _active = Active(&self.active);
            self.peak.fetch_max(count, Ordering::SeqCst);
            let node = request.state.iter().find_map(|input| {
                if let Input::Text { text } = input {
                    text.strip_prefix("NODE:").map(str::to_owned)
                } else {
                    None
                }
            });
            self.calls.lock().unwrap().push(Call {
                node: node.clone(),
                model: settings.snapshot.profile.model.clone(),
                context: context.into(),
            });
            if let Some(node) = &node {
                self.started.send(node.clone()).unwrap();
                if let Some(gate) = self.gates.get(node) {
                    gate.acquire().await.unwrap().forget();
                }
                if self.fail.contains(node) {
                    return Err(ProviderError::Transport(if self.transient {
                        crate::transport::TransportError::Connect
                    } else {
                        crate::transport::TransportError::Contract
                    }));
                }
            }
            let fault = self.publication_fault.lock().unwrap().clone();
            if node.as_deref() == Some("high")
                && let Some(worker) = fault
            {
                worker.call(|ledger|ledger.transaction::<_,crate::storage::StorageError>("inject node publication failure",|tx| {
                    tx.execute_batch("CREATE TRIGGER IF NOT EXISTS reject_high_publication BEFORE UPDATE OF state ON attempt_states WHEN NEW.state='completed' AND EXISTS(SELECT 1 FROM attempts WHERE id=NEW.attempt_id AND node_id='high') BEGIN SELECT RAISE(ABORT,'fault'); END;").unwrap();Ok(())
                })).await.unwrap().unwrap();
            }
            let mut outcome: TurnOutcome = serde_json::from_str(include_str!(
                "../../../fixtures/contracts/decision-outcome.json"
            ))
            .unwrap();
            outcome.provider.provider = settings.snapshot.profile_name.clone();
            outcome.provider.model = settings.snapshot.profile.model.clone();
            if outcome.provider.model == "frozen-alias" {
                outcome.provider.model = "served-version-2026-10".into();
            }
            Ok(outcome)
        }
    }
    fn engine(
        gated: &[&str],
        failed: &[&str],
    ) -> (
        Arc<Engine<Fake, ByteCounter, TokioClock>>,
        tokio::sync::mpsc::UnboundedReceiver<String>,
    ) {
        let (started, rx) = tokio::sync::mpsc::unbounded_channel();
        (
            Arc::new(Engine {
                provider: Fake {
                    publication_fault: Mutex::new(None),
                    calls: Mutex::new(vec![]),
                    gates: gated
                        .iter()
                        .map(|id| (id.to_string(), tokio::sync::Semaphore::new(0)))
                        .collect(),
                    started,
                    transient: false,
                    fail: failed
                        .iter()
                        .map(std::string::ToString::to_string)
                        .collect(),
                    active: AtomicUsize::new(0),
                    peak: AtomicUsize::new(0),
                },
                counter: ByteCounter,
                clock: TokioClock,
            }),
            rx,
        )
    }
    fn config() -> Config {
        Config {
            defaults: ModelSettings::default(),
            default_provider: Some("fixture".into()),
            default_model: Some("offline".into()),
            output: OutputFormat::Json,
            store: None,
            execution: ExecutionSettings {
                max_attempts: 1,
                ..ExecutionSettings::default()
            },
            providers: BTreeMap::from([(
                "fixture".into(),
                ProviderDefinition {
                    connection: ConnectionSettings::default(),
                    defaults: ModelSettings::default(),
                    protocol: Protocol::SystemOne,
                    base_url: "http://127.0.0.1:8080".into(),
                    api_key_env: None,
                    models: BTreeMap::from([
                        (
                            "offline".into(),
                            ModelSettings {
                                connection: ConnectionSettings::default(),
                                per_question_context_tokens: None,
                                max_questions: None,
                                max_choices: None,
                                max_score_levels: None,
                                max_request_bytes: None,
                                input_modalities: None,
                                context_budget_tokens: Some(100_000),
                                context_window_tokens: Some(100_000),
                                reserved_output_tokens: Some(128),
                            },
                        ),
                        (
                            "high-model".into(),
                            ModelSettings {
                                connection: ConnectionSettings::default(),
                                per_question_context_tokens: None,
                                max_questions: None,
                                max_choices: None,
                                max_score_levels: None,
                                max_request_bytes: None,
                                input_modalities: None,
                                context_budget_tokens: Some(100_000),
                                ..Default::default()
                            },
                        ),
                        (
                            "low-model".into(),
                            ModelSettings {
                                connection: ConnectionSettings::default(),
                                per_question_context_tokens: None,
                                max_questions: None,
                                max_choices: None,
                                max_score_levels: None,
                                max_request_bytes: None,
                                input_modalities: None,
                                context_budget_tokens: Some(100_000),
                                ..Default::default()
                            },
                        ),
                        (
                            "frozen-alias".into(),
                            ModelSettings {
                                connection: ConnectionSettings::default(),
                                per_question_context_tokens: None,
                                max_questions: None,
                                max_choices: None,
                                max_score_levels: None,
                                max_request_bytes: None,
                                input_modalities: None,
                                context_budget_tokens: Some(100_000),
                                ..Default::default()
                            },
                        ),
                    ]),
                    max_attempts: None,
                    attempt_timeout_seconds: None,
                },
            )]),
        }
    }
    fn request() -> WorkflowRequest {
        let mut workflow: WorkflowSpec =
            serde_json::from_str(include_str!("../../../fixtures/contracts/workflow.json"))
                .unwrap();
        for node in &mut workflow.nodes {
            node.turn.state.push(Input::Text {
                text: format!("NODE:{}", node.id),
            });
        }
        WorkflowRequest {
            workflow,
            session: SessionSelection::New { name: None },
            updates: vec![],
            required_records: vec![],
            request_id: Some(Id::generate().to_string()),
        }
    }
    fn ordinary() -> EvaluationRequest {
        EvaluationRequest {
            standing: StandingChanges::default(),
            turn: serde_json::from_str(include_str!(
                "../../../fixtures/contracts/decision-request.json"
            ))
            .unwrap(),
            session: SessionSelection::New { name: None },
            updates: vec![],
            required_records: vec![],
            model: ModelSelection::default(),
            request_id: None,
        }
    }
    fn record(key: &str, value: serde_json::Value) -> ContextRecord {
        ContextRecord {
            id: Id::generate(),
            key: key.into(),
            value: Data::new(value).unwrap(),
            kind: ContextKind::Commitment,
        }
    }
    async fn evaluate(
        engine: &Arc<Engine<Fake, ByteCounter, TokioClock>>,
        worker: &LedgerWorker,
        config: &Config,
        request: &EvaluationRequest,
    ) -> super::super::EvaluationResult {
        engine
            .evaluate(
                worker,
                EvaluationInput {
                    request,
                    config,
                    overrides: &RuntimeOverrides::default(),
                    home: Path::new("/unused"),
                    retry_interrupted: false,
                },
                |_| None,
                &CancellationToken::new(),
            )
            .await
            .unwrap()
    }
    struct Observed {
        low_done: tokio::sync::Notify,
        events: Mutex<Vec<(usize, NodeStatus)>>,
    }
    impl ScheduleObserver for Observed {
        async fn changed(&self, node: usize, progress: NodeProgress) -> Result<(), ErrorReport> {
            self.events.lock().unwrap().push((node, progress.status));
            if node == 2 && progress.status == NodeStatus::Completed {
                self.low_done.notify_one();
            }
            Ok(())
        }
    }
    impl Observed {
        fn new() -> Arc<Self> {
            Arc::new(Self {
                low_done: tokio::sync::Notify::new(),
                events: Mutex::new(vec![]),
            })
        }
        fn completed(&self) -> usize {
            self.events
                .lock()
                .unwrap()
                .iter()
                .filter(|(_, status)| *status == NodeStatus::Completed)
                .count()
        }
    }
    async fn open_writer(store: &Store) -> LedgerWorker {
        LedgerWorker::open(store.0.clone(), OpenMode::ReadWriteCreate)
            .await
            .unwrap()
    }
    /// Run one ledger read on the worker, panicking if the worker or the read fails.
    async fn read<R: Send + 'static, E: std::fmt::Debug + Send + 'static>(
        worker: &LedgerWorker,
        operation: impl FnOnce(&mut crate::storage::Ledger) -> Result<R, E> + Send + 'static,
    ) -> R {
        worker.call(operation).await.unwrap().unwrap()
    }
    /// The workflow fixture with both conditional branches forced to run.
    fn both_branches() -> WorkflowRequest {
        let mut request = request();
        request.workflow.nodes[1].when = None;
        request.workflow.nodes[2].when = None;
        request
    }
    fn from_session(session: Id) -> SessionSelection {
        SessionSelection::From {
            sources: vec![crate::history::SourceSelection {
                session: SessionSelector::Id(session),
                frontier: None,
            }],
            name: None,
        }
    }
    fn continuing(session: Id) -> SessionSelection {
        SessionSelection::Continue {
            session: SessionSelector::Id(session),
        }
    }
    /// Start a durable workflow on its own task so the test can interleave with its nodes.
    fn spawn_workflow<T: RetryClock + Send + 'static, O: ScheduleObserver>(
        engine: &Arc<Engine<Fake, ByteCounter, T>>,
        worker: &LedgerWorker,
        request: &WorkflowRequest,
        config: &Config,
        cancel: CancellationToken,
        observer: Arc<O>,
    ) -> tokio::task::JoinHandle<Result<WorkflowResult, WorkflowEngineError>> {
        let engine = engine.clone();
        let worker = worker.clone();
        let request = request.clone();
        let config = config.clone();
        tokio::spawn(async move {
            engine
                .evaluate_workflow_observed(
                    Some(&worker),
                    WorkflowInput {
                        request: &request,
                        config: &config,
                        overrides: &RuntimeOverrides::default(),
                        home: Path::new("/unused"),
                    },
                    Arc::new(|_| None),
                    cancel,
                    observer,
                )
                .await
        })
    }
    /// Wait until both parallel branches, `high` and `low`, are inside the provider.
    async fn both_branches_started(started: &mut tokio::sync::mpsc::UnboundedReceiver<String>) {
        let mut siblings = HashSet::new();
        while siblings.len() < 2 {
            let node = started.recv().await.unwrap();
            if node == "high" || node == "low" {
                siblings.insert(node);
            }
        }
    }
    async fn saved_definition(
        worker: &LedgerWorker,
        request: &WorkflowRequest,
    ) -> WorkflowDefinition {
        let selected = request.clone();
        read(worker, move |ledger| ledger.saved_workflow(&selected))
            .await
            .unwrap()
            .definition
    }
    /// Poll until the aborted owner has released the run, then return its inspection.
    async fn released_inspection(
        worker: &LedgerWorker,
        run: Id,
    ) -> crate::storage::workflows::recovery::RunInspection {
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            loop {
                let inspection = read(worker, move |ledger| ledger.inspect_workflow(run)).await;
                if !inspection.live_owner {
                    break inspection;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap()
    }
    async fn active_context(
        worker: &LedgerWorker,
        session: Id,
    ) -> Vec<crate::context::projection::ActiveRecord> {
        read(worker, move |ledger| {
            ledger.context_at(
                &ledger.context_sources(session).unwrap(),
                &[],
                session,
                Id::generate(),
            )
        })
        .await
    }
    /// Provider calls made for workflow nodes, excluding ordinary evaluations.
    fn node_calls(provider: &Fake) -> Vec<Call> {
        provider
            .calls
            .lock()
            .unwrap()
            .iter()
            .filter(|call| call.node.is_some())
            .cloned()
            .collect()
    }
    /// The turn a saved node published as its source.
    fn published_turn(run: &StoredRun, node: usize) -> Id {
        run.nodes[node]
            .output
            .as_ref()
            .unwrap()
            .source
            .as_ref()
            .unwrap()
            .frontier[0]
    }
    /// Records that must reach exactly the workflow nodes that introduce them.
    struct BranchFacts {
        adoption: ContextRecord,
        left: ContextRecord,
        right: ContextRecord,
    }
    impl BranchFacts {
        fn new() -> Self {
            Self {
                adoption: record("adoption", serde_json::json!("CALLER_ADOPTED")),
                left: record("left_fact", serde_json::json!("LEFT_ONLY")),
                right: record("right_fact", serde_json::json!("RIGHT_ONLY")),
            }
        }
    }
    /// Both branches on their own models over a frozen base that adopts the caller's
    /// record, with each branch appending a fact its sibling must not see.
    fn isolated_branches(source: Id, facts: &BranchFacts) -> WorkflowRequest {
        let mut request = both_branches();
        request.workflow.nodes[1].model.model = Some("high-model".into());
        request.workflow.nodes[2].model.model = Some("low-model".into());
        request.workflow.state = vec![Input::Text {
            text: "FROZEN_WORKFLOW_BASE".into(),
        }];
        request.session = from_session(source);
        request.updates = vec![ContextUpdate::Pin {
            target: PinTarget::New {
                record: facts.adoption.clone(),
            },
        }];
        request.workflow.nodes[1].updates = vec![ContextUpdate::Append {
            record: facts.left.clone(),
        }];
        request.workflow.nodes[2].updates = vec![ContextUpdate::Append {
            record: facts.right.clone(),
        }];
        request
    }
    async fn assert_session_writer_is_held(
        engine: &Arc<Engine<Fake, ByteCounter, TokioClock>>,
        worker: &LedgerWorker,
        config: &Config,
        session: Id,
    ) {
        let mut competing = ordinary();
        competing.session = continuing(session);
        assert!(matches!(
            engine
                .evaluate(
                    worker,
                    EvaluationInput {
                        request: &competing,
                        config,
                        overrides: &RuntimeOverrides::default(),
                        home: Path::new("/unused"),
                        retry_interrupted: false
                    },
                    |_| None,
                    &CancellationToken::new()
                )
                .await,
            Err(EngineError::Replay(ReplayError::Guard(
                GuardError::WriterActive
            )))
        ));
    }
    fn assert_branches_isolated(provider: &Fake) {
        let calls = node_calls(provider);
        assert_eq!(calls.len(), 5);
        let call = |node: &str| {
            calls
                .iter()
                .find(|call| call.node.as_deref() == Some(node))
                .unwrap()
        };
        let (high, low) = (call("high"), call("low"));
        assert_eq!(high.model, "high-model");
        assert_eq!(low.model, "low-model");
        assert!(high.context.contains("FROZEN_WORKFLOW_BASE"));
        assert!(high.context.contains("CALLER_ADOPTED"));
        assert!(!high.context.contains("RIGHT_ONLY"));
        assert!(!high.context.contains("NODE:low"));
        assert!(!low.context.contains("LEFT_ONLY"));
    }
    /// Both branch turns are parented on the initial node's turn, and one frontier is published.
    async fn assert_branches_fork_from_initial(worker: &LedgerWorker, run: Id) {
        let saved = read(worker, move |ledger| ledger.workflow_run(run)).await;
        assert_eq!(saved.frontier.len(), 1);
        let initial = published_turn(&saved, 0);
        for node in [1, 2] {
            let turn = published_turn(&saved, node);
            let parents = read(worker, move |ledger| ledger.turn(turn)).await.parents;
            assert_eq!(parents, vec![initial]);
        }
    }
    #[tokio::test]
    async fn durable_parallel_nodes_freeze_sources_models_and_publish_one_final_frontier() {
        let store = Store::new();
        let worker = open_writer(&store).await;
        let config = config();
        let (engine, mut started) = engine(&["high", "low"], &[]);
        let budget = record("budget", serde_json::json!(8));
        let mut seed = ordinary();
        seed.updates = vec![ContextUpdate::Pin {
            target: PinTarget::New {
                record: budget.clone(),
            },
        }];
        let source = evaluate(&engine, &worker, &config, &seed).await;
        let facts = BranchFacts::new();
        let request = isolated_branches(source.turn.session, &facts);
        let observer = Observed::new();
        let task = spawn_workflow(
            &engine,
            &worker,
            &request,
            &config,
            CancellationToken::new(),
            observer.clone(),
        );
        both_branches_started(&mut started).await;
        let main = saved_definition(&worker, &request).await.session;
        assert_session_writer_is_held(&engine, &worker, &config, main).await;
        // The source session advances while the run stays frozen on its original frontier.
        let mut update = ordinary();
        update.session = continuing(source.turn.session);
        update.updates = vec![ContextUpdate::Supersede {
            targets: vec![budget.id],
            replacement: record("budget", serde_json::json!(12)),
            pinned: None,
        }];
        evaluate(&engine, &worker, &config, &update).await;
        evaluate(&engine, &worker, &config, &ordinary()).await;
        engine.provider.gates["low"].add_permits(1);
        observer.low_done.notified().await;
        engine.provider.gates["high"].add_permits(1);
        let result = task.await.unwrap().unwrap();
        assert_eq!(result.result.status, RunStatus::Completed);
        assert!(result.persisted);
        assert_eq!(result.sources[0].frontier, vec![source.turn.id]);
        assert_eq!(engine.provider.active.load(Ordering::SeqCst), 0);
        assert!(engine.provider.peak.load(Ordering::SeqCst) >= 2);
        assert_branches_isolated(&engine.provider);
        assert_branches_fork_from_initial(&worker, result.run.unwrap()).await;
        let active = active_context(&worker, main).await;
        for expected in [&budget, &facts.adoption, &facts.left, &facts.right] {
            assert!(active.iter().any(|active| active.record == *expected));
        }
        assert_eq!(observer.completed(), 5);
        let before = engine.provider.calls.lock().unwrap().len();
        let replay = engine
            .evaluate_workflow(
                Some(&worker),
                WorkflowInput {
                    request: &request,
                    config: &Config::default(),
                    overrides: &RuntimeOverrides::default(),
                    home: Path::new("/missing"),
                },
                Arc::new(|_| panic!("replay must not read credentials")),
                CancellationToken::new(),
            )
            .await
            .unwrap();
        assert!(replay.replayed);
        assert_eq!(replay.result, result.result);
        assert_eq!(engine.provider.calls.lock().unwrap().len(), before);
    }
    #[tokio::test]
    async fn independent_roots_share_one_base_and_explicit_node_profiles_need_no_default() {
        let store = Store::new();
        let worker = open_writer(&store).await;
        let (engine, _events) = engine(&[], &[]);
        let mut config = config();
        config.default_provider = None;
        config.default_model = None;
        let mut other = config.providers["fixture"].clone();
        other
            .models
            .insert("other-model".into(), other.models["offline"].clone());
        config.providers.insert("other".into(), other);
        let mut request = request();
        let mut left = request.workflow.nodes[0].clone();
        left.id = "left".into();
        left.turn.state.push(Input::Text {
            text: "left-root".into(),
        });
        left.model.provider = Some("fixture".into());
        left.model.model = Some("offline".into());
        let mut right = left.clone();
        right.id = "right".into();
        right.model.provider = Some("other".into());
        right.model.model = Some("other-model".into());
        let mut join = left.clone();
        join.id = "joined".into();
        join.dependencies = vec!["left".into(), "right".into()];
        request.workflow.nodes = vec![left, right, join];
        request.workflow.outputs.clear();
        request.workflow.state = vec![Input::Text {
            text: "COMMON_BASE".into(),
        }];
        let pinned = record("shared", serde_json::json!(true));
        request.updates = vec![ContextUpdate::Pin {
            target: PinTarget::New {
                record: pinned.clone(),
            },
        }];
        let result = engine
            .evaluate_workflow(
                Some(&worker),
                WorkflowInput {
                    request: &request,
                    config: &config,
                    overrides: &RuntimeOverrides::default(),
                    home: Path::new("/unused"),
                },
                Arc::new(|_| None),
                CancellationToken::new(),
            )
            .await
            .unwrap();
        assert_eq!(result.result.status, RunStatus::Completed);
        assert_eq!(engine.provider.calls.lock().unwrap().len(), 3);
        let main = result.session.unwrap();
        let active = worker
            .call(move |ledger| {
                ledger.context_at(
                    &ledger.context_sources(main).unwrap(),
                    &[],
                    main,
                    Id::generate(),
                )
            })
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            active
                .iter()
                .filter(|active| active.record.id == pinned.id)
                .count(),
            1
        );
        assert!(result.result.nodes.iter().all(|node| {
            node.output
                .as_ref()
                .unwrap()
                .evaluation
                .as_ref()
                .unwrap()
                .manifest
                .records
                .iter()
                .any(|record| record.record == pinned.id && record.protected)
        }));
    }
    #[tokio::test]
    async fn failed_branch_retains_saved_partial_progress_and_ephemeral_runs_write_nothing() {
        let store = Store::new();
        let worker = open_writer(&store).await;
        let (engine, _events) = engine(&[], &["high"]);
        let config = config();
        let mut request = request();
        request.workflow.nodes[1].when = None;
        request.workflow.nodes[2].when = None;
        let partial = engine
            .evaluate_workflow(
                Some(&worker),
                WorkflowInput {
                    request: &request,
                    config: &config,
                    overrides: &RuntimeOverrides::default(),
                    home: Path::new("/unused"),
                },
                Arc::new(|_| None),
                CancellationToken::new(),
            )
            .await
            .unwrap();
        assert_eq!(partial.result.status, RunStatus::Partial);
        assert_eq!(partial.result.nodes[1].status, NodeStatus::Failed);
        assert_eq!(partial.result.nodes[2].status, NodeStatus::Completed);
        assert_eq!(partial.result.nodes[3].status, NodeStatus::Blocked);
        let main = partial.session.unwrap();
        let frontier = partial.frontier.clone().unwrap();
        assert_eq!(frontier.len(), 1);
        assert_eq!(
            worker
                .call(move |ledger| ledger.session(&SessionSelector::Id(main)))
                .await
                .unwrap()
                .unwrap()
                .frontier,
            frontier
        );
        let readonly = LedgerWorker::open(store.0.clone(), OpenMode::ReadOnlyExisting)
            .await
            .unwrap();
        let bytes = std::fs::read(store.0.join("ledger.sqlite3")).unwrap();
        let session_count = worker
            .call(|ledger| ledger.list_sessions().unwrap().len())
            .await
            .unwrap();
        request.request_id = None;
        request.workflow.mode = RunMode::Ephemeral;
        request.session = from_session(main);
        let ephemeral = engine
            .evaluate_workflow(
                Some(&readonly),
                WorkflowInput {
                    request: &request,
                    config: &config,
                    overrides: &RuntimeOverrides::default(),
                    home: Path::new("/missing-home"),
                },
                Arc::new(|_| None),
                CancellationToken::new(),
            )
            .await
            .unwrap();
        assert!(!ephemeral.persisted);
        assert!(ephemeral.run.is_none());
        assert!(ephemeral.session.is_none());
        assert!(ephemeral.frontier.is_none());
        assert_eq!(ephemeral.result.status, RunStatus::Partial);
        assert!(
            ephemeral
                .result
                .nodes
                .iter()
                .filter_map(|node| node.output.as_ref())
                .all(|output| output.source.is_none())
        );
        assert_eq!(
            std::fs::read(store.0.join("ledger.sqlite3")).unwrap(),
            bytes
        );
        assert_eq!(
            worker
                .call(|ledger| ledger.list_sessions().unwrap().len())
                .await
                .unwrap(),
            session_count
        );
    }
    #[tokio::test]
    async fn cancellation_saves_interrupted_nodes_and_releases_the_run_owner() {
        use crate::storage::workflows::recovery::Disposition;
        let store = Store::new();
        let worker = open_writer(&store).await;
        let (engine, mut started) = engine(&["high", "low"], &[]);
        let config = config();
        let request = both_branches();
        let cancel = CancellationToken::new();
        let task = spawn_workflow(
            &engine,
            &worker,
            &request,
            &config,
            cancel.clone(),
            Arc::new(schedule::NoopObserver),
        );
        both_branches_started(&mut started).await;
        let run = saved_definition(&worker, &request).await.run;
        let inspection = read(&worker, move |ledger| ledger.inspect_workflow(run)).await;
        assert!(inspection.live_owner);
        assert!(
            inspection.nodes[1..3]
                .iter()
                .all(|node| node.disposition == Disposition::LiveRunning)
        );
        cancel.cancel();
        let result = task.await.unwrap().unwrap();
        assert_eq!(result.result.status, RunStatus::Interrupted);
        assert_eq!(result.result.nodes[0].status, NodeStatus::Completed);
        assert!(
            result.result.nodes[1..]
                .iter()
                .all(|node| node.status == NodeStatus::Interrupted)
        );
        assert_eq!(engine.provider.active.load(Ordering::SeqCst), 0);
        let main = result.session.unwrap();
        assert!(
            worker
                .call(move |ledger| ledger.session_guard(main).is_ok())
                .await
                .unwrap()
        );
        let attempts = read(&worker, move |ledger| ledger.list_attempts(main)).await;
        assert_eq!(attempts.len(), 3);
        assert_eq!(
            attempts
                .iter()
                .filter(|attempt| attempt
                    .progress
                    .error
                    .as_ref()
                    .is_some_and(|error| error.code == crate::protocol::OUTCOME_UNKNOWN))
                .count(),
            2
        );
        let (restarted, _events) = self::engine(&[], &[]);
        let done = restarted
            .resume_workflow(&worker, run, Arc::new(|_| None), CancellationToken::new())
            .await
            .unwrap();
        assert_eq!(done.result.status, RunStatus::Completed);
        assert_eq!(done.result.nodes[0], result.result.nodes[0]);
        assert_eq!(restarted.provider.calls.lock().unwrap().len(), 4);
        let inspection = read(&worker, move |ledger| ledger.inspect_workflow(run)).await;
        assert_eq!(inspection.nodes[1].uncertain_prior_calls, 1);
        assert_eq!(inspection.nodes[2].uncertain_prior_calls, 1);
        assert_eq!(inspection.nodes[1].attempts.len(), 2);
    }
    struct BrokenObserver;
    impl ScheduleObserver for BrokenObserver {
        async fn changed(&self, _node: usize, _progress: NodeProgress) -> Result<(), ErrorReport> {
            Err(ErrorReport {
                category: ErrorCategory::Internal,
                code: "sink_closed".into(),
                message: "Progress sink closed".into(),
                retryable: false,
            })
        }
    }
    #[tokio::test]
    async fn failed_progress_sink_persists_interruption_before_returning_an_error() {
        let store = Store::new();
        let worker = open_writer(&store).await;
        let (engine, _events) = engine(&[], &[]);
        let config = config();
        let request = request();
        let error = engine
            .evaluate_workflow_observed(
                Some(&worker),
                WorkflowInput {
                    request: &request,
                    config: &config,
                    overrides: &RuntimeOverrides::default(),
                    home: Path::new("/unused"),
                },
                Arc::new(|_| None),
                CancellationToken::new(),
                Arc::new(BrokenObserver),
            )
            .await
            .unwrap_err();
        let WorkflowEngineError::Execution { run, .. } = error else {
            panic!("expected interrupted run identity")
        };
        let stored = worker
            .call(move |ledger| ledger.workflow_run(run))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(stored.status, RunStatus::Interrupted);
        assert!(
            stored
                .nodes
                .iter()
                .all(|node| node.status == NodeStatus::Interrupted)
        );
        assert!(engine.provider.calls.lock().unwrap().is_empty());
    }
    #[tokio::test]
    async fn fresh_ephemeral_workflow_needs_no_store_and_returns_no_saved_identity() {
        let store = Store::new();
        let home = store.0.join("absent-home");
        let (engine, _events) = engine(&[], &[]);
        let config = config();
        let mut request = request();
        request.workflow.mode = RunMode::Ephemeral;
        request.request_id = None;
        request.workflow.state = vec![Input::Text {
            text: "IN_MEMORY_BASE".into(),
        }];
        let result = engine
            .evaluate_workflow(
                None,
                WorkflowInput {
                    request: &request,
                    config: &config,
                    overrides: &RuntimeOverrides::default(),
                    home: &home,
                },
                Arc::new(|_| None),
                CancellationToken::new(),
            )
            .await
            .unwrap();
        assert_eq!(result.result.status, RunStatus::Completed);
        assert!(!result.persisted);
        assert!(result.run.is_none());
        assert!(result.session.is_none());
        assert!(result.frontier.is_none());
        assert!(result.request_id.is_none());
        assert!(!store.0.exists());
        assert_eq!(engine.provider.calls.lock().unwrap().len(), 4);
        for call in engine.provider.calls.lock().unwrap().iter() {
            assert!(call.context.contains("IN_MEMORY_BASE"));
        }
    }
    #[tokio::test]
    async fn failed_publication_settles_abandoned_attempt_and_saves_other_branch_progress() {
        let store = Store::new();
        let worker = open_writer(&store).await;
        let (engine, _events) = engine(&[], &[]);
        *engine.provider.publication_fault.lock().unwrap() = Some(worker.clone());
        let config = config();
        let mut request = request();
        request.workflow.nodes[1].when = None;
        request.workflow.nodes[2].when = None;
        let result = engine
            .evaluate_workflow(
                Some(&worker),
                WorkflowInput {
                    request: &request,
                    config: &config,
                    overrides: &RuntimeOverrides::default(),
                    home: Path::new("/unused"),
                },
                Arc::new(|_| None),
                CancellationToken::new(),
            )
            .await
            .unwrap();
        assert_eq!(result.result.status, RunStatus::Partial);
        assert_eq!(result.result.nodes[1].status, NodeStatus::Failed);
        assert_eq!(result.result.nodes[2].status, NodeStatus::Completed);
        let run = result.run.unwrap();
        let main = result.session.unwrap();
        let snapshot = worker
            .call(move |ledger| ledger.workflow_run(run))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(snapshot.status, RunStatus::Partial);
        assert_eq!(snapshot.frontier, result.frontier.unwrap());
        assert_eq!(snapshot.frontier.len(), 1);
        let attempts = worker
            .call(move |ledger| ledger.list_attempts(main))
            .await
            .unwrap()
            .unwrap();
        assert!(attempts.iter().all(|attempt| !matches!(
            attempt.progress.state,
            crate::storage::turns::AttemptStatus::Running
                | crate::storage::turns::AttemptStatus::Pending
        )));
        let high = attempts
            .iter()
            .find(|attempt| {
                attempt
                    .intent
                    .workflow
                    .as_ref()
                    .is_some_and(|scope| scope.node == 1)
            })
            .unwrap();
        assert_eq!(
            high.progress.state,
            crate::storage::turns::AttemptStatus::Interrupted
        );
        assert_eq!(
            high.progress.error.as_ref().unwrap().code,
            crate::protocol::OUTCOME_UNKNOWN
        );
        *engine.provider.publication_fault.lock().unwrap() = None;
        worker
            .call(|ledger| {
                ledger.transaction::<_, crate::storage::StorageError>(
                    "clear publication fault",
                    |tx| {
                        tx.execute_batch("DROP TRIGGER reject_high_publication")
                            .unwrap();
                        Ok(())
                    },
                )
            })
            .await
            .unwrap()
            .unwrap();
        let (restarted, _rx) = self::engine(&[], &[]);
        let done = restarted
            .resume_workflow(&worker, run, Arc::new(|_| None), CancellationToken::new())
            .await
            .unwrap();
        assert_eq!(done.result.status, RunStatus::Completed);
        assert_eq!(done.result.nodes[0], result.result.nodes[0]);
        assert_eq!(done.result.nodes[2], result.result.nodes[2]);
        assert_eq!(restarted.provider.calls.lock().unwrap().len(), 3);
        let inspection = worker
            .call(move |ledger| ledger.inspect_workflow(run))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(inspection.nodes[1].uncertain_prior_calls, 1);
        assert_eq!(inspection.nodes[1].attempts.len(), 2);
    }
    async fn run_fixture(
        engine: &Arc<Engine<Fake, ByteCounter, TokioClock>>,
        worker: &LedgerWorker,
        config: &Config,
        request: &WorkflowRequest,
    ) -> WorkflowResult {
        engine
            .evaluate_workflow(
                Some(worker),
                WorkflowInput {
                    request,
                    config,
                    overrides: &RuntimeOverrides::default(),
                    home: Path::new("/unused"),
                },
                Arc::new(|_| None),
                CancellationToken::new(),
            )
            .await
            .unwrap()
    }

    /// Each round saves its own result, every high-node attempt records its round, and the
    /// inspection never exposes a credential value.
    async fn assert_rounds_saved_without_secrets(
        worker: &LedgerWorker,
        run: Id,
        results: &[&WorkflowResult],
    ) {
        let rounds = read(worker, move |ledger| ledger.workflow_round_results(run)).await;
        assert_eq!(
            rounds.iter().map(|round| round.round).collect::<Vec<_>>(),
            vec![0, 1, 2]
        );
        assert_eq!(rounds.len(), results.len());
        for (round, result) in rounds.iter().zip(results) {
            assert_eq!(round.result, result.result);
        }
        let inspected = read(worker, move |ledger| ledger.inspect_workflow(run)).await;
        assert!(!inspected.live_owner);
        assert_eq!(inspected.nodes[1].attempts.len(), 3);
        assert_eq!(
            inspected.nodes[1]
                .attempts
                .iter()
                .map(|attempt| attempt.intent.workflow.as_ref().unwrap().round)
                .collect::<Vec<_>>(),
            vec![0, 1, 2]
        );
        let encoded = crate::codec::encode_toon(&inspected).unwrap();
        for secret in ["old-secret", "new-secret", "replacement-secret"] {
            assert!(!encoded.contains(secret));
        }
    }
    #[tokio::test]
    async fn explicit_rounds_reuse_success_and_skips_preserve_failures_and_freeze_settings() {
        let store = Store::new();
        let worker = open_writer(&store).await;
        let (first, _rx) = engine(&[], &["high"]);
        let mut config = config();
        config.providers.get_mut("fixture").unwrap().api_key_env = Some("SAVED_KEY_SOURCE".into());
        let mut request = request();
        request.workflow.nodes[1].model.model = Some("frozen-alias".into());
        let result = first
            .evaluate_workflow(
                Some(&worker),
                WorkflowInput {
                    request: &request,
                    config: &config,
                    overrides: &RuntimeOverrides::default(),
                    home: Path::new("/unused"),
                },
                Arc::new(|name| {
                    assert_eq!(name.to_string(), "env:SAVED_KEY_SOURCE");
                    Some("old-secret".into())
                }),
                CancellationToken::new(),
            )
            .await
            .unwrap();
        assert_eq!(result.result.status, RunStatus::Partial);
        assert_eq!(result.result.nodes[2].status, NodeStatus::Skipped);
        let run = result.run.unwrap();
        let initial = result.result.nodes[0].clone();
        let skipped = result.result.nodes[2].clone();
        // An explicit retry can exhaust its own bounded attempt budget again.
        let again = first
            .resume_workflow(
                &worker,
                run,
                Arc::new(|_| Some("new-secret".into())),
                CancellationToken::new(),
            )
            .await
            .unwrap();
        assert_eq!(again.result.status, RunStatus::Partial);
        assert_eq!(again.result.start_order, vec![1]);
        assert_eq!(again.result.nodes[0], initial);
        assert_eq!(again.result.nodes[2], skipped);
        let (restarted, _rx2) = engine(&[], &[]);
        let looked_up = Arc::new(Mutex::new(Vec::new()));
        let captured = looked_up.clone();
        // Config/workflow file edits cannot be passed to resume; only the saved run is accepted.
        config.providers.clear();
        request.workflow.nodes.clear();
        let done = restarted
            .resume_workflow(
                &worker,
                run,
                Arc::new(move |name| {
                    captured.lock().unwrap().push(name.to_string());
                    Some("replacement-secret".into())
                }),
                CancellationToken::new(),
            )
            .await
            .unwrap();
        assert_eq!(done.result.status, RunStatus::Completed);
        assert_eq!(done.result.start_order, vec![1, 3, 4]);
        let served = &done.result.nodes[1].output.as_ref().unwrap().outcome;
        assert_eq!(served.provider.model, "served-version-2026-10");
        assert_eq!(done.result.nodes[0], initial);
        assert_eq!(done.result.nodes[2], skipped);
        assert_eq!(*looked_up.lock().unwrap(), vec!["env:SAVED_KEY_SOURCE"; 3]);
        assert_eq!(
            restarted.provider.calls.lock().unwrap()[0].model,
            "frozen-alias"
        );
        assert_rounds_saved_without_secrets(&worker, run, &[&result, &again, &done]).await;
        let replay = restarted
            .resume_workflow(
                &worker,
                run,
                Arc::new(|_| panic!("completed replay needs no credentials")),
                CancellationToken::new(),
            )
            .await
            .unwrap();
        assert!(replay.replayed);
        assert_eq!(restarted.provider.calls.lock().unwrap().len(), 3);
    }

    #[tokio::test]
    async fn abandoned_before_dispatch_is_inspectable_read_only_and_resumes_under_one_owner() {
        let store = Store::new();
        let worker = open_writer(&store).await;
        let request = request();
        let config = config();
        let (settings, defaults) = resolve_settings(
            &WorkflowInput {
                request: &request,
                config: &config,
                overrides: &RuntimeOverrides::default(),
                home: Path::new("/unused"),
            },
            &request,
            &ModelSelection::default(),
        )
        .unwrap();
        let selected = request.clone();
        let owner = worker
            .call(move |ledger| ledger.accept_workflow(&selected, settings, defaults))
            .await
            .unwrap()
            .unwrap();
        let WorkflowDecision::Accepted(owner) = owner else {
            panic!("new owner")
        };
        let run = owner.frozen().definition.run;
        let reader = LedgerWorker::open(store.0.clone(), OpenMode::ReadOnlyExisting)
            .await
            .unwrap();
        let bytes = std::fs::read(store.0.join("ledger.sqlite3")).unwrap();
        let inspected = reader
            .call(move |ledger| ledger.inspect_workflow(run))
            .await
            .unwrap()
            .unwrap();
        assert!(inspected.live_owner);
        assert!(inspected.nodes.iter().all(|node| node.disposition
            == crate::storage::workflows::recovery::Disposition::NeverDispatched));
        assert_eq!(
            bytes,
            std::fs::read(store.0.join("ledger.sqlite3")).unwrap()
        );
        assert!(matches!(
            worker
                .call(move |ledger| ledger.resume_workflow(run))
                .await
                .unwrap()
                .unwrap(),
            WorkflowDecision::InProgress
        ));
        drop(owner);
        let owner = worker
            .call(move |ledger| ledger.resume_workflow(run))
            .await
            .unwrap()
            .unwrap();
        let WorkflowDecision::Accepted(owner) = owner else {
            panic!("recovered owner")
        };
        assert_eq!(
            worker
                .call(move |ledger| ledger.workflow_round_results(run))
                .await
                .unwrap()
                .unwrap()
                .len(),
            1
        );
        assert!(matches!(
            worker
                .call(move |ledger| ledger.resume_workflow(run))
                .await
                .unwrap()
                .unwrap(),
            WorkflowDecision::InProgress
        ));
        drop(owner);
        let (engine, _rx) = engine(&[], &[]);
        let done = engine
            .resume_workflow(&worker, run, Arc::new(|_| None), CancellationToken::new())
            .await
            .unwrap();
        assert_eq!(done.result.status, RunStatus::Completed);
        assert_eq!(engine.provider.calls.lock().unwrap().len(), 4);
        let rounds = worker
            .call(move |ledger| ledger.workflow_round_results(run))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(rounds.len(), 3);
        assert!(
            rounds[..2]
                .iter()
                .all(|round| round.result.status == RunStatus::Interrupted)
        );
    }

    #[tokio::test]
    async fn advanced_session_head_rejects_resume_without_mutating_saved_run() {
        let store = Store::new();
        let worker = open_writer(&store).await;
        let (first, _rx) = engine(&[], &["high"]);
        let config = config();
        let partial = run_fixture(&first, &worker, &config, &request()).await;
        let run = partial.run.unwrap();
        let main = partial.session.unwrap();
        let mut next = ordinary();
        next.session = continuing(main);
        evaluate(&first, &worker, &config, &next).await;
        let before = std::fs::read(store.0.join("ledger.sqlite3")).unwrap();
        let (restarted, _rx2) = engine(&[], &[]);
        assert!(matches!(
            restarted
                .resume_workflow(&worker, run, Arc::new(|_| None), CancellationToken::new())
                .await,
            Err(WorkflowEngineError::Repository(WorkflowError::AdvancedHead))
        ));
        assert_eq!(
            before,
            std::fs::read(store.0.join("ledger.sqlite3")).unwrap()
        );
        let inspected = worker
            .call(move |ledger| ledger.inspect_workflow(run))
            .await
            .unwrap()
            .unwrap();
        assert!(!inspected.session_head_compatible);
        assert!(!inspected.live_owner);
        assert_eq!(inspected.run.result, Some(partial.result));
        assert!(restarted.provider.calls.lock().unwrap().is_empty());
    }
    #[tokio::test]
    async fn owner_drop_during_dispatch_keeps_unknown_receipt_and_frozen_source_snapshot() {
        let store = Store::new();
        let worker = open_writer(&store).await;
        let (first, mut started) = engine(&["initial"], &[]);
        let config = config();
        let old = record("budget", serde_json::json!(8));
        let mut seed = ordinary();
        seed.updates = vec![ContextUpdate::Append {
            record: old.clone(),
        }];
        let source = evaluate(&first, &worker, &config, &seed).await;
        let mut request = request();
        request.session = from_session(source.turn.session);
        let runner = first.clone();
        let repository = worker.clone();
        let selected = request.clone();
        let settings = config.clone();
        let task =
            tokio::spawn(
                async move { run_fixture(&runner, &repository, &settings, &selected).await },
            );
        assert_eq!(started.recv().await.unwrap(), "initial");
        let definition = saved_definition(&worker, &request).await;
        let run = definition.run;
        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
        let inspection = released_inspection(&worker, run).await;
        assert_eq!(
            inspection.nodes[0].disposition,
            crate::storage::workflows::recovery::Disposition::ProviderOutcomeUnknown
        );
        assert_eq!(inspection.nodes[0].uncertain_prior_calls, 1);
        assert_eq!(first.provider.active.load(Ordering::SeqCst), 0);
        let mut change = ordinary();
        change.session = continuing(source.turn.session);
        change.updates = vec![ContextUpdate::Supersede {
            targets: vec![old.id],
            replacement: record("budget", serde_json::json!(12)),
            pinned: Some(false),
        }];
        evaluate(&first, &worker, &config, &change).await;
        let (restarted, _rx) = engine(&[], &[]);
        let done = restarted
            .resume_workflow(&worker, run, Arc::new(|_| None), CancellationToken::new())
            .await
            .unwrap();
        assert_eq!(done.result.status, RunStatus::Completed);
        assert_eq!(done.sources, definition.base.sources);
        assert_eq!(restarted.provider.calls.lock().unwrap().len(), 4);
        assert!(
            restarted.provider.calls.lock().unwrap()[0]
                .context
                .contains(&old.id.to_string())
        );
        let inspection = worker
            .call(move |ledger| ledger.inspect_workflow(run))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(inspection.nodes[0].attempts.len(), 2);
        assert_eq!(inspection.nodes[0].uncertain_prior_calls, 1);
        assert_eq!(
            inspection.nodes[0].attempts[0].progress.state,
            crate::storage::turns::AttemptStatus::Interrupted
        );
    }
    struct PausedRetryClock {
        waiting: tokio::sync::Notify,
    }
    impl RetryClock for PausedRetryClock {
        fn now(&self) -> std::time::SystemTime {
            std::time::SystemTime::UNIX_EPOCH
        }
        async fn wait(&self, _delay: std::time::Duration, cancel: &CancellationToken) -> bool {
            self.waiting.notify_one();
            cancel.cancelled().await;
            false
        }
    }
    #[tokio::test]
    async fn recorded_failed_call_is_never_reported_as_never_dispatched_during_retry_backoff() {
        use crate::storage::workflows::recovery::Disposition;
        let store = Store::new();
        let worker = open_writer(&store).await;
        let (base, _rx) = engine(&[], &["initial"]);
        let Ok(mut base) = Arc::try_unwrap(base) else {
            panic!("single owner")
        };
        base.provider.transient = true;
        let runner = Arc::new(Engine {
            provider: base.provider,
            counter: base.counter,
            clock: PausedRetryClock {
                waiting: tokio::sync::Notify::new(),
            },
        });
        let request = request();
        let mut config = config();
        config.execution.max_attempts = 2;
        let selected = request.clone();
        let repository = worker.clone();
        let engine = runner.clone();
        let task = tokio::spawn(async move {
            engine
                .evaluate_workflow(
                    Some(&repository),
                    WorkflowInput {
                        request: &selected,
                        config: &config,
                        overrides: &RuntimeOverrides::default(),
                        home: Path::new("/unused"),
                    },
                    Arc::new(|_| None),
                    CancellationToken::new(),
                )
                .await
        });
        runner.clock.waiting.notified().await;
        let definition = saved_definition(&worker, &request).await;
        let run = definition.run;
        let before = std::fs::read(store.0.join("ledger.sqlite3")).unwrap();
        let inspection = worker
            .call(move |ledger| ledger.inspect_workflow(run))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(inspection.nodes[0].disposition, Disposition::RetryWaiting);
        assert_eq!(inspection.nodes[0].attempts.len(), 1);
        assert_eq!(
            inspection.nodes[0].attempts[0].progress.state,
            crate::storage::turns::AttemptStatus::Failed
        );
        assert_eq!(
            before,
            std::fs::read(store.0.join("ledger.sqlite3")).unwrap()
        );
        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
        let inspection = released_inspection(&worker, run).await;
        assert_eq!(
            inspection.nodes[0].disposition,
            Disposition::InterruptedBeforeCall
        );
        assert_eq!(inspection.nodes[0].uncertain_prior_calls, 0);
        assert_eq!(inspection.nodes[0].attempts.len(), 1);
    }
    /// The abandoned round's events end in exactly one `RunFinished`, and rounds never
    /// interleave in the saved event log.
    async fn assert_abandoned_round_closed_once(worker: &LedgerWorker, run: Id) {
        use crate::storage::workflows::WorkflowEventKind;
        let events = read(worker, move |ledger| ledger.workflow_events(run)).await;
        let previous = events
            .iter()
            .filter(|event| event.event.round == 0)
            .collect::<Vec<_>>();
        assert_eq!(
            previous.last().unwrap().event.kind,
            WorkflowEventKind::RunFinished
        );
        assert_eq!(
            previous
                .iter()
                .filter(|event| event.event.kind == WorkflowEventKind::RunFinished)
                .count(),
            1
        );
        assert!(
            events
                .windows(2)
                .all(|pair| pair[0].event.round <= pair[1].event.round)
        );
    }
    #[tokio::test]
    async fn owner_exit_between_completions_preserves_prior_round_order_and_reuses_saved_nodes() {
        let store = Store::new();
        let worker = open_writer(&store).await;
        let (first, mut started) = engine(&["high", "low"], &[]);
        let request = both_branches();
        let config = config();
        let observer = Observed::new();
        let task = spawn_workflow(
            &first,
            &worker,
            &request,
            &config,
            CancellationToken::new(),
            observer.clone(),
        );
        both_branches_started(&mut started).await;
        first.provider.gates["low"].add_permits(1);
        observer.low_done.notified().await;
        let run = saved_definition(&worker, &request).await.run;
        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
        let inspection = released_inspection(&worker, run).await;
        let (restarted, _rx) = engine(&[], &[]);
        let done = restarted
            .resume_workflow(&worker, run, Arc::new(|_| None), CancellationToken::new())
            .await
            .unwrap();
        assert_eq!(done.result.status, RunStatus::Completed);
        assert_eq!(done.result.nodes[0], inspection.run.nodes[0]);
        assert_eq!(done.result.nodes[2], inspection.run.nodes[2]);
        assert_eq!(done.result.start_order, vec![1, 3, 4]);
        assert_eq!(restarted.provider.calls.lock().unwrap().len(), 3);
        let rounds = read(&worker, move |ledger| ledger.workflow_round_results(run)).await;
        assert_eq!(rounds[0].result.start_order, vec![0, 1, 2]);
        assert_eq!(rounds[0].result.completion_order, vec![0, 2]);
        assert_eq!(rounds[0].result.nodes[1].status, NodeStatus::Interrupted);
        assert_eq!(rounds[0].result.nodes[2], done.result.nodes[2]);
        assert_abandoned_round_closed_once(&worker, run).await;
    }
    /// The saved event log is gap-free, versioned, closes each of the two rounds once, and
    /// links attempts and the initial node's turn.
    fn assert_event_log_spans_both_rounds(
        recorded: &[crate::storage::workflows::events::StoredEvent],
        run: Id,
        initial: Id,
    ) {
        assert!(
            recorded
                .iter()
                .enumerate()
                .all(|(index, event)| event.sequence == index as u64
                    && event.event.version == 1
                    && event.run == run)
        );
        for round in 0..=1 {
            assert_eq!(
                recorded
                    .iter()
                    .filter(|event| event.event.round == round
                        && event.event.kind
                            == crate::storage::workflows::WorkflowEventKind::RunFinished)
                    .count(),
                1
            );
        }
        assert!(recorded.iter().any(|event| event.event.attempt.is_some()));
        assert!(
            recorded
                .iter()
                .any(|event| event.event.turn == Some(initial))
        );
    }
    /// Retrieve the intermediate initial-node turn the compact report omits.
    async fn assert_intermediate_turn_retrievable(worker: &LedgerWorker, initial: Id) {
        let turn = read(worker, move |ledger| ledger.turn(initial)).await;
        assert_eq!(turn.outcome.outcomes.len(), 4);
        assert!(
            turn.request
                .state
                .iter()
                .any(|input| matches!(input,Input::Text {text} if text=="NODE:initial"))
        );
    }
    #[tokio::test]
    async fn compact_reports_and_event_streams_survive_partial_resume_and_retrieve_intermediates() {
        use crate::protocol::{EventKind, Report, stream::CollectEvents, validate_events};
        let store = Store::new();
        let worker = open_writer(&store).await;
        let (first, _rx) = engine(&[], &["high"]);
        let request = request();
        let config = config();
        let sink = Arc::new(CollectEvents::default());
        let envelope = first
            .evaluate_workflow_report(
                Some(&worker),
                WorkflowInput {
                    request: &request,
                    config: &config,
                    overrides: &RuntimeOverrides::default(),
                    home: Path::new("/unused"),
                },
                Arc::new(|_| None),
                CancellationToken::new(),
                sink.clone(),
            )
            .await
            .unwrap();
        let Report::Run(partial) = envelope.result else {
            panic!("partial report")
        };
        let run = partial.run.unwrap();
        assert_eq!(partial.status, RunStatus::Partial);
        assert!(partial.outputs[0].required_missing);
        assert!(partial.nodes.iter().all(|node| node.outcome.is_none()));
        let events = sink.snapshot().unwrap();
        validate_events(&events).unwrap();
        assert_eq!(
            events
                .iter()
                .filter(|event| matches!(event.event, EventKind::Terminal { .. }))
                .count(),
            1
        );
        assert!(
            !serde_json::to_string(&events[..events.len() - 1])
                .unwrap()
                .contains("NODE:")
        );
        let initial = partial.nodes[0].turn.unwrap();
        assert_intermediate_turn_retrievable(&worker, initial).await;
        let (restarted, _rx2) = engine(&[], &[]);
        let resumed_sink = Arc::new(CollectEvents::default());
        let finished = restarted
            .resume_workflow_report(
                &worker,
                run,
                Arc::new(|_| None),
                CancellationToken::new(),
                resumed_sink.clone(),
            )
            .await
            .unwrap();
        let Report::Run(done) = finished.result else {
            panic!("completed report")
        };
        assert_eq!(done.status, RunStatus::Completed);
        assert_eq!(done.nodes[0].turn, Some(initial));
        assert_eq!(done.nodes[2].status, NodeStatus::Skipped);
        validate_events(&resumed_sink.snapshot().unwrap()).unwrap();
        assert_eq!(restarted.provider.calls.lock().unwrap().len(), 3);
        let recorded = read(&worker, move |ledger| ledger.workflow_events(run)).await;
        assert_event_log_spans_both_rounds(&recorded, run, initial);
        let session = done.session.unwrap();
        let export = read(&worker, move |ledger| {
            ledger.export_session(&SessionSelector::Id(session))
        })
        .await;
        assert_eq!(export.workflows.len(), 1);
        assert_eq!(export.workflows[0].rounds.len(), 2);
        assert_eq!(export.workflows[0].events, recorded);
        assert_eq!(export.node_intents.len(), 5);
        assert_eq!(export.attempts.len(), 5);
        let final_turn = done.nodes[4].turn.unwrap();
        let inspected = read(&worker, move |ledger| ledger.inspect_turn(final_turn)).await;
        assert_eq!(inspected.node_intents.len(), 1);
        assert!(inspected.workflow.is_some());
        let replay_sink = Arc::new(CollectEvents::default());
        let replay = restarted
            .resume_workflow_report(
                &worker,
                run,
                Arc::new(|_| panic!("replay")),
                CancellationToken::new(),
                replay_sink.clone(),
            )
            .await
            .unwrap();
        let Report::Run(replay) = replay.result else {
            panic!("replay")
        };
        assert!(replay.replayed);
        assert_eq!(replay.outputs, done.outputs);
        assert_eq!(replay_sink.snapshot().unwrap().len(), 2);
        validate_events(&replay_sink.snapshot().unwrap()).unwrap();
    }

    #[tokio::test]
    async fn cancellation_and_preflight_errors_each_emit_one_truthful_terminal_event() {
        use crate::protocol::{ErrorCategory, Report, stream::CollectEvents, validate_events};
        let store = Store::new();
        let worker = open_writer(&store).await;
        let (first, mut started) = engine(&["initial"], &[]);
        let request = request();
        let config = config();
        let sink = Arc::new(CollectEvents::default());
        let runner = first.clone();
        let selected = request.clone();
        let repository = worker.clone();
        let settings = config.clone();
        let events = sink.clone();
        let cancel = CancellationToken::new();
        let token = cancel.clone();
        let task = tokio::spawn(async move {
            runner
                .evaluate_workflow_report(
                    Some(&repository),
                    WorkflowInput {
                        request: &selected,
                        config: &settings,
                        overrides: &RuntimeOverrides::default(),
                        home: Path::new("/unused"),
                    },
                    Arc::new(|_| None),
                    token,
                    events,
                )
                .await
                .unwrap()
        });
        assert_eq!(started.recv().await.unwrap(), "initial");
        cancel.cancel();
        let canceled = task.await.unwrap();
        let Report::Run(report) = canceled.result else {
            panic!("recoverable run")
        };
        assert_eq!(report.status, RunStatus::Interrupted);
        assert!(report.run.is_some());
        assert_eq!(first.provider.active.load(Ordering::SeqCst), 0);
        validate_events(&sink.snapshot().unwrap()).unwrap();
        let mut invalid = request;
        invalid.workflow.nodes[0].dependencies = vec!["final".into()];
        let rejected = Arc::new(CollectEvents::default());
        let before = std::fs::read(store.0.join("ledger.sqlite3")).unwrap();
        let result = first
            .evaluate_workflow_report(
                Some(&worker),
                WorkflowInput {
                    request: &invalid,
                    config: &config,
                    overrides: &RuntimeOverrides::default(),
                    home: Path::new("/unused"),
                },
                Arc::new(|_| None),
                CancellationToken::new(),
                rejected.clone(),
            )
            .await
            .unwrap();
        assert!(
            matches!(result.result,Report::Error(error) if error.category==ErrorCategory::Input)
        );
        assert_eq!(rejected.snapshot().unwrap().len(), 2);
        validate_events(&rejected.snapshot().unwrap()).unwrap();
        assert_eq!(
            before,
            std::fs::read(store.0.join("ledger.sqlite3")).unwrap()
        );
    }
    struct TerminalFaultSink {
        events: crate::protocol::stream::CollectEvents,
        worker: LedgerWorker,
    }
    impl crate::protocol::stream::EventSink for TerminalFaultSink {
        async fn send(&self, event: crate::protocol::EngineEvent) -> Result<(), ErrorReport> {
            if matches!(&event.event,crate::protocol::EventKind::NodeState {node,status:NodeStatus::Completed,..} if node=="final")
            {
                self.worker.call(|ledger|ledger.transaction::<_,crate::storage::StorageError>("inject terminal publication failure",|tx| {
                    tx.execute_batch("CREATE TRIGGER reject_run_finish BEFORE UPDATE OF result_toon ON workflow_runs WHEN NEW.result_toon IS NOT NULL BEGIN SELECT RAISE(ABORT,'fault'); END;").unwrap();Ok(())
                })).await.unwrap().unwrap();
            }
            crate::protocol::stream::EventSink::send(&self.events, event).await
        }
    }
    #[tokio::test]
    async fn terminal_publication_failure_retains_started_run_reference_and_reuses_every_saved_node()
     {
        use crate::protocol::{ErrorCategory, Report, stream::CollectEvents, validate_events};
        let store = Store::new();
        let worker = open_writer(&store).await;
        let (first, _rx) = engine(&[], &[]);
        let request = request();
        let config = config();
        let sink = Arc::new(TerminalFaultSink {
            events: CollectEvents::default(),
            worker: worker.clone(),
        });
        let result = first
            .evaluate_workflow_report(
                Some(&worker),
                WorkflowInput {
                    request: &request,
                    config: &config,
                    overrides: &RuntimeOverrides::default(),
                    home: Path::new("/unused"),
                },
                Arc::new(|_| None),
                CancellationToken::new(),
                sink.clone(),
            )
            .await
            .unwrap();
        assert!(
            matches!(result.result,Report::Error(error) if error.category==ErrorCategory::Storage)
        );
        let events = sink.events.snapshot().unwrap();
        validate_events(&events).unwrap();
        let run = events[0].run.unwrap();
        assert_eq!(result.recovery.unwrap().run, Some(run));
        let inspection = worker
            .call(move |ledger| ledger.inspect_workflow(run))
            .await
            .unwrap()
            .unwrap();
        assert!(!inspection.live_owner);
        assert!(
            inspection
                .run
                .nodes
                .iter()
                .all(|node| matches!(node.status, NodeStatus::Completed | NodeStatus::Skipped))
        );
        worker
            .call(|ledger| {
                ledger.transaction::<_, crate::storage::StorageError>(
                    "clear terminal failure",
                    |tx| {
                        tx.execute_batch("DROP TRIGGER reject_run_finish").unwrap();
                        Ok(())
                    },
                )
            })
            .await
            .unwrap()
            .unwrap();
        let (restarted, _rx2) = engine(&[], &[]);
        let stream = Arc::new(CollectEvents::default());
        let done = restarted
            .resume_workflow_report(
                &worker,
                run,
                Arc::new(|_| None),
                CancellationToken::new(),
                stream.clone(),
            )
            .await
            .unwrap();
        assert!(matches!(done.result,Report::Run(report) if report.status==RunStatus::Completed));
        assert!(restarted.provider.calls.lock().unwrap().is_empty());
        validate_events(&stream.snapshot().unwrap()).unwrap();
    }
}
