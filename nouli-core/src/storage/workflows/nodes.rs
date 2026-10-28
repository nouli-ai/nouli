//! Node intents, attempts and branch-frontier publication for one run owner.
use crate::{
    history::{EvaluationRequest, SourcePoint},
    identity::Id,
};
use serde::{Deserialize, Serialize};
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NodeAttemptScope {
    pub run: Id,
    pub node: usize,
    pub round: u32,
    pub sources: Vec<SourcePoint>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NodeIntent {
    pub run: Id,
    pub node: usize,
    pub round: u32,
    pub request: EvaluationRequest,
    pub sources: Vec<SourcePoint>,
}

use super::{
    WorkflowError, WorkflowEvent, WorkflowEventKind, WorkflowOwner, child_key, event, node_name,
    validate_owner,
};
use crate::storage::{
    Ledger, StorageError, sql_error,
    turns::{
        AttemptInput, AttemptProgress, AttemptStatus, PreparedAttempt, TurnError, progress_event,
    },
};
use crate::{
    codec,
    data::{Data, Input},
    history::{EvaluationManifest, FinalizedTurn, HistoryEvent, ModelSelection},
    identity::SessionSelector,
    outcome::TurnOutcome,
    workflow::{
        NodeStatus, compile,
        schedule::{NodeEvaluation, NodeOutput, NodeProgress, compiled_binding},
    },
};
use rusqlite::{Connection, OptionalExtension, Transaction, params};
use std::collections::HashSet;

fn read_progress(
    connection: &Connection,
    run: Id,
    node: &str,
) -> Result<NodeProgress, WorkflowError> {
    let row: Option<(String, String)> = connection
        .query_row(
            "SELECT state,payload_toon FROM workflow_nodes WHERE run_id=?1 AND node_id=?2",
            params![run.to_string(), node],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(|error| sql_error("read node progress", error))?;
    let (state, payload) = row.ok_or(WorkflowError::Snapshot)?;
    let progress: NodeProgress = Ledger::decode_payload(&payload)?;
    if state != node_name(progress.status) {
        return Err(WorkflowError::Snapshot);
    }
    Ok(progress)
}
fn put_progress(
    tx: &Transaction<'_>,
    owner: &WorkflowOwner,
    node: usize,
    progress: &NodeProgress,
    turn: Option<Id>,
) -> Result<(), StorageError> {
    let definition = &owner.frozen.definition;
    let changed=tx.execute("UPDATE workflow_nodes SET state=?3,turn_id=?4,payload_toon=?5 WHERE run_id=?1 AND node_id=?2",params![definition.run.to_string(),definition.request.workflow.nodes[node].id,node_name(progress.status),turn.map(|id|id.to_string()),Ledger::encode_payload(progress)?]).map_err(|error|sql_error("record node progress",error))?;
    if changed != 1 {
        return Err(StorageError::Conflict);
    }
    Ok(())
}
pub(crate) fn node_output(turn: &FinalizedTurn) -> NodeOutput {
    NodeOutput {
        outcome: turn.outcome.clone(),
        source: Some(SourcePoint {
            session: turn.session,
            frontier: vec![turn.id],
        }),
        evaluation: Some(NodeEvaluation {
            request: turn.request.clone(),
            updates: turn.updates.clone(),
            manifest: turn.manifest.clone(),
        }),
    }
}
fn node_event(
    node: usize,
    kind: WorkflowEventKind,
    progress: Option<NodeProgress>,
    attempt: Option<Id>,
    turn: Option<Id>,
) -> WorkflowEvent {
    WorkflowEvent {
        kind,
        node: Some(node),
        node_status: progress
            .as_ref()
            .map(|progress| progress.status)
            .or_else(|| turn.map(|_| NodeStatus::Completed)),
        run_status: None,
        progress,
        attempt,
        turn,
    }
}
struct ExpectedScope {
    sources: Vec<SourcePoint>,
    ancestors: Vec<usize>,
    inputs: Vec<crate::workflow::schedule::ResolvedBinding>,
}
fn expected_scope(
    ledger: &Ledger,
    owner: &WorkflowOwner,
    node: usize,
) -> Result<ExpectedScope, WorkflowError> {
    let definition = &owner.frozen.definition;
    let plan = compile::compile(&definition.request.workflow)?;
    let declared = plan.nodes().get(node).ok_or(WorkflowError::Snapshot)?;
    let states = definition
        .request
        .workflow
        .nodes
        .iter()
        .map(|node| read_progress(&ledger.connection, definition.run, &node.id))
        .collect::<Result<Vec<_>, _>>()?;
    if declared.dependencies.iter().any(|parent| {
        matches!(
            states[*parent].status,
            NodeStatus::Pending | NodeStatus::Running
        )
    }) {
        return Err(WorkflowError::Conflict);
    }
    let parents: Vec<_> = declared
        .dependencies
        .iter()
        .filter(|parent| states[**parent].status == NodeStatus::Completed)
        .copied()
        .collect();
    let sources = if parents.is_empty() {
        definition.base.sources.clone()
    } else {
        parents
            .iter()
            .map(|parent| {
                states[*parent]
                    .output
                    .as_ref()
                    .and_then(|output| output.source.clone())
                    .ok_or(WorkflowError::Snapshot)
            })
            .collect::<Result<Vec<_>, _>>()?
    };
    let mut ancestors = HashSet::new();
    for parent in parents {
        ancestors.insert(parent);
        ancestors.extend(states[parent].ancestors.iter().copied());
    }
    let ordered = plan
        .order()
        .iter()
        .filter(|ancestor| ancestors.contains(ancestor))
        .copied()
        .collect();
    let inputs = declared
        .inputs
        .iter()
        .map(|binding| compiled_binding(binding, &states).map_err(|_| WorkflowError::Snapshot))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(ExpectedScope {
        sources,
        ancestors: ordered,
        inputs,
    })
}
pub(crate) fn request_for(
    frozen: &super::FrozenWorkflow,
    node: usize,
    inputs: &[crate::workflow::schedule::ResolvedBinding],
) -> Result<EvaluationRequest, WorkflowError> {
    let definition = &frozen.definition;
    let node_definition = definition
        .request
        .workflow
        .nodes
        .get(node)
        .ok_or(WorkflowError::Snapshot)?;
    let mut turn = node_definition.turn.clone();
    if !inputs.is_empty() {
        turn.state.push(Input::Structured {
            data: Data::new(
                codec::to_value(&serde_json::json!({"workflow_inputs":inputs}))
                    .map_err(|_| WorkflowError::Snapshot)?,
            )
            .map_err(|_| WorkflowError::Snapshot)?,
        });
    }
    let mut required = definition.request.required_records.clone();
    for id in &node_definition.required_records {
        if !required.contains(id) {
            required.push(*id);
        }
    }
    let settings = frozen.settings.get(node).ok_or(WorkflowError::Snapshot)?;
    Ok(EvaluationRequest {
        turn,
        session: if definition.base.runtime_only {
            crate::history::SessionSelection::New { name: None }
        } else {
            crate::history::SessionSelection::Continue {
                session: SessionSelector::Id(definition.session),
            }
        },
        updates: node_definition.updates.clone(),
        required_records: required,
        model: ModelSelection {
            provider: Some(settings.profile_name.clone()),
            model: Some(settings.profile.model.clone()),
        },
        request_id: if definition.base.runtime_only {
            None
        } else {
            Some(child_key(definition.run, node))
        },
    })
}

impl Ledger {
    pub fn workflow_node_progress(
        &self,
        owner: &WorkflowOwner,
        node: usize,
    ) -> Result<NodeProgress, WorkflowError> {
        let definition = &owner.frozen.definition;
        let name = &definition
            .request
            .workflow
            .nodes
            .get(node)
            .ok_or(WorkflowError::Snapshot)?
            .id;
        read_progress(&self.connection, definition.run, name)
    }
    pub fn record_workflow_progress(
        &mut self,
        owner: &WorkflowOwner,
        node: usize,
        progress: NodeProgress,
    ) -> Result<(), WorkflowError> {
        validate_owner(self, owner)?;
        let definition = &owner.frozen.definition;
        let name = &definition
            .request
            .workflow
            .nodes
            .get(node)
            .ok_or(WorkflowError::Snapshot)?
            .id;
        let current = read_progress(&self.connection, definition.run, name)?;
        if current.status == NodeStatus::Completed {
            if progress.status == NodeStatus::Completed && current != progress {
                return Err(WorkflowError::Conflict);
            }
            return Ok(());
        }
        if current == progress {
            return Ok(());
        }
        if !current.status.can_transition_to(progress.status) {
            return Err(WorkflowError::Conflict);
        }
        if progress.status == NodeStatus::Running {
            let ExpectedScope {
                ancestors, inputs, ..
            } = expected_scope(self, owner, node)?;
            if progress.ancestors != ancestors
                || progress.inputs != inputs
                || inputs.iter().any(|input| input.required_missing)
            {
                return Err(WorkflowError::Snapshot);
            }
        }
        if progress.status == NodeStatus::Completed {
            return Err(WorkflowError::Conflict);
        }
        self.transaction::<_,StorageError>("record workflow node transition",|tx| {
            put_progress(tx,owner,node,&progress,None)?;
            if matches!(progress.status,NodeStatus::Interrupted|NodeStatus::Failed) {
                let mut statement=tx.prepare("SELECT a.id,s.state FROM attempts a JOIN attempt_states s ON s.attempt_id=a.id WHERE a.run_id=?1 AND a.node_id=?2 AND s.state IN ('pending','running')").map_err(|error|sql_error("read interrupted node attempts",error))?;
                let rows=statement.query_map(params![definition.run.to_string(),name],|row|Ok((row.get::<_,String>(0)?,row.get::<_,String>(1)?))).map_err(|error|sql_error("read node attempt states",error))?.collect::<Result<Vec<_>,_>>().map_err(|error|sql_error("read node attempts",error))?;drop(statement);
                for (id,state) in rows {let id=Id::try_from(id).map_err(|_|StorageError::Payload)?;tx.execute("UPDATE attempt_states SET state='interrupted' WHERE attempt_id=?1",[id.to_string()]).map_err(|error|sql_error("interrupt node attempt",error))?;progress_event(tx,id,&AttemptProgress {state:AttemptStatus::Interrupted,error:Some(crate::protocol::ErrorReport {category:crate::protocol::ErrorCategory::Interrupted,code:if state=="running" {"provider_outcome_unknown"} else {"not_dispatched"}.into(),message:"Workflow node interrupted".into(),retryable:false})})?;}
            }
            event(tx,definition.run,&node_event(node,WorkflowEventKind::NodeState,Some(progress.clone()),None,None))?;Ok(())
        })?;
        Ok(())
    }
    pub fn accept_workflow_node(
        &mut self,
        owner: &WorkflowOwner,
        node: usize,
        request: &EvaluationRequest,
        sources: &[SourcePoint],
    ) -> Result<NodeIntent, WorkflowError> {
        validate_owner(self, owner)?;
        let definition = &owner.frozen.definition;
        let name = &definition
            .request
            .workflow
            .nodes
            .get(node)
            .ok_or(WorkflowError::Snapshot)?
            .id;
        let progress = read_progress(&self.connection, definition.run, name)?;
        let ExpectedScope {
            sources: expected,
            ancestors,
            inputs,
        } = expected_scope(self, owner, node)?;
        if progress.status != NodeStatus::Running
            || progress.ancestors != ancestors
            || progress.inputs != inputs
            || sources != expected
            || *request != request_for(&owner.frozen, node, &inputs)?
        {
            return Err(WorkflowError::Snapshot);
        }
        let intent = NodeIntent {
            run: definition.run,
            node,
            round: owner.round,
            request: request.clone(),
            sources: sources.to_vec(),
        };
        let payload = Self::encode_payload(&intent)?;
        self.transaction::<_,StorageError>("accept workflow node intent",|tx| {
            let existing:Option<String>=tx.query_row("SELECT payload_toon FROM workflow_node_intents WHERE run_id=?1 AND node_id=?2 AND round=?3",params![definition.run.to_string(),name,owner.round],|row|row.get(0)).optional().map_err(|error|sql_error("inspect node intent",error))?;
            if let Some(existing)=existing {if existing!=payload {return Err(StorageError::Conflict);}}
            else {tx.execute("INSERT INTO workflow_node_intents(run_id,node_id,round,payload_toon) VALUES(?1,?2,?3,?4)",params![definition.run.to_string(),name,owner.round,payload]).map_err(|error|sql_error("save node intent",error))?;event(tx,definition.run,&node_event(node,WorkflowEventKind::NodeIntent,None,None,None))?;}Ok(())
        })?;
        Ok(intent)
    }
    pub fn prepare_workflow_attempt(
        &mut self,
        owner: &WorkflowOwner,
        intent: &NodeIntent,
        manifest: EvaluationManifest,
    ) -> Result<PreparedAttempt, WorkflowError> {
        validate_owner(self, owner)?;
        let definition = &owner.frozen.definition;
        let node = intent.node;
        let name = &definition
            .request
            .workflow
            .nodes
            .get(node)
            .ok_or(WorkflowError::Snapshot)?
            .id;
        if intent.run != definition.run
            || intent.round != owner.round
            || read_progress(&self.connection, definition.run, name)?.status != NodeStatus::Running
        {
            return Err(WorkflowError::Conflict);
        }
        manifest.validate()?;
        let settings = &owner.frozen.settings[node];
        if manifest.sources != intent.sources
            || manifest.provider.provider != settings.profile_name
            || manifest.provider.model != settings.profile.model
            || intent.request.required_records.iter().any(|id| {
                !manifest.records.iter().any(|record| {
                    record.record == *id
                        && record.protected
                        && record.representation != crate::history::Representation::Omitted
                })
            })
        {
            return Err(WorkflowError::Snapshot);
        }
        let history = self.ancestry(&intent.sources)?;
        crate::context::projection::project_with_base(
            &history,
            &intent.sources,
            Some(&definition.base),
            &intent.request.updates,
            crate::context::projection::RevisionOrigin::Pending {
                session: Some(definition.session),
                token: Id::generate(),
            },
        )
        .map_err(TurnError::from)?;
        let current = self.session(&SessionSelector::Id(definition.session))?;
        let attempt = PreparedAttempt {
            workflow: Some(NodeAttemptScope {
                run: definition.run,
                node,
                round: owner.round,
                sources: intent.sources.clone(),
            }),
            id: Id::generate(),
            owner: Some(owner.session_guard.owner),
            session: definition.session,
            expected_frontier: intent
                .sources
                .iter()
                .flat_map(|source| source.frontier.iter().copied())
                .collect(),
            expected_revision: current.revision,
            input: AttemptInput {
                request: intent.request.turn.clone(),
                updates: intent.request.updates.clone(),
                manifest,
                request_id: intent.request.request_id.clone(),
            },
        };
        self.transaction::<_,StorageError>("prepare workflow attempt",|tx| {
            let saved:String=tx.query_row("SELECT payload_toon FROM workflow_node_intents WHERE run_id=?1 AND node_id=?2 AND round=?3",params![definition.run.to_string(),name,owner.round],|row|row.get(0)).map_err(|error|sql_error("verify accepted node intent",error))?;
            if saved!=Self::encode_payload(intent)? {return Err(StorageError::Conflict);}
            let live:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM attempts a JOIN attempt_states s ON s.attempt_id=a.id WHERE a.run_id=?1 AND a.node_id=?2 AND s.state IN ('pending','running'))",params![definition.run.to_string(),name],|row|row.get(0)).map_err(|error|sql_error("check live node attempt",error))?;if live {return Err(StorageError::Conflict);}
            tx.execute("INSERT INTO attempts(id,session_id,run_id,node_id,payload_toon) VALUES(?1,?2,?3,?4,?5)",params![attempt.id.to_string(),definition.session.to_string(),definition.run.to_string(),name,Self::encode_payload(&attempt)?]).map_err(|error|sql_error("insert workflow attempt",error))?;
            tx.execute("INSERT INTO attempt_states(attempt_id,state) VALUES(?1,'running')",[attempt.id.to_string()]).map_err(|error|sql_error("start workflow attempt",error))?;
            progress_event(tx,attempt.id,&AttemptProgress {state:AttemptStatus::Running,error:None})?;event(tx,definition.run,&node_event(node,WorkflowEventKind::AttemptStarted,None,Some(attempt.id),None))?;Ok(())
        })?;
        Ok(attempt)
    }
    pub fn fail_workflow_attempt(
        &mut self,
        owner: &WorkflowOwner,
        attempt: Id,
        progress: AttemptProgress,
    ) -> Result<(), WorkflowError> {
        validate_owner(self, owner)?;
        let snapshot = self.attempt(attempt)?;
        let scope = snapshot.intent.workflow.ok_or(WorkflowError::Snapshot)?;
        if snapshot.intent.owner != Some(owner.session_guard.owner)
            || scope.run != owner.frozen.definition.run
            || scope.round != owner.round
            || !matches!(
                snapshot.progress.state,
                AttemptStatus::Pending | AttemptStatus::Running
            )
            || !matches!(
                progress.state,
                AttemptStatus::Failed | AttemptStatus::Interrupted
            )
            || progress.error.is_none()
        {
            return Err(WorkflowError::Conflict);
        }
        self.transaction::<_,StorageError>("finish workflow attempt",|tx| {
            let state=if progress.state==AttemptStatus::Failed {"failed"} else {"interrupted"};
            let changed=tx.execute("UPDATE attempt_states SET state=?2 WHERE attempt_id=?1 AND state IN ('pending','running')",params![attempt.to_string(),state]).map_err(|error|sql_error("record failed workflow attempt",error))?;if changed!=1 {return Err(StorageError::Conflict);}
            progress_event(tx,attempt,&progress)?;event(tx,scope.run,&node_event(scope.node,WorkflowEventKind::AttemptFinished,None,Some(attempt),None))?;Ok(())
        })?;
        Ok(())
    }
    pub fn finalize_workflow_attempt(
        &mut self,
        owner: &WorkflowOwner,
        attempt: Id,
        outcome: TurnOutcome,
    ) -> Result<FinalizedTurn, WorkflowError> {
        validate_owner(self, owner)?;
        let snapshot = self.attempt(attempt)?;
        let prepared = snapshot.intent;
        let scope = prepared.workflow.as_ref().ok_or(WorkflowError::Snapshot)?;
        let definition = &owner.frozen.definition;
        if prepared.owner != Some(owner.session_guard.owner)
            || scope.run != definition.run
            || scope.round != owner.round
            || !matches!(
                snapshot.progress.state,
                AttemptStatus::Pending | AttemptStatus::Running
            )
        {
            return Err(WorkflowError::Conflict);
        }
        let node = scope.node;
        let name = &definition
            .request
            .workflow
            .nodes
            .get(node)
            .ok_or(WorkflowError::Snapshot)?
            .id;
        let mut progress = read_progress(&self.connection, definition.run, name)?;
        if progress.status != NodeStatus::Running {
            return Err(WorkflowError::Conflict);
        }
        let history = self.ancestry(&scope.sources)?;
        crate::context::projection::project_with_base(
            &history,
            &scope.sources,
            Some(&definition.base),
            &prepared.input.updates,
            crate::context::projection::RevisionOrigin::Pending {
                session: Some(definition.session),
                token: prepared.id,
            },
        )
        .map_err(TurnError::from)?;
        let mut manifest = prepared.input.manifest;
        manifest.provider = outcome.provider.clone();
        let mut seen = HashSet::new();
        let turn = FinalizedTurn {
            context_base: Some(definition.base.clone()),
            workflow_run: Some(definition.run),
            runtime_only: false,
            workflow_node: Some(name.clone()),
            id: Id::generate(),
            session: definition.session,
            parents: scope
                .sources
                .iter()
                .flat_map(|source| source.frontier.iter().copied())
                .filter(|id| seen.insert(*id))
                .collect(),
            request: prepared.input.request,
            outcome,
            updates: prepared.input.updates,
            manifest,
        };
        turn.validate()?;
        progress.status = NodeStatus::Completed;
        progress.output = Some(node_output(&turn));
        progress.error = None;
        self.transaction::<_,StorageError>("publish workflow node",|tx| {
            let (revision,published):(i64,i64)=tx.query_row("SELECT s.revision,r.published_revision FROM sessions s JOIN workflow_runs r ON r.session_id=s.id WHERE r.id=?1 AND r.state='running' AND r.owner_id=?2",params![definition.run.to_string(),owner.session_guard.owner.to_string()],|row|Ok((row.get(0)?,row.get(1)?))).map_err(|error|sql_error("verify workflow publication owner",error))?;
            if revision!=published || revision==i64::MAX {return Err(StorageError::Conflict);}
            super::super::publication::insert_turn(tx,&turn,attempt,revision)?;put_progress(tx,owner,node,&progress,Some(turn.id))?;
            let mut statement=tx.prepare("WITH RECURSIVE ancestors(id) AS (SELECT p.parent_id FROM turn_parents p JOIN workflow_nodes n ON n.turn_id=p.turn_id WHERE n.run_id=?1 AND n.state='completed' UNION SELECT p.parent_id FROM turn_parents p JOIN ancestors a ON p.turn_id=a.id) SELECT n.turn_id FROM workflow_nodes n WHERE n.run_id=?1 AND n.state='completed' AND n.turn_id NOT IN (SELECT id FROM ancestors) ORDER BY n.declaration_order").map_err(|error|sql_error("project workflow branch frontier",error))?;
            let heads=statement.query_map([definition.run.to_string()],|row|row.get::<_,String>(0)).map_err(|error|sql_error("read completed branches",error))?.collect::<Result<Vec<_>,_>>().map_err(|error|sql_error("read branch heads",error))?;drop(statement);
            let heads=heads.into_iter().map(|id|Id::try_from(id).map_err(|_|StorageError::Payload)).collect::<Result<Vec<_>,_>>()?;let frontier=Self::encode_payload(&heads)?;
            let changed=tx.execute("UPDATE sessions SET frontier_toon=?2,revision=revision+1 WHERE id=?1 AND revision=?3",params![definition.session.to_string(),frontier,revision]).map_err(|error|sql_error("publish workflow branch frontier",error))?;if changed!=1 {return Err(StorageError::Conflict);}
            tx.execute("UPDATE workflow_runs SET frontier_toon=?2,published_revision=?3 WHERE id=?1",params![definition.run.to_string(),frontier,revision+1]).map_err(|error|sql_error("record published workflow frontier",error))?;
            let changed=tx.execute("UPDATE request_reservations SET state='completed',result_toon=?2 WHERE request_id=?1 AND kind='workflow_node' AND state='pending' AND owner_id=?3",params![child_key(definition.run,node),Self::encode_payload(&turn)?,owner.session_guard.owner.to_string()]).map_err(|error|sql_error("complete stable node identity",error))?;if changed!=1 {return Err(StorageError::Conflict);}
            tx.execute("INSERT INTO session_events(session_id,sequence,payload_toon) SELECT ?1,COALESCE(MAX(sequence),-1)+1,?2 FROM session_events WHERE session_id=?1",params![definition.session.to_string(),Self::encode_payload(&HistoryEvent::TurnFinalized {session:definition.session,turn:turn.id,frontier:heads})?]).map_err(|error|sql_error("record workflow publication",error))?;
            event(tx,definition.run,&node_event(node,WorkflowEventKind::NodeState,None,Some(attempt),Some(turn.id)))?;Ok(())
        })?;
        Ok(turn)
    }
}
