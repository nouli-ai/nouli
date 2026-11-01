//! Stable durable event ordering and structural references across retry rounds.
use super::{WorkflowError, WorkflowEvent};
use crate::{
    decision::CONTRACT_VERSION,
    identity::Id,
    storage::{Ledger, sql_error},
};
use serde::{Deserialize, Serialize};
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StoredEvent {
    pub sequence: u64,
    pub run: Id,
    pub event: WorkflowEvent,
}
impl Ledger {
    /// Lists a workflow run's events in stored sequence order.
    ///
    /// # Errors
    ///
    /// - Returns any [`Ledger::workflow_run`] error for the run.
    /// - Returns [`WorkflowError::Snapshot`] if an event has an unsupported
    ///   version or the sequence has gaps.
    /// - Returns [`WorkflowError::Storage`] if the events cannot be read or
    ///   decoded.
    pub fn workflow_events(&self, run: Id) -> Result<Vec<StoredEvent>, WorkflowError> {
        self.workflow_run(run)?;
        read_events(&self.connection, run)
    }
}
pub(in crate::storage) fn read_events(
    connection: &rusqlite::Connection,
    run: Id,
) -> Result<Vec<StoredEvent>, WorkflowError> {
    let mut statement = connection
        .prepare(
            "SELECT sequence,payload_toon FROM workflow_events WHERE run_id=?1 ORDER BY sequence",
        )
        .map_err(|error| sql_error("list ordered workflow events", error))?;
    let rows = statement
        .query_map([run.to_string()], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
        })
        .map_err(|error| sql_error("read ordered workflow events", error))?;
    let mut events = Vec::new();
    for row in rows {
        let (sequence, payload) =
            row.map_err(|error| sql_error("decode workflow event row", error))?;
        let sequence = u64::try_from(sequence).map_err(|_| WorkflowError::Snapshot)?;
        let event: WorkflowEvent = Ledger::decode_payload(&payload)?;
        if event.version != CONTRACT_VERSION || sequence != events.len() as u64 {
            return Err(WorkflowError::Snapshot);
        }
        events.push(StoredEvent {
            sequence,
            run,
            event,
        });
    }
    Ok(events)
}
