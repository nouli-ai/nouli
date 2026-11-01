use std::process::Command;

#[test]
fn help_and_version_work_without_configuration() {
    for (argument, expected) in [
        ("--help", "Usage: nouli"),
        ("--version", concat!("nouli ", env!("CARGO_PKG_VERSION"))),
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_nouli"))
            .arg(argument)
            .env_clear()
            .output()
            .unwrap();
        assert!(output.status.success());
        assert!(String::from_utf8(output.stdout).unwrap().contains(expected));
        assert_eq!(String::from_utf8_lossy(&output.stderr), "");
    }
}

/// Clap prints doc comments verbatim, so Markdown quoting would show up in `--help`.
#[test]
fn help_text_has_no_markdown_backticks() {
    let mut pending = vec![vec![]];
    while let Some(path) = pending.pop() {
        let output = Command::new(env!("CARGO_BIN_EXE_nouli"))
            .args(&path)
            .arg("--help")
            .env_clear()
            .output()
            .unwrap();
        assert!(output.status.success(), "nouli {path:?} --help failed");
        let help = String::from_utf8(output.stdout).unwrap();
        assert!(
            !help.contains('`'),
            "nouli {path:?} --help contains a backtick"
        );
        let commands = help.lines().skip_while(|line| *line != "Commands:").skip(1);
        for line in commands.take_while(|line| !line.is_empty()) {
            let name = line.split_whitespace().next().unwrap();
            if name != "help" {
                pending.push([path.as_slice(), &[name.to_owned()]].concat());
            }
        }
    }
}

#[test]
fn unknown_arguments_fail_without_protocol_output() {
    let output = Command::new(env!("CARGO_BIN_EXE_nouli"))
        .arg("--unknown")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert_eq!(String::from_utf8_lossy(&output.stdout), "");
    assert_ne!(String::from_utf8_lossy(&output.stderr), "");
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
    assert_eq!(String::from_utf8_lossy(&output.stderr), "");
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
                assert_eq!(String::from_utf8_lossy(&output.stderr), "");
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
                assert_eq!(String::from_utf8_lossy(&output.stderr), "");
            }
            _ => {
                assert_eq!(String::from_utf8_lossy(&output.stdout), "");
                assert!(
                    String::from_utf8(output.stderr)
                        .unwrap()
                        .contains("command_required")
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
        assert_eq!(String::from_utf8_lossy(&output.stderr), "");
    }
}

#[test]
fn bare_workflow_commands_provide_actionable_usage_without_configuration() {
    for arguments in [
        vec!["workflow"],
        vec!["workflow", "run"],
        vec!["workflow", "show"],
        vec!["workflow", "resume"],
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_nouli"))
            .args(arguments)
            .env_clear()
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(2));
        assert_eq!(String::from_utf8_lossy(&output.stdout), "");
        assert!(String::from_utf8(output.stderr).unwrap().contains("Usage:"));
    }
}
#[test]
fn missing_workflow_resume_never_creates_a_store() {
    let root = std::env::temp_dir().join(format!("nouli-resume-missing-{}", std::process::id()));
    let run = nouli_core::identity::Id::generate();
    let output = Command::new(env!("CARGO_BIN_EXE_nouli"))
        .env_clear()
        .env("HOME", &root)
        .arg("--store")
        .arg(root.join("store"))
        .args(["workflow", "resume", &run.to_string(), "--json"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(6));
    let result: nouli_core::protocol::ResultEnvelope =
        serde_json::from_slice(&output.stdout).unwrap();
    assert!(
        matches!(result.result,nouli_core::protocol::Report::Error(error) if error.code=="missing_ledger")
    );
    assert!(!root.exists());
}
