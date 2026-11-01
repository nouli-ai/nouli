//! Blocking local ledger primitives. Call from an owned blocking worker when
//! embedding in an async runtime; transactions never span a provider await.

pub mod context;
pub mod decisions;
pub mod guards;
pub mod inspection;
mod publication;
pub mod replay;
pub mod reservations;
mod schema;
pub mod sessions;
pub mod sources;
pub mod turns;
pub mod worker;
pub mod workflows;

use std::io::Read;
use std::{
    fs::{self, OpenOptions},
    path::{Path, PathBuf},
    time::Duration,
};

use rusqlite::{Connection, OpenFlags, Transaction, TransactionBehavior};
use serde::{Serialize, de::DeserializeOwned};
use thiserror::Error;

use crate::{
    codec::{self, InputFormat},
    config::resolve::{FROZEN_SETTINGS_VERSION, frozen_settings_version},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpenMode {
    ReadWriteCreate,
    ReadOnlyExisting,
}

#[derive(Debug, Error)]
pub enum StorageError {
    #[error("cannot {operation} at {path}: {source}")]
    Io {
        operation: &'static str,
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("SQLite {operation} failed ({code:?})")]
    Sqlite {
        operation: &'static str,
        code: Option<rusqlite::ErrorCode>,
    },
    #[error("ledger is busy")]
    Busy,
    #[error("ledger is read-only")]
    ReadOnly,
    #[error("database is not a Nouli ledger")]
    WrongStore,
    #[error("ledger schema {found} is newer than supported schema {supported}")]
    NewerSchema { found: u32, supported: u32 },
    #[error("read-only ledger requires migration from schema {found}")]
    MigrationRequired { found: u32 },
    #[error("unsupported frozen settings; use a fresh store with --store DIR")]
    FrozenSettings,
    #[error("stored structured payload is invalid")]
    Payload,
    #[error("stored projection changed concurrently")]
    Conflict,
}

pub(super) fn sql_error(operation: &'static str, error: &rusqlite::Error) -> StorageError {
    let code = error.sqlite_error_code();
    if matches!(
        code,
        Some(rusqlite::ErrorCode::DatabaseBusy | rusqlite::ErrorCode::DatabaseLocked)
    ) {
        StorageError::Busy
    } else {
        StorageError::Sqlite { operation, code }
    }
}

/// Validate frozen settings without choosing a caller's error category.
fn freeze_settings(
    settings: &crate::config::resolve::SettingsSnapshot,
) -> Result<(), crate::config::ConfigError> {
    settings.profile.validate()?;
    settings.execution.validate()?;
    Ok(())
}

fn freeze_store(
    ledger: &Ledger,
    operation: &'static str,
) -> Result<std::path::PathBuf, StorageError> {
    std::fs::canonicalize(ledger.path().parent().ok_or(StorageError::Payload)?).map_err(|source| {
        StorageError::Io {
            operation,
            path: ledger.path().to_owned(),
            source,
        }
    })
}

/// Validate a value known by its owning loader to hold frozen settings.
fn validate_frozen_settings_version(value: &serde_json::Value) -> Result<(), StorageError> {
    let version = frozen_settings_version(value).map_err(|_| StorageError::Payload)?;
    if version != FROZEN_SETTINGS_VERSION {
        return Err(StorageError::FrozenSettings);
    }
    Ok(())
}

pub struct Ledger {
    connection: Connection,
    path: PathBuf,
    mode: OpenMode,
}

fn prepare_store(root: &Path, path: &Path) -> Result<(), StorageError> {
    let mut directory = fs::DirBuilder::new();
    directory.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        directory.mode(0o700);
    }
    directory.create(root).map_err(|source| StorageError::Io {
        operation: "create store directory",
        path: root.to_owned(),
        source,
    })?;
    let mut file = OpenOptions::new();
    file.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        file.mode(0o600);
    }
    match file.open(path) {
        Ok(_) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => Ok(()),
        Err(source) => Err(StorageError::Io {
            operation: "create ledger",
            path: path.to_owned(),
            source,
        }),
    }
}

