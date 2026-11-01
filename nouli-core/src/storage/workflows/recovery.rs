//! Explicit retry rounds and read-only inspection of uncertain work.
use super::StoredRun;
use crate::{identity::Id, storage::turns::AttemptSnapshot};
use serde::{Deserialize, Serialize};
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Disposition {
    NeverDispatched,
    LiveRunning,
    RetryWaiting,
    TerminalFailed,
    InterruptedBeforeCall,
    ProviderOutcomeUnknown,
    Completed,
    Skipped,
    Blocked,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NodeReceipt {
    pub node: usize,
    pub disposition: Disposition,
    pub uncertain_prior_calls: usize,
    pub attempts: Vec<AttemptSnapshot>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RunInspection {
    pub run: StoredRun,
    pub live_owner: bool,
    pub session_head_compatible: bool,
    pub nodes: Vec<NodeReceipt>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RoundResult {
    pub round: u32,
    pub frontier: Vec<Id>,
    pub published_revision: i64,
    pub result: ScheduleResult,
}

use super::{
    WorkflowDecision, WorkflowError, WorkflowEvent, WorkflowEventKind, WorkflowOwner, child_key,
    event,
};
use crate::{
    codec,
    identity::SessionSelector,
    protocol::{ErrorCategory, ErrorReport},
    storage::{
        Ledger,
        guards::{self, GuardError},
        sql_error,
        turns::{AttemptProgress, AttemptStatus, progress_event},
    },
    workflow::{
        NodeStatus, RunStatus,
        schedule::{NodeProgress, ScheduleResult, project_outputs},
    },
};
use rusqlite::{Connection, OptionalExtension, params};

fn attempts(
    ledger: &Ledger,
    connection: &Connection,
    run: Id,
) -> Result<Vec<AttemptSnapshot>, WorkflowError> {
    let mut statement = connection
        .prepare("SELECT id FROM attempts WHERE run_id=?1 ORDER BY rowid")
        .map_err(|error| sql_error("list workflow attempts", &error))?;
    let ids = statement
        .query_map([run.to_string()], |row| row.get::<_, String>(0))
        .map_err(|error| sql_error("read workflow attempt IDs", &error))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| sql_error("decode workflow attempt IDs", &error))?;
    ids.into_iter()
        .map(|id| {
            ledger
                .attempt(Id::try_from(id).map_err(|_| WorkflowError::Snapshot)?)
                .map_err(WorkflowError::from)
        })
        .collect()
}
fn unknown(attempt: &AttemptSnapshot) -> bool {
    attempt
        .progress
        .error
        .as_ref()
        .is_some_and(|error| error.code == "provider_outcome_unknown")
}
fn disposition(node: &NodeProgress, attempts: &[AttemptSnapshot], live: bool) -> Disposition {
    if node.status == NodeStatus::Completed {
        return Disposition::Completed;
    }
    if node.status == NodeStatus::Skipped {
        return Disposition::Skipped;
    }
    if live
        && attempts
            .iter()
            .any(|attempt| attempt.progress.state == AttemptStatus::Running)
    {
        return Disposition::LiveRunning;
    }
    if attempts.iter().any(|attempt| {
        unknown(attempt) || (!live && attempt.progress.state == AttemptStatus::Running)
    }) {
        return Disposition::ProviderOutcomeUnknown;
    }
    if node.status == NodeStatus::Running
        && attempts
            .last()
            .is_some_and(|attempt| attempt.progress.state == AttemptStatus::Failed)
    {
        return if live {
            Disposition::RetryWaiting
        } else {
            Disposition::InterruptedBeforeCall
        };
    }
    match node.status {
        NodeStatus::Failed => Disposition::TerminalFailed,
        NodeStatus::Blocked => Disposition::Blocked,
        NodeStatus::Interrupted => Disposition::InterruptedBeforeCall,
        _ if !live
            && attempts.iter().any(|attempt| {
                matches!(
                    attempt.progress.state,
                    AttemptStatus::Pending | AttemptStatus::Interrupted
                )
            }) =>
        {
            Disposition::InterruptedBeforeCall
        }
        _ => Disposition::NeverDispatched,
    }
}
fn settled(mut nodes: Vec<NodeProgress>) -> Vec<NodeProgress> {
    for node in &mut nodes {
        if matches!(node.status, NodeStatus::Pending | NodeStatus::Running) {
            node.status = NodeStatus::Interrupted;
            node.error = Some(ErrorReport {
                category: ErrorCategory::Interrupted,
                code: "owner_exited".into(),
                message: "Workflow owner exited before settling this node".into(),
                retryable: false,
            });
        }
    }
    nodes
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WorkflowInspection {
    pub version: u32,
    pub inspection: RunInspection,
    pub events: Vec<super::events::StoredEvent>,
    pub rounds: Vec<RoundResult>,
}
pub(in crate::storage) fn read_round_results(
    connection: &Connection,
    run: Id,
) -> Result<Vec<RoundResult>, WorkflowError> {
    let mut statement = connection.prepare(
            "SELECT round,frontier_toon,published_revision,payload_toon FROM workflow_round_results WHERE run_id=?1 ORDER BY round"
        ).map_err(|error| sql_error("list workflow retry rounds", &error))?;
    let rows = statement
        .query_map([run.to_string()], |row| {
            Ok((
                row.get::<_, u32>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, String>(3)?,
            ))
        })
        .map_err(|error| sql_error("read workflow retry rounds", &error))?;
    rows.map(|row| {
        let (round, frontier, published_revision, result) =
            row.map_err(|error| sql_error("decode workflow retry round", &error))?;
        Ok(RoundResult {
            round,
            frontier: Ledger::decode_payload(&frontier)?,
            published_revision,
            result: Ledger::decode_payload(&result)?,
        })
    })
    .collect()
}
impl Ledger {
    /// Immutable results of every settled retry round, in acceptance order.
    ///
    /// # Errors
    ///
    /// - Returns any [`Ledger::workflow_run`] error for the run.
    /// - Returns [`WorkflowError::Storage`] if the round results cannot be read
    ///   or decoded.
    pub fn workflow_round_results(&self, run: Id) -> Result<Vec<RoundResult>, WorkflowError> {
        self.workflow_run(run)?;
        read_round_results(&self.connection, run)
    }

    /// Inspects a run's stored progress, attempts, live ownership, and session
    /// head compatibility in one read snapshot without changing it.
    ///
    /// # Errors
    ///
    /// - Returns any [`Ledger::workflow_run`] error for the run.
    /// - Returns [`WorkflowError::Turn`] if an attempt cannot be read, and
    ///   [`WorkflowError::Snapshot`] if an attempt ID is invalid.
    /// - Returns [`WorkflowError::Guard`] if the run lock cannot be probed, and
    ///   [`WorkflowError::Session`] if the run's session cannot be read.
    /// - Returns [`WorkflowError::Storage`] if the ledger cannot be read.
    pub fn inspect_workflow(&self, run: Id) -> Result<RunInspection, WorkflowError> {
        let tx = self
            .connection
            .unchecked_transaction()
            .map_err(|error| sql_error("begin recovery inspection", &error))?;
        let inspection = self.inspect_workflow_record(run)?;
        tx.commit()
            .map_err(|error| sql_error("finish recovery inspection", &error))?;
        Ok(inspection)
    }
    /// Inspects a run together with its ordered events and settled round
    /// results in one read snapshot.
    ///
    /// # Errors
    ///
    /// - Returns any [`Ledger::workflow_run`] error for the run.
    /// - Returns [`WorkflowError::Turn`] if an attempt cannot be read, and
    ///   [`WorkflowError::Snapshot`] if an attempt ID is invalid.
    /// - Returns [`WorkflowError::Guard`] if the run lock cannot be probed, and
    ///   [`WorkflowError::Session`] if the run's session cannot be read.
    /// - Returns [`WorkflowError::Storage`] if the ledger cannot be read.
    /// - Returns [`WorkflowError::Snapshot`] if the stored events are
    ///   inconsistent, and [`WorkflowError::Storage`] if events or round results
    ///   cannot be decoded.
    pub fn inspect_workflow_full(&self, run: Id) -> Result<WorkflowInspection, WorkflowError> {
        let tx = self
            .connection
            .unchecked_transaction()
            .map_err(|error| sql_error("begin complete run inspection", &error))?;
        let inspection = self.inspect_workflow_record(run)?;
        let events = super::events::read_events(&tx, run)?;
        let rounds = read_round_results(&tx, run)?;
        tx.commit()
            .map_err(|error| sql_error("finish complete run inspection", &error))?;
        Ok(WorkflowInspection {
            version: crate::decision::CONTRACT_VERSION,
            inspection,
            events,
            rounds,
        })
    }
    fn inspect_workflow_record(&self, run: Id) -> Result<RunInspection, WorkflowError> {
        let stored = super::read_run(&self.connection, run, || {})?;
        let all = attempts(self, &self.connection, run)?;
        let live = guards::is_locked(self, &format!("run-{run}"))?;
        let session = super::sessions::read_session(
            &self.connection,
            &SessionSelector::Id(stored.frozen.definition.session),
        )?;
        let compatible =
            session.revision == stored.published_revision && session.frontier == stored.frontier;
        let nodes = stored
            .nodes
            .iter()
            .enumerate()
            .map(|(node, progress)| {
                let attempts: Vec<_> = all
                    .iter()
                    .filter(|attempt| {
                        attempt
                            .intent
                            .workflow
                            .as_ref()
                            .is_some_and(|scope| scope.node == node)
                    })
                    .cloned()
                    .collect();
                NodeReceipt {
                    node,
                    disposition: disposition(progress, &attempts, live),
                    uncertain_prior_calls: attempts
                        .iter()
                        .filter(|attempt| {
                            unknown(attempt)
                                || (!live && attempt.progress.state == AttemptStatus::Running)
                        })
                        .count(),
                    attempts,
                }
            })
            .collect();
        Ok(RunInspection {
            run: stored,
            live_owner: live,
            session_head_compatible: compatible,
            nodes,
        })
    }
    /// Starts an explicit retry round for an unfinished run, interrupting
    /// orphaned attempts and resetting unsettled nodes, or reports a completed
    /// or in-progress run.
    ///
    /// # Errors
    ///
    /// - Returns any [`Ledger::workflow_run`] error for the run.
    /// - Returns [`WorkflowError::AdvancedHead`] if the session head moved past
    ///   the run's published frontier.
    /// - Returns [`WorkflowError::Snapshot`] if the run has no request ID, its
    ///   reservation does not point at the run, or its rounds, attempts, or
    ///   outputs are inconsistent.
    /// - Returns [`WorkflowError::Conflict`] if the run, its reservations, or a
    ///   node changed state concurrently.
    /// - Returns [`WorkflowError::Guard`] if a guard cannot be acquired for a
    ///   reason other than an active request or run writer, including an
    ///   active session writer.
    /// - Returns [`WorkflowError::Session`], [`WorkflowError::Turn`],
    ///   [`WorkflowError::History`], [`WorkflowError::Compile`],
    ///   [`WorkflowError::IntentConflict`], or [`WorkflowError::Storage`] if the
    ///   session, attempts, frozen request, or ledger cannot be read or written.
    pub fn resume_workflow(&mut self, run: Id) -> Result<WorkflowDecision, WorkflowError> {
        let first = self.workflow_run(run)?;
        if first.status == RunStatus::Completed {
            return Ok(WorkflowDecision::Saved(Box::new(first)));
        }
        let key = first
            .frozen
            .definition
            .request
            .request_id
            .as_deref()
            .ok_or(WorkflowError::Snapshot)?;
        let request_guard = match guards::lock(
            self,
            &format!(
                "request-{}",
                codec::fingerprint(&key).map_err(|_| WorkflowError::Snapshot)?
            ),
        ) {
            Ok(guard) => guard,
            Err(GuardError::WriterActive) => return Ok(WorkflowDecision::InProgress),
            Err(error) => return Err(error.into()),
        };
        let run_guard = match self.run_guard(run) {
            Ok(guard) => guard,
            Err(GuardError::WriterActive) => return Ok(WorkflowDecision::InProgress),
            Err(error) => return Err(error.into()),
        };
        let session_guard = guards::acquire_session(self, first.frozen.definition.session)?;
        let stored = self.workflow_run(run)?;
        if stored.status == RunStatus::Completed {
            return Ok(WorkflowDecision::Saved(Box::new(stored)));
        }
        let session = self.session(&SessionSelector::Id(stored.frozen.definition.session))?;
        if session.revision != stored.published_revision || session.frontier != stored.frontier {
            return Err(WorkflowError::AdvancedHead);
        }
        let previous: i64 = self
            .connection
            .query_row(
                "SELECT MAX(round) FROM workflow_rounds WHERE run_id=?1",
                [run.to_string()],
                |row| row.get(0),
            )
            .map_err(|error| sql_error("read last retry round", &error))?;
        let previous = u32::try_from(previous).map_err(|_| WorkflowError::Snapshot)?;
        let round = previous.checked_add(1).ok_or(WorkflowError::Snapshot)?;
        let snapshots = attempts(self, &self.connection, run)?;
        let old_nodes = settled(stored.nodes.clone());
        let graph = crate::workflow::compile::compile(&stored.frozen.definition.request.workflow)?;
        let (start_order, completion_order) = super::round_order(self, run, previous)?;
        let old_result = if let Some(result) = &stored.result {
            result.clone()
        } else {
            ScheduleResult {
                status: RunStatus::Interrupted,
                outputs: project_outputs(&graph, &old_nodes)
                    .map_err(|_| WorkflowError::Snapshot)?,
                nodes: old_nodes.clone(),
                start_order,
                completion_order,
            }
        };
        let key = stored
            .frozen
            .definition
            .request
            .request_id
            .as_deref()
            .ok_or(WorkflowError::Snapshot)?;
        self.transaction::<_,WorkflowError>("start explicit workflow retry round",|tx| {
            let current=super::sessions::read_session(tx,&SessionSelector::Id(session.id))?;
            if current.revision!=stored.published_revision || current.frontier!=stored.frontier {return Err(WorkflowError::AdvancedHead);}
            let saved_run=super::request_run(tx,key,&super::fingerprint(&stored.frozen.definition.request)?)?;
            if saved_run!=Some(run) {return Err(WorkflowError::Snapshot);}
            for attempt in &snapshots {if matches!(attempt.progress.state,AttemptStatus::Pending|AttemptStatus::Running) {
                let scope=attempt.intent.workflow.as_ref().ok_or(WorkflowError::Snapshot)?;
                let code=if attempt.progress.state==AttemptStatus::Running {"provider_outcome_unknown"} else {"not_dispatched"};
                let progress=AttemptProgress {state:AttemptStatus::Interrupted,error:Some(ErrorReport {category:ErrorCategory::Interrupted,code:code.into(),message:"Previous workflow owner exited".into(),retryable:false})};
                tx.execute("UPDATE attempt_states SET state='interrupted' WHERE attempt_id=?1 AND state IN ('pending','running')",[attempt.intent.id.to_string()]).map_err(|error|sql_error("recover orphan attempt",&error))?;progress_event(tx,attempt.intent.id,&progress)?;
                event(tx,run,&WorkflowEvent {version:crate::decision::CONTRACT_VERSION,round:previous,kind:WorkflowEventKind::Recovered,progress:None,node:Some(scope.node),node_status:Some(NodeStatus::Interrupted),run_status:None,attempt:Some(attempt.intent.id),turn:None})?;
            }}
            let old:Option<String>=tx.query_row("SELECT payload_toon FROM workflow_round_results WHERE run_id=?1 AND round=?2",params![run.to_string(),previous],|row|row.get(0)).optional().map_err(|error|sql_error("inspect prior round result",&error))?;
            if old.is_none() {
                tx.execute("INSERT INTO workflow_round_results(run_id,round,state,frontier_toon,published_revision,payload_toon) VALUES(?1,?2,'interrupted',?3,?4,?5)",params![run.to_string(),previous,Self::encode_payload(&stored.frontier)?,stored.published_revision,Self::encode_payload(&old_result)?]).map_err(|error|sql_error("preserve interrupted retry round",&error))?;
                for (node,progress) in old_nodes.iter().enumerate() {
                    if matches!(stored.nodes[node].status,NodeStatus::Pending|NodeStatus::Running) {
                        event(tx,run,&WorkflowEvent {version:crate::decision::CONTRACT_VERSION,round:previous,kind:WorkflowEventKind::Recovered,progress:Some(progress.clone()),node:Some(node),node_status:Some(progress.status),run_status:None,attempt:None,turn:None})?;
                    }
                }
                event(tx,run,&WorkflowEvent {version:crate::decision::CONTRACT_VERSION,round:previous,kind:WorkflowEventKind::RunFinished,progress:None,node:None,node_status:None,run_status:Some(RunStatus::Interrupted),attempt:None,turn:None})?;
            }
            let changed=tx.execute("UPDATE workflow_runs SET owner_id=?2,state='running',result_toon=NULL WHERE id=?1 AND state!='completed'",params![run.to_string(),session_guard.owner.to_string()]).map_err(|error|sql_error("renew workflow owner",&error))?;if changed!=1 {return Err(WorkflowError::Conflict);}
            let changed=tx.execute("UPDATE request_reservations SET owner_id=?2,state='pending' WHERE request_id=?1 AND kind='workflow' AND state!='completed'",params![key,session_guard.owner.to_string()]).map_err(|error|sql_error("renew workflow reservation",&error))?;if changed!=1 {return Err(WorkflowError::Conflict);}
            tx.execute("INSERT INTO workflow_rounds(run_id,round,owner_id) VALUES(?1,?2,?3)",params![run.to_string(),round,session_guard.owner.to_string()]).map_err(|error|sql_error("append explicit retry round",&error))?;
            event(tx,run,&WorkflowEvent {version:crate::decision::CONTRACT_VERSION,round,kind:WorkflowEventKind::RoundStarted,progress:None,node:None,node_status:None,run_status:Some(RunStatus::Running),attempt:None,turn:None})?;
            for (node,progress) in old_nodes.iter().enumerate() {
                if matches!(progress.status,NodeStatus::Completed|NodeStatus::Skipped) {continue;}
                let name=&stored.frozen.definition.request.workflow.nodes[node].id;
                let pending=NodeProgress {status:NodeStatus::Pending,output:None,error:None,ancestors:vec![],inputs:vec![]};
                event(tx,run,&WorkflowEvent {version:crate::decision::CONTRACT_VERSION,round,kind:WorkflowEventKind::Recovered,progress:Some(pending.clone()),node:Some(node),node_status:Some(NodeStatus::Pending),run_status:None,attempt:None,turn:None})?;
                let changed=tx.execute("UPDATE workflow_nodes SET state='pending',turn_id=NULL,payload_toon=?3 WHERE run_id=?1 AND node_id=?2 AND state!='completed'",params![run.to_string(),name,Self::encode_payload(&pending)?]).map_err(|error|sql_error("reset eligible node for explicit retry",&error))?;if changed!=1 {return Err(WorkflowError::Conflict);}
                let changed=tx.execute("UPDATE request_reservations SET owner_id=?2,state='pending' WHERE request_id=?1 AND kind='workflow_node' AND state!='completed'",params![child_key(run,node),session_guard.owner.to_string()]).map_err(|error|sql_error("renew node reservation owner",&error))?;if changed!=1 {return Err(WorkflowError::Conflict);}
            }
            Ok(())
        })?;
        Ok(WorkflowDecision::Accepted(Box::new(WorkflowOwner {
            frozen: stored.frozen,
            round,
            session_guard,
            _run_guard: run_guard,
            _request_guard: request_guard,
        })))
    }
}
