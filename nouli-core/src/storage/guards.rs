//! Cross-process ownership released by the OS when its file handle closes.

use std::{
    fs::{self, File, OpenOptions, TryLockError},
    path::PathBuf,
};
use thiserror::Error;

use super::{Ledger, OpenMode, StorageError, sessions::SessionError};
use crate::identity::{Id, SessionSelector};

#[derive(Debug, Error)]
pub enum GuardError {
    #[error("another writer owns this operation")]
    WriterActive,
    #[error(transparent)]
    Storage(#[from] StorageError),
    #[error(transparent)]
    Session(#[from] SessionError),
}

pub(super) struct FileGuard {
    _file: File,
}

/// A workflow run has one scheduler owner, independently of its node sessions.
pub struct RunGuard {
    pub run: Id,
    _lock: FileGuard,
}

pub struct SessionGuard {
    pub(super) owner: Id,
    pub(super) session: Id,
    pub(super) ledger_path: PathBuf,
    _lock: FileGuard,
}

pub(super) fn lock(ledger: &Ledger, key: &str) -> Result<FileGuard, GuardError> {
    if ledger.mode() != OpenMode::ReadWriteCreate {
        return Err(StorageError::ReadOnly.into());
    }
    let root = ledger
        .path()
        .parent()
        .ok_or(StorageError::Payload)?
        .join("locks");
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(&root).map_err(|source| StorageError::Io {
        operation: "create guard directory",
        path: root.clone(),
        source,
    })?;
    let path = root.join(format!("{key}.lock"));
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true).truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let file = options.open(&path).map_err(|source| StorageError::Io {
        operation: "open writer guard",
        path: path.clone(),
        source,
    })?;
    match file.try_lock() {
        Ok(()) => Ok(FileGuard { _file: file }),
        Err(TryLockError::WouldBlock) => Err(GuardError::WriterActive),
        Err(TryLockError::Error(source)) => Err(StorageError::Io {
            operation: "lock writer guard",
            path,
            source,
        }
        .into()),
    }
}

/// Probe an existing identity without creating or changing store files.
pub(super) fn is_locked(ledger: &Ledger, key: &str) -> Result<bool, GuardError> {
    let path = ledger
        .path()
        .parent()
        .ok_or(StorageError::Payload)?
        .join("locks")
        .join(format!("{key}.lock"));
    let file = match File::open(&path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(source) => {
            return Err(StorageError::Io {
                operation: "inspect writer guard",
                path,
                source,
            }
            .into());
        }
    };
    match file.try_lock() {
        Ok(()) => Ok(false),
        Err(TryLockError::WouldBlock) => Ok(true),
        Err(TryLockError::Error(source)) => Err(StorageError::Io {
            operation: "probe writer guard",
            path,
            source,
        }
        .into()),
    }
}

pub(super) fn acquire_session(ledger: &Ledger, id: Id) -> Result<SessionGuard, GuardError> {
    let guard = lock(ledger, &format!("session-{id}"))?;
    let ledger_path = fs::canonicalize(ledger.path()).map_err(|source| StorageError::Io {
        operation: "identify guarded ledger",
        path: ledger.path().to_owned(),
        source,
    })?;
    Ok(SessionGuard {
        owner: Id::generate(),
        session: id,
        ledger_path,
        _lock: guard,
    })
}

impl SessionGuard {
    pub fn session(&self) -> Id {
        self.session
    }
    pub(super) fn validate(&self, ledger: &Ledger, session: Id) -> Result<(), StorageError> {
        if ledger.mode() != OpenMode::ReadWriteCreate {
            return Err(StorageError::ReadOnly);
        }
        let path = fs::canonicalize(ledger.path()).map_err(|source| StorageError::Io {
            operation: "identify ledger",
            path: ledger.path().to_owned(),
            source,
        })?;
        if self.session != session || self.ledger_path != path {
            return Err(StorageError::Conflict);
        }
        Ok(())
    }
}

impl Ledger {
    pub fn run_guard(&self, run: Id) -> Result<RunGuard, GuardError> {
        Ok(RunGuard {
            run,
            _lock: lock(self, &format!("run-{run}"))?,
        })
    }
    pub fn session_guard(&self, id: Id) -> Result<SessionGuard, GuardError> {
        self.session(&SessionSelector::Id(id))?;
        acquire_session(self, id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::history::ModelSelection;

    struct Store(PathBuf);
    impl Store {
        fn new() -> Self {
            Self(std::env::temp_dir().join(format!("nouli-guards-{}", Id::generate())))
        }
        fn open(&self) -> Ledger {
            Ledger::open(&self.0, OpenMode::ReadWriteCreate).unwrap()
        }
    }
    impl Drop for Store {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    #[test]
    fn read_only_probe_never_creates_lock_files_and_observes_live_ownership() {
        let store = Store::new();
        let ledger = store.open();
        let reader = Ledger::open(&store.0, OpenMode::ReadOnlyExisting).unwrap();
        let run = Id::generate();
        let key = format!("run-{run}");
        assert!(!is_locked(&reader, &key).unwrap());
        assert!(!store.0.join("locks").exists());
        let owner = ledger.run_guard(run).unwrap();
        let before = fs::read_dir(store.0.join("locks")).unwrap().count();
        assert!(is_locked(&reader, &key).unwrap());
        drop(owner);
        assert!(!is_locked(&reader, &key).unwrap());
        assert_eq!(before, fs::read_dir(store.0.join("locks")).unwrap().count());
    }

    #[test]
    fn independent_handles_serialize_one_session_without_blocking_other_sessions() {
        let store = Store::new();
        let mut first = store.open();
        let a = first
            .create_session(None, ModelSelection::default())
            .unwrap();
        let b = first
            .create_session(None, ModelSelection::default())
            .unwrap();
        let second = store.open();
        let run = Id::generate();
        let run_guard = first.run_guard(run).unwrap();
        assert!(matches!(
            second.run_guard(run),
            Err(GuardError::WriterActive)
        ));
        drop(run_guard);
        assert!(second.run_guard(run).is_ok());
        let guard = first.session_guard(a.id).unwrap();
        assert!(matches!(
            second.session_guard(a.id),
            Err(GuardError::WriterActive)
        ));
        let other = second.session_guard(b.id).unwrap();
        assert!(guard.validate(&second, b.id).is_err());
        let elsewhere = Store::new();
        assert!(guard.validate(&elsewhere.open(), a.id).is_err());
        drop(guard);
        assert!(second.session_guard(a.id).is_ok());
        drop(other);
    }
}
