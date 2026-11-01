//! Frozen workflow ownership and short durable repository operations.
mod completion;
pub mod events;
pub mod nodes;
pub mod recovery;
use super::{
    Ledger, StorageError,
    guards::{self, FileGuard, GuardError, RunGuard, SessionGuard},
    sessions::{self, Session, SessionError},
    sources::{self, SourceError},
    sql_error,
    turns::TurnError,
};
use crate::{
    codec,
    config::resolve::SettingsSnapshot,
    data::Data,
    history::{HistoryError, ModelSelection, SessionSelection, base::ContextBase},
    identity::{Id, SessionSelector},
    workflow::{
        NodeStatus, RunStatus, WorkflowRequest,
        compile::{self, CompileError},
        schedule::{NodeProgress, ScheduleResult},
    },
};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use std::{collections::HashSet, fs};
use thiserror::Error;
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WorkflowDefinition {
    pub run: Id,
    pub session: Id,
    pub request: WorkflowRequest,
    pub base: ContextBase,
    pub defaults: ModelSelection,
    pub initial_frontier: Vec<Id>,
    pub initial_revision: i64,
    pub plan: Data,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FrozenWorkflow {
    pub definition: WorkflowDefinition,
    pub settings: Vec<SettingsSnapshot>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StoredRun {
    pub frozen: FrozenWorkflow,
    pub status: RunStatus,
    pub frontier: Vec<Id>,
    pub published_revision: i64,
    pub nodes: Vec<NodeProgress>,
    pub result: Option<ScheduleResult>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WorkflowEvent {
    #[serde(default = "event_version")]
    pub version: u32,
    #[serde(default)]
    pub round: u32,
    pub kind: WorkflowEventKind,
    pub progress: Option<NodeProgress>,
    pub node: Option<usize>,
    pub node_status: Option<NodeStatus>,
    pub run_status: Option<RunStatus>,
    pub attempt: Option<Id>,
    pub turn: Option<Id>,
}
fn event_version() -> u32 {
    crate::decision::CONTRACT_VERSION
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowEventKind {
    RunStarted,
    NodeState,
    NodeIntent,
    AttemptStarted,
    AttemptFinished,
    RunFinished,
    Recovered,
    RoundStarted,
}
pub struct WorkflowOwner {
    pub(super) frozen: FrozenWorkflow,
    pub(super) round: u32,
    pub(super) session_guard: SessionGuard,
    pub(super) _run_guard: RunGuard,
    _request_guard: FileGuard,
}
impl WorkflowOwner {
    #[must_use]
    pub fn frozen(&self) -> &FrozenWorkflow {
        &self.frozen
    }
}
pub enum WorkflowDecision {
    Accepted(Box<WorkflowOwner>),
    Saved(Box<StoredRun>),
    InProgress,
    Existing { run: Id, status: RunStatus },
}
#[derive(Debug, Error)]
pub enum WorkflowError {
    #[error(transparent)]
    Storage(#[from] StorageError),
    #[error(transparent)]
    Session(#[from] SessionError),
    #[error(transparent)]
    Source(#[from] SourceError),
    #[error(transparent)]
    Turn(#[from] TurnError),
    #[error(transparent)]
    History(#[from] HistoryError),
    #[error(transparent)]
    Guard(#[from] GuardError),
    #[error(transparent)]
    Compile(#[from] CompileError),
    #[error("request ID identifies different intent or request kind")]
    IntentConflict,
    #[error("workflow does not exist")]
    UnknownRun,
    #[error("session head advanced beyond the saved workflow frontier")]
    AdvancedHead,
    #[error("workflow ownership or node state changed")]
    Conflict,
    #[error("workflow snapshot is inconsistent")]
    Snapshot,
}
fn validate(request: &WorkflowRequest) -> Result<(), WorkflowError> {
    compile::compile(&request.workflow)?;
    request.session.validate()?;
    if request
        .request_id
        .as_ref()
        .is_some_and(|id| id.trim().is_empty())
        || request
            .required_records
            .iter()
            .collect::<HashSet<_>>()
            .len()
            != request.required_records.len()
    {
        return Err(HistoryError::InvalidSelection.into());
    }
    for update in &request.updates {
        update.validate().map_err(HistoryError::from)?;
    }
    Ok(())
}
/// Validates a workflow request and returns the fingerprint of its intent.
///
/// # Errors
///
/// - Returns [`WorkflowError::Compile`] if the workflow does not compile.
/// - Returns [`WorkflowError::History`] if the session selection, request ID,
///   required records, or context updates are invalid.
/// - Returns [`WorkflowError::Snapshot`] if the intent cannot be fingerprinted.
pub fn fingerprint(request: &WorkflowRequest) -> Result<String, WorkflowError> {
    #[derive(Serialize)]
    struct Intent<'a> {
        workflow: &'a crate::workflow::WorkflowSpec,
        session: &'a SessionSelection,
        updates: &'a [crate::context::ContextUpdate],
        required: &'a [Id],
    }
    validate(request)?;
    codec::fingerprint(&Intent {
        workflow: &request.workflow,
        session: &request.session,
        updates: &request.updates,
        required: &request.required_records,
    })
    .map_err(|_| WorkflowError::Snapshot)
}
pub(super) fn run_name(status: RunStatus) -> &'static str {
    match status {
        RunStatus::Pending => "pending",
        RunStatus::Running => "running",
        RunStatus::Completed => "completed",
        RunStatus::Partial => "partial",
        RunStatus::Failed => "failed",
        RunStatus::Interrupted => "interrupted",
    }
}
pub(super) fn node_name(status: NodeStatus) -> &'static str {
    match status {
        NodeStatus::Pending => "pending",
        NodeStatus::Running => "running",
        NodeStatus::Completed => "completed",
        NodeStatus::Skipped => "skipped",
        NodeStatus::Failed => "failed",
        NodeStatus::Blocked => "blocked",
        NodeStatus::Interrupted => "interrupted",
    }
}
pub(super) fn event(
    tx: &rusqlite::Transaction<'_>,
    run: Id,
    event: &WorkflowEvent,
) -> Result<(), StorageError> {
    tx.execute("INSERT INTO workflow_events(run_id,sequence,payload_toon) SELECT ?1,COALESCE(MAX(sequence),-1)+1,?2 FROM workflow_events WHERE run_id=?1",params![run.to_string(),Ledger::encode_payload(event)?]).map_err(|error|sql_error("append workflow event",&error))?;
    Ok(())
}
pub(super) fn round_order(
    ledger: &Ledger,
    run: Id,
    round: u32,
) -> Result<(Vec<usize>, Vec<usize>), WorkflowError> {
    let mut statement = ledger
        .connection
        .prepare("SELECT payload_toon FROM workflow_events WHERE run_id=?1 ORDER BY sequence")
        .map_err(|error| sql_error("read workflow event order", &error))?;
    let payloads = statement
        .query_map([run.to_string()], |row| row.get::<_, String>(0))
        .map_err(|error| sql_error("read workflow events", &error))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| sql_error("decode workflow events", &error))?;
    drop(statement);
    let mut start_order = Vec::new();
    let mut completion_order = Vec::new();
    for payload in payloads {
        let event: WorkflowEvent = Ledger::decode_payload(&payload)?;
        if event.round != round {
            continue;
        }
        if let Some(node) = event.node {
            if event.kind == WorkflowEventKind::NodeState
                && event.node_status == Some(NodeStatus::Running)
            {
                start_order.push(node);
            }
            if event.turn.is_some()
                || (event.kind == WorkflowEventKind::NodeState
                    && event.node_status == Some(NodeStatus::Failed))
            {
                completion_order.push(node);
            }
        }
    }
    Ok((start_order, completion_order))
}
pub(super) fn child_key(run: Id, node: usize) -> String {
    format!("{run}/node/{node}")
}
pub(super) fn validate_owner(ledger: &Ledger, owner: &WorkflowOwner) -> Result<(), WorkflowError> {
    let definition = &owner.frozen.definition;
    owner.session_guard.validate(ledger, definition.session)?;
    let row: Option<(String, String)> = ledger
        .connection
        .query_row(
            "SELECT owner_id,state FROM workflow_runs WHERE id=?1 AND session_id=?2",
            params![definition.run.to_string(), definition.session.to_string()],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(|error| sql_error("verify run owner", &error))?;
    if row != Some((owner.session_guard.owner.to_string(), "running".into())) {
        return Err(WorkflowError::Conflict);
    }
    Ok(())
}
fn request_run(
    connection: &Connection,
    key: &str,
    hash: &str,
) -> Result<Option<Id>, WorkflowError> {
    let row: Option<(String, String, String)> = connection
        .query_row(
            "SELECT kind,fingerprint,frozen_toon FROM request_reservations WHERE request_id=?1",
            [key],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()
        .map_err(|error| sql_error("inspect workflow request", &error))?;
    row.map(|(kind, saved, run)| {
        if kind != "workflow" || saved != hash {
            return Err(WorkflowError::IntentConflict);
        }
        Ledger::decode_payload(&run).map_err(WorkflowError::from)
    })
    .transpose()
}
pub(super) fn frozen(connection: &Connection, run: Id) -> Result<FrozenWorkflow, WorkflowError> {
    let row: Option<(String, String)> = connection
        .query_row(
            "SELECT definition_toon,settings_toon FROM workflow_runs WHERE id=?1",
            [run.to_string()],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(|error| sql_error("read frozen workflow", &error))?;
    let (definition, settings) = row.ok_or(WorkflowError::UnknownRun)?;
    let definition: WorkflowDefinition = Ledger::decode_payload(&definition)?;
    let settings: Vec<SettingsSnapshot> = Ledger::decode_payload(&settings)?;
    let plan = compile::compile(&definition.request.workflow)?;
    if definition.run != run
        || definition.base.id != run
        || definition.base.session != definition.session
        || definition.base.runtime_only
        || definition.base.state != definition.request.workflow.state
        || definition.base.updates != definition.request.updates
        || settings.len() != plan.nodes().len()
        || codec::fingerprint(&plan).map_err(|_| WorkflowError::Snapshot)?
            != codec::fingerprint(&definition.plan).map_err(|_| WorkflowError::Snapshot)?
    {
        return Err(WorkflowError::Snapshot);
    }
    for setting in &settings {
        setting
            .profile
            .validate()
            .map_err(|_| WorkflowError::Snapshot)?;
        setting
            .execution
            .validate()
            .map_err(|_| WorkflowError::Snapshot)?;
    }
    Ok(FrozenWorkflow {
        definition,
        settings,
    })
}
pub(super) fn read_run(
    connection: &Connection,
    run: Id,
    before_nodes: impl FnOnce(),
) -> Result<StoredRun, WorkflowError> {
    let frozen = frozen(connection, run)?;
    let row:(String,String,i64,Option<String>)=connection.query_row("SELECT state,frontier_toon,published_revision,result_toon FROM workflow_runs WHERE id=?1",[run.to_string()],|row|Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?))).map_err(|error|sql_error("read workflow progress",&error))?;
    let status: RunStatus = serde_json::from_value(serde_json::Value::String(row.0))
        .map_err(|_| WorkflowError::Snapshot)?;
    before_nodes();
    let mut statement = connection
        .prepare(
            "SELECT payload_toon FROM workflow_nodes WHERE run_id=?1 ORDER BY declaration_order",
        )
        .map_err(|error| sql_error("list workflow nodes", &error))?;
    let payloads = statement
        .query_map([run.to_string()], |row| row.get::<_, String>(0))
        .map_err(|error| sql_error("read node snapshots", &error))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| sql_error("read node rows", &error))?;
    let nodes = payloads
        .into_iter()
        .map(|payload| Ledger::decode_payload(&payload))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(StoredRun {
        frozen,
        status,
        frontier: Ledger::decode_payload(&row.1)?,
        published_revision: row.2,
        nodes,
        result: row
            .3
            .map(|payload| Ledger::decode_payload(&payload))
            .transpose()?,
    })
}

impl Ledger {
    /// Returns the frozen workflow previously saved for the request's ID, if any.
    ///
    /// # Errors
    ///
    /// - Returns any [`fingerprint`] validation error for the request.
    /// - Returns [`WorkflowError::IntentConflict`] if the request ID was saved
    ///   for a different intent or request kind.
    /// - Returns [`WorkflowError::UnknownRun`] or [`WorkflowError::Snapshot`] if
    ///   the saved run is missing or inconsistent, and [`WorkflowError::Storage`]
    ///   if the ledger cannot be read or decoded.
    pub fn saved_workflow(
        &self,
        request: &WorkflowRequest,
    ) -> Result<Option<FrozenWorkflow>, WorkflowError> {
        let hash = fingerprint(request)?;
        let Some(key) = request.request_id.as_deref() else {
            return Ok(None);
        };
        request_run(&self.connection, key, &hash)?
            .map(|run| frozen(&self.connection, run))
            .transpose()
    }
    /// Reads a consistent snapshot of a stored workflow run.
    ///
    /// # Errors
    ///
    /// - Returns [`WorkflowError::UnknownRun`] if the run does not exist.
    /// - Returns [`WorkflowError::Snapshot`] or [`WorkflowError::Compile`] if the
    ///   frozen definition, settings, or status are inconsistent.
    /// - Returns [`WorkflowError::Storage`] if the ledger cannot be read or a
    ///   stored payload cannot be decoded.
    pub fn workflow_run(&self, run: Id) -> Result<StoredRun, WorkflowError> {
        self.read_workflow_with(run, || {})
    }
    fn read_workflow_with(
        &self,
        run: Id,
        before_nodes: impl FnOnce(),
    ) -> Result<StoredRun, WorkflowError> {
        let tx = self
            .connection
            .unchecked_transaction()
            .map_err(|error| sql_error("begin workflow snapshot", &error))?;
        let stored = read_run(&tx, run, before_nodes)?;
        tx.commit()
            .map_err(|error| sql_error("finish workflow snapshot", &error))?;
        Ok(stored)
    }
    /// Freezes a durable workflow request and takes ownership of its new run,
    /// or reports a saved, in-progress, or previously exited run for the same
    /// request ID.
    ///
    /// # Errors
    ///
    /// - Returns [`WorkflowError::Snapshot`] if the workflow is not durable, the
    ///   settings do not match the compiled nodes, or a setting is invalid.
    /// - Returns [`WorkflowError::History`] if the request has no request ID or
    ///   fails [`fingerprint`] validation, and [`WorkflowError::IntentConflict`]
    ///   if the request ID was saved for a different intent.
    /// - Returns [`WorkflowError::Guard`] with [`GuardError::WriterActive`] if
    ///   another writer holds the session, or another [`WorkflowError::Guard`]
    ///   if a guard cannot be acquired. Another process running the same
    ///   request yields [`WorkflowDecision::InProgress`] instead.
    /// - Returns [`WorkflowError::Session`] or [`WorkflowError::Source`] if the
    ///   selected session, defaults, or sources cannot be resolved.
    /// - Returns [`WorkflowError::Storage`] if the ledger is read-only, busy, or
    ///   cannot be read or written.
    pub fn accept_workflow(
        &mut self,
        request: &WorkflowRequest,
        mut settings: Vec<SettingsSnapshot>,
        defaults: ModelSelection,
    ) -> Result<WorkflowDecision, WorkflowError> {
        if request.workflow.mode != crate::workflow::RunMode::Durable {
            return Err(WorkflowError::Snapshot);
        }
        let hash = fingerprint(request)?;
        let key = request
            .request_id
            .as_deref()
            .ok_or(HistoryError::InvalidSelection)?;
        if let Some(run) = request_run(&self.connection, key, &hash)? {
            let saved = self.workflow_run(run)?;
            if saved.status == RunStatus::Completed {
                return Ok(WorkflowDecision::Saved(Box::new(saved)));
            }
        }
        let lock_key = format!(
            "request-{}",
            codec::fingerprint(&key).map_err(|_| WorkflowError::Snapshot)?
        );
        let request_guard = match guards::lock(self, &lock_key) {
            Ok(guard) => guard,
            Err(GuardError::WriterActive) => return Ok(WorkflowDecision::InProgress),
            Err(error) => return Err(error.into()),
        };
        if let Some(run) = request_run(&self.connection, key, &hash)? {
            let saved = self.workflow_run(run)?;
            if saved.status == RunStatus::Completed {
                return Ok(WorkflowDecision::Saved(Box::new(saved)));
            }
            self.transaction::<_,StorageError>("record exited workflow owner",|tx| {
                tx.execute("UPDATE workflow_runs SET state='interrupted' WHERE id=?1 AND state IN ('pending','running')",[run.to_string()]).map_err(|error|sql_error("interrupt exited workflow",&error))?;
                tx.execute("UPDATE request_reservations SET state='interrupted' WHERE request_id=?1 AND state='pending'",[key]).map_err(|error|sql_error("interrupt workflow reservation",&error))?;Ok(())
            })?;
            return Ok(WorkflowDecision::Existing {
                run,
                status: if matches!(saved.status, RunStatus::Running | RunStatus::Pending) {
                    RunStatus::Interrupted
                } else {
                    saved.status
                },
            });
        }
        let plan = compile::compile(&request.workflow)?;
        if settings.len() != plan.nodes().len() {
            return Err(WorkflowError::Snapshot);
        }
        sessions::validate_defaults(&defaults)?;
        let store = fs::canonicalize(self.path().parent().ok_or(StorageError::Payload)?).map_err(
            |source| StorageError::Io {
                operation: "freeze workflow store",
                path: self.path().to_owned(),
                source,
            },
        )?;
        for setting in &mut settings {
            setting
                .profile
                .validate()
                .map_err(|_| WorkflowError::Snapshot)?;
            setting
                .execution
                .validate()
                .map_err(|_| WorkflowError::Snapshot)?;
            setting.store.clone_from(&store);
        }
        let (session, name, new) = match &request.session {
            SessionSelection::Continue { session } => (self.session(session)?.id, None, false),
            SessionSelection::New { name } | SessionSelection::From { name, .. } => {
                (Id::generate(), name.clone(), true)
            }
        };
        let session_guard = guards::acquire_session(self, session)?;
        let run = Id::generate();
        let run_guard = self.run_guard(run)?;
        let definition=self.transaction::<_,WorkflowError>("accept frozen workflow",|tx| {
            let mut current=if new {Session {id:session,name,defaults:defaults.clone(),frontier:vec![],revision:0}} else {sessions::read_session(tx,&SessionSelector::Id(session))?};
            let sources=sources::resolve_selection(tx,&request.session,&current)?;
            if new {sessions::insert_session(tx,&mut current,&sources)?;}
            let base=ContextBase {id:run,session,sources,state:request.workflow.state.clone(),updates:request.updates.clone(),runtime_only:false};
            let definition=WorkflowDefinition {run,session,request:request.clone(),base,defaults,initial_frontier:current.frontier.clone(),initial_revision:current.revision,plan:Data::new(codec::to_value(&plan).map_err(|_|WorkflowError::Snapshot)?).map_err(|_|WorkflowError::Snapshot)?};
            tx.execute("INSERT INTO request_reservations(session_id,request_id,fingerprint,owner_id,state,frozen_toon,kind) VALUES(?1,?2,?3,?4,'pending',?5,'workflow')",params![session.to_string(),key,hash,session_guard.owner.to_string(),Self::encode_payload(&run)?]).map_err(|error|sql_error("reserve workflow request",&error))?;
            tx.execute("INSERT INTO workflow_runs(id,session_id,definition_toon,settings_toon,state,frontier_toon,owner_id,published_revision) VALUES(?1,?2,?3,?4,'running',?5,?6,?7)",params![run.to_string(),session.to_string(),Self::encode_payload(&definition)?,Self::encode_payload(&settings)?,Self::encode_payload(&current.frontier)?,session_guard.owner.to_string(),current.revision]).map_err(|error|sql_error("save frozen workflow",&error))?;
            tx.execute("INSERT INTO workflow_rounds(run_id,round,owner_id) VALUES(?1,0,?2)",params![run.to_string(),session_guard.owner.to_string()]).map_err(|error|sql_error("record initial workflow round",&error))?;
            for ((index,node),order) in request.workflow.nodes.iter().enumerate().zip(0_i64..) {
                let pending=NodeProgress {status:NodeStatus::Pending,output:None,error:None,ancestors:vec![],inputs:vec![]};
                tx.execute("INSERT INTO workflow_nodes(run_id,node_id,declaration_order,state,payload_toon) VALUES(?1,?2,?3,'pending',?4)",params![run.to_string(),node.id,order,Self::encode_payload(&pending)?]).map_err(|error|sql_error("save pending node",&error))?;
                tx.execute("INSERT INTO request_reservations(session_id,request_id,fingerprint,owner_id,state,frozen_toon,kind) VALUES(?1,?2,?3,?4,'pending',?5,'workflow_node')",params![session.to_string(),child_key(run,index),codec::fingerprint(&(run,index)).map_err(|_|WorkflowError::Snapshot)?,session_guard.owner.to_string(),Self::encode_payload(&(run,index))?]).map_err(|error|sql_error("reserve stable node identity",&error))?;
            }
            event(tx,run,&WorkflowEvent {version:crate::decision::CONTRACT_VERSION,
                    round:0,kind:WorkflowEventKind::RunStarted,progress:None,node:None,node_status:None,run_status:Some(RunStatus::Running),attempt:None,turn:None})?;Ok(definition)
        })?;
        Ok(WorkflowDecision::Accepted(Box::new(WorkflowOwner {
            round: 0,
            frozen: FrozenWorkflow {
                definition,
                settings,
            },
            session_guard,
            _run_guard: run_guard,
            _request_guard: request_guard,
        })))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        config::{Config, OutputFormat},
        identity::SessionSelector,
        storage::OpenMode,
        workflow::{
            reference::{Comparison, Condition, Operand},
            schedule::project_outputs,
        },
    };
    use std::path::PathBuf;
    struct Store(PathBuf);
    impl Store {
        fn new() -> Self {
            Self(std::env::temp_dir().join(format!("nouli-run-repo-{}", Id::generate())))
        }
        fn open(&self) -> Ledger {
            Ledger::open(&self.0, OpenMode::ReadWriteCreate).unwrap()
        }
    }
    impl Drop for Store {
        fn drop(&mut self) {
            if self.0.exists() {
                fs::remove_dir_all(&self.0).unwrap();
            }
        }
    }
    fn request() -> WorkflowRequest {
        let mut workflow: crate::workflow::WorkflowSpec =
            serde_json::from_str(include_str!("../../../fixtures/contracts/workflow.json"))
                .unwrap();
        workflow.outputs.clear();
        for node in &mut workflow.nodes {
            node.when = Some(Condition::Compare {
                left: Operand::Literal {
                    value: Data::new(serde_json::json!(1)).unwrap(),
                },
                comparison: Comparison::Eq,
                right: Operand::Literal {
                    value: Data::new(serde_json::json!(0)).unwrap(),
                },
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
    fn settings(request: &WorkflowRequest) -> Vec<SettingsSnapshot> {
        let config =
            Config::parse(include_str!("../../../fixtures/config/providers.toml")).unwrap();
        let mut profile = config
            .resolve_snapshot(
                &crate::config::resolve::RuntimeOverrides {
                    selection: ModelSelection {
                        provider: Some("local".into()),
                        model: Some("offline".into()),
                    },
                    context_budget_tokens: Some(4096),
                    ..Default::default()
                },
                &ModelSelection::default(),
                &ModelSelection::default(),
                std::path::Path::new("/unused"),
            )
            .unwrap()
            .profile;
        profile.context_budget_tokens = Some(4096);
        request
            .workflow
            .nodes
            .iter()
            .map(|_| SettingsSnapshot {
                format_version: 2,
                profile_name: "fixture".into(),
                profile: profile.clone(),
                execution: config.execution.clone(),
                output: OutputFormat::Json,
                store: "/unused".into(),
            })
            .collect()
    }
    fn owner(ledger: &mut Ledger, request: &WorkflowRequest) -> Box<WorkflowOwner> {
        let WorkflowDecision::Accepted(owner) = ledger
            .accept_workflow(request, settings(request), ModelSelection::default())
            .unwrap()
        else {
            panic!("expected owner")
        };
        owner
    }
    fn skipped_result(request: &WorkflowRequest) -> ScheduleResult {
        let nodes = request
            .workflow
            .nodes
            .iter()
            .map(|_| NodeProgress {
                status: NodeStatus::Skipped,
                output: None,
                error: None,
                ancestors: vec![],
                inputs: vec![],
            })
            .collect::<Vec<_>>();
        let graph = compile::compile(&request.workflow).unwrap();
        let outputs = project_outputs(&graph, &nodes).unwrap();
        ScheduleResult {
            status: RunStatus::Completed,
            nodes,
            start_order: vec![],
            completion_order: vec![],
            outputs,
        }
    }
    #[test]
    fn terminal_publication_rejects_unsettled_nodes_live_attempts_and_misclassified_outputs() {
        let store = Store::new();
        let mut ledger = store.open();
        let request = request();
        let owner = owner(&mut ledger, &request);
        let run = owner.frozen().definition.run;
        let before = ledger.workflow_run(run).unwrap();
        let early = ScheduleResult {
            status: RunStatus::Completed,
            nodes: before.nodes.clone(),
            start_order: vec![],
            completion_order: vec![],
            outputs: vec![],
        };
        assert!(ledger.finish_workflow(&owner, &early).is_err());
        assert_eq!(ledger.workflow_run(run).unwrap(), before);
        let result = skipped_result(&request);
        for (node, progress) in result.nodes.iter().enumerate() {
            ledger
                .record_workflow_progress(&owner, node, progress)
                .unwrap();
        }
        let mut wrong = result.clone();
        wrong.outputs.clear();
        assert!(ledger.finish_workflow(&owner, &wrong).is_err());
        let attempt = Id::generate();
        let session = owner.frozen().definition.session;
        ledger.connection.execute("INSERT INTO attempts(id,session_id,run_id,node_id,payload_toon) VALUES(?1,?2,?3,'initial','audit fault')",params![attempt.to_string(),session.to_string(),run.to_string()]).unwrap();
        ledger
            .connection
            .execute(
                "INSERT INTO attempt_states(attempt_id,state) VALUES(?1,'running')",
                [attempt.to_string()],
            )
            .unwrap();
        assert!(ledger.finish_workflow(&owner, &result).is_err());
        ledger
            .connection
            .execute(
                "UPDATE attempt_states SET state='interrupted' WHERE attempt_id=?1",
                [attempt.to_string()],
            )
            .unwrap();
        let completed = ledger.finish_workflow(&owner, &result).unwrap();
        assert_eq!(completed.status, RunStatus::Completed);
        assert!(ledger.finish_workflow(&owner, &result).is_err());
        let count: i64 = ledger
            .connection
            .query_row(
                "SELECT COUNT(*) FROM workflow_events WHERE run_id=?1",
                [run.to_string()],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 7);
        assert_eq!(ledger.workflow_run(run).unwrap(), completed);
    }
    #[test]
    fn aggregate_read_keeps_header_nodes_and_result_in_one_snapshot() {
        let store = Store::new();
        let mut ledger = store.open();
        let request = request();
        let owner = owner(&mut ledger, &request);
        let run = owner.frozen().definition.run;
        let result = skipped_result(&request);
        let readonly = Ledger::open(&store.0, OpenMode::ReadOnlyExisting).unwrap();
        let mut writer = store.open();
        writer
            .connection
            .busy_timeout(std::time::Duration::ZERO)
            .unwrap();
        let publish = |writer: &mut Ledger| -> Result<(), rusqlite::Error> {
            let tx = writer
                .connection
                .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
            for (index, node) in request.workflow.nodes.iter().enumerate() {
                tx.execute("UPDATE workflow_nodes SET state='skipped',payload_toon=?3 WHERE run_id=?1 AND node_id=?2",params![run.to_string(),node.id,Ledger::encode_payload(&result.nodes[index]).unwrap()])?;
            }
            tx.execute(
                "UPDATE workflow_runs SET state='completed',result_toon=?2 WHERE id=?1",
                params![run.to_string(), Ledger::encode_payload(&result).unwrap()],
            )?;
            tx.commit()
        };
        let mut blocked = false;
        let before = readonly
            .read_workflow_with(run, || {
                let error = publish(&mut writer).unwrap_err();
                blocked = matches!(
                    error.sqlite_error_code(),
                    Some(rusqlite::ErrorCode::DatabaseBusy | rusqlite::ErrorCode::DatabaseLocked)
                );
            })
            .unwrap();
        assert!(blocked);
        assert_eq!(before.status, RunStatus::Running);
        assert!(
            before
                .nodes
                .iter()
                .all(|node| node.status == NodeStatus::Pending)
        );
        assert!(before.result.is_none());
        publish(&mut writer).unwrap();
        let after = readonly.workflow_run(run).unwrap();
        assert_eq!(after.status, RunStatus::Completed);
        assert!(
            after
                .nodes
                .iter()
                .all(|node| node.status == NodeStatus::Skipped)
        );
        assert_eq!(after.result, Some(result));
    }
    #[test]
    fn first_node_publication_rolls_back_shared_base_and_branch_head_on_failure() {
        use crate::{
            context::builder::{ContextBuilder, ContextInput},
            context::compact::Staged,
            context::{ContextKind, ContextRecord, ContextUpdate, PinTarget},
            history::SourcePoint,
            limits::{ResolvedLimits, count::ByteCounter},
            outcome::{ProviderIdentity, TurnOutcome},
            storage::turns::AttemptStatus,
        };
        let store = Store::new();
        let mut ledger = store.open();
        let mut request = request();
        request.workflow.nodes[0].when = None;
        let record = ContextRecord {
            id: Id::generate(),
            key: "adopted".into(),
            value: Data::new(serde_json::json!(8)).unwrap(),
            kind: ContextKind::Commitment,
        };
        request.updates = vec![ContextUpdate::Pin {
            target: PinTarget::New {
                record: record.clone(),
            },
        }];
        let owner = owner(&mut ledger, &request);
        let definition = &owner.frozen.definition;
        let main = definition.session;
        let run = definition.run;
        ledger
            .record_workflow_progress(
                &owner,
                0,
                &NodeProgress {
                    status: NodeStatus::Running,
                    output: None,
                    error: None,
                    ancestors: vec![],
                    inputs: vec![],
                },
            )
            .unwrap();
        let evaluation = nodes::request_for(&owner.frozen, 0, &[]).unwrap();
        let intent = ledger
            .accept_workflow_node(&owner, 0, &evaluation, &[])
            .unwrap();
        let settings = &owner.frozen.settings[0];
        let plan = ContextBuilder::build(
            ContextInput {
                context_base: Some(&definition.base),
                runtime_only: false,
                history: &[],
                sources: &[],
                request: &evaluation.turn,
                updates: &evaluation.updates,
                required: &[],
                workflow_base: &[],
                session: main,
                turn: Id::generate(),
                provider: ProviderIdentity {
                    provider: "fixture".into(),
                    model: "offline".into(),
                    upstream_provider: None,
                    request_id: None,
                },
            },
            &ResolvedLimits::for_profile(&settings.profile).unwrap(),
            &ByteCounter,
            &Staged,
        )
        .unwrap();
        let attempt = ledger
            .prepare_workflow_attempt(&owner, &intent, plan.manifest)
            .unwrap();
        let outcome: TurnOutcome = serde_json::from_str(include_str!(
            "../../../fixtures/contracts/decision-outcome.json"
        ))
        .unwrap();
        ledger.connection.execute_batch("CREATE TRIGGER reject_workflow_publication BEFORE UPDATE OF state ON attempt_states WHEN NEW.state='completed' BEGIN SELECT RAISE(ABORT,'fault'); END;").unwrap();
        assert!(
            ledger
                .finalize_workflow_attempt(&owner, attempt.id, outcome.clone())
                .is_err()
        );
        let count: i64 = ledger
            .connection
            .query_row(
                "SELECT COUNT(*) FROM context_bases WHERE id=?1",
                [run.to_string()],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 0);
        assert_eq!(
            ledger.session(&SessionSelector::Id(main)).unwrap().frontier,
            [] as [crate::identity::Id; 0]
        );
        assert_eq!(
            ledger.attempt(attempt.id).unwrap().progress.state,
            AttemptStatus::Running
        );
        assert_eq!(
            ledger.workflow_run(run).unwrap().nodes[0].status,
            NodeStatus::Running
        );
        ledger
            .connection
            .execute_batch("DROP TRIGGER reject_workflow_publication")
            .unwrap();
        let turn = ledger
            .finalize_workflow_attempt(&owner, attempt.id, outcome)
            .unwrap();
        let active = ledger
            .context_at(
                &[SourcePoint {
                    session: main,
                    frontier: vec![turn.id],
                }],
                &[],
                main,
                Id::generate(),
            )
            .unwrap();
        assert!(active.iter().any(|active| active.record == record));
    }
}
