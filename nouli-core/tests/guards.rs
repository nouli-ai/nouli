//! Cross-process guard release, in its own test binary.
//!
//! Spawning a child copies every open descriptor until the child execs, and a lock lasts while
//! any copy is open. Keeping the only spawning test out of the unit-test binary keeps the unit
//! tests' drop-then-reacquire assertions deterministic.

use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};

use nouli_core::history::ModelSelection;
use nouli_core::identity::Id;
use nouli_core::storage::guards::GuardError;
use nouli_core::storage::{Ledger, OpenMode};

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
            .args(["--exact", "child_lock_holder", "--ignored", "--nocapture"])
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
    ledger.session_guard(session.id).unwrap();
}
