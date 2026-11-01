//! Atomic versioned migrations for Nouli-owned databases only.

use super::{StorageError, sql_error};
use rusqlite::{Connection, TransactionBehavior};

pub(super) const VERSION: u32 = 5;
const APPLICATION_ID: i64 = 0x4e4f_554c;
pub(super) const MIGRATIONS: &[&str] = &[
    include_str!("schema/0001.sql"),
    include_str!("schema/0002.sql"),
    include_str!("schema/0003.sql"),
    include_str!("schema/0004.sql"),
    include_str!("schema/0005.sql"),
];

fn identity(connection: &Connection) -> Result<(i64, u32), StorageError> {
    let app = connection
        .pragma_query_value(None, "application_id", |row| row.get(0))
        .map_err(|error| sql_error("read application identity", &error))?;
    let version = connection
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .map_err(|error| sql_error("read schema version", &error))?;
    Ok((app, version))
}

pub(super) fn check_read_only(connection: &Connection) -> Result<(), StorageError> {
    let (app, version) = identity(connection)?;
    if app != APPLICATION_ID {
        return Err(StorageError::WrongStore);
    }
    if version > VERSION {
        return Err(StorageError::NewerSchema {
            found: version,
            supported: VERSION,
        });
    }
    if version < VERSION {
        return Err(StorageError::MigrationRequired { found: version });
    }
    let journal: String = connection
        .pragma_query_value(None, "journal_mode", |row| row.get(0))
        .map_err(|error| sql_error("read journal mode", &error))?;
    if !journal.eq_ignore_ascii_case("delete") {
        return Err(StorageError::WrongStore);
    }
    Ok(())
}

