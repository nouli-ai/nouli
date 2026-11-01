//! Durable catalogs and explicit selectors. Unknown selectors never create data.

mod names;

use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use super::{Ledger, StorageError, sql_error};
use crate::{
    history::{HistoryEvent, ModelSelection, SourcePoint},
    identity::{Id, SessionName, SessionSelector},
};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Session {
    pub id: Id,
    pub name: Option<SessionName>,
    pub defaults: ModelSelection,
    pub frontier: Vec<Id>,
    pub revision: i64,
}

#[derive(Debug, Error)]
pub enum SessionError {
    #[error("session selector does not exist")]
    UnknownSession,
    #[error("session name already exists")]
    NameCollision,
    #[error("automatic session names are exhausted; supply an explicit name")]
    NamesExhausted,
    #[error("session defaults contain an empty provider/model")]
    InvalidDefaults,
    #[error(transparent)]
    Storage(#[from] StorageError),
}

type Row = (String, Option<String>, String, String, i64);

fn decode(row: Row) -> Result<Session, SessionError> {
    let id = Id::try_from(row.0).map_err(|_| StorageError::Payload)?;
    let name = row
        .1
        .map(SessionName::try_from)
        .transpose()
        .map_err(|_| StorageError::Payload)?;
    Ok(Session {
        id,
        name,
        defaults: Ledger::decode_payload(&row.2)?,
        frontier: Ledger::decode_payload(&row.3)?,
        revision: row.4,
    })
}

pub(super) fn validate_defaults(defaults: &ModelSelection) -> Result<(), SessionError> {
    if [&defaults.provider, &defaults.model]
        .iter()
        .any(|value| value.as_ref().is_some_and(|s| s.trim().is_empty()))
    {
        return Err(SessionError::InvalidDefaults);
    }
    Ok(())
}

/// Reports a constraint failure as a name collision only when another session
/// already holds `name`; any other constraint failure stays a storage error.
fn creation_error(
    connection: &rusqlite::Connection,
    session: Id,
    name: Option<&SessionName>,
    error: StorageError,
) -> SessionError {
    let constraint = matches!(
        error,
        StorageError::Sqlite {
            code: Some(rusqlite::ErrorCode::ConstraintViolation),
            ..
        }
    );
    let taken = |name: &SessionName| {
        connection
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM sessions WHERE name=?1 AND id!=?2)",
                params![name.as_str(), session.to_string()],
                |row| row.get::<_, bool>(0),
            )
            .unwrap_or(false)
    };
    if constraint && name.is_some_and(taken) {
        SessionError::NameCollision
    } else {
        SessionError::Storage(error)
    }
}

pub(super) fn insert_session(
    transaction: &rusqlite::Connection,
    session: &mut Session,
    sources: &[SourcePoint],
) -> Result<(), SessionError> {
    names::assign(transaction, session)?;
    let defaults = Ledger::encode_payload(&session.defaults)?;
    let frontier = Ledger::encode_payload(&session.frontier)?;
    transaction
        .execute(
            "INSERT INTO sessions(id,name,defaults_toon,frontier_toon) VALUES(?1,?2,?3,?4)",
            params![
                session.id.to_string(),
                session.name.as_ref().map(SessionName::as_str),
                defaults,
                frontier
            ],
        )
        .map_err(|error| {
            creation_error(
                transaction,
                session.id,
                session.name.as_ref(),
                sql_error("insert session", &error),
            )
        })?;
    let event = Ledger::encode_payload(&HistoryEvent::SessionCreated {
        session: session.id,
        sources: sources.to_vec(),
    })?;
    transaction
        .execute(
            "INSERT INTO session_events(session_id,sequence,payload_toon) VALUES(?1,0,?2)",
            params![session.id.to_string(), event],
        )
        .map_err(|error| sql_error("record session creation", &error))?;
    for (position, source) in (0_i64..).zip(sources) {
        transaction.execute("INSERT INTO session_sources(session_id,position,source_session,frontier_toon) VALUES(?1,?2,?3,?4)",params![session.id.to_string(),position,source.session.to_string(),Ledger::encode_payload(&source.frontier)?]).map_err(|error|sql_error("freeze session source",&error))?;
    }
    Ok(())
}

