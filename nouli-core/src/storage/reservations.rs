//! Request reservation kinds, ownership locks, and transactional transitions.

use rusqlite::{Connection, OptionalExtension, Transaction, params};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use super::{
    Ledger, StorageError,
    guards::{self, FileGuard, GuardError},
    sql_error,
};
use crate::{codec, identity::Id};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum ReservationKind {
    Evaluation,
    Workflow,
    WorkflowNode,
}

impl ReservationKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::Evaluation => "evaluation",
            Self::Workflow => "workflow",
            Self::WorkflowNode => "workflow_node",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReservationState {
    Pending,
    Completed,
    Interrupted,
}

impl ReservationState {
    pub(super) fn parse(value: &str) -> Result<Self, StorageError> {
        match value {
            "pending" => Ok(Self::Pending),
            "completed" => Ok(Self::Completed),
            "interrupted" => Ok(Self::Interrupted),
            _ => Err(StorageError::Payload),
        }
    }

    pub(super) fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Completed => "completed",
            Self::Interrupted => "interrupted",
        }
    }
}

#[derive(Debug, Error)]
pub(super) enum LockError {
    #[error("cannot fingerprint request lock")]
    Fingerprint,
    #[error(transparent)]
    Guard(#[from] GuardError),
}

pub(super) fn lock_request(ledger: &Ledger, key: &str) -> Result<Option<FileGuard>, LockError> {
    let lock_key = format!(
        "request-{}",
        codec::fingerprint(&key).map_err(|_| LockError::Fingerprint)?
    );
    match guards::lock(ledger, &lock_key) {
        Ok(guard) => Ok(Some(guard)),
        Err(GuardError::WriterActive) => Ok(None),
        Err(error) => Err(error.into()),
    }
}

pub(super) fn matches_kind(
    connection: &Connection,
    key: &str,
    expected: ReservationKind,
) -> Result<Option<bool>, StorageError> {
    let kind: Option<String> = connection
        .query_row(
            "SELECT kind FROM request_reservations WHERE request_id=?1",
            [key],
            |row| row.get(0),
        )
        .optional()
        .map_err(|error| sql_error("read request kind", &error))?;
    Ok(kind.map(|kind| kind == expected.as_str()))
}

pub(super) struct EvaluationRow {
    pub session: String,
    pub fingerprint: String,
    pub state: ReservationState,
    pub frozen: String,
    pub result: Option<String>,
    pub recovery: Option<String>,
}

pub(super) fn evaluation(
    connection: &Connection,
    key: &str,
) -> Result<Option<EvaluationRow>, StorageError> {
    type Row = (
        String,
        String,
        String,
        String,
        Option<String>,
        Option<String>,
    );
    const SQL: &str = "SELECT session_id,fingerprint,state,frozen_toon,result_toon,recovery_kind FROM request_reservations WHERE request_id=?1";
    let row: Option<Row> = connection
        .query_row(SQL, [key], |row| {
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
                row.get(5)?,
            ))
        })
        .optional()
        .map_err(|error| sql_error("read request reservation", &error))?;
    row.map(|(session, fingerprint, state, frozen, result, recovery)| {
        Ok(EvaluationRow {
            session,
            fingerprint,
            state: ReservationState::parse(&state)?,
            frozen,
            result,
            recovery,
        })
    })
    .transpose()
}

pub(super) fn workflow(
    connection: &Connection,
    key: &str,
) -> Result<Option<(bool, String, String)>, StorageError> {
    let row: Option<(String, String, String)> = connection
        .query_row(
            "SELECT kind,fingerprint,frozen_toon FROM request_reservations WHERE request_id=?1",
            [key],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()
        .map_err(|error| sql_error("inspect workflow request", &error))?;
    Ok(row.map(|(kind, hash, frozen)| (kind == ReservationKind::Workflow.as_str(), hash, frozen)))
}

pub(super) fn owner(
    connection: &Connection,
    key: &str,
    session: Id,
) -> Result<Option<(String, ReservationState, String)>, StorageError> {
    const SQL: &str = "SELECT owner_id,state,frozen_toon FROM request_reservations WHERE request_id=?1 AND session_id=?2";
    let row: Option<(String, String, String)> = connection
        .query_row(SQL, params![key, session.to_string()], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        })
        .optional()
        .map_err(|error| sql_error("validate request owner", &error))?;
    row.map(|(owner, state, frozen)| Ok((owner, ReservationState::parse(&state)?, frozen)))
        .transpose()
}

