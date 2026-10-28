//! Terminal workflow publication and controlled host-side interruption.
use super::{
    WorkflowError, WorkflowEvent, WorkflowEventKind, WorkflowOwner, event, run_name, validate_owner,
};
use crate::storage::{Ledger, StorageError, sql_error};
use crate::{
    protocol::{ErrorCategory, ErrorReport},
    workflow::{NodeStatus, RunStatus, compile, schedule::ScheduleResult},
};
use rusqlite::params;
impl Ledger {
    pub fn interrupt_workflow(
        &mut self,
        owner: &WorkflowOwner,
    ) -> Result<super::StoredRun, WorkflowError> {
        validate_owner(self, owner)?;
        let run = owner.frozen.definition.run;
        let snapshot = self.workflow_run(run)?;
        for (node, mut progress) in snapshot.nodes.into_iter().enumerate() {
            if matches!(progress.status, NodeStatus::Pending | NodeStatus::Running) {
                progress.status = NodeStatus::Interrupted;
                progress.error = Some(ErrorReport {
                    category: ErrorCategory::Interrupted,
                    code: "workflow_interrupted".into(),
                    message: "Workflow execution interrupted".into(),
                    retryable: false,
                });
                self.record_workflow_progress(owner, node, progress)?;
            }
        }
        let snapshot = self.workflow_run(run)?;
        let graph = compile::compile(&owner.frozen.definition.request.workflow)?;
        let (start_order, completion_order) = super::round_order(self, run, owner.round)?;
        let outputs = crate::workflow::schedule::project_outputs(&graph, &snapshot.nodes)
            .map_err(|_| WorkflowError::Snapshot)?;
        self.finish_workflow(
            owner,
            &ScheduleResult {
                status: RunStatus::Interrupted,
                nodes: snapshot.nodes,
                start_order,
                completion_order,
                outputs,
            },
        )
    }

    pub fn finish_workflow(
        &mut self,
        owner: &WorkflowOwner,
        result: &ScheduleResult,
    ) -> Result<super::StoredRun, WorkflowError> {
        validate_owner(self, owner)?;
        let definition = &owner.frozen.definition;
        let current = self.workflow_run(definition.run)?;
        if current.nodes != result.nodes
            || result
                .nodes
                .iter()
                .any(|node| matches!(node.status, NodeStatus::Pending | NodeStatus::Running))
            || matches!(result.status, RunStatus::Pending | RunStatus::Running)
        {
            return Err(WorkflowError::Conflict);
        }
        let live:bool=self.connection.query_row("SELECT EXISTS(SELECT 1 FROM attempts a JOIN attempt_states s ON s.attempt_id=a.id WHERE a.run_id=?1 AND s.state IN ('pending','running'))",[definition.run.to_string()],|row|row.get(0)).map_err(|error|sql_error("verify settled workflow attempts",error))?;
        if live {
            return Err(WorkflowError::Conflict);
        }
        let graph = compile::compile(&definition.request.workflow)?;
        let outputs = crate::workflow::schedule::project_outputs(&graph, &current.nodes)
            .map_err(|_| WorkflowError::Snapshot)?;
        if outputs != result.outputs
            || result
                .start_order
                .iter()
                .chain(&result.completion_order)
                .any(|node| *node >= current.nodes.len())
        {
            return Err(WorkflowError::Snapshot);
        }
        let partial = current
            .nodes
            .iter()
            .any(|node| matches!(node.status, NodeStatus::Failed | NodeStatus::Blocked))
            || outputs.iter().any(|output| output.required_missing);
        let terminal = if current
            .nodes
            .iter()
            .any(|node| node.status == NodeStatus::Interrupted)
        {
            RunStatus::Interrupted
        } else if partial {
            if current
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
        if result.status != terminal && result.status != RunStatus::Interrupted {
            return Err(WorkflowError::Conflict);
        }
        let key = definition
            .request
            .request_id
            .as_deref()
            .ok_or(WorkflowError::Snapshot)?;
        self.transaction::<_,StorageError>("finish workflow run",|tx| {
            let changed=tx.execute("UPDATE workflow_runs SET state=?2,result_toon=?3 WHERE id=?1 AND state='running' AND owner_id=?4",params![definition.run.to_string(),run_name(result.status),Self::encode_payload(result)?,owner.session_guard.owner.to_string()]).map_err(|error|sql_error("save workflow result",error))?;if changed!=1 {return Err(StorageError::Conflict);}
            let state=if result.status==RunStatus::Completed {"completed"} else {"interrupted"};
            let changed=tx.execute("UPDATE request_reservations SET state=?2,result_toon=?3 WHERE request_id=?1 AND state='pending' AND owner_id=?4",params![key,state,Self::encode_payload(result)?,owner.session_guard.owner.to_string()]).map_err(|error|sql_error("save workflow replay result",error))?;if changed!=1 {return Err(StorageError::Conflict);}
            tx.execute("INSERT INTO workflow_round_results(run_id,round,state,frontier_toon,published_revision,payload_toon) SELECT id,?2,state,frontier_toon,published_revision,?3 FROM workflow_runs WHERE id=?1",params![definition.run.to_string(),owner.round,Self::encode_payload(result)?]).map_err(|error|sql_error("append workflow round result",error))?;
            event(tx,definition.run,&WorkflowEvent {round:owner.round,kind:WorkflowEventKind::RunFinished,progress:None,node:None,node_status:None,run_status:Some(result.status),attempt:None,turn:None})?;Ok(())
        })?;
        self.workflow_run(definition.run)
    }
}
