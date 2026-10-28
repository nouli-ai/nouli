use std::process::Command;

#[test]
fn help_and_version_work_without_configuration() {
    for (argument, expected) in [("--help", "Usage: nouli"), ("--version", "nouli 0.1.0")] {
        let output = Command::new(env!("CARGO_BIN_EXE_nouli"))
            .arg(argument)
            .env_clear()
            .output()
            .unwrap();
        assert!(output.status.success());
        assert!(String::from_utf8(output.stdout).unwrap().contains(expected));
        assert!(output.stderr.is_empty());
    }
}

#[test]
fn unknown_arguments_fail_without_protocol_output() {
    let output = Command::new(env!("CARGO_BIN_EXE_nouli"))
        .arg("--unknown")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    assert!(!output.stderr.is_empty());
}

#[test]
fn settings_flags_validate_without_creating_a_store_or_reading_keys() {
    let home = std::env::temp_dir().join(format!("nouli-cli-home-{}", std::process::id()));
    let store = home.join("alternate-ledger");
    let output = Command::new(env!("CARGO_BIN_EXE_nouli"))
        .args(["--store"])
        .arg(&store)
        .env_clear()
        .env("HOME", &home)
        .output()
        .unwrap();
    assert!(output.status.success());
    assert!(!store.exists());
    assert!(!home.exists());
    let output = Command::new(env!("CARGO_BIN_EXE_nouli"))
        .args(["--config"])
        .arg(home.join("missing.toml"))
        .env_clear()
        .env("HOME", &home)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
}
