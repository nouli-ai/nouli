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
    identity::SessionSelector,
    protocol::{ErrorCategory, ErrorReport, OUTCOME_UNKNOWN, OWNER_EXITED},
    storage::{
        Ledger,
        guards::{self, GuardError},
        reservations,
        sessions::Session,
        sql_error,
        turns::{AttemptStatus, interrupt_attempt},
    },
    workflow::{
        NodeStatus, RunStatus,
        schedule::{NodeProgress, ScheduleResult, project_outputs},
    },
};
use rusqlite::{Connection, OptionalExtension, Transaction, params};

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
        .is_some_and(|error| error.code == OUTCOME_UNKNOWN)
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
                code: OWNER_EXITED.into(),
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
        let Some(request_guard) = reservations::lock_request(self, request_key(&first)?).map_err(
            |error| match error {
                reservations::LockError::Fingerprint => WorkflowError::Snapshot,
                reservations::LockError::Guard(error) => WorkflowError::Guard(error),
            },
        )?
        else {
            return Ok(WorkflowDecision::InProgress);
        };
        let Some(run_guard) = unless_writer_active(self.run_guard(run))? else {
            return Ok(WorkflowDecision::InProgress);
        };
        let session_guard = guards::acquire_session(self, first.frozen.definition.session)?;
        let stored = self.workflow_run(run)?;
        if stored.status == RunStatus::Completed {
            return Ok(WorkflowDecision::Saved(Box::new(stored)));
        }
        let session = self.session(&SessionSelector::Id(stored.frozen.definition.session))?;
        ensure_head_unmoved(&session, &stored)?;
        let retry = self.plan_retry_round(run, &stored, session_guard.owner)?;
        self.transaction::<_, WorkflowError>("start explicit workflow retry round", |tx| {
            retry.start(tx)
        })?;
        let round = retry.round;
        Ok(WorkflowDecision::Accepted(Box::new(WorkflowOwner {
            frozen: stored.frozen,
            round,
            session_guard,
            _run_guard: run_guard,
            _request_guard: request_guard,
        })))
    }

    /// Reads everything the next retry round needs before its write
    /// transaction: the round numbers, the run's attempts, and the result the
    /// interrupted round settles to.
    fn plan_retry_round<'a>(
        &self,
        run: Id,
        stored: &'a StoredRun,
        owner: Id,
    ) -> Result<RetryRound<'a>, WorkflowError> {
        let previous = self.last_round(run)?;
        let round = previous.checked_add(1).ok_or(WorkflowError::Snapshot)?;
        let attempts = attempts(self, &self.connection, run)?;
        let settled_nodes = settled(stored.nodes.clone());
        let graph = crate::workflow::compile::compile(&stored.frozen.definition.request.workflow)?;
        let (start_order, completion_order) = super::round_order(self, run, previous)?;
        let previous_result = if let Some(result) = &stored.result {
            result.clone()
        } else {
            ScheduleResult {
                status: RunStatus::Interrupted,
                outputs: project_outputs(&graph, &settled_nodes)
                    .map_err(|_| WorkflowError::Snapshot)?,
                nodes: settled_nodes.clone(),
                start_order,
                completion_order,
            }
        };
        Ok(RetryRound {
            run,
            key: request_key(stored)?,
            stored,
            owner,
            previous,
            round,
            attempts,
            settled_nodes,
            previous_result,
        })
    }

    fn last_round(&self, run: Id) -> Result<u32, WorkflowError> {
        let previous: i64 = self
            .connection
            .query_row(
                "SELECT MAX(round) FROM workflow_rounds WHERE run_id=?1",
                [run.to_string()],
                |row| row.get(0),
            )
            .map_err(|error| sql_error("read last retry round", &error))?;
        u32::try_from(previous).map_err(|_| WorkflowError::Snapshot)
    }
}

fn request_key(run: &StoredRun) -> Result<&str, WorkflowError> {
    run.frozen
        .definition
        .request
        .request_id
        .as_deref()
        .ok_or(WorkflowError::Snapshot)
}