pub(super) fn session_requests(
    connection: &Connection,
    session: Id,
) -> Result<Vec<(String, ReservationState, String)>, StorageError> {
    const SQL: &str = "SELECT request_id,state,frozen_toon FROM request_reservations WHERE kind='evaluation' AND session_id=?1 ORDER BY rowid";
    let mut statement = connection
        .prepare(SQL)
        .map_err(|error| sql_error("list accepted session requests", &error))?;
    let rows = statement
        .query_map([session.to_string()], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })
        .map_err(|error| sql_error("read accepted session requests", &error))?;
    rows.map(|row| {
        let (key, state, frozen) =
            row.map_err(|error| sql_error("decode accepted session request", &error))?;
        Ok((key, ReservationState::parse(&state)?, frozen))
    })
    .collect()
}

pub(super) fn referenced_request(
    connection: &Connection,
    key: &str,
) -> Result<Option<(ReservationState, String)>, StorageError> {
    const SQL: &str = "SELECT state,frozen_toon FROM request_reservations WHERE kind='evaluation' AND request_id=?1";
    let row: Option<(String, String)> = connection
        .query_row(SQL, [key], |row| Ok((row.get(0)?, row.get(1)?)))
        .optional()
        .map_err(|error| sql_error("read referenced accepted request", &error))?;
    row.map(|(state, frozen)| Ok((ReservationState::parse(&state)?, frozen)))
        .transpose()
}

pub(super) fn interrupt_evaluation(
    tx: &Transaction<'_>,
    key: &str,
    recovery: &str,
) -> Result<(), StorageError> {
    const SQL: &str = "UPDATE request_reservations SET state='interrupted',recovery_kind=?2 WHERE request_id=?1 AND state='pending'";
    tx.execute(SQL, params![key, recovery])
        .map_err(|error| sql_error("record interrupted request", &error))?;
    Ok(())
}

pub(super) fn interrupt_workflow(tx: &Transaction<'_>, key: &str) -> Result<(), StorageError> {
    const SQL: &str = "UPDATE request_reservations SET state='interrupted' WHERE request_id=?1 AND state='pending'";
    tx.execute(SQL, [key])
        .map_err(|error| sql_error("interrupt workflow reservation", &error))?;
    Ok(())
}

fn require_changed(changed: usize) -> Result<(), StorageError> {
    if changed != 1 {
        return Err(StorageError::Conflict);
    }
    Ok(())
}

pub(super) fn reclaim_evaluation(
    tx: &Transaction<'_>,
    key: &str,
    owner: Id,
) -> Result<(), StorageError> {
    const SQL: &str = "UPDATE request_reservations SET owner_id=?2,state='pending',recovery_kind=NULL WHERE request_id=?1 AND state='interrupted'";
    let changed = tx
        .execute(SQL, params![key, owner.to_string()])
        .map_err(|error| sql_error("reserve explicit retry", &error))?;
    require_changed(changed)
}

pub(super) fn insert_evaluation(
    tx: &Transaction<'_>,
    session: Id,
    key: &str,
    hash: &str,
    owner: Id,
    frozen: &str,
) -> Result<(), StorageError> {
    const SQL: &str = "INSERT INTO request_reservations(session_id,request_id,fingerprint,owner_id,state,frozen_toon) VALUES(?1,?2,?3,?4,'pending',?5)";
    tx.execute(
        SQL,
        params![session.to_string(), key, hash, owner.to_string(), frozen],
    )
    .map_err(|error| sql_error("reserve unique request", &error))?;
    Ok(())
}

pub(super) fn insert_workflow(
    tx: &Transaction<'_>,
    session: Id,
    key: &str,
    hash: &str,
    owner: Id,
    frozen: &str,
) -> Result<(), StorageError> {
    const SQL: &str = "INSERT INTO request_reservations(session_id,request_id,fingerprint,owner_id,state,frozen_toon,kind) VALUES(?1,?2,?3,?4,'pending',?5,'workflow')";
    tx.execute(
        SQL,
        params![session.to_string(), key, hash, owner.to_string(), frozen],
    )
    .map_err(|error| sql_error("reserve workflow request", &error))?;
    Ok(())
}

