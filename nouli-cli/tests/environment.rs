#[path = "support/harness.rs"]
pub mod harness;
#[path = "support/loopback.rs"]
pub mod loopback;

use harness::{Home, QUESTION, report, run_report};
use loopback::Server;
use nouli_core::{
    identity::Id,
    protocol::{EngineEvent, EventKind, Report, ResultEnvelope, validate_events},
};
use std::{fs, process::Output};

fn configure(home: &Home, server: &Server) {
    home.configure(server, "typesafe", true);
    let path = home.0.join("config.toml");
    let config = fs::read_to_string(&path)
        .unwrap()
        .replace("NOULI_FIXTURE_KEY", "TYPESAFE_TOKEN");
    fs::write(path, config).unwrap();
}

fn failure(output: Output, code: &str) {
    assert_eq!(output.status.code(), Some(2));
    assert!(output.stderr.is_empty());
    let envelope: ResultEnvelope = serde_json::from_slice(&output.stdout).unwrap();
    let Report::Error(error) = envelope.result else {
        panic!("error report")
    };
    assert_eq!(error.code, code);
}

/// A fresh session in a store of its own, so `home.store()` can stay absent.
fn throwaway(home: &Home) -> std::process::Command {
    let mut command = home.command_at(&home.0.join(format!("throwaway-{}", Id::generate())));
    command.args([&["new", "Throwaway"][..], &QUESTION].concat());
    command
}

fn create(name: &str) -> Vec<&str> {
    [&["new", name][..], &QUESTION].concat()
}

fn missing_credential(output: Output) {
    assert_eq!(output.status.code(), Some(3));
    let envelope: ResultEnvelope = serde_json::from_slice(&output.stdout).unwrap();
    let Report::Error(error) = envelope.result else {
        panic!("error report")
    };
    assert_eq!(error.code, "credential_unavailable");
}

#[test]
fn automatic_env_credentials_support_stateless_persisted_continued_and_replayed_decisions() {
    let server = Server::with_handler(harness::typesafe);
    let home = Home::create();
    configure(&home, &server);
    let env = home.0.join(".env");
    // Quotes, export prefixes, comments, CRLF and a UTF-8 BOM are ordinary file input.
    fs::write(&env, "\u{feff}# Local credentials\r\nexport TYPESAFE_TOKEN='FILE_CREDENTIAL_SENTINEL'\r\nHOME=/ignored\r\nOTHER=\"two words # literal\"\r\n").unwrap();
    let mut contents = fs::read_to_string(&env).unwrap();
    contents.push_str("UNUSED_COMMAND='$(touch must-not-execute)'\n");
    fs::write(&env, contents).unwrap();
    let stateless = report(throwaway(&home).output().unwrap());
    assert!(!home.0.join("must-not-execute").exists());
    assert!(stateless.session.is_some() && stateless.turn.is_some());
    assert!(!home.store().exists());
    let args = [&create("Origin")[..], &["--request-id", "dotenv-initial"]].concat();
    let first = report(home.run(&args));
    let next = report(home.run([&["add", "Origin", "New facts"][..], &QUESTION].concat()));
    assert_eq!(next.session, first.session);
    assert!(
        server
            .authorizations
            .lock()
            .unwrap()
            .iter()
            .all(|value| value.as_deref() == Some("Bearer FILE_CREDENTIAL_SENTINEL"))
    );
    // Completed replay validates the file, but needs no current credential.
    fs::write(&env, "OTHER=unrelated\n").unwrap();
    let replay = report(home.run(&args));
    assert!(replay.replayed && replay.turn == first.turn);
    assert_eq!(server.count(), 3);
    let database = fs::read(home.store().join("ledger.sqlite3")).unwrap();
    assert!(
        !database
            .windows(b"FILE_CREDENTIAL_SENTINEL".len())
            .any(|window| window == b"FILE_CREDENTIAL_SENTINEL")
    );
    assert!(
        !serde_json::to_string(&first)
            .unwrap()
            .contains("FILE_CREDENTIAL_SENTINEL")
    );
    fs::write(&env, "TYPESAFE_TOKEN='BROKEN_PRIVATE_VALUE\n").unwrap();
    failure(home.run(&args), "invalid_env_file");
    assert_eq!(
        database,
        fs::read(home.store().join("ledger.sqlite3")).unwrap()
    );
    assert_eq!(server.count(), 3);
}

