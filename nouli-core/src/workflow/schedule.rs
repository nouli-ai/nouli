//! Bounded owned tasks, typed conditions and ancestry from active dependencies.
use super::{
    NodeStatus, RunStatus, WorkflowNode,
    compile::{CompiledBinding, CompiledReference, CompiledWorkflow},
    reference::{MissingPolicy, ReferenceValue, ResultField, ResultReference},
    resolve::{self, ResolveError},
};
use crate::{
    data::Input,
    decision::TurnRequest,
    history::SourcePoint,
    outcome::TurnOutcome,
    protocol::{ErrorCategory, ErrorReport},
};
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, HashSet},
    future::Future,
    sync::Arc,
};
use thiserror::Error;
use tokio::task::{Id as TaskId, JoinError, JoinSet};
use tokio_util::sync::CancellationToken;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NodeEvaluation {
    pub request: TurnRequest,
    pub updates: Vec<crate::context::ContextUpdate>,
    pub manifest: crate::history::EvaluationManifest,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NodeOutput {
    pub outcome: TurnOutcome,
    pub source: Option<SourcePoint>,
    #[serde(default)]
    pub evaluation: Option<NodeEvaluation>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ResolvedBinding {
    pub name: String,
    pub reference: ResultReference,
    pub value: ReferenceValue,
    pub defaulted: bool,
    pub required_missing: bool,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NodeProgress {
    pub status: NodeStatus,
    pub output: Option<NodeOutput>,
    pub error: Option<ErrorReport>,
    pub ancestors: Vec<usize>,
    pub inputs: Vec<ResolvedBinding>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AncestorResult {
    pub node: usize,
    pub id: String,
    pub request: TurnRequest,
    pub output: NodeOutput,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NodeCall {
    pub node: usize,
    pub definition: WorkflowNode,
    pub base: Vec<Input>,
    pub inputs: Vec<ResolvedBinding>,
    pub parents: Vec<usize>,
    pub ancestors: Vec<AncestorResult>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ScheduleResult {
    pub status: RunStatus,
    pub nodes: Vec<NodeProgress>,
    pub start_order: Vec<usize>,
    pub completion_order: Vec<usize>,
    pub outputs: Vec<ResolvedBinding>,
}

/// Futures must be cancellation-safe on drop. The scheduler owns and joins every
/// task, including aborted tasks, before returning a result or error.
pub trait NodeExecutor: Send + Sync + 'static {
    fn execute(
        &self,
        call: NodeCall,
        cancel: CancellationToken,
    ) -> impl Future<Output = Result<NodeOutput, ErrorReport>> + Send;
}
pub trait ScheduleObserver: Send + Sync + 'static {
    fn started(
        &self,
        _run: Option<crate::identity::Id>,
    ) -> impl Future<Output = Result<(), ErrorReport>> + Send {
        async { Ok(()) }
    }
    fn changed(
        &self,
        node: usize,
        progress: NodeProgress,
    ) -> impl Future<Output = Result<(), ErrorReport>> + Send;
}
#[derive(Default)]
pub struct NoopObserver;
impl ScheduleObserver for NoopObserver {
    async fn changed(&self, _node: usize, _progress: NodeProgress) -> Result<(), ErrorReport> {
        Ok(())
    }
}

#[derive(Debug, Error)]
pub enum ScheduleError {
    #[error("parallelism must be positive")]
    Parallelism,
    #[error("workflow progress observer failed")]
    Observer(ErrorReport),
    #[error("scheduler task or graph state is inconsistent")]
    State,
    #[error(transparent)]
    Resolve(#[from] ResolveError),
}
fn error(category: ErrorCategory, code: &str) -> ErrorReport {
    ErrorReport {
        category,
        code: code.into(),
        message: code.replace('_', " "),
        retryable: false,
    }
}
fn source(
    selector: &CompiledReference,
    nodes: &[NodeProgress],
) -> Result<ReferenceValue, ResolveError> {
    let node = nodes.get(selector.node).ok_or(ResolveError::Incomplete)?;
    resolve::select(
        &selector.reference,
        node.status,
        node.output.as_ref().map(|output| &output.outcome),
    )
}
fn binding(
    name: String,
    reference: ResultReference,
    raw: ReferenceValue,
) -> Result<ResolvedBinding, ResolveError> {
    let defaulted = matches!(raw, ReferenceValue::Absent { .. })
        && matches!(reference.on_missing, MissingPolicy::Default { .. });
    let (value, required_missing) = match resolve::apply(&reference, raw.clone()) {
        Ok(value) => (value, false),
        Err(ResolveError::Required { .. }) => (raw, true),
        Err(error) => return Err(error),
    };
    Ok(ResolvedBinding {
        name,
        reference,
        value,
        defaulted,
        required_missing,
    })
}
pub(crate) fn compiled_binding(
    binding: &CompiledBinding,
    nodes: &[NodeProgress],
) -> Result<ResolvedBinding, ResolveError> {
    self::binding(
        binding.name.clone(),
        binding.selector.reference.clone(),
        source(&binding.selector, nodes)?,
    )
}

type TaskResult = Result<(TaskId, (usize, Result<NodeOutput, ErrorReport>)), JoinError>;

struct Scheduler<'a, E, O> {
    observer: Arc<O>,
    graph: &'a CompiledWorkflow,
    executor: Arc<E>,
    limit: usize,
    cancel: CancellationToken,
    nodes: Vec<NodeProgress>,
    tasks: JoinSet<(usize, Result<NodeOutput, ErrorReport>)>,
    owners: HashMap<TaskId, usize>,
    start_order: Vec<usize>,
    completion_order: Vec<usize>,
}
impl<E: NodeExecutor, O: ScheduleObserver> Scheduler<'_, E, O> {
    fn prepare(&mut self, id: usize) -> Result<Option<NodeCall>, ResolveError> {
        let plan = &self.graph.nodes()[id];
        let definition = &self.graph.definition().nodes[id];
        if let Some(condition) = &definition.when {
            let value = resolve::evaluate(condition, &|reference| {
                let selector = plan
                    .condition_references
                    .iter()
                    .find(|selector| selector.reference == *reference)
                    .ok_or(ResolveError::Incomplete)?;
                source(selector, &self.nodes)
            })?;
            if !value {
                self.nodes[id].status = NodeStatus::Skipped;
                return Ok(None);
            }
        }
        let inputs = plan
            .inputs
            .iter()
            .map(|binding| compiled_binding(binding, &self.nodes))
            .collect::<Result<Vec<_>, _>>()?;
        let required_missing = inputs.iter().any(|input| input.required_missing);
        self.nodes[id].inputs = inputs.clone();
        let failed_parent = plan.dependencies.iter().any(|parent| {
            matches!(
                self.nodes[*parent].status,
                NodeStatus::Failed | NodeStatus::Blocked | NodeStatus::Interrupted
            ) && !plan.inputs.iter().any(|input| {
                input.selector.node == *parent
                    && !matches!(input.selector.reference.on_missing, MissingPolicy::Required)
            }) && !plan.condition_references.iter().any(|selector| {
                selector.node == *parent
                    && (selector.handles_missing
                        || !matches!(selector.reference.on_missing, MissingPolicy::Required))
            })
        });
        if required_missing || failed_parent {
            self.nodes[id].status = NodeStatus::Blocked;
            self.nodes[id].error =
                Some(error(ErrorCategory::Contract, "required_input_unavailable"));
            return Ok(None);
        }
        if !plan.dependencies.is_empty()
            && plan
                .dependencies
                .iter()
                .all(|parent| self.nodes[*parent].status == NodeStatus::Skipped)
            && inputs.is_empty()
            && definition.when.is_none()
        {
            self.nodes[id].status = NodeStatus::Skipped;
            return Ok(None);
        }
        let parents: Vec<_> = plan
            .dependencies
            .iter()
            .filter(|parent| self.nodes[**parent].status == NodeStatus::Completed)
            .copied()
            .collect();
        let mut ancestors = HashSet::new();
        for parent in &parents {
            ancestors.insert(*parent);
            ancestors.extend(self.nodes[*parent].ancestors.iter().copied());
        }
        let ordered: Vec<_> = self
            .graph
            .order()
            .iter()
            .filter(|ancestor| ancestors.contains(ancestor))
            .copied()
            .collect();
        self.nodes[id].ancestors = ordered.clone();
        let ancestors = ordered
            .into_iter()
            .map(|node| {
                Ok(AncestorResult {
                    node,
                    id: self.graph.definition().nodes[node].id.clone(),
                    request: self.graph.definition().nodes[node].turn.clone(),
                    output: self.nodes[node]
                        .output
                        .clone()
                        .ok_or(ResolveError::Incomplete)?,
                })
            })
            .collect::<Result<Vec<_>, ResolveError>>()?;
        Ok(Some(NodeCall {
            node: id,
            definition: definition.clone(),
            base: self.graph.base_state().to_vec(),
            inputs,
            parents,
            ancestors,
        }))
    }
    fn complete(&mut self, result: TaskResult, canceling: bool) -> Result<usize, ScheduleError> {
        let (id, result) = match result {
            Ok((task, (id, result))) => {
                if self.owners.remove(&task) != Some(id) {
                    return Err(ScheduleError::State);
                }
                (id, result)
            }
            Err(error) => {
                let id = self
                    .owners
                    .remove(&error.id())
                    .ok_or(ScheduleError::State)?;
                if canceling && error.is_cancelled() {
                    self.nodes[id].status = NodeStatus::Interrupted;
                    self.nodes[id].error =
                        Some(self::error(ErrorCategory::Interrupted, "node_interrupted"));
                    return Ok(id);
                }
                (
                    id,
                    Err(self::error(ErrorCategory::Internal, "executor_failed")),
                )
            }
        };
        self.completion_order.push(id);
        match result {
            Ok(output) => {
                let valid_source = output.source.as_ref().is_none_or(|source| {
                    !source.frontier.is_empty()
                        && source.frontier.iter().collect::<HashSet<_>>().len()
                            == source.frontier.len()
                });
                if output
                    .outcome
                    .validate(&self.graph.definition().nodes[id].turn)
                    .is_err()
                    || !valid_source
                {
                    self.nodes[id].status = NodeStatus::Failed;
                    self.nodes[id].error =
                        Some(error(ErrorCategory::Contract, "invalid_node_outcome"));
                } else {
                    self.nodes[id].status = NodeStatus::Completed;
                    self.nodes[id].output = Some(output);
                }
            }
            Err(error) => {
                self.nodes[id].status = NodeStatus::Failed;
                self.nodes[id].error = Some(error);
            }
        }
        Ok(id)
    }
    async fn drive(&mut self) -> Result<bool, ScheduleError> {
        loop {
            if self.cancel.is_cancelled() {
                return Ok(true);
            }
            while self.tasks.len() < self.limit {
                if self.cancel.is_cancelled() {
                    return Ok(true);
                }
                // Re-select from declaration order after every synchronous
                // transition, including a skip that unlocks an earlier node.
                let ready = (0..self.nodes.len()).find(|id| {
                    self.nodes[*id].status == NodeStatus::Pending
                        && self.graph.nodes()[*id].dependencies.iter().all(|parent| {
                            !matches!(
                                self.nodes[*parent].status,
                                NodeStatus::Pending | NodeStatus::Running
                            )
                        })
                });
                let Some(id) = ready else {
                    break;
                };
                match self.prepare(id) {
                    Ok(Some(call)) => {
                        let executor = self.executor.clone();
                        let cancel = self.cancel.child_token();
                        self.nodes[id].status = NodeStatus::Running;
                        self.observer
                            .changed(id, self.nodes[id].clone())
                            .await
                            .map_err(ScheduleError::Observer)?;
                        self.start_order.push(id);
                        let task = self
                            .tasks
                            .spawn(async move { (id, executor.execute(call, cancel).await) });
                        self.owners.insert(task.id(), id);
                    }
                    Ok(None) => {
                        self.observer
                            .changed(id, self.nodes[id].clone())
                            .await
                            .map_err(ScheduleError::Observer)?;
                    }
                    Err(_) => {
                        self.nodes[id].status = NodeStatus::Blocked;
                        self.nodes[id].error = Some(error(
                            ErrorCategory::Contract,
                            "condition_input_unavailable",
                        ));
                        self.observer
                            .changed(id, self.nodes[id].clone())
                            .await
                            .map_err(ScheduleError::Observer)?;
                    }
                }
            }
            if self.tasks.is_empty() {
                if self
                    .nodes
                    .iter()
                    .all(|node| !matches!(node.status, NodeStatus::Pending | NodeStatus::Running))
                {
                    return Ok(false);
                }
                return Err(ScheduleError::State);
            }
            tokio::select! {biased;_=self.cancel.cancelled()=>return Ok(true),result=self.tasks.join_next_with_id()=>{let id=self.complete(result.ok_or(ScheduleError::State)?,false)?;self.observer.changed(id,self.nodes[id].clone()).await.map_err(ScheduleError::Observer)?;}}
        }
    }
}

pub(crate) fn project_outputs(
    graph: &CompiledWorkflow,
    nodes: &[NodeProgress],
) -> Result<Vec<ResolvedBinding>, ResolveError> {
    if !graph.outputs().is_empty() {
        return graph
            .outputs()
            .iter()
            .map(|binding| compiled_binding(binding, nodes))
            .collect();
    }
    let mut outputs = Vec::new();
    for node in graph.fallback_outputs() {
        for question in &graph.definition().nodes[*node].turn.questions {
            let reference = ResultReference {
                node: graph.definition().nodes[*node].id.clone(),
                question: question.id.clone(),
                field: ResultField::Outcome,
                pointer: None,
                on_missing: MissingPolicy::Optional,
            };
            let value = resolve::select(
                &reference,
                nodes[*node].status,
                nodes[*node].output.as_ref().map(|output| &output.outcome),
            )?;
            outputs.push(binding(
                format!("{}.{}", reference.node, reference.question),
                reference,
                value,
            )?);
        }
    }
    Ok(outputs)
}

pub async fn schedule<E: NodeExecutor>(
    graph: Arc<CompiledWorkflow>,
    executor: Arc<E>,
    parallelism: usize,
    cancel: CancellationToken,
) -> Result<ScheduleResult, ScheduleError> {
    schedule_observed(graph, executor, Arc::new(NoopObserver), parallelism, cancel).await
}

pub async fn schedule_observed<E: NodeExecutor, O: ScheduleObserver>(
    graph: Arc<CompiledWorkflow>,
    executor: Arc<E>,
    observer: Arc<O>,
    parallelism: usize,
    cancel: CancellationToken,
) -> Result<ScheduleResult, ScheduleError> {
    let nodes = graph
        .nodes()
        .iter()
        .map(|_| NodeProgress {
            status: NodeStatus::Pending,
            output: None,
            error: None,
            ancestors: vec![],
            inputs: vec![],
        })
        .collect();
    schedule_resumed(graph, executor, observer, parallelism, cancel, nodes).await
}

/// Execute a repository-validated snapshot, reusing only completed and skipped nodes.
pub(crate) async fn schedule_resumed<E: NodeExecutor, O: ScheduleObserver>(
    graph: Arc<CompiledWorkflow>,
    executor: Arc<E>,
    observer: Arc<O>,
    parallelism: usize,
    cancel: CancellationToken,
    nodes: Vec<NodeProgress>,
) -> Result<ScheduleResult, ScheduleError> {
    if parallelism == 0 {
        return Err(ScheduleError::Parallelism);
    }
    if nodes.len() != graph.nodes().len()
        || nodes.iter().any(|node| {
            !matches!(
                node.status,
                NodeStatus::Pending | NodeStatus::Completed | NodeStatus::Skipped
            ) || (node.status == NodeStatus::Completed) != node.output.is_some()
        })
    {
        return Err(ScheduleError::State);
    }
    for (index, node) in nodes.iter().enumerate() {
        if let Some(output) = &node.output {
            output
                .outcome
                .validate(&graph.definition().nodes[index].turn)
                .map_err(|_| ScheduleError::State)?;
        }
    }
    let mut scheduler = Scheduler {
        observer,
        graph: &graph,
        executor,
        limit: parallelism.min(graph.definition().max_parallel),
        cancel,
        nodes,
        tasks: JoinSet::new(),
        owners: HashMap::new(),
        start_order: vec![],
        completion_order: vec![],
    };
    let result = scheduler.drive().await;
    let canceling = !matches!(result, Ok(false));
    if canceling {
        scheduler.tasks.abort_all();
    }
    let mut cleanup_error = None;
    while let Some(result) = scheduler.tasks.join_next_with_id().await {
        match scheduler.complete(result, canceling) {
            Ok(id) => {
                if let Err(error) = scheduler
                    .observer
                    .changed(id, scheduler.nodes[id].clone())
                    .await
                {
                    cleanup_error = Some(ScheduleError::Observer(error));
                }
            }
            Err(error) => cleanup_error = Some(error),
        }
    }
    let canceled = result?;
    if let Some(error) = cleanup_error {
        return Err(error);
    }
    if canceled {
        for (index, node) in scheduler.nodes.iter_mut().enumerate() {
            if matches!(node.status, NodeStatus::Pending | NodeStatus::Running) {
                node.status = NodeStatus::Interrupted;
                node.error = Some(error(ErrorCategory::Interrupted, "node_interrupted"));
                scheduler
                    .observer
                    .changed(index, node.clone())
                    .await
                    .map_err(ScheduleError::Observer)?;
            }
        }
    }
    let outputs = project_outputs(scheduler.graph, &scheduler.nodes)?;
    let partial = scheduler
        .nodes
        .iter()
        .any(|node| matches!(node.status, NodeStatus::Failed | NodeStatus::Blocked))
        || outputs.iter().any(|output| output.required_missing);
    let status = if canceled {
        RunStatus::Interrupted
    } else if partial {
        if scheduler
            .nodes
            .iter()
            .any(|node| node.status == NodeStatus::Completed)
        {
            RunStatus::Partial
        } else {
            RunStatus::Failed
        }
    } else {
        RunStatus::Completed
    };
    Ok(ScheduleResult {
        status,
        nodes: scheduler.nodes,
        start_order: scheduler.start_order,
        completion_order: scheduler.completion_order,
        outputs,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        data::Data,
        workflow::{OutputBinding, WorkflowSpec, compile::compile, reference::Absence},
    };
    use std::sync::{
        Mutex,
        atomic::{AtomicUsize, Ordering},
    };
    fn fixture() -> WorkflowSpec {
        serde_json::from_str(include_str!("../../../fixtures/contracts/workflow.json")).unwrap()
    }
    fn outcome() -> TurnOutcome {
        serde_json::from_str(include_str!(
            "../../../fixtures/contracts/decision-outcome.json"
        ))
        .unwrap()
    }
    #[derive(Debug)]
    enum Event {
        Start(String),
        Finish(String),
    }
    struct Controlled {
        events: tokio::sync::mpsc::UnboundedSender<Event>,
        gates: HashMap<String, tokio::sync::Semaphore>,
        active: AtomicUsize,
        peak: AtomicUsize,
        calls: Mutex<Vec<NodeCall>>,
    }
    struct Active<'a>(&'a AtomicUsize);
    impl Drop for Active<'_> {
        fn drop(&mut self) {
            self.0.fetch_sub(1, Ordering::SeqCst);
        }
    }
    impl NodeExecutor for Controlled {
        async fn execute(
            &self,
            call: NodeCall,
            _cancel: CancellationToken,
        ) -> Result<NodeOutput, ErrorReport> {
            let active = self.active.fetch_add(1, Ordering::SeqCst) + 1;
            let _active = Active(&self.active);
            self.peak.fetch_max(active, Ordering::SeqCst);
            let id = call.definition.id.clone();
            self.calls.lock().unwrap().push(call);
            self.events.send(Event::Start(id.clone())).unwrap();
            self.gates[&id].acquire().await.unwrap().forget();
            self.events.send(Event::Finish(id)).unwrap();
            Ok(NodeOutput {
                outcome: outcome(),
                source: None,
                evaluation: None,
            })
        }
    }
    fn controlled(
        spec: &WorkflowSpec,
    ) -> (Arc<Controlled>, tokio::sync::mpsc::UnboundedReceiver<Event>) {
        let (events, rx) = tokio::sync::mpsc::unbounded_channel();
        (
            Arc::new(Controlled {
                events,
                gates: spec
                    .nodes
                    .iter()
                    .map(|node| (node.id.clone(), tokio::sync::Semaphore::new(0)))
                    .collect(),
                active: AtomicUsize::new(0),
                peak: AtomicUsize::new(0),
                calls: Mutex::new(vec![]),
            }),
            rx,
        )
    }
    async fn event(rx: &mut tokio::sync::mpsc::UnboundedReceiver<Event>, start: bool, id: &str) {
        match rx.recv().await.unwrap() {
            Event::Start(actual) if start => assert_eq!(actual, id),
            Event::Finish(actual) if !start => assert_eq!(actual, id),
            other => panic!("unexpected event {other:?}"),
        }
    }
    async fn parallel_run(first: &str) -> ScheduleResult {
        let mut spec = fixture();
        spec.nodes[1].when = None;
        spec.nodes[2].when = None;
        let (executor, mut events) = controlled(&spec);
        let running = executor.clone();
        let graph = Arc::new(compile(&spec).unwrap());
        let task = tokio::spawn(async move {
            schedule(graph, running, 2, CancellationToken::new())
                .await
                .unwrap()
        });
        event(&mut events, true, "initial").await;
        executor.gates["initial"].add_permits(1);
        event(&mut events, false, "initial").await;
        let mut siblings = HashSet::new();
        for _ in 0..2 {
            let Event::Start(id) = events.recv().await.unwrap() else {
                panic!("siblings must start together")
            };
            siblings.insert(id);
        }
        assert_eq!(siblings, HashSet::from(["high".into(), "low".into()]));
        assert_eq!(executor.active.load(Ordering::SeqCst), 2);
        let second = if first == "high" { "low" } else { "high" };
        executor.gates[first].add_permits(1);
        event(&mut events, false, first).await;
        executor.gates[second].add_permits(1);
        event(&mut events, false, second).await;
        event(&mut events, true, "join").await;
        executor.gates["join"].add_permits(1);
        event(&mut events, false, "join").await;
        event(&mut events, true, "final").await;
        executor.gates["final"].add_permits(1);
        event(&mut events, false, "final").await;
        let result = task.await.unwrap();
        assert_eq!(executor.active.load(Ordering::SeqCst), 0);
        assert_eq!(executor.peak.load(Ordering::SeqCst), 2);
        result
    }
    #[tokio::test]
    async fn ready_siblings_are_parallel_bounded_and_join_ancestry_ignores_completion_order() {
        let first = parallel_run("high").await;
        let second = parallel_run("low").await;
        assert_eq!(first.start_order, vec![0, 1, 2, 3, 4]);
        assert_eq!(first.start_order, second.start_order);
        assert_ne!(first.completion_order, second.completion_order);
        assert_eq!(
            first
                .nodes
                .iter()
                .map(|node| &node.ancestors)
                .collect::<Vec<_>>(),
            second
                .nodes
                .iter()
                .map(|node| &node.ancestors)
                .collect::<Vec<_>>()
        );
        assert_eq!(first.nodes[3].ancestors, vec![0, 1, 2]);
        assert_eq!(first.status, RunStatus::Completed);
    }
    #[tokio::test]
    async fn configured_single_slot_serializes_an_independent_ready_set() {
        let mut spec = fixture();
        spec.nodes[1].when = None;
        spec.nodes[2].when = None;
        let (executor, mut events) = controlled(&spec);
        let running = executor.clone();
        let graph = Arc::new(compile(&spec).unwrap());
        let task = tokio::spawn(async move {
            schedule(graph, running, 1, CancellationToken::new())
                .await
                .unwrap()
        });
        while let Some(event) = events.recv().await {
            match event {
                Event::Start(id) => executor.gates[&id].add_permits(1),
                Event::Finish(id) if id == "final" => break,
                _ => {}
            }
        }
        let result = task.await.unwrap();
        assert_eq!(result.start_order, vec![0, 1, 2, 3, 4]);
        assert_eq!(executor.peak.load(Ordering::SeqCst), 1);
        assert_eq!(executor.active.load(Ordering::SeqCst), 0);
    }

    struct Immediate {
        fail: HashSet<String>,
        calls: Mutex<Vec<NodeCall>>,
    }
    impl NodeExecutor for Immediate {
        async fn execute(
            &self,
            call: NodeCall,
            _cancel: CancellationToken,
        ) -> Result<NodeOutput, ErrorReport> {
            let failed = self.fail.contains(&call.definition.id);
            self.calls.lock().unwrap().push(call);
            if failed {
                Err(error(ErrorCategory::Transport, "fixture_failure"))
            } else {
                Ok(NodeOutput {
                    outcome: outcome(),
                    source: None,
                    evaluation: None,
                })
            }
        }
    }
    #[tokio::test]
    async fn fractional_score_routes_one_branch_and_active_join_keeps_usable_results() {
        let spec = fixture();
        let executor = Arc::new(Immediate {
            fail: HashSet::new(),
            calls: Mutex::new(vec![]),
        });
        let result = schedule(
            Arc::new(compile(&spec).unwrap()),
            executor.clone(),
            3,
            CancellationToken::new(),
        )
        .await
        .unwrap();
        assert_eq!(result.nodes[1].status, NodeStatus::Completed);
        assert_eq!(result.nodes[2].status, NodeStatus::Skipped);
        assert_eq!(result.nodes[3].status, NodeStatus::Completed);
        assert_eq!(result.nodes[3].ancestors, vec![0, 1]);
        assert_eq!(result.nodes[4].ancestors, vec![0, 1, 3]);
        assert_eq!(result.status, RunStatus::Completed);
        assert!(matches!(
            result.outputs[1].value,
            ReferenceValue::Absent {
                reason: Absence::Skipped
            }
        ));
        assert_eq!(executor.calls.lock().unwrap().len(), 4);
    }
    #[tokio::test]
    async fn failures_and_refused_fields_block_needed_consumers_but_not_independent_or_defaulted_work()
     {
        let mut spec = fixture();
        let mut independent = spec.nodes[0].clone();
        independent.id = "independent".into();
        spec.nodes.push(independent);
        let mut recovery = spec.nodes[0].clone();
        recovery.id = "recovery".into();
        recovery.dependencies = vec!["high".into()];
        recovery.inputs = vec![OutputBinding {
            name: "fallback_score".into(),
            reference: ResultReference {
                node: "high".into(),
                question: "quality".into(),
                field: ResultField::Score,
                pointer: None,
                on_missing: MissingPolicy::Default {
                    value: Data::new(serde_json::json!(0)).unwrap(),
                },
            },
        }];
        spec.nodes.push(recovery);
        let mut usable = spec.nodes[0].clone();
        usable.id = "usable".into();
        usable.dependencies = vec!["initial".into()];
        usable.inputs = vec![OutputBinding {
            name: "choice".into(),
            reference: ResultReference {
                node: "initial".into(),
                question: "travel".into(),
                field: ResultField::Selected,
                pointer: None,
                on_missing: MissingPolicy::Required,
            },
        }];
        spec.nodes.push(usable.clone());
        usable.id = "needs_refused".into();
        usable.inputs[0].reference.question = "private".into();
        usable.inputs[0].reference.field = ResultField::Probability;
        spec.nodes.push(usable);
        let executor = Arc::new(Immediate {
            fail: HashSet::from(["high".into()]),
            calls: Mutex::new(vec![]),
        });
        let result = schedule(
            Arc::new(compile(&spec).unwrap()),
            executor,
            3,
            CancellationToken::new(),
        )
        .await
        .unwrap();
        assert_eq!(result.status, RunStatus::Partial);
        assert_eq!(result.nodes[3].status, NodeStatus::Blocked);
        assert_eq!(result.nodes[5].status, NodeStatus::Completed);
        assert_eq!(result.nodes[6].status, NodeStatus::Completed);
        assert!(result.nodes[6].inputs[0].defaulted);
        assert_eq!(result.nodes[7].status, NodeStatus::Completed);
        assert_eq!(result.nodes[8].status, NodeStatus::Blocked);
        assert!(matches!(
            result.nodes[8].inputs[0].value,
            ReferenceValue::Absent {
                reason: Absence::Refused
            }
        ));
    }
    #[tokio::test]
    async fn reverse_declarations_reach_fixed_point_and_restart_ready_ties_after_skips() {
        let original = fixture();
        let mut parent = original.nodes[0].clone();
        parent.id = "parent".into();
        parent.when = Some(super::super::reference::Condition::Compare {
            left: super::super::reference::Operand::Literal {
                value: Data::new(serde_json::json!(1)).unwrap(),
            },
            comparison: super::super::reference::Comparison::Eq,
            right: super::super::reference::Operand::Literal {
                value: Data::new(serde_json::json!(0)).unwrap(),
            },
        });
        let mut child = original.nodes[0].clone();
        child.id = "child".into();
        child.dependencies = vec!["parent".into()];
        let mut spec = original.clone();
        spec.nodes = vec![child.clone(), parent.clone()];
        spec.outputs.clear();
        let executor = Arc::new(Immediate {
            fail: HashSet::new(),
            calls: Mutex::new(vec![]),
        });
        let skipped = schedule(
            Arc::new(compile(&spec).unwrap()),
            executor.clone(),
            2,
            CancellationToken::new(),
        )
        .await
        .unwrap();
        assert!(
            skipped
                .nodes
                .iter()
                .all(|node| node.status == NodeStatus::Skipped)
        );
        assert!(skipped.start_order.is_empty());
        child.inputs = vec![OutputBinding {
            name: "optional_parent".into(),
            reference: ResultReference {
                node: "parent".into(),
                question: "quality".into(),
                field: ResultField::Score,
                pointer: None,
                on_missing: MissingPolicy::Optional,
            },
        }];
        let mut independent = original.nodes[0].clone();
        independent.id = "independent".into();
        spec.nodes = vec![child, parent, independent];
        let result = schedule(
            Arc::new(compile(&spec).unwrap()),
            executor,
            2,
            CancellationToken::new(),
        )
        .await
        .unwrap();
        assert_eq!(result.start_order, vec![0, 2]);
        assert_eq!(result.nodes[0].status, NodeStatus::Completed);
    }
    #[tokio::test]
    async fn reverse_declared_blocked_chains_propagate_without_a_running_task() {
        let mut spec = fixture();
        let mut parent = spec.nodes[0].clone();
        parent.id = "parent".into();
        let mut child = parent.clone();
        child.id = "child".into();
        child.dependencies = vec!["parent".into()];
        let mut grandchild = child.clone();
        grandchild.id = "grandchild".into();
        grandchild.dependencies = vec!["child".into()];
        spec.nodes = vec![grandchild, child, parent];
        spec.outputs.clear();
        let result = schedule(
            Arc::new(compile(&spec).unwrap()),
            Arc::new(Immediate {
                fail: HashSet::from(["parent".into()]),
                calls: Mutex::new(vec![]),
            }),
            2,
            CancellationToken::new(),
        )
        .await
        .unwrap();
        assert_eq!(result.nodes[2].status, NodeStatus::Failed);
        assert_eq!(result.nodes[1].status, NodeStatus::Blocked);
        assert_eq!(result.nodes[0].status, NodeStatus::Blocked);
        assert_eq!(result.start_order, vec![2]);
    }

    #[tokio::test]
    async fn cancellation_aborts_and_joins_all_owned_tasks() {
        let spec = fixture();
        let (executor, mut events) = controlled(&spec);
        let running = executor.clone();
        let cancel = CancellationToken::new();
        let token = cancel.clone();
        let graph = Arc::new(compile(&spec).unwrap());
        let task = tokio::spawn(async move { schedule(graph, running, 2, token).await.unwrap() });
        event(&mut events, true, "initial").await;
        cancel.cancel();
        let result = task.await.unwrap();
        assert_eq!(result.status, RunStatus::Interrupted);
        assert!(
            result
                .nodes
                .iter()
                .all(|node| node.status == NodeStatus::Interrupted)
        );
        assert_eq!(executor.active.load(Ordering::SeqCst), 0);
    }
}
