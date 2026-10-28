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
    use std::io::{BufRead, BufReader, Read, Write};
    use std::process::{Child, Command, Stdio};

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
    struct Process(Child);
    impl Drop for Process {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
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

    #[test]
    #[ignore = "subprocess helper invoked by owner_exit_releases_os_guard"]
    fn child_lock_holder() {
        let root = std::env::var_os("NOULI_TEST_GUARD_ROOT").unwrap();
        let id = Id::try_from(std::env::var("NOULI_TEST_GUARD_SESSION").unwrap()).unwrap();
        let ledger = Ledger::open(&PathBuf::from(root), OpenMode::ReadWriteCreate).unwrap();
        let _guard = ledger.session_guard(id).unwrap();
        let mut stdout = std::io::stdout().lock();
        stdout.write_all(b"NOULI_READY\n").unwrap();
        stdout.flush().unwrap();
        let _ = std::io::stdin().read(&mut [0u8; 1]);
    }

    #[test]
    fn owner_exit_releases_os_guard() {
        let store = Store::new();
        let mut ledger = store.open();
        let session = ledger
            .create_session(None, ModelSelection::default())
            .unwrap();
        let mut process = Process(
            Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "storage::guards::tests::child_lock_holder",
                    "--ignored",
                    "--nocapture",
                ])
                .env_clear()
                .env("NOULI_TEST_GUARD_ROOT", &store.0)
                .env("NOULI_TEST_GUARD_SESSION", session.id.to_string())
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .spawn()
                .unwrap(),
        );
        let output = process.0.stdout.take().unwrap();
        let mut ready = false;
        for line in BufReader::new(output).lines() {
            if line.unwrap().contains("NOULI_READY") {
                ready = true;
                break;
            }
        }
        assert!(
            ready,
            "child must own the guard before contention is checked"
        );
        assert!(matches!(
            ledger.session_guard(session.id),
            Err(GuardError::WriterActive)
        ));
        process.0.kill().unwrap();
        process.0.wait().unwrap();
        assert!(ledger.session_guard(session.id).is_ok());
    }
}