/// Treats an active writer as "someone else is resuming this run" rather than
/// as an error.
fn unless_writer_active<G>(guard: Result<G, GuardError>) -> Result<Option<G>, WorkflowError> {
    match guard {
        Ok(guard) => Ok(Some(guard)),
        Err(GuardError::WriterActive) => Ok(None),
        Err(error) => Err(error.into()),
    }
}

fn ensure_head_unmoved(session: &Session, stored: &StoredRun) -> Result<(), WorkflowError> {
    if session.revision != stored.published_revision || session.frontier != stored.frontier {
        return Err(WorkflowError::AdvancedHead);
    }
    Ok(())
}

fn attempt_recovered(round: u32, node: usize, attempt: Id) -> WorkflowEvent {
    WorkflowEvent {
        version: crate::decision::CONTRACT_VERSION,
        round,
        kind: WorkflowEventKind::Recovered,
        progress: None,
        node: Some(node),
        node_status: Some(NodeStatus::Interrupted),
        run_status: None,
        attempt: Some(attempt),
        turn: None,
    }
}

fn node_recovered(round: u32, node: usize, progress: NodeProgress) -> WorkflowEvent {
    WorkflowEvent {
        version: crate::decision::CONTRACT_VERSION,
        round,
        kind: WorkflowEventKind::Recovered,
        node_status: Some(progress.status),
        progress: Some(progress),
        node: Some(node),
        run_status: None,
        attempt: None,
        turn: None,
    }
}

fn run_event(round: u32, kind: WorkflowEventKind, status: RunStatus) -> WorkflowEvent {
    WorkflowEvent {
        version: crate::decision::CONTRACT_VERSION,
        round,
        kind,
        progress: None,
        node: None,
        node_status: None,
        run_status: Some(status),
        attempt: None,
        turn: None,
    }
}

/// An explicit retry round planned from a stored run, written by
/// [`RetryRound::start`] inside one transaction.
struct RetryRound<'a> {
    run: Id,
    key: &'a str,
    stored: &'a StoredRun,
    /// The resuming process's owner ID.
    owner: Id,
    /// The round being closed out.
    previous: u32,
    /// The round being started.
    round: u32,
    attempts: Vec<AttemptSnapshot>,
    /// The stored nodes with every unsettled node marked interrupted.
    settled_nodes: Vec<NodeProgress>,
    /// The result recorded for the previous round if it has none yet.
    previous_result: ScheduleResult,
}

