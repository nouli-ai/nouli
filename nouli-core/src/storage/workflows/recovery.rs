//! Explicit retry rounds for unfinished workflow runs.
use super::StoredRun;
use super::{
    WorkflowDecision, WorkflowError, WorkflowEvent, WorkflowEventKind, WorkflowOwner, child_key,
    event,
};
use crate::{identity::Id, storage::turns::AttemptSnapshot};
use crate::{
    identity::SessionSelector,
    protocol::{ErrorCategory, ErrorReport, OWNER_EXITED},
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

pub(in crate::storage) use super::inspection::read_round_results;
pub use super::inspection::{
    Disposition, NodeReceipt, RoundResult, RunInspection, WorkflowInspection,
};

pub(super) fn attempts(
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
impl Ledger {
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
        const PRESERVE_INTERRUPTED_RETRY_ROUND: &str = "INSERT INTO workflow_round_results(run_id,round,state,frontier_toon,published_revision,payload_toon) VALUES(?1,?2,'interrupted',?3,?4,?5)";

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
        tx.execute(
            PRESERVE_INTERRUPTED_RETRY_ROUND,
            params![
                self.run.to_string(),
                self.previous,
                Ledger::encode_payload(&self.stored.frontier)?,
                self.stored.published_revision,
                Ledger::encode_payload(&self.previous_result)?
            ],
        )
        .map_err(|error| sql_error("preserve interrupted retry round", &error))?;
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
        const RENEW_WORKFLOW_OWNER: &str = "UPDATE workflow_runs SET owner_id=?2,state='running',result_toon=NULL WHERE id=?1 AND state!='completed'";

        let changed = tx
            .execute(
                RENEW_WORKFLOW_OWNER,
                params![self.run.to_string(), self.owner.to_string()],
            )
            .map_err(|error| sql_error("renew workflow owner", &error))?;
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
        const RESET_ELIGIBLE_NODE_FOR_EXPLICIT_RETRY: &str = "UPDATE workflow_nodes SET state='pending',turn_id=NULL,payload_toon=?3 WHERE run_id=?1 AND node_id=?2 AND state!='completed'";

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
            let changed = tx
                .execute(
                    RESET_ELIGIBLE_NODE_FOR_EXPLICIT_RETRY,
                    params![
                        self.run.to_string(),
                        name,
                        Ledger::encode_payload(&pending)?
                    ],
                )
                .map_err(|error| sql_error("reset eligible node for explicit retry", &error))?;
            if changed != 1 {
                return Err(WorkflowError::Conflict);
            }
            reservations::reclaim_node(tx, &child_key(self.run, node), self.owner)?;
        }
        Ok(())
    }
}
