//! Read-only workflow inspection and immutable results of settled retry rounds.
use super::{StoredRun, WorkflowError, recovery::attempts};
use crate::{
    identity::{Id, SessionSelector},
    protocol::OUTCOME_UNKNOWN,
    storage::{
        Ledger, guards, sql_error,
        turns::{AttemptSnapshot, AttemptStatus},
    },
    workflow::{
        NodeStatus,
        schedule::{NodeProgress, ScheduleResult},
    },
};
use rusqlite::Connection;
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
    const LIST_WORKFLOW_RETRY_ROUNDS: &str = "SELECT round,frontier_toon,published_revision,payload_toon FROM workflow_round_results WHERE run_id=?1 ORDER BY round";

    let mut statement = connection
        .prepare(LIST_WORKFLOW_RETRY_ROUNDS)
        .map_err(|error| sql_error("list workflow retry rounds", &error))?;
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
}
