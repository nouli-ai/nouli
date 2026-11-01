#[path = "support/harness.rs"]
#[allow(dead_code, reason = "each test uses a subset of the helpers")]
mod harness;
#[path = "support/loopback.rs"]
#[allow(dead_code, reason = "each test uses a subset of the helpers")]
mod loopback;

use harness::{Home, QUESTION, QUESTIONS, report};
use nouli_core::{
    history::{ModelSelection, SourceSelection},
    identity::{SessionName, SessionSelector},
    storage::{Ledger, OpenMode},
};
use std::process::{Command, Output};

fn text(output: Output) -> String {
    assert!(output.status.success(), "{output:?}");
    assert_eq!(String::from_utf8_lossy(&output.stderr), "");
    String::from_utf8(output.stdout).unwrap()
}

fn command(home: &Home) -> Command {
    home.bare_command(true)
}

#[test]
fn view_prints_captured_chains_without_provider_calls_or_store_changes() {
    let server = loopback::Server::with_handler(harness::typesafe);
    let home = Home::create();
    home.configure(&server, "typesafe", false);
    for name in ["Travel", "Appointments"] {
        report(&home.run([&["new", name, name][..], &QUESTION].concat()));
        report(&home.run([&["add", name, "More evidence"][..], &QUESTIONS[1]].concat()));
    }
    let combined = report(
        &home.run(
            [
                &["new", "Combined", "Combine histories"][..],
                &QUESTIONS[2],
                &["--from", "Travel", "--from", "Appointments"],
            ]
            .concat(),
        ),
    );
    let initial = text(home.run(["view", "Combined"]));
    assert!(initial.starts_with("Session: Combined ("));
    assert!(initial.contains("┌─ Travel"));
    assert!(initial.contains("┌─ Appointments"));
    assert!(initial.contains("▼"));
    assert_eq!(initial.matches("┌─ Travel").count(), 2);
    assert_eq!(initial.matches("┌─ Appointments").count(), 2);
    assert_eq!(initial.matches("┌─ Combined").count(), 1);
    assert_eq!(initial.matches("▶ ").count(), 5);
    let first_row = initial.lines().find(|line| line.contains("┌─ ")).unwrap();
    assert!(first_row.contains("Travel") && first_row.contains("Appointments"));
    assert!(initial.find("┌─ Travel").unwrap() < initial.find("┌─ Combined").unwrap());
    assert!(initial.contains("Choices") && initial.contains("Probability"));
    assert!(initial.contains("P(yes)="));
    assert!(initial.contains("Expected score:"));
    assert!(!initial.contains('\u{1b}'));

    report(&home.run([&["add", "Travel", "Later source evidence"][..], &QUESTION].concat()));
    report(&home.run([&["new", "Unrelated", "Unrelated"][..], &QUESTION].concat()));
    let calls = server.count();
    // Store-only discovery must tolerate removed/invalid provider definitions.
    let config = "output = 'json'\n[providers.local]\nprotocol = 'removed-protocol'\n";
    std::fs::write(home.0.join("config.toml"), config).unwrap();
    std::fs::write(home.0.join(".env"), "UNFINISHED='\n").unwrap();
    let database = home.ledger_path();
    let before = std::fs::read(&database).unwrap();
    for selector in ["Combined".to_owned(), combined.session.unwrap().to_string()] {
        for format in [vec![], vec!["--format", "auto"], vec!["--format", "text"]] {
            let mut args = vec!["view", &selector, "--env-file", "does-not-exist.env"];
            args.extend(format);
            assert_eq!(text(home.run(&args)), initial);
        }
    }
    let mermaid = text(home.run(["view", "Combined", "--mermaid"]));
    assert!(mermaid.starts_with("%% Nouli session: Combined ("));
    assert!(mermaid.contains("\nflowchart TD\n"));
    assert_eq!(mermaid.matches("[\"<b>Travel").count(), 2);
    assert_eq!(mermaid.matches("[\"<b>Appointments").count(), 2);
    assert_eq!(mermaid.matches("[\"<b>Combined").count(), 1);
    assert_eq!(mermaid.matches("▶ ").count(), 5);
    assert_eq!(mermaid.matches(" --> ").count(), 4);
    assert!(!mermaid.contains('\u{1b}') && !mermaid.contains('┌'));
    assert_eq!(server.count(), calls);
    assert_eq!(std::fs::read(&database).unwrap(), before);
    assert_eq!(
        std::fs::read_to_string(home.0.join("config.toml")).unwrap(),
        config
    );

    // A new sourced session can have inherited history before its first turn.
    let mut ledger = Ledger::open(&home.store(), OpenMode::ReadWriteCreate).unwrap();
    ledger
        .create_session_from(
            Some(SessionName::try_from("Inherited".to_owned()).unwrap()),
            ModelSelection::default(),
            &[SourceSelection {
                session: SessionSelector::Id(combined.session.unwrap()),
                frontier: None,
            }],
        )
        .unwrap();
    drop(ledger);
    let inherited = text(home.run(["view", "Inherited"]));
    assert_eq!(inherited.matches("▶ ").count(), 5);
    assert!(!inherited.contains("[latest]"));
    assert!(!inherited.contains("No finalized turns"));
}

