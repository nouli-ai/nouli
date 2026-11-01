//! Shared immutable turn publication inside caller-owned short transactions.
use super::{
    Ledger, StorageError, sql_error,
    turns::{AttemptProgress, AttemptStatus, progress_event},
};
use crate::{history::FinalizedTurn, identity::Id};
use rusqlite::{OptionalExtension, Transaction, params};

pub(super) fn insert_turn(
    tx: &Transaction<'_>,
    turn: &FinalizedTurn,
    attempt: Id,
    ordinal: i64,
) -> Result<(), StorageError> {
    const INSERT_CONTEXT_RECORD: &str =
        "INSERT INTO context_records(id,turn_id,record_key,payload_toon) VALUES(?1,?2,?3,?4)";
    const COMPLETE_ATTEMPT: &str = "UPDATE attempt_states SET state='completed',finalized_turn=?2 WHERE attempt_id=?1 AND state IN ('pending','running')";

    if turn.runtime_only {
        return Err(StorageError::Payload);
    }
    insert_context_base(tx, turn)?;
    tx.execute(
        "INSERT INTO turns(id,session_id,ordinal,payload_toon) VALUES(?1,?2,?3,?4)",
        params![
            turn.id.to_string(),
            turn.session.to_string(),
            ordinal,
            Ledger::encode_payload(turn)?
        ],
    )
    .map_err(|error| sql_error("insert finalized turn", &error))?;
    for (position, parent) in (0_i64..).zip(&turn.parents) {
        tx.execute(
            "INSERT INTO turn_parents(turn_id,parent_id,position) VALUES(?1,?2,?3)",
            params![turn.id.to_string(), parent.to_string(), position],
        )
        .map_err(|error| sql_error("record ancestry", &error))?;
    }
    for (position, update) in (0_i64..).zip(&turn.updates) {
        tx.execute(
            "INSERT INTO context_events(turn_id,position,payload_toon) VALUES(?1,?2,?3)",
            params![
                turn.id.to_string(),
                position,
                Ledger::encode_payload(update)?
            ],
        )
        .map_err(|error| sql_error("record context event", &error))?;
        if let Some(record) = update.new_record() {
            tx.execute(
                INSERT_CONTEXT_RECORD,
                params![
                    record.id.to_string(),
                    turn.id.to_string(),
                    record.key,
                    Ledger::encode_payload(record)?
                ],
            )
            .map_err(|error| sql_error("insert context record", &error))?;
        }
    }
    tx.execute(
        "INSERT INTO manifests(turn_id,payload_toon) VALUES(?1,?2)",
        params![turn.id.to_string(), Ledger::encode_payload(&turn.manifest)?],
    )
    .map_err(|error| sql_error("record manifest", &error))?;
    let changed = tx
        .execute(
            COMPLETE_ATTEMPT,
            params![attempt.to_string(), turn.id.to_string()],
        )
        .map_err(|error| sql_error("complete attempt", &error))?;
    if changed != 1 {
        return Err(StorageError::Conflict);
    }
    progress_event(
        tx,
        attempt,
        &AttemptProgress {
            state: AttemptStatus::Completed,
            error: None,
        },
    )?;
    Ok(())
}

fn insert_context_base(tx: &Transaction<'_>, turn: &FinalizedTurn) -> Result<(), StorageError> {
    const PUBLISH_SHARED_CONTEXT_RECORD: &str =
        "INSERT INTO context_records(id,base_id,record_key,payload_toon) VALUES(?1,?2,?3,?4)";
    if let Some(base) = &turn.context_base {
        let payload = Ledger::encode_payload(base)?;
        let existing: Option<String> = tx
            .query_row(
                "SELECT payload_toon FROM context_bases WHERE id=?1",
                [base.id.to_string()],
                |row| row.get(0),
            )
            .optional()
            .map_err(|error| sql_error("read shared context base", &error))?;
        if let Some(existing) = existing {
            if existing != payload {
                return Err(StorageError::Conflict);
            }
        } else {
            tx.execute(
                "INSERT INTO context_bases(id,session_id,payload_toon) VALUES(?1,?2,?3)",
                params![base.id.to_string(), base.session.to_string(), payload],
            )
            .map_err(|error| sql_error("publish shared context base", &error))?;
            for update in &base.updates {
                if let Some(record) = update.new_record() {
                    tx.execute(
                        PUBLISH_SHARED_CONTEXT_RECORD,
                        params![
                            record.id.to_string(),
                            base.id.to_string(),
                            record.key,
                            Ledger::encode_payload(record)?
                        ],
                    )
                    .map_err(|error| sql_error("publish shared context record", &error))?;
                }
            }
        }
    }
    Ok(())
}