pub(super) fn insert_node(
    tx: &Transaction<'_>,
    session: Id,
    key: &str,
    hash: &str,
    owner: Id,
    frozen: &str,
) -> Result<(), StorageError> {
    const SQL: &str = "INSERT INTO request_reservations(session_id,request_id,fingerprint,owner_id,state,frozen_toon,kind) VALUES(?1,?2,?3,?4,'pending',?5,'workflow_node')";
    tx.execute(
        SQL,
        params![session.to_string(), key, hash, owner.to_string(), frozen],
    )
    .map_err(|error| sql_error("reserve stable node identity", &error))?;
    Ok(())
}

pub(super) fn complete_evaluation(
    tx: &Transaction<'_>,
    key: &str,
    payload: &str,
    owner: Id,
    session: Id,
) -> Result<(), StorageError> {
    const SQL: &str = "UPDATE request_reservations SET state='completed',result_toon=?2 WHERE request_id=?1 AND owner_id=?3 AND session_id=?4 AND state='pending'";
    let changed = tx
        .execute(
            SQL,
            params![key, payload, owner.to_string(), session.to_string()],
        )
        .map_err(|error| sql_error("complete request reservation", &error))?;
    require_changed(changed)
}

pub(super) fn complete_node(
    tx: &Transaction<'_>,
    key: &str,
    payload: &str,
    owner: Id,
) -> Result<(), StorageError> {
    const SQL: &str = "UPDATE request_reservations SET state='completed',result_toon=?2 WHERE request_id=?1 AND kind='workflow_node' AND state='pending' AND owner_id=?3";
    let changed = tx
        .execute(SQL, params![key, payload, owner.to_string()])
        .map_err(|error| sql_error("complete stable node identity", &error))?;
    require_changed(changed)
}

pub(super) fn finish_workflow(
    tx: &Transaction<'_>,
    key: &str,
    state: ReservationState,
    payload: &str,
    owner: Id,
) -> Result<(), StorageError> {
    const SQL: &str = "UPDATE request_reservations SET state=?2,result_toon=?3 WHERE request_id=?1 AND state='pending' AND owner_id=?4";
    let changed = tx
        .execute(
            SQL,
            params![key, state.as_str(), payload, owner.to_string()],
        )
        .map_err(|error| sql_error("save workflow replay result", &error))?;
    require_changed(changed)
}

pub(super) fn reclaim_workflow(
    tx: &Transaction<'_>,
    key: &str,
    owner: Id,
) -> Result<(), StorageError> {
    const SQL: &str = "UPDATE request_reservations SET owner_id=?2,state='pending' WHERE request_id=?1 AND kind='workflow' AND state!='completed'";
    let changed = tx
        .execute(SQL, params![key, owner.to_string()])
        .map_err(|error| sql_error("renew workflow reservation", &error))?;
    require_changed(changed)
}

pub(super) fn reclaim_node(tx: &Transaction<'_>, key: &str, owner: Id) -> Result<(), StorageError> {
    const SQL: &str = "UPDATE request_reservations SET owner_id=?2,state='pending' WHERE request_id=?1 AND kind='workflow_node' AND state!='completed'";
    let changed = tx
        .execute(SQL, params![key, owner.to_string()])
        .map_err(|error| sql_error("renew node reservation owner", &error))?;
    require_changed(changed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reservation_names_keep_their_stored_and_json_spellings() {
        for (state, text) in [
            (ReservationState::Pending, "pending"),
            (ReservationState::Completed, "completed"),
            (ReservationState::Interrupted, "interrupted"),
        ] {
            assert_eq!(state.as_str(), text);
            assert_eq!(ReservationState::parse(text).unwrap(), state);
            assert_eq!(serde_json::to_value(state).unwrap(), text);
            assert_eq!(
                serde_json::from_value::<ReservationState>(serde_json::json!(text)).unwrap(),
                state
            );
        }
        for (kind, text) in [
            (ReservationKind::Evaluation, "evaluation"),
            (ReservationKind::Workflow, "workflow"),
            (ReservationKind::WorkflowNode, "workflow_node"),
        ] {
            assert_eq!(kind.as_str(), text);
            assert_eq!(serde_json::to_value(kind).unwrap(), text);
        }
    }
}
