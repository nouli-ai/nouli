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
    let error: nouli_core::protocol::ResultEnvelope =
        serde_json::from_slice(&output.stdout).unwrap();
    assert!(
        matches!(error.result,nouli_core::protocol::Report::Error(error) if error.category==nouli_core::protocol::ErrorCategory::Configuration)
    );
    assert!(output.stderr.is_empty());
}

#[test]
fn explicit_output_modes_never_emit_placeholder_prose_or_create_history() {
    let home = std::env::temp_dir().join(format!("nouli-render-home-{}", std::process::id()));
    for args in [vec!["--json"], vec!["--jsonl"], vec!["--format", "text"]] {
        let output = Command::new(env!("CARGO_BIN_EXE_nouli"))
            .args(&args)
            .env_clear()
            .env("HOME", &home)
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(2));
        match args[0] {
            "--json" => {
                let result: nouli_core::protocol::ResultEnvelope =
                    serde_json::from_slice(&output.stdout).unwrap();
                assert!(matches!(
                    result.result,
                    nouli_core::protocol::Report::Error(_)
                ));
                assert!(output.stderr.is_empty());
            }
            "--jsonl" => {
                let events = String::from_utf8(output.stdout)
                    .unwrap()
                    .lines()
                    .map(|line| {
                        serde_json::from_str::<nouli_core::protocol::EngineEvent>(line).unwrap()
                    })
                    .collect::<Vec<_>>();
                nouli_core::protocol::validate_events(&events).unwrap();
                assert!(output.stderr.is_empty());
            }
            _ => {
                assert!(output.stdout.is_empty());
                assert!(
                    String::from_utf8(output.stderr)
                        .unwrap()
                        .contains("decision_input_required")
                );
            }
        }
    }
    assert!(!home.exists());
}

#[cfg(unix)]
#[test]
fn closed_help_and_version_outputs_return_the_io_exit_code() {
    use std::{
        os::{fd::OwnedFd, unix::net::UnixStream},
        process::Stdio,
    };
    for argument in ["--help", "--version"] {
        let (writer, reader) = UnixStream::pair().unwrap();
        drop(reader);
        let output = Command::new(env!("CARGO_BIN_EXE_nouli"))
            .arg(argument)
            .env_clear()
            .stdout(Stdio::from(OwnedFd::from(writer)))
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(6));
        assert!(output.stderr.is_empty());
    }
}
