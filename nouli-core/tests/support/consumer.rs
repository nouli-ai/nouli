//! A standalone downstream application's source, compiled by package_consumer.
use nouli_core::{history::ModelSelection, storage::{Ledger, OpenMode, private_file}};

fn main() {
    let root = std::path::PathBuf::from(std::env::args_os().nth(1).unwrap());
    let mut ledger = Ledger::open(&root, OpenMode::ReadWriteCreate).unwrap();
    let session = ledger.create_session(Some("Downstream".to_owned().try_into().unwrap()), ModelSelection::default()).unwrap();
    let guard = ledger.session_guard(session.id).unwrap();
    assert!(ledger.session_guard(session.id).is_err());
    drop(guard);
    let guard = ledger.session_guard(session.id).unwrap();
    drop(guard);
    drop(ledger);
    let readonly = Ledger::open(&root, OpenMode::ReadOnlyExisting).unwrap();
    assert_eq!(readonly.list_sessions().unwrap().len(), 1);
    drop(readonly);
    let credential = root.join("credentials.toml");
    let _lock = private_file::lock_beside(&credential).unwrap();
    private_file::publish(&credential, b"downstream value").unwrap();
    assert!(private_file::open_read(&credential).is_ok());
}