impl RetryRound<'_> {
    fn start(&self, tx: &Transaction<'_>) -> Result<(), WorkflowError> {
        self.verify_unchanged(tx)?;
        self.interrupt_orphan_attempts(tx)?;
        self.preserve_previous_round(tx)?;
        self.renew_run_ownership(tx)?;
        self.reset_unsettled_nodes(tx)
    }

    /// Rechecks, under the write lock, that the session head has not moved and
    /// that the request still points at this run.
    fn verify_unchanged(&self, tx: &Transaction<'_>) -> Result<(), WorkflowError> {
        let current = super::sessions::read_session(
            tx,
            &SessionSelector::Id(self.stored.frozen.definition.session),
        )?;
        ensure_head_unmoved(&current, self.stored)?;
        let saved_run = super::request_run(
            tx,
            self.key,
            &super::fingerprint(&self.stored.frozen.definition.request)?,
        )?;
        if saved_run != Some(self.run) {
            return Err(WorkflowError::Snapshot);
        }
        Ok(())
    }

    fn interrupt_orphan_attempts(&self, tx: &Transaction<'_>) -> Result<(), WorkflowError> {
        for attempt in &self.attempts {
            if !matches!(
                attempt.progress.state,
                AttemptStatus::Pending | AttemptStatus::Running
            ) {
                continue;
            }
            let scope = attempt
                .intent
                .workflow
                .as_ref()
                .ok_or(WorkflowError::Snapshot)?;
            if !interrupt_attempt(
                tx,
                attempt.intent.id,
                attempt.progress.state,
                "Previous workflow owner exited",
            )? {
                continue;
            }
            event(
                tx,
                self.run,
                &attempt_recovered(self.previous, scope.node, attempt.intent.id),
            )?;
        }
        Ok(())
    }

    /// Records the previous round's interrupted result and node outcomes,
    /// unless that round already settled with a result of its own.
    fn preserve_previous_round(&self, tx: &Transaction<'_>) -> Result<(), WorkflowError> {
        let old: Option<String> = tx
            .query_row(
                "SELECT payload_toon FROM workflow_round_results WHERE run_id=?1 AND round=?2",
                params![self.run.to_string(), self.previous],
                |row| row.get(0),
            )
            .optional()
            .map_err(|error| sql_error("inspect prior round result", &error))?;
        if old.is_some() {
            return Ok(());
        }
        tx.execute("INSERT INTO workflow_round_results(run_id,round,state,frontier_toon,published_revision,payload_toon) VALUES(?1,?2,'interrupted',?3,?4,?5)",params![self.run.to_string(),self.previous,Ledger::encode_payload(&self.stored.frontier)?,self.stored.published_revision,Ledger::encode_payload(&self.previous_result)?]).map_err(|error|sql_error("preserve interrupted retry round",&error))?;
        for (node, progress) in self.settled_nodes.iter().enumerate() {
            if matches!(
                self.stored.nodes[node].status,
                NodeStatus::Pending | NodeStatus::Running
            ) {
                event(
                    tx,
                    self.run,
                    &node_recovered(self.previous, node, progress.clone()),
                )?;
            }
        }
        event(
            tx,
            self.run,
            &run_event(
                self.previous,
                WorkflowEventKind::RunFinished,
                RunStatus::Interrupted,
            ),
        )?;
        Ok(())
    }

    /// Hands the run and its request reservation to the new owner and opens the
    /// new round.
    fn renew_run_ownership(&self, tx: &Transaction<'_>) -> Result<(), WorkflowError> {
        let changed=tx.execute("UPDATE workflow_runs SET owner_id=?2,state='running',result_toon=NULL WHERE id=?1 AND state!='completed'",params![self.run.to_string(),self.owner.to_string()]).map_err(|error|sql_error("renew workflow owner",&error))?;
        if changed != 1 {
            return Err(WorkflowError::Conflict);
        }
        reservations::reclaim_workflow(tx, self.key, self.owner)?;
        tx.execute(
            "INSERT INTO workflow_rounds(run_id,round,owner_id) VALUES(?1,?2,?3)",
            params![self.run.to_string(), self.round, self.owner.to_string()],
        )
        .map_err(|error| sql_error("append explicit retry round", &error))?;
        event(
            tx,
            self.run,
            &run_event(
                self.round,
                WorkflowEventKind::RoundStarted,
                RunStatus::Running,
            ),
        )?;
        Ok(())
    }

    /// Returns every node that did not complete or skip to pending under the
    /// new owner, so the new round can run it again.
    fn reset_unsettled_nodes(&self, tx: &Transaction<'_>) -> Result<(), WorkflowError> {
        for (node, progress) in self.settled_nodes.iter().enumerate() {
            if matches!(progress.status, NodeStatus::Completed | NodeStatus::Skipped) {
                continue;
            }
            let name = &self.stored.frozen.definition.request.workflow.nodes[node].id;
            let pending = NodeProgress {
                status: NodeStatus::Pending,
                output: None,
                error: None,
                ancestors: vec![],
                inputs: vec![],
            };
            event(
                tx,
                self.run,
                &node_recovered(self.round, node, pending.clone()),
            )?;
            let changed=tx.execute("UPDATE workflow_nodes SET state='pending',turn_id=NULL,payload_toon=?3 WHERE run_id=?1 AND node_id=?2 AND state!='completed'",params![self.run.to_string(),name,Ledger::encode_payload(&pending)?]).map_err(|error|sql_error("reset eligible node for explicit retry",&error))?;
            if changed != 1 {
                return Err(WorkflowError::Conflict);
            }
            reservations::reclaim_node(tx, &child_key(self.run, node), self.owner)?;
        }
        Ok(())
    }
}