pub(super) fn migrate(
    connection: &mut Connection,
    migrations: &[&str],
) -> Result<(), StorageError> {
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|error| sql_error("begin migration", &error))?;
    let (app, version) = identity(&transaction)?;
    let supported = u32::try_from(migrations.len()).expect("migration count fits in u32");
    if version > supported {
        return Err(StorageError::NewerSchema {
            found: version,
            supported,
        });
    }
    if app != APPLICATION_ID && app != 0 {
        return Err(StorageError::WrongStore);
    }
    if app == 0 || version == 0 {
        let tables: i64 = transaction
            .query_row(
                "SELECT count(*) FROM sqlite_schema WHERE name NOT LIKE 'sqlite_%'",
                [],
                |row| row.get(0),
            )
            .map_err(|error| sql_error("inspect empty ledger", &error))?;
        if tables != 0 || version != 0 {
            return Err(StorageError::WrongStore);
        }
    }
    for (next, sql) in (1_u32..).zip(migrations).skip(version as usize) {
        transaction
            .execute_batch(sql)
            .map_err(|error| sql_error("migrate schema", &error))?;
        transaction
            .pragma_update(None, "user_version", next)
            .map_err(|error| sql_error("set schema version", &error))?;
    }
    if app != APPLICATION_ID {
        transaction
            .pragma_update(None, "application_id", APPLICATION_ID)
            .map_err(|error| sql_error("set ledger identity", &error))?;
    }
    transaction
        .commit()
        .map_err(|error| sql_error("commit migration", &error))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn migrations_advance_atomically_and_rollback_all_intermediate_changes() {
        let mut connection = Connection::open_in_memory().unwrap();
        assert!(
            migrate(
                &mut connection,
                &[
                    MIGRATIONS[0],
                    "CREATE TABLE migration_probe(id); INVALID SQL;"
                ]
            )
            .is_err()
        );
        assert_eq!(identity(&connection).unwrap(), (0, 0));
        let tables: i64 = connection
            .query_row(
                "SELECT count(*) FROM sqlite_schema WHERE type='table'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(tables, 0);
        migrate(&mut connection, &MIGRATIONS[..1]).unwrap();
        migrate(
            &mut connection,
            &[MIGRATIONS[0], "CREATE TABLE migration_probe(id);"],
        )
        .unwrap();
        assert_eq!(identity(&connection).unwrap(), (APPLICATION_ID, 2));
    }

    #[test]
    fn existing_v1_catalog_survives_the_v2_attempt_projection_migration() {
        let mut connection = Connection::open_in_memory().unwrap();
        connection
            .pragma_update(None, "foreign_keys", true)
            .unwrap();
        migrate(&mut connection, &MIGRATIONS[..1]).unwrap();
        connection.execute("INSERT INTO sessions(id,name,defaults_toon,frontier_toon) VALUES('s','kept','{}','[]')",[]).unwrap();
        migrate(&mut connection, MIGRATIONS).unwrap();
        let name: String = connection
            .query_row("SELECT name FROM sessions WHERE id='s'", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(name, "kept");
        assert_eq!(identity(&connection).unwrap().1, VERSION);
    }
    #[test]
    fn existing_v3_context_records_survive_shared_base_migration() {
        const INSERT_CONTEXT_RECORDS: &str = "INSERT INTO context_records(id,turn_id,record_key,payload_toon) VALUES('r','t','budget','value: 18446744073709551615')";

        let mut connection = Connection::open_in_memory().unwrap();
        connection
            .pragma_update(None, "foreign_keys", true)
            .unwrap();
        connection
            .pragma_update(None, "recursive_triggers", true)
            .unwrap();
        migrate(&mut connection, &MIGRATIONS[..3]).unwrap();
        connection
            .execute(
                "INSERT INTO sessions(id,defaults_toon,frontier_toon) VALUES('s','{}','[]')",
                [],
            )
            .unwrap();
        connection.execute("INSERT INTO turns(id,session_id,ordinal,payload_toon) VALUES('t','s',0,'state: retained')",[]).unwrap();
        connection.execute(INSERT_CONTEXT_RECORDS, []).unwrap();
        migrate(&mut connection, MIGRATIONS).unwrap();
        let row: (String, Option<String>, Option<String>, String, String) = connection
            .query_row(
                "SELECT id,turn_id,base_id,record_key,payload_toon FROM context_records",
                [],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                    ))
                },
            )
            .unwrap();
        assert_eq!(
            row,
            (
                "r".into(),
                Some("t".into()),
                None,
                "budget".into(),
                "value: 18446744073709551615".into()
            )
        );
        assert!(
            connection
                .execute(
                    "UPDATE context_records SET record_key='lost' WHERE id='r'",
                    []
                )
                .is_err()
        );
        assert!(
            connection
                .execute("DELETE FROM context_records WHERE id='r'", [])
                .is_err()
        );
        assert!(
            !connection
                .prepare("PRAGMA foreign_key_check")
                .unwrap()
                .exists([])
                .unwrap()
        );
    }
    #[test]
    fn existing_v4_terminal_run_is_backfilled_as_an_immutable_round_result() {
        const INSERT_WORKFLOW_RUNS: &str = "INSERT INTO workflow_runs(id,session_id,definition_toon,settings_toon,state,frontier_toon,result_toon,published_revision) VALUES('r','s','definition','settings','interrupted','frontier','saved-result',3)";
        const SELECT_WORKFLOW_ROUND_RESULTS: &str = "SELECT round,state,frontier_toon,published_revision,payload_toon FROM workflow_round_results WHERE run_id='r'";
        const INSERT_WORKFLOW_ROUND_RESULTS: &str = "INSERT OR REPLACE INTO workflow_round_results(run_id,round,state,frontier_toon,published_revision,payload_toon) VALUES('r',0,'completed','[]',4,'changed')";

        let mut connection = Connection::open_in_memory().unwrap();
        connection
            .pragma_update(None, "foreign_keys", true)
            .unwrap();
        connection
            .pragma_update(None, "recursive_triggers", true)
            .unwrap();
        migrate(&mut connection, &MIGRATIONS[..4]).unwrap();
        connection
            .execute(
                "INSERT INTO sessions(id,defaults_toon,frontier_toon) VALUES('s','{}','[]')",
                [],
            )
            .unwrap();
        connection.execute(INSERT_WORKFLOW_RUNS, []).unwrap();
        connection
            .execute(
                "INSERT INTO workflow_rounds(run_id,round,owner_id) VALUES('r',0,'old-owner')",
                [],
            )
            .unwrap();
        migrate(&mut connection, MIGRATIONS).unwrap();
        let saved: (u32, String, String, i64, String) = connection
            .query_row(SELECT_WORKFLOW_ROUND_RESULTS, [], |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                ))
            })
            .unwrap();
        assert_eq!(
            saved,
            (
                0,
                "interrupted".into(),
                "frontier".into(),
                3,
                "saved-result".into()
            )
        );
        for sql in [
            "UPDATE workflow_round_results SET state='completed'",
            "DELETE FROM workflow_round_results",
            INSERT_WORKFLOW_ROUND_RESULTS,
        ] {
            assert!(connection.execute(sql, []).is_err());
        }
        assert!(
            !connection
                .prepare("PRAGMA foreign_key_check")
                .unwrap()
                .exists([])
                .unwrap()
        );
    }
}