#[test]
fn exported_values_win_including_empty_values_but_malformed_files_still_fail() {
    let server = Server::with_handler(harness::typesafe);
    let home = Home::create();
    configure(&home, &server);
    fs::write(home.0.join(".env"), "TYPESAFE_TOKEN=FILE_SENTINEL\n").unwrap();
    report(
        throwaway(&home)
            .env("TYPESAFE_TOKEN", "SHELL_SENTINEL")
            .output()
            .unwrap(),
    );
    assert_eq!(
        server.authorizations.lock().unwrap()[0].as_deref(),
        Some("Bearer SHELL_SENTINEL")
    );
    missing_credential(
        home.command()
            .env("TYPESAFE_TOKEN", "")
            .args(create("Empty"))
            .output()
            .unwrap(),
    );
    fs::write(
        home.0.join(".env"),
        "TYPESAFE_TOKEN='BROKEN_PRIVATE_VALUE\n",
    )
    .unwrap();
    failure(
        home.command()
            .env("TYPESAFE_TOKEN", "SHELL_SENTINEL")
            .args(create("Broken"))
            .output()
            .unwrap(),
        "invalid_env_file",
    );
    assert!(!home.store().exists());
    fs::remove_file(home.0.join(".env")).unwrap();
    report(
        throwaway(&home)
            .env("TYPESAFE_TOKEN", "SHELL_SENTINEL")
            .output()
            .unwrap(),
    );
    assert_eq!(server.count(), 2);
}

#[test]
fn discovery_stays_in_cwd_and_explicit_files_replace_the_automatic_file() {
    let server = Server::with_handler(harness::typesafe);
    let home = Home::create();
    configure(&home, &server);
    fs::write(home.0.join(".env"), "TYPESAFE_TOKEN=PARENT_SENTINEL\n").unwrap();
    let nested = home.0.join("nested");
    fs::create_dir(&nested).unwrap();
    missing_credential(
        home.command()
            .current_dir(&nested)
            .args(create("Nested"))
            .output()
            .unwrap(),
    );
    assert!(!home.store().exists());
    fs::write(nested.join(".env"), "TYPESAFE_TOKEN=CURRENT_SENTINEL\n").unwrap();
    fs::write(
        home.0.join("alternate.env"),
        "TYPESAFE_TOKEN=EXPLICIT_SENTINEL\n",
    )
    .unwrap();
    report(throwaway(&home).current_dir(&nested).output().unwrap());
    report(
        throwaway(&home)
            .current_dir(&nested)
            .args(["--env-file", "../alternate.env"])
            .output()
            .unwrap(),
    );
    failure(
        home.command()
            .current_dir(&nested)
            .args(["--env-file", "missing.env"])
            .args(create("Missing"))
            .output()
            .unwrap(),
        "env_file_not_found",
    );
    fs::write(home.0.join("alternate.env"), "OTHER=unrelated\n").unwrap();
    missing_credential(
        home.command()
            .current_dir(&nested)
            .args(["--env-file", "../alternate.env"])
            .args(create("Alternate"))
            .output()
            .unwrap(),
    );
    assert_eq!(
        *server.authorizations.lock().unwrap(),
        [
            Some("Bearer CURRENT_SENTINEL".into()),
            Some("Bearer EXPLICIT_SENTINEL".into())
        ]
    );
    assert_eq!(server.count(), 2);
}

#[test]
fn file_errors_are_safe_in_all_output_formats_and_never_create_a_store() {
    let server = Server::with_handler(harness::typesafe);
    let home = Home::create();
    configure(&home, &server);
    let path = home.0.join(".env");
    fs::write(&path, "TYPESAFE_TOKEN='BROKEN_PRIVATE_VALUE\n").unwrap();
    for format in ["json", "text", "jsonl"] {
        let output = home.run([&["--format", format][..], &create("Origin")].concat());
        assert_eq!(output.status.code(), Some(2));
        let visible = format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(visible.contains("invalid_env_file") && visible.contains(path.to_str().unwrap()));
        assert!(!visible.contains("BROKEN_PRIVATE_VALUE"));
    }
    fs::write(&path, [0xff, 0xfe]).unwrap();
    failure(home.run(create("Origin")), "invalid_env_file");
    fs::remove_file(&path).unwrap();
    fs::create_dir(&path).unwrap();
    failure(home.run(create("Origin")), "env_file_unreadable");
    assert!(!home.store().exists());
    assert_eq!(server.count(), 0);
}

