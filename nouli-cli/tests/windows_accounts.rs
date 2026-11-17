//! Manual two-account acceptance checks. Run the three ignored tests in order
//! through Cargo under the VM accounts documented in docs/windows.md.
#![cfg(windows)]

#[path = "support/security.rs"]
#[allow(dead_code, reason = "this acceptance check uses only the ACL setter")]
mod security;

use nouli_core::{
    history::ModelSelection,
    storage::{Ledger, OpenMode, private_file},
};
use std::{fs, io, path::PathBuf};

const MARKER: &[u8] = b"Nouli two-account acceptance fixture v1";

fn root() -> PathBuf {
    let root = PathBuf::from(std::env::var_os("NOULI_TEST_ACCOUNTS_ROOT").expect("fixture path"));
    assert!(root.is_absolute());
    assert!(
        root.file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with("nouli-acl-")
    );
    root
}

#[test]
#[ignore = "requires administrator account; see docs/windows.md"]
fn administrator_creates_private_artifacts_under_public_parent() {
    let root = root();
    fs::create_dir(&root).unwrap(); // Refuse to overwrite an existing directory.
    security::set_public(&root);
    fs::write(root.join("fixture-marker"), MARKER).unwrap();
    let auth = root.join("credentials/auth.toml");
    let _lock = private_file::lock_beside(&auth).unwrap();
    private_file::publish(&auth, b"fixture credential").unwrap();
    let ledger_path = root.join("ledger");
    let mut ledger = Ledger::open(&ledger_path, OpenMode::ReadWriteCreate).unwrap();
    let session = ledger
        .create_session(None, ModelSelection::default())
        .unwrap();
    let _guard = ledger.session_guard(session.id).unwrap();
    fs::write(
        root.join("guard-name"),
        format!("session-{}.lock", session.id),
    )
    .unwrap();
}

fn denied(result: io::Result<impl Sized>) {
    match result {
        Err(error) => assert_eq!(error.kind(), io::ErrorKind::PermissionDenied),
        Ok(_) => panic!("the second account accessed a protected artifact"),
    }
}

#[test]
#[ignore = "requires a distinct standard account; see docs/windows.md"]
fn standard_account_is_denied_private_artifacts() {
    let root = root();
    assert_eq!(fs::read(root.join("fixture-marker")).unwrap(), MARKER);
    // Positive control: access to the shared parent itself is unrestricted.
    let control = root.join("standard-account-control");
    fs::write(&control, b"control").unwrap();
    fs::remove_file(control).unwrap();
    let guard = fs::read_to_string(root.join("guard-name")).unwrap();
    for path in [
        root.join("credentials/auth.toml"),
        root.join("credentials/auth.toml.lock"),
        root.join("ledger/ledger.sqlite3"),
        root.join("ledger/locks").join(guard),
    ] {
        denied(fs::read(&path));
        denied(fs::OpenOptions::new().write(true).open(&path));
    }
    denied(fs::write(root.join("ledger/injected"), b"refused"));
    denied(fs::write(root.join("credentials/injected"), b"refused"));
}

#[test]
#[ignore = "requires administrator account after the denial check; see docs/windows.md"]
fn administrator_cleans_up_account_fixture() {
    let root = root();
    assert_eq!(fs::read(root.join("fixture-marker")).unwrap(), MARKER);
    fs::remove_dir_all(root).unwrap();
}