#[test]
fn empty_view_and_help_do_not_initialize_config_or_load_env() {
    let home = Home::create();
    std::fs::write(home.0.join(".env"), "BAD='\n").unwrap();
    let help = text(command(&home).args(["view", "--help"]).output().unwrap());
    assert!(help.contains("<SESSION>") && !help.contains("--session"));
    assert!(!home.store().exists());
    let mut ledger = Ledger::open(&home.store(), OpenMode::ReadWriteCreate).unwrap();
    ledger
        .create_session(
            Some(SessionName::try_from("Empty".to_owned()).unwrap()),
            ModelSelection::default(),
        )
        .unwrap();
    drop(ledger);
    let database = home.ledger_path();
    let before = std::fs::read(&database).unwrap();
    let output = text(command(&home).args(["view", "Empty"]).output().unwrap());
    assert!(output.contains("Session: Empty"));
    assert!(output.contains("No finalized turns"));
    assert!(!home.0.join(".nouli").exists());
    assert_eq!(std::fs::read(&database).unwrap(), before);
}

#[test]
fn view_rejects_structured_formats_and_missing_or_competing_inputs() {
    let home = Home::create();
    for format in [
        vec!["--json"],
        vec!["--jsonl"],
        vec!["--format", "json"],
        vec!["--format", "jsonl"],
        vec!["--format", "toon"],
    ] {
        for before in [true, false] {
            let mut cmd = command(&home);
            if before {
                cmd.args(&format);
            }
            cmd.args(["view", "Empty"]);
            if !before {
                cmd.args(&format);
            }
            let output = cmd.output().unwrap();
            assert_eq!(output.status.code(), Some(2), "{output:?}");
            assert!(
                String::from_utf8_lossy(&output.stdout)
                    .contains("Session graphs support text only")
            );
        }
    }
    for args in [
        vec!["view", "Empty", "--mermaid", "--format", "text"],
        vec!["--format", "auto", "view", "Empty", "--mermaid"],
        vec!["--json", "view", "Empty", "--mermaid"],
    ] {
        let output = command(&home).args(args).output().unwrap();
        assert_eq!(output.status.code(), Some(2), "{output:?}");
        // Text errors use stderr; structured errors stay on stdout.
        let report = [output.stdout, output.stderr].concat();
        assert!(String::from_utf8_lossy(&report).contains("--mermaid cannot be combined"));
    }
    for args in [
        vec!["view"],
        vec!["view", "--json"],
        vec!["-s", "competing state", "view", "Empty"],
        vec!["--session", "Empty", "view"],
    ] {
        assert_eq!(
            command(&home).args(args).output().unwrap().status.code(),
            Some(2)
        );
    }
    assert!(!home.store().exists());
    assert!(!home.0.join(".nouli").exists());
}

#[test]
fn unknown_session_and_missing_store_match_existing_inspection_errors() {
    let home = Home::create();
    for existing_store in [false, true] {
        if existing_store {
            drop(Ledger::open(&home.store(), OpenMode::ReadWriteCreate).unwrap());
        }
        let shown = command(&home)
            .args(["sessions", "show", "Missing", "--format", "text"])
            .output()
            .unwrap();
        let viewed = command(&home)
            .args(["view", "Missing", "--format", "text"])
            .output()
            .unwrap();
        assert!(!viewed.status.success());
        assert_eq!(viewed.status.code(), shown.status.code());
        assert_eq!(viewed.stderr, shown.stderr);
        assert_eq!(viewed.stdout, shown.stdout);
        assert_eq!(home.store().exists(), existing_store);
        assert!(!home.0.join(".nouli").exists());
    }
}
