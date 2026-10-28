//! Durable catalogs and explicit selectors. Unknown selectors never create data.

use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use super::{Ledger, StorageError, sql_error};
use crate::{
    history::{HistoryEvent, ModelSelection},
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

impl Ledger {
    pub fn create_session(
        &mut self,
        name: Option<SessionName>,
        defaults: ModelSelection,
    ) -> Result<Session, SessionError> {
        if [&defaults.provider, &defaults.model]
            .iter()
            .any(|value| value.as_ref().is_some_and(|s| s.trim().is_empty()))
        {
            return Err(SessionError::InvalidDefaults);
        }
        let session = Session {
            id: Id::generate(),
            name,
            defaults,
            frontier: vec![],
            revision: 0,
        };
        let default_payload = Self::encode_payload(&session.defaults)?;
        let frontier = Self::encode_payload(&session.frontier)?;
        let event = Self::encode_payload(&HistoryEvent::SessionCreated {
            session: session.id,
            sources: vec![],
        })?;
        self.transaction("create session", |transaction| {
            transaction
                .execute(
                    "INSERT INTO sessions(id,name,defaults_toon,frontier_toon) VALUES(?1,?2,?3,?4)",
                    params![
                        session.id.to_string(),
                        session.name.as_ref().map(SessionName::as_str),
                        default_payload,
                        frontier
                    ],
                )
                .map_err(|error| sql_error("insert session", error))?;
            transaction
                .execute(
                    "INSERT INTO session_events(session_id,sequence,payload_toon) VALUES(?1,0,?2)",
                    params![session.id.to_string(), event],
                )
                .map_err(|error| sql_error("record session creation", error))?;
            Ok(())
        })
        .map_err(|error| {
            if session.name.is_some()
                && matches!(
                    error,
                    StorageError::Sqlite {
                        code: Some(rusqlite::ErrorCode::ConstraintViolation),
                        ..
                    }
                )
            {
                SessionError::NameCollision
            } else {
                SessionError::Storage(error)
            }
        })?;
        Ok(session)
    }

    pub fn session(&self, selector: &SessionSelector) -> Result<Session, SessionError> {
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
        let row: Option<Row> = self
            .connection
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
            .map_err(|error| sql_error("read session", error))?;
        decode(row.ok_or(SessionError::UnknownSession)?)
    }

    pub fn list_sessions(&self) -> Result<Vec<Session>, SessionError> {
        let mut statement = self
            .connection
            .prepare(
                "SELECT id,name,defaults_toon,frontier_toon,revision FROM sessions ORDER BY id",
            )
            .map_err(|error| sql_error("list sessions", error))?;
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
            .map_err(|error| sql_error("list sessions", error))?;
        rows.map(|row| decode(row.map_err(|error| sql_error("decode session row", error))?))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::OpenMode;

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
