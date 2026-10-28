//! Atomic versioned migrations for Nouli-owned databases only.

use super::{StorageError, sql_error};
use rusqlite::{Connection, TransactionBehavior};

pub(super) const VERSION: u32 = 1;
const APPLICATION_ID: i64 = 0x4e4f554c;
pub(super) const MIGRATIONS: &[&str] = &[include_str!("schema/0001.sql")];

fn identity(connection: &Connection) -> Result<(i64, u32), StorageError> {
    let app = connection
        .pragma_query_value(None, "application_id", |row| row.get(0))
        .map_err(|error| sql_error("read application identity", error))?;
    let version = connection
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .map_err(|error| sql_error("read schema version", error))?;
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
        .map_err(|error| sql_error("read journal mode", error))?;
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
        .map_err(|error| sql_error("begin migration", error))?;
    let (app, version) = identity(&transaction)?;
    let supported = migrations.len() as u32;
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
            .map_err(|error| sql_error("inspect empty ledger", error))?;
        if tables != 0 || version != 0 {
            return Err(StorageError::WrongStore);
        }
    }
    for (index, sql) in migrations.iter().enumerate().skip(version as usize) {
        transaction
            .execute_batch(sql)
            .map_err(|error| sql_error("migrate schema", error))?;
        transaction
            .pragma_update(None, "user_version", (index + 1) as u32)
            .map_err(|error| sql_error("set schema version", error))?;
    }
    transaction
        .pragma_update(None, "application_id", APPLICATION_ID)
        .map_err(|error| sql_error("set ledger identity", error))?;
    transaction
        .commit()
        .map_err(|error| sql_error("commit migration", error))
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
        migrate(&mut connection, MIGRATIONS).unwrap();
        migrate(
            &mut connection,
            &[MIGRATIONS[0], "CREATE TABLE migration_probe(id);"],
        )
        .unwrap();
        assert_eq!(identity(&connection).unwrap(), (APPLICATION_ID, 2));
    }
}