fn check_read_only_header(path: &Path) -> Result<(), StorageError> {
    let mut file = fs::File::open(path).map_err(|source| StorageError::Io {
        operation: "read ledger header",
        path: path.to_owned(),
        source,
    })?;
    let mut header = [0_u8; 20];
    file.read_exact(&mut header)
        .map_err(|source| StorageError::Io {
            operation: "read ledger header",
            path: path.to_owned(),
            source,
        })?;
    // SQLite file format offsets18/19: rollback=1, WAL=2. Reject before
    // SQLite can create read-only WAL/SHM sidecars for an unsupported store.
    if &header[..16] != b"SQLite format 3\0" || header[18] != 1 || header[19] != 1 {
        return Err(StorageError::WrongStore);
    }
    Ok(())
}

impl Ledger {
    /// Explicit root selection; opening read-only never creates or migrates it.
    ///
    /// # Errors
    ///
    /// - [`StorageError::Io`] if the store directory or ledger file cannot be
    ///   created, or a read-only ledger's header cannot be read.
    /// - [`StorageError::WrongStore`] if the file is not a Nouli ledger in
    ///   rollback-journal mode.
    /// - [`StorageError::NewerSchema`] if the schema is newer than supported, or
    ///   [`StorageError::MigrationRequired`] if a read-only ledger is outdated.
    /// - [`StorageError::Busy`] or [`StorageError::Sqlite`] if opening,
    ///   configuring, or migrating the database fails.
    pub fn open(root: &Path, mode: OpenMode) -> Result<Self, StorageError> {
        let path = root.join("ledger.sqlite3");
        if mode == OpenMode::ReadWriteCreate {
            prepare_store(root, &path)?;
        } else {
            check_read_only_header(&path)?;
        }
        let flags = match mode {
            OpenMode::ReadWriteCreate => OpenFlags::SQLITE_OPEN_READ_WRITE,
            OpenMode::ReadOnlyExisting => OpenFlags::SQLITE_OPEN_READ_ONLY,
        };
        let mut connection =
            Connection::open_with_flags(&path, flags | OpenFlags::SQLITE_OPEN_NO_MUTEX)
                .map_err(|error| sql_error("open", &error))?;
        connection
            .busy_timeout(Duration::from_secs(2))
            .map_err(|error| sql_error("set busy timeout", &error))?;
        connection
            .pragma_update(None, "foreign_keys", true)
            .map_err(|error| sql_error("enable foreign keys", &error))?;
        connection
            .pragma_update(None, "recursive_triggers", true)
            .map_err(|error| sql_error("enable immutable replacement guards", &error))?;
        match mode {
            OpenMode::ReadWriteCreate => {
                schema::migrate(&mut connection, schema::MIGRATIONS)?;
                // DELETE avoids read-only readers creating/checkpointing WAL/SHM
                // sidecars. Short writes still permit independent model calls.
                let journal: String = connection
                    .pragma_update_and_check(None, "journal_mode", "DELETE", |row| row.get(0))
                    .map_err(|error| sql_error("set journal mode", &error))?;
                if !journal.eq_ignore_ascii_case("delete") {
                    return Err(StorageError::WrongStore);
                }
                connection
                    .pragma_update(None, "synchronous", "FULL")
                    .map_err(|error| sql_error("set durability", &error))?;
            }
            OpenMode::ReadOnlyExisting => schema::check_read_only(&connection)?,
        }
        tracing::debug!(schema = schema::VERSION, "ledger opened");
        Ok(Self {
            connection,
            path,
            mode,
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
    pub fn schema_version(&self) -> u32 {
        schema::VERSION
    }
    pub fn mode(&self) -> OpenMode {
        self.mode
    }

    /// Synchronous closures cannot retain a transaction across an async await.
    ///
    /// # Errors
    ///
    /// Returns [`StorageError::ReadOnly`] if the ledger was opened read-only,
    /// [`StorageError::Busy`] or [`StorageError::Sqlite`] if beginning or
    /// committing the transaction fails, and any error returned by `write`.
    pub fn transaction<T, E: From<StorageError>>(
        &mut self,
        operation: &'static str,
        write: impl FnOnce(&Transaction<'_>) -> Result<T, E>,
    ) -> Result<T, E> {
        if self.mode == OpenMode::ReadOnlyExisting {
            return Err(StorageError::ReadOnly.into());
        }
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| E::from(sql_error(operation, &error)))?;
        let result = write(&transaction)?;
        transaction
            .commit()
            .map_err(|error| E::from(sql_error("commit", &error)))?;
        Ok(result)
    }

    /// Full supported data is serialized with the same strict TOON codec used
    /// for model context. Credential values have no Serialize implementation.
    ///
    /// # Errors
    ///
    /// Returns [`StorageError::Payload`] if the value cannot be encoded as TOON.
    pub fn encode_payload<T: Serialize>(value: &T) -> Result<String, StorageError> {
        codec::encode_toon(value).map_err(|_| StorageError::Payload)
    }
    /// Decodes a stored TOON payload into `T`.
    ///
    /// # Errors
    ///
    /// Returns [`StorageError::Payload`] if the TOON payload cannot be decoded
    /// into `T`, including when a direct or nested frozen-settings snapshot is
    /// unsupported. Use [`Ledger::decode_settings`] for typed snapshot decoding
    /// that distinguishes unsupported versions; built-in frozen request and
    /// workflow loaders validate their known settings fields separately.
    pub fn decode_payload<T: DeserializeOwned>(payload: &str) -> Result<T, StorageError> {
        let value = codec::decode_value(payload.as_bytes(), InputFormat::Toon)
            .map_err(|_| StorageError::Payload)?;
        serde_json::from_value(value).map_err(|_| StorageError::Payload)
    }

    /// Decodes a stored frozen-settings snapshot, distinguishing an unsupported
    /// format version from a malformed payload.
    ///
    /// # Errors
    ///
    /// Returns [`StorageError::FrozenSettings`] if the raw snapshot has a
    /// missing or unsupported format version, or [`StorageError::Payload`] if
    /// its syntax, fields, or provider profile cannot be decoded.
    pub fn decode_settings(
        payload: &str,
    ) -> Result<crate::config::resolve::SettingsSnapshot, StorageError> {
        let value = codec::decode_value(payload.as_bytes(), InputFormat::Toon)
            .map_err(|_| StorageError::Payload)?;
        validate_frozen_settings_version(&value)?;
        serde_json::from_value(value).map_err(|_| StorageError::Payload)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::resolve::RuntimeOverrides;
    use crate::history::ModelSelection;
    use crate::identity::Id;

    #[test]
    fn real_legacy_profiles_fail_with_fresh_store_guidance_before_profile_decoding() {
        let legacy = serde_json::json!({"profile_name":"old","profile":{"protocol":"typesafe", "model":"old", "base_url":"https://example.com"}, "execution":crate::config::ExecutionSettings::default(), "output":"auto", "store":"/tmp/old"});
        let payload = Ledger::encode_payload(&legacy).unwrap();
        assert!(matches!(
            Ledger::decode_settings(&payload),
            Err(StorageError::FrozenSettings)
        ));
        assert!(matches!(
            super::workflows::decode_settings(&Ledger::encode_payload(&vec![legacy]).unwrap()),
            Err(StorageError::FrozenSettings)
        ));
    }

    #[test]
    fn unrelated_versions_and_snapshot_shaped_evidence_keep_the_requested_contract() {
        #[derive(serde::Deserialize)]
        struct OtherPayload {
            required: bool,
        }

        #[derive(serde::Deserialize)]
        struct CustomWrapper {
            settings: Vec<crate::config::resolve::SettingsSnapshot>,
        }

        let snapshot = serde_json::json!({
            "format_version": 99, "profile_name": "evidence", "profile": {},
            "execution": crate::config::ExecutionSettings::default(),
            "output": "auto", "store": "/tmp/evidence"
        });
        let root_snapshot = Ledger::encode_payload(&snapshot).unwrap();
        assert!(matches!(
            Ledger::decode_payload::<bool>(&root_snapshot),
            Err(StorageError::Payload)
        ));
        assert!(matches!(
            Ledger::decode_payload::<crate::config::resolve::SettingsSnapshot>(&root_snapshot),
            Err(StorageError::Payload)
        ));
        for value in [
            snapshot.clone(),
            serde_json::json!({"format_version":99,"evidence":snapshot}),
            serde_json::json!({"settings":[snapshot]}),
        ] {
            let encoded = Ledger::encode_payload(&value).unwrap();
            assert_eq!(
                Ledger::decode_payload::<serde_json::Value>(&encoded).unwrap(),
                value
            );
            let error = match Ledger::decode_payload::<OtherPayload>(&encoded) {
                Err(error) => error,
                Ok(payload) => panic!("unexpected success: {}", payload.required),
            };
            assert_eq!(error.report().code, "invalid_saved_payload");
        }

        let wrapper = Ledger::encode_payload(&serde_json::json!({"settings":[snapshot]})).unwrap();
        let error = match Ledger::decode_payload::<CustomWrapper>(&wrapper) {
            Err(error) => error,
            Ok(wrapper) => panic!("unexpected success: {} settings", wrapper.settings.len()),
        };
        assert_eq!(error.report().code, "invalid_saved_payload");
    }

    struct Store(PathBuf);
    impl Store {
        fn new() -> Self {
            Self(std::env::temp_dir().join(format!("nouli-store-{}", Id::generate())))
        }
        fn open(&self) -> Ledger {
            Ledger::open(&self.0, OpenMode::ReadWriteCreate).unwrap()
        }
    }
    impl Drop for Store {
        fn drop(&mut self) {
            if self.0.exists() {
                fs::remove_dir_all(&self.0).unwrap();
            }
        }
    }

    #[test]
    fn reopening_current_writable_store_never_rewrites_the_database() {
        let store = Store::new();
        let ledger = Ledger::open(&store.0, OpenMode::ReadWriteCreate).unwrap();
        drop(ledger);
        let before = fs::read(store.0.join("ledger.sqlite3")).unwrap();
        let ledger = Ledger::open(&store.0, OpenMode::ReadWriteCreate).unwrap();
        drop(ledger);
        assert!(
            before == fs::read(store.0.join("ledger.sqlite3")).unwrap(),
            "Opening an initialized store changed database bytes"
        );
    }
    #[test]
    fn create_reopen_and_read_only_preserve_schema_and_files() {
        let store = Store::new();
        let ledger = store.open();
        assert_eq!(ledger.schema_version(), schema::VERSION);
        let path = ledger.path().to_owned();
        drop(ledger);
        let before = fs::read(&path).unwrap();
        let mut readonly = Ledger::open(&store.0, OpenMode::ReadOnlyExisting).unwrap();
        assert!(matches!(
            readonly.transaction::<_, StorageError>("forbidden", |_| Ok(())),
            Err(StorageError::ReadOnly)
        ));
        drop(readonly);
        assert_eq!(fs::read(&path).unwrap(), before);
        assert_eq!(fs::read_dir(&store.0).unwrap().count(), 1);
        assert_eq!(store.open().schema_version(), schema::VERSION);
    }

    #[test]
    fn missing_read_only_never_creates_a_directory() {
        let store = Store::new();
        assert!(Ledger::open(&store.0, OpenMode::ReadOnlyExisting).is_err());
        assert!(!store.0.exists());
    }

    #[test]
    fn read_only_rejects_wal_before_sqlite_can_create_sidecars() {
        let store = Store::new();
        let ledger = store.open();
        ledger
            .connection
            .pragma_update(None, "journal_mode", "WAL")
            .unwrap();
        drop(ledger);
        let before = fs::read(store.0.join("ledger.sqlite3")).unwrap();
        assert_eq!(fs::read_dir(&store.0).unwrap().count(), 1);
        assert!(matches!(
            Ledger::open(&store.0, OpenMode::ReadOnlyExisting),
            Err(StorageError::WrongStore)
        ));
        assert_eq!(fs::read_dir(&store.0).unwrap().count(), 1);
        assert_eq!(before, fs::read(store.0.join("ledger.sqlite3")).unwrap());
    }

    #[test]
    fn newer_older_and_unrelated_stores_fail_without_migration() {
        let store = Store::new();
        let ledger = store.open();
        ledger
            .connection
            .pragma_update(None, "user_version", schema::VERSION + 1)
            .unwrap();
        drop(ledger);
        let before = fs::read(store.0.join("ledger.sqlite3")).unwrap();
        assert!(matches!(
            Ledger::open(&store.0, OpenMode::ReadOnlyExisting),
            Err(StorageError::NewerSchema { found, .. }) if found==schema::VERSION+1
        ));
        assert!(matches!(
            Ledger::open(&store.0, OpenMode::ReadWriteCreate),
            Err(StorageError::NewerSchema { found, .. }) if found==schema::VERSION+1
        ));
        assert_eq!(before, fs::read(store.0.join("ledger.sqlite3")).unwrap());
        let connection = Connection::open(store.0.join("ledger.sqlite3")).unwrap();
        connection.pragma_update(None, "user_version", 0).unwrap();
        drop(connection);
        assert!(matches!(
            Ledger::open(&store.0, OpenMode::ReadOnlyExisting),
            Err(StorageError::MigrationRequired { found: 0 })
        ));
        let other = Store::new();
        fs::create_dir(&other.0).unwrap();
        let connection = Connection::open(other.0.join("ledger.sqlite3")).unwrap();
        connection
            .execute_batch("CREATE TABLE unrelated(secret TEXT);")
            .unwrap();
        drop(connection);
        let before = fs::read(other.0.join("ledger.sqlite3")).unwrap();
        assert!(matches!(
            Ledger::open(&other.0, OpenMode::ReadWriteCreate),
            Err(StorageError::WrongStore)
        ));
        assert_eq!(before, fs::read(other.0.join("ledger.sqlite3")).unwrap());
    }

    #[test]
    fn short_transactions_enforce_foreign_keys_and_rollback_errors() {
        let store = Store::new();
        let mut ledger = store.open();
        let failure=ledger.transaction::<_, StorageError>("test rollback",|transaction| {
            transaction.execute("INSERT INTO sessions(id,defaults_toon,frontier_toon) VALUES('fixture','{}','[]')",[]).map_err(|error|sql_error("insert test session",&error))?;
            Err::<(),_>(StorageError::Payload)
        });
        assert!(matches!(failure, Err(StorageError::Payload)));
        let count: i64 = ledger
            .connection
            .query_row("SELECT count(*) FROM sessions", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 0);
        assert!(ledger.transaction::<_, StorageError>("foreign key",|transaction| {
            transaction.execute("INSERT INTO turns(id,session_id,ordinal,payload_toon) VALUES('turn','missing',0,'{}')",[]).map_err(|error|sql_error("insert test turn",&error))?;
            Ok(())
        }).is_err());
    }

    #[test]
    fn immutable_rows_reject_update_delete_and_replacement() {
        let store = Store::new();
        let mut ledger = store.open();
        ledger.transaction::<_, StorageError>("insert fixture",|transaction| {
            transaction.execute_batch("INSERT INTO sessions(id,defaults_toon,frontier_toon) VALUES('s','{}','[]'); INSERT INTO turns(id,session_id,ordinal,payload_toon) VALUES('t','s',0,'original');").map_err(|error|sql_error("insert fixtures",&error))?;
            Ok(())
        }).unwrap();
        for sql in [
            "UPDATE turns SET payload_toon='changed' WHERE id='t'",
            "DELETE FROM turns WHERE id='t'",
            "INSERT OR REPLACE INTO turns(id,session_id,ordinal,payload_toon) VALUES('t','s',0,'changed')",
        ] {
            assert!(
                ledger
                    .transaction::<_, StorageError>("immutable write", |transaction| {
                        transaction
                            .execute(sql, [])
                            .map_err(|error| sql_error("immutable test", &error))?;
                        Ok(())
                    })
                    .is_err()
            );
            let payload: String = ledger
                .connection
                .query_row("SELECT payload_toon FROM turns WHERE id='t'", [], |row| {
                    row.get(0)
                })
                .unwrap();
            assert_eq!(payload, "original");
        }
        ledger
            .transaction::<_, StorageError>("mutable projection", |transaction| {
                transaction
                    .execute("UPDATE sessions SET revision=revision+1 WHERE id='s'", [])
                    .map_err(|error| sql_error("update projection", &error))?;
                Ok(())
            })
            .unwrap();
    }

    #[test]
    fn full_width_payloads_and_settings_round_trip_without_credentials() {
        let value = serde_json::json!({"min":i64::MIN,"max":u64::MAX,"fraction":1.43,"null":null,"text":"🦀\nquote: \""});
        let encoded = Ledger::encode_payload(&value).unwrap();
        assert_eq!(
            Ledger::decode_payload::<serde_json::Value>(&encoded).unwrap(),
            value
        );
        let config =
            crate::config::Config::parse(include_str!("../../fixtures/config/providers.toml"))
                .unwrap();
        let settings = config
            .resolve(
                &RuntimeOverrides::default(),
                &ModelSelection::default(),
                &ModelSelection::default(),
                Path::new("/home"),
                |_| Some("SECRET_SENTINEL".into()),
            )
            .unwrap();
        let encoded = Ledger::encode_payload(&settings.snapshot).unwrap();
        assert!(!encoded.contains("SECRET_SENTINEL"));
        let decoded: crate::config::resolve::SettingsSnapshot =
            Ledger::decode_payload(&encoded).unwrap();
        assert_eq!(decoded, settings.snapshot);
        assert_eq!(
            Ledger::decode_settings(&encoded).unwrap(),
            settings.snapshot
        );
    }
}
