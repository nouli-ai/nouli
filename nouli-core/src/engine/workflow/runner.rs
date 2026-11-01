//! Node execution and context assembly for saved and ephemeral workflow branches.
use super::{CredentialLookup, report};
use crate::{
    engine::{
        Engine,
        execution::{DecisionProvider, RetryClock},
        run::{DurableRecorder, EphemeralRecorder, Recorder, RunInput},
    },
    history::{EvaluationRequest, FinalizedTurn, SourcePoint, StandingChanges},
    identity::Id,
    limits::count::TokenCounter,
    protocol::ErrorReport,
    storage::{
        worker::LedgerWorker,
        workflows::{FrozenWorkflow, WorkflowError, WorkflowOwner},
    },
    workflow::{
        compile::CompiledWorkflow,
        node::{node_output, parent_sources, request_for},
        schedule::{
            AncestorResult, NodeCall, NodeEvaluation, NodeExecutor, NodeOutput, NodeProgress,
            ScheduleObserver,
        },
    },
};
use std::{collections::HashSet, sync::Arc};
use tokio_util::sync::CancellationToken;

pub(super) enum Target {
    Durable {
        worker: LedgerWorker,
        owner: Arc<WorkflowOwner>,
    },
    Ephemeral {
        history: Vec<FinalizedTurn>,
        ids: Vec<Id>,
    },
}
pub(super) struct Runner<P, C, T, O> {
    pub(super) observer: Arc<O>,
    pub(super) engine: Arc<Engine<P, C, T>>,
    pub(super) frozen: Arc<FrozenWorkflow>,
    pub(super) graph: Arc<CompiledWorkflow>,
    pub(super) target: Target,
    pub(super) lookup: CredentialLookup,
}
/// What one node evaluates against and where its attempts are recorded.
struct NodeContext<R> {
    sources: Vec<SourcePoint>,
    history: Vec<FinalizedTurn>,
    recorder: R,
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
        let node = call.node;
        let request = request_for(&self.frozen, node, &call.inputs).map_err(report)?;
        match &self.target {
            Target::Durable { worker, owner } => {
                let context = self.durable_context(worker, owner, &call, &request).await?;
                let turn = self.execute_node(context, &request, node, &cancel).await?;
                Ok(node_output(&turn))
            }
            Target::Ephemeral { history, ids } => {
                let context = self.ephemeral_context(history, ids, &call)?;
                let result = self.execute_node(context, &request, node, &cancel).await?;
                Ok(ephemeral_node_output(result, request))
            }
        }
    }
}
impl<
    P: DecisionProvider + Send + 'static,
    C: TokenCounter + Sync + Send + 'static,
    T: RetryClock + Send + 'static,
    O: ScheduleObserver,
> Runner<P, C, T, O>
{
    /// Evaluate a node while preserving the recording policy's result type.
    async fn execute_node<R: Recorder>(
        &self,
        context: NodeContext<R>,
        request: &EvaluationRequest,
        node: usize,
        cancel: &CancellationToken,
    ) -> Result<R::Output, ErrorReport> {
        let definition = &self.frozen.definition;
        self.engine
            .execute(
                RunInput {
                    context_base: Some(&definition.base),
                    runtime_only: definition.base.runtime_only,
                    request,
                    sources: &context.sources,
                    history: &context.history,
                    settings: self.frozen.settings[node].clone(),
                    session: definition.session,
                },
                context.recorder,
                |name| (self.lookup)(name),
                cancel,
            )
            .await
            .map_err(report)
    }
    /// Accept the node in the ledger and read the saved history of its parents' turns.
    async fn durable_context<'a>(
        &self,
        worker: &'a LedgerWorker,
        owner: &Arc<WorkflowOwner>,
        call: &NodeCall,
        request: &EvaluationRequest,
    ) -> Result<NodeContext<DurableRecorder<'a>>, ErrorReport> {
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
            recorder: DurableRecorder::Workflow {
                worker,
                owner: owner.clone(),
                intent: Arc::new(intent),
            },
        })
    }
    /// Extend the base history with in-memory turns for every completed ancestor.
    fn ephemeral_context(
        &self,
        history: &[FinalizedTurn],
        ids: &[Id],
        call: &NodeCall,
    ) -> Result<NodeContext<EphemeralRecorder>, ErrorReport> {
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
            parent_sources(&call.parents, |parent| {
                Some(SourcePoint {
                    session: definition.session,
                    frontier: vec![ids[parent]],
                })
            })
            .ok_or_else(|| report(WorkflowError::Snapshot))?
        };
        Ok(NodeContext {
            sources,
            history: selected,
            recorder: EphemeralRecorder,
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
    parent_sources(&call.parents, |parent| {
        call.ancestors
            .iter()
            .find(|ancestor| ancestor.node == parent)
            .and_then(|ancestor| ancestor.output.source.clone())
    })
    .ok_or(WorkflowError::Snapshot)
}
fn ephemeral_node_output(
    (outcome, manifest): (
        crate::outcome::TurnOutcome,
        crate::history::EvaluationManifest,
    ),
    request: EvaluationRequest,
) -> NodeOutput {
    NodeOutput {
        outcome,
        source: None,
        evaluation: Some(NodeEvaluation {
            request: request.turn,
            updates: request.updates,
            manifest,
        }),
    }
}