#[test]
fn help_invalid_input_and_read_only_inspection_do_not_load_env_files() {
    let server = Server::with_handler(harness::typesafe);
    let home = Home::create();
    configure(&home, &server);
    fs::write(home.0.join(".env"), "TYPESAFE_TOKEN=INSPECTION_SENTINEL\n").unwrap();
    report(home.run(create("Origin")));
    fs::write(
        home.0.join(".env"),
        "TYPESAFE_TOKEN='BROKEN_PRIVATE_VALUE\n",
    )
    .unwrap();
    for args in [
        vec![],
        vec!["--help"],
        vec!["--version"],
        vec!["sessions", "list"],
        vec!["sessions", "show", "Origin"],
        vec!["sessions", "export", "Origin"],
        vec!["context", "Origin"],
        vec!["sessions", "list", "--env-file", "missing.env"],
    ] {
        assert!(home.run(&args).status.success(), "{args:?}");
    }
    failure(
        home.run(["new", "Other", "-q", "not grammar"]),
        "invalid_decision_input",
    );
    assert_eq!(server.count(), 1);
}

#[test]
fn workflow_run_and_resume_resolve_file_credentials_from_the_saved_variable_name() {
    use std::sync::atomic::{AtomicBool, Ordering};
    let fail = AtomicBool::new(true);
    let server = Server::with_handler(move |body| {
        if body["state"].as_str().unwrap().contains("NODE:high")
            && fail.swap(false, Ordering::SeqCst)
        {
            (503, "{}".into())
        } else {
            harness::typesafe(body)
        }
    });
    let home = Home::create();
    configure(&home, &server);
    fs::write(home.0.join(".env"), "TYPESAFE_TOKEN=ORIGINAL_SENTINEL\n").unwrap();
    let file = home.0.join("workflow.json");
    fs::write(&file, serde_json::to_vec(&harness::workflow()).unwrap()).unwrap();
    let partial = run_report(home.run(["workflow", "run", file.to_str().unwrap()]), 4);
    let run = partial.run.unwrap().to_string();
    fs::write(home.0.join("config.toml"), "output='json'\n").unwrap();
    fs::write(
        home.0.join(".env"),
        "TYPESAFE_TOKEN='BROKEN_PRIVATE_VALUE\n",
    )
    .unwrap();
    assert!(home.run(["workflow", "show", &run]).status.success());
    fs::write(home.0.join("keys.env"), "TYPESAFE_TOKEN=ROTATED_SENTINEL\n").unwrap();
    let output = home.run([
        "workflow",
        "resume",
        &run,
        "--env-file",
        "keys.env",
        "--jsonl",
    ]);
    assert!(output.status.success() && output.stderr.is_empty());
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(!text.contains("ORIGINAL_SENTINEL") && !text.contains("ROTATED_SENTINEL"));
    let events: Vec<EngineEvent> = text
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    validate_events(&events).unwrap();
    let EventKind::Terminal { result } = &events.last().unwrap().event else {
        panic!("terminal")
    };
    let Report::Run(done) = &result.result else {
        panic!("run")
    };
    assert_eq!(done.status, nouli_core::workflow::RunStatus::Completed);
    assert_eq!(done.nodes[0].turn, partial.nodes[0].turn);
    let authorizations = server.authorizations.lock().unwrap();
    assert!(
        authorizations[..2]
            .iter()
            .all(|value| value.as_deref() == Some("Bearer ORIGINAL_SENTINEL"))
    );
    assert!(
        authorizations[2..]
            .iter()
            .all(|value| value.as_deref() == Some("Bearer ROTATED_SENTINEL"))
    );
    drop(authorizations);
    let count = server.count();
    fs::write(home.0.join("keys.env"), "OTHER=unrelated\n").unwrap();
    let replay = run_report(
        home.run(["workflow", "resume", &run, "--env-file", "keys.env"]),
        0,
    );
    assert!(replay.replayed);
    assert_eq!(server.count(), count);
}