pub(super) fn read_session(
    connection: &rusqlite::Connection,
    selector: &SessionSelector,
) -> Result<Session, SessionError> {
    let (query, value) = match selector {
        SessionSelector::Id(id) => (
            "SELECT id,name,defaults_toon,frontier_toon,revision FROM sessions WHERE id=?1",
            id.to_string(),
        ),
        SessionSelector::Name(name) => (
            "SELECT id,name,defaults_toon,frontier_toon,revision FROM sessions WHERE name=?1",
            name.as_str().to_owned(),
        ),
    };
    let row: Option<Row> = connection
        .query_row(query, [value], |row| {
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
            ))
        })
        .optional()
        .map_err(|error| sql_error("read session", &error))?;
    decode(row.ok_or(SessionError::UnknownSession)?)
}

pub(super) fn read_sources(
    connection: &rusqlite::Connection,
    id: Id,
) -> Result<Vec<SourcePoint>, SessionError> {
    let mut statement=connection.prepare("SELECT source_session,frontier_toon FROM session_sources WHERE session_id=?1 ORDER BY position").map_err(|error|sql_error("read frozen session sources",&error))?;
    let rows = statement
        .query_map([id.to_string()], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .map_err(|error| sql_error("read source rows", &error))?;
    rows.map(|row| {
        let (session, frontier) = row.map_err(|error| sql_error("decode source row", &error))?;
        Ok(SourcePoint {
            session: Id::try_from(session).map_err(|_| StorageError::Payload)?,
            frontier: Ledger::decode_payload(&frontier)?,
        })
    })
    .collect()
}

impl Ledger {
    /// Returns the frozen source points a session was created from.
    ///
    /// # Errors
    ///
    /// Returns [`SessionError::UnknownSession`] if the session does not exist,
    /// or [`SessionError::Storage`] if the SQLite query fails or a stored row
    /// is invalid.
    pub fn session_sources(&self, id: Id) -> Result<Vec<SourcePoint>, SessionError> {
        self.session(&SessionSelector::Id(id))?;
        read_sources(&self.connection, id)
    }
    /// Creates an empty session, assigning an automatic name when none is given.
    ///
    /// # Errors
    ///
    /// - [`SessionError::InvalidDefaults`] if the provider or model is blank.
    /// - [`SessionError::NameCollision`] if the name is already taken, or
    ///   [`SessionError::NamesExhausted`] if no automatic name is available.
    /// - [`SessionError::Storage`] if the ledger is read-only, encoding fails,
    ///   or the SQLite write fails.
    pub fn create_session(
        &mut self,
        name: Option<SessionName>,
        defaults: ModelSelection,
    ) -> Result<Session, SessionError> {
        validate_defaults(&defaults)?;
        let mut session = Session {
            id: Id::generate(),
            name,
            defaults,
            frontier: vec![],
            revision: 0,
        };
        self.transaction::<_, SessionError>("create session", |transaction| {
            insert_session(transaction, &mut session, &[])
        })?;
        Ok(session)
    }
    /// Reads the session matching an ID or name selector.
    ///
    /// # Errors
    ///
    /// Returns [`SessionError::UnknownSession`] if no session matches, or
    /// [`SessionError::Storage`] if the SQLite query fails or the stored row is
    /// invalid.
    pub fn session(&self, selector: &SessionSelector) -> Result<Session, SessionError> {
        read_session(&self.connection, selector)
    }

    /// Change only the human selector and record the rename atomically.
    /// Keep identity, ancestry, defaults and the accepted-writer revision fixed.
    ///
    /// # Errors
    ///
    /// Returns [`SessionError::UnknownSession`] if the session does not exist,
    /// [`SessionError::NameCollision`] if another session has the name, or
    /// [`SessionError::Storage`] if the ledger is read-only or the SQLite write
    /// fails.
    pub fn rename_session(&mut self, id: Id, name: SessionName) -> Result<Session, SessionError> {
        self.transaction("rename session",|transaction|{
            let mut session=read_session(transaction,&SessionSelector::Id(id))?;
            if session.name.as_ref()==Some(&name){return Ok(session);}
            let event=HistoryEvent::SessionRenamed{session:id,previous_name:session.name.clone(),name:name.clone()};
            transaction.execute("UPDATE sessions SET name=?2 WHERE id=?1",params![id.to_string(),name.as_str()])
                .map_err(|error|creation_error(transaction,id,Some(&name),sql_error("rename session",&error)))?;
            transaction.execute("INSERT INTO session_events(session_id,sequence,payload_toon) SELECT ?1,COALESCE(MAX(sequence),-1)+1,?2 FROM session_events WHERE session_id=?1",params![id.to_string(),Ledger::encode_payload(&event)?])
                .map_err(|error|sql_error("record session rename",&error))?;
            session.name=Some(name);
            Ok(session)
        })
    }
    /// Lists every session in the ledger, ordered by ID.
    ///
    /// # Errors
    ///
    /// Returns [`SessionError::Storage`] if the SQLite query fails or a stored
    /// row is invalid.
    pub fn list_sessions(&self) -> Result<Vec<Session>, SessionError> {
        let mut statement = self
            .connection
            .prepare(
                "SELECT id,name,defaults_toon,frontier_toon,revision FROM sessions ORDER BY id",
            )
            .map_err(|error| sql_error("list sessions", &error))?;
        let rows = statement
            .query_map([], |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                ))
            })
            .map_err(|error| sql_error("list sessions", &error))?;
        rows.map(|row| decode(row.map_err(|error| sql_error("decode session row", &error))?))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::history::StandingChanges;
    use crate::storage::OpenMode;

    #[test]
    fn rename_is_durable_atomic_and_keeps_accepted_work_and_history_by_id() {
        use crate::{history::FinalizedTurn, storage::turns::AttemptInput};
        let root = std::env::temp_dir().join(format!("nouli-rename-{}", Id::generate()));
        let mut ledger = Ledger::open(&root, OpenMode::ReadWriteCreate).unwrap();
        let old = SessionName::try_from("old-name".to_owned()).unwrap();
        let new = SessionName::try_from("new-name".to_owned()).unwrap();
        let session = ledger
            .create_session(
                Some(old.clone()),
                ModelSelection {
                    provider: Some("kept".into()),
                    model: Some("same".into()),
                },
            )
            .unwrap();
        let taken = ledger
            .create_session(
                Some("taken".to_owned().try_into().unwrap()),
                ModelSelection::default(),
            )
            .unwrap();
        let fixture: serde_json::Value =
            serde_json::from_str(include_str!("../../../fixtures/contracts/history.json")).unwrap();
        let template: FinalizedTurn = serde_json::from_value(fixture["turns"][0].clone()).unwrap();
        let input = || AttemptInput {
            request: template.request.clone(),
            updates: vec![],
            standing: StandingChanges::default(),
            manifest: template.manifest.clone(),
            request_id: None,
        };
        let guard = ledger.session_guard(session.id).unwrap();
        let first = ledger.prepare_attempt(&guard, input()).unwrap();
        let first = ledger
            .finalize_attempt(&guard, first.id, template.outcome.clone())
            .unwrap();
        let pending = ledger.prepare_attempt(&guard, input()).unwrap();
        let before = ledger.session(&SessionSelector::Id(session.id)).unwrap();
        let renamed = ledger.rename_session(session.id, new.clone()).unwrap();
        assert_eq!(renamed.id, before.id);
        assert_eq!(renamed.frontier, before.frontier);
        assert_eq!(renamed.defaults, before.defaults);
        assert_eq!(renamed.revision, before.revision);
        assert_eq!(ledger.turn(first.id).unwrap(), first);
        assert!(matches!(
            ledger.session(&SessionSelector::Name(old)),
            Err(SessionError::UnknownSession)
        ));
        assert_eq!(
            ledger.session(&SessionSelector::Name(new.clone())).unwrap(),
            renamed
        );
        let count = |ledger: &Ledger| {
            ledger
                .connection
                .query_row(
                    "SELECT count(*) FROM session_events WHERE session_id=?1",
                    [session.id.to_string()],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap()
        };
        let events = count(&ledger);
        assert_eq!(
            ledger.rename_session(session.id, new.clone()).unwrap(),
            renamed
        );
        assert_eq!(count(&ledger), events, "same-name rename is a no-op");
        assert!(matches!(
            ledger.rename_session(session.id, taken.name.unwrap()),
            Err(SessionError::NameCollision)
        ));
        assert_eq!(count(&ledger), events);
        assert_eq!(
            ledger.session(&SessionSelector::Id(session.id)).unwrap(),
            renamed
        );
        let payload:String=ledger.connection.query_row("SELECT payload_toon FROM session_events WHERE session_id=?1 ORDER BY sequence DESC LIMIT 1",[session.id.to_string()],|row|row.get(0)).unwrap();
        assert!(
            matches!(Ledger::decode_payload::<HistoryEvent>(&payload).unwrap(),HistoryEvent::SessionRenamed{session:renamed_id,..} if renamed_id==session.id)
        );
        ledger.connection.execute_batch("CREATE TEMP TRIGGER reject_rename_event BEFORE INSERT ON session_events BEGIN SELECT RAISE(ABORT,'injected failure'); END;").unwrap();
        assert!(
            ledger
                .rename_session(session.id, "rollback".to_owned().try_into().unwrap())
                .is_err()
        );
        assert_eq!(
            ledger.session(&SessionSelector::Id(session.id)).unwrap(),
            renamed
        );
        ledger
            .connection
            .execute_batch("DROP TRIGGER reject_rename_event;")
            .unwrap();
        ledger
            .finalize_attempt(&guard, pending.id, template.outcome)
            .unwrap();
        assert_eq!(ledger.turn(first.id).unwrap(), first);
        assert!(matches!(
            ledger.rename_session(Id::generate(), new.clone()),
            Err(SessionError::UnknownSession)
        ));
        drop(guard);
        drop(ledger);
        let mut reopened = Ledger::open(&root, OpenMode::ReadOnlyExisting).unwrap();
        assert_eq!(
            reopened
                .session(&SessionSelector::Name(new.clone()))
                .unwrap()
                .id,
            session.id
        );
        assert!(matches!(
            reopened.rename_session(session.id, new),
            Err(SessionError::Storage(StorageError::ReadOnly))
        ));
        drop(reopened);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn non_name_constraint_failures_are_not_reported_as_name_collisions() {
        let root = std::env::temp_dir().join(format!("nouli-constraint-{}", Id::generate()));
        let mut ledger = Ledger::open(&root, OpenMode::ReadWriteCreate).unwrap();
        let session = ledger
            .create_session(None, ModelSelection::default())
            .unwrap();
        let mut duplicate_id = session.clone();
        duplicate_id.name = Some(SessionName::try_from("Unused".to_owned()).unwrap());
        let result = ledger.transaction("insert duplicate id", |transaction| {
            insert_session(transaction, &mut duplicate_id, &[])
        });
        assert!(
            matches!(
                result,
                Err(SessionError::Storage(StorageError::Sqlite {
                    code: Some(rusqlite::ErrorCode::ConstraintViolation),
                    ..
                }))
            ),
            "{result:?}"
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn catalog_reopens_case_sensitive_names_and_unknown_selection_never_creates() {
        let root = std::env::temp_dir().join(format!("nouli-catalog-{}", Id::generate()));
        let mut ledger = Ledger::open(&root, OpenMode::ReadWriteCreate).unwrap();
        let name = SessionName::try_from("Dinner".to_owned()).unwrap();
        let session = ledger
            .create_session(Some(name.clone()), ModelSelection::default())
            .unwrap();
        assert!(matches!(
            ledger.create_session(Some(name.clone()), ModelSelection::default()),
            Err(SessionError::NameCollision)
        ));
        ledger
            .create_session(
                Some(SessionName::try_from("dinner".to_owned()).unwrap()),
                ModelSelection::default(),
            )
            .unwrap();
        assert!(matches!(
            ledger.session(&SessionSelector::Id(Id::generate())),
            Err(SessionError::UnknownSession)
        ));
        assert_eq!(ledger.list_sessions().unwrap().len(), 2);
        drop(ledger);
        let ledger = Ledger::open(&root, OpenMode::ReadOnlyExisting).unwrap();
        assert_eq!(
            ledger.session(&SessionSelector::Name(name)).unwrap(),
            session
        );
        drop(ledger);
        std::fs::remove_dir_all(root).unwrap();
    }
}
