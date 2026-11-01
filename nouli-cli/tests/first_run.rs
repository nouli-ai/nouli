#[path = "support/harness.rs"]
#[allow(dead_code, reason = "each test uses a subset of the helpers")]
mod harness;
#[path = "support/loopback.rs"]
#[allow(dead_code, reason = "each test uses a subset of the helpers")]
mod loopback;
use harness::{Home, QUESTION, report};
use nouli_core::{
    config::Config,
    identity::Id,
    protocol::{ErrorCategory, Report, ResultEnvelope},
};
use std::{
    path::Path,
    process::{Command, Output},
};
fn command(home: &Home) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_nouli"));
    command
        .env_clear()
        .env("HOME", &home.0)
        .current_dir(&home.0);
    command
}
fn error(output: Output, exit: i32, code: &str) -> nouli_core::protocol::ErrorReport {
    assert_eq!(output.status.code(), Some(exit));
    assert_eq!(String::from_utf8_lossy(&output.stderr), "");
    let envelope: ResultEnvelope = serde_json::from_slice(&output.stdout).unwrap();
    let Report::Error(error) = envelope.result else {
        panic!("expected error report")
    };
    assert_eq!(error.code, code);
    error
}
#[test]
fn first_evaluation_creates_private_starter_and_actionable_errors_without_a_ledger() {
    for ephemeral in [false, true] {
        let home = Home::create();
        let mut call = command(&home);
        call.args([
            if ephemeral { "whatif" } else { "new" },
            "First",
            "First run",
        ])
        .args(QUESTION);
        let failure = error(call.output().unwrap(), 2, "providers_required");
        let path = home.0.join(".nouli/config.toml");
        assert!(failure.message.contains(path.to_str().unwrap()));
        assert!(
            failure.message.contains("default_provider") && failure.message.contains("--provider")
        );
        let contents = std::fs::read_to_string(&path).unwrap();
        let config = Config::parse(&contents).unwrap();
        assert!(config.default_provider.is_none());
        assert!(config.default_model.is_none());
        assert!(config.providers.is_empty());
        assert!(!path.parent().unwrap().join("ledger.sqlite3").exists());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
            assert_eq!(
                std::fs::metadata(path.parent().unwrap())
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o700
            );
        }
        error(
            command(&home)
                .args([&["new", "First"][..], &QUESTION, &["--provider", "openai"]].concat())
                .output()
                .unwrap(),
            2,
            "providers_required",
        );
        assert_eq!(std::fs::read_to_string(&path).unwrap(), contents);
        assert!(!path.parent().unwrap().join("ledger.sqlite3").exists());
    }
}
#[test]
fn help_invalid_input_inspection_and_resume_do_not_initialize_configuration() {
    let home = Home::create();
    let run = Id::generate().to_string();
    for arguments in [
        vec![],
        vec!["--help"],
        vec!["--version"],
        vec!["--json"],
        vec!["new", "Invalid", "-q", "invalid json"],
        vec!["sessions", "list"],
        vec!["workflow", "resume", &run],
        vec!["workflow", "show", &run],
    ] {
        command(&home).args(arguments).output().unwrap();
        assert!(!home.0.join(".nouli").exists());
    }
    let missing = home.0.join("explicit/missing.toml");
    let failure = error(
        command(&home)
            .arg("--config")
            .arg(&missing)
            .args([&["new", "First"][..], &QUESTION].concat())
            .output()
            .unwrap(),
        2,
        "configuration_not_found",
    );
    assert!(failure.message.contains(missing.to_str().unwrap()));
    assert!(!missing.exists() && !home.0.join(".nouli").exists());
}
fn configure_default(home: &Home, server: &loopback::Server, suffix: &str) -> std::path::PathBuf {
    home.configure(server, "typesafe", false);
    let directory = home.0.join(".nouli");
    std::fs::create_dir(&directory).unwrap();
    let path = directory.join("config.toml");
    let config = std::fs::read_to_string(home.0.join("config.toml"))
        .unwrap()
        .replace(
            "[providers.local]\n",
            &format!("[providers.local]\n{suffix}"),
        );
    std::fs::write(&path, config).unwrap();
    path
}
#[test]
fn configured_first_run_persists_and_replays_without_requiring_current_profiles() {
    let server = loopback::Server::with_handler(harness::typesafe);
    let home = Home::create();
    let path = configure_default(&home, &server, "");
    let original = std::fs::read(&path).unwrap();
    let args = [
        &["new", "First"][..],
        &QUESTION,
        &["--request-id", "first-run"],
    ]
    .concat();
    let first = report(command(&home).args(&args).output().unwrap());
    let continued = report(
        command(&home)
            .args([&["add", "First"][..], &QUESTION].concat())
            .output()
            .unwrap(),
    );
    assert_eq!(first.session, continued.session);
    assert_eq!(std::fs::read(&path).unwrap(), original);
    let database = std::fs::read(path.parent().unwrap().join("ledger.sqlite3")).unwrap();
    std::fs::remove_file(&path).unwrap();
    let replay = report(command(&home).args(&args).output().unwrap());
    assert!(replay.replayed);
    assert_eq!(replay.turn, first.turn);
    assert_eq!(server.count(), 2);
    assert!(
        database == std::fs::read(path.parent().unwrap().join("ledger.sqlite3")).unwrap(),
        "replay changed saved database"
    );
    assert!(
        Config::load(&path, false)
            .unwrap()
            .default_provider
            .is_none()
    );
}
#[test]
fn explicit_config_errors_and_invalid_budgets_leave_existing_files_untouched() {
    let server = loopback::Server::with_handler(harness::typesafe);
    for (text, code, exit) in [
        ("output='json'\n", "providers_required", 2),
        ("invalid TOML !", "invalid_configuration_syntax", 2),
        (
            "default_provider='local'\ndefault_model='unknown'\n[providers.local]\nprotocol='system-one'\nbase_url='http://localhost:8000'\n[providers.local.models.unknown]\ncontext_budget_tokens=0\n",
            "invalid_configuration",
            2,
        ),
    ] {
        let home = Home::create();
        let path = home.0.join("existing.toml");
        std::fs::write(&path, text).unwrap();
        let output = command(&home)
            .arg("--config")
            .arg(&path)
            .args([&["new", "First"][..], &QUESTION].concat())
            .output()
            .unwrap();
        let failure = error(output, exit, code);
        if failure.category == ErrorCategory::Configuration {
            assert!(failure.message.contains(path.to_str().unwrap()));
        }
        assert_eq!(std::fs::read_to_string(&path).unwrap(), text);
        assert!(!home.0.join(".nouli").exists());
    }
    assert_eq!(server.count(), 0);
}

#[test]
fn model_selection_shares_one_provider_and_missing_models_fail_before_first_store() {
    let server = loopback::Server::with_handler(harness::typesafe);
    let home = Home::create();
    let path = home.0.join("config.toml");
    let config = format!(
        r#"default_provider = "local"
[execution]
max_attempts = 1
[providers.local]
protocol = "system-one"
base_url = "{}"
[providers.local.models.a]
context_budget_tokens = 4096
reserved_output_tokens = 128
[providers.local.models.b]
context_budget_tokens = 8192
reserved_output_tokens = 256
"#,
        server.url
    );
    std::fs::write(&path, &config).unwrap();
    let create = [&["new", "Models"][..], &QUESTION].concat();
    let failure = error(home.run(&create), 2, "model_required");
    assert!(failure.message.contains("default_model") && failure.message.contains("--model"));
    assert!(failure.message.contains(path.to_str().unwrap()));
    assert!(!home.store().exists());
    assert_eq!(server.count(), 0);
    // Sessions live in a separate store so the workflow checks below still
    // see no ledger.
    let sessions = home.0.join("sessions");
    for (model, budget, reserve) in [("a", 4096, 128), ("b", 8192, 256)] {
        let result = report(
            home.command_at(&sessions)
                .args([&["new", model][..], &QUESTION, &["--model", model]].concat())
                .output()
                .unwrap(),
        );
        assert_eq!(result.manifest.budget, budget);
        assert_eq!(result.manifest.reserved_output_tokens, reserve);
    }
    let result = report(
        home.command_at(&sessions)
            .args(
                [
                    &["new", "unlisted"][..],
                    &QUESTION,
                    &["--model", "unlisted", "--context-budget", "2048"],
                ]
                .concat(),
            )
            .output()
            .unwrap(),
    );
    assert_eq!(result.manifest.budget, 2048);
    assert_eq!(result.manifest.reserved_output_tokens, 0);
    assert!(!home.store().exists());
    let mut graph = harness::workflow();
    let file = home.0.join("workflow.json");
    std::fs::write(&file, serde_json::to_vec(&graph).unwrap()).unwrap();
    error(
        home.run(["workflow", "run", file.to_str().unwrap()]),
        2,
        "model_required",
    );
    assert!(!home.store().exists());
    for node in &mut graph.nodes {
        node.model.model = Some("a".into());
    }
    std::fs::write(&file, serde_json::to_vec(&graph).unwrap()).unwrap();
    let ephemeral = harness::run_report(
        home.run(["workflow", "run", file.to_str().unwrap(), "--no-session"]),
        0,
    );
    assert_eq!(ephemeral.status, nouli_core::workflow::RunStatus::Completed);
    assert!(!home.store().exists());
    let result = harness::run_report(home.run(["workflow", "run", file.to_str().unwrap()]), 0);
    assert_eq!(result.status, nouli_core::workflow::RunStatus::Completed);
    let calls = server.calls.lock().unwrap();
    assert_eq!(calls[0]["model"], "a");
    assert_eq!(calls[1]["model"], "b");
    assert_eq!(calls[2]["model"], "unlisted");
    assert_eq!(calls[3]["model"], "a");
    assert_eq!(std::fs::read_to_string(path).unwrap(), config);
}
#[test]
fn workflow_setup_validates_every_node_before_first_store_and_preserves_frozen_resume() {
    let once = std::sync::atomic::AtomicBool::new(true);
    let server = loopback::Server::with_handler(move |body| {
        if body["state"].as_str().unwrap().contains("NODE:high")
            && once.swap(false, std::sync::atomic::Ordering::SeqCst)
        {
            (503, "{}".into())
        } else {
            harness::typesafe(body)
        }
    });
    let home = Home::create();
    let file = home.0.join("workflow.json");
    let mut graph = harness::workflow();
    std::fs::write(&file, serde_json::to_vec(&graph).unwrap()).unwrap();
    let failure = error(
        command(&home)
            .args(["workflow", "run", file.to_str().unwrap()])
            .output()
            .unwrap(),
        2,
        "providers_required",
    );
    let path = home.0.join(".nouli/config.toml");
    assert!(failure.message.contains(path.to_str().unwrap()));
    assert!(!path.parent().unwrap().join("ledger.sqlite3").exists());
    home.configure(&server, "typesafe", false);
    std::fs::copy(home.0.join("config.toml"), &path).unwrap();
    graph.nodes[1].model.provider = Some("unknown".into());
    std::fs::write(&file, serde_json::to_vec(&graph).unwrap()).unwrap();
    error(
        command(&home)
            .args(["workflow", "run", file.to_str().unwrap()])
            .output()
            .unwrap(),
        2,
        "unknown_provider",
    );
    assert_eq!(server.count(), 0);
    assert!(!path.parent().unwrap().join("ledger.sqlite3").exists());
    graph.nodes[1].model.provider = None;
    std::fs::write(&file, serde_json::to_vec(&graph).unwrap()).unwrap();
    let partial = harness::run_report(
        command(&home)
            .args(["workflow", "run", file.to_str().unwrap()])
            .output()
            .unwrap(),
        4,
    );
    std::fs::remove_file(&path).unwrap();
    let done = harness::run_report(
        command(&home)
            .args(["workflow", "resume", &partial.run.unwrap().to_string()])
            .output()
            .unwrap(),
        0,
    );
    assert_eq!(done.nodes[0].turn, partial.nodes[0].turn);
    assert!(!path.exists());
    assert_eq!(server.count(), 5);
}
#[test]
fn setup_write_failure_reports_path_and_never_creates_a_ledger() {
    let home = Home::create();
    std::fs::write(home.0.join(".nouli"), "existing file").unwrap();
    // Loading discovers that .nouli cannot be traversed, without changing it.
    let result = command(&home)
        .args([&["new", "First"][..], &QUESTION].concat())
        .output()
        .unwrap();
    assert_eq!(result.status.code(), Some(2));
    let envelope: ResultEnvelope = serde_json::from_slice(&result.stdout).unwrap();
    let Report::Error(failure) = envelope.result else {
        panic!("error")
    };
    assert!(failure.message.contains("config.toml"));
    assert_eq!(
        std::fs::read_to_string(home.0.join(".nouli")).unwrap(),
        "existing file"
    );
    assert!(!Path::new(&home.0.join(".nouli/ledger.sqlite3")).exists());
}

#[test]
fn a_preexisting_empty_ledger_still_gets_configuration_path_guidance() {
    let home = Home::create();
    let store = home.0.join(".nouli");
    drop(
        nouli_core::storage::Ledger::open(&store, nouli_core::storage::OpenMode::ReadWriteCreate)
            .unwrap(),
    );
    let before = std::fs::read(store.join("ledger.sqlite3")).unwrap();
    let failure = error(
        command(&home)
            .args([&["new", "First"][..], &QUESTION].concat())
            .output()
            .unwrap(),
        2,
        "providers_required",
    );
    assert!(
        failure
            .message
            .contains(store.join("config.toml").to_str().unwrap())
    );
    assert!(
        before == std::fs::read(store.join("ledger.sqlite3")).unwrap(),
        "setup error changed existing ledger"
    );
}

#[test]
fn existing_store_credential_errors_name_source_and_completed_replay_reads_no_key() {
    let server = loopback::Server::with_handler(harness::typesafe);
    let home = Home::create();
    let path = configure_default(&home, &server, "api_key_env='FROZEN_TEST_KEY'\n");
    drop(
        nouli_core::storage::Ledger::open(
            &home.0.join(".nouli"),
            nouli_core::storage::OpenMode::ReadWriteCreate,
        )
        .unwrap(),
    );
    let args = [
        &["new", "Existing"][..],
        &QUESTION,
        &["--request-id", "existing-credentials"],
    ]
    .concat();
    let failure = error(
        command(&home).args(&args).output().unwrap(),
        3,
        "credential_unavailable",
    );
    assert!(
        failure.message.contains("FROZEN_TEST_KEY")
            && failure.message.contains(path.to_str().unwrap())
    );
    assert_eq!(server.count(), 0);
    let result = report(
        command(&home)
            .args(&args)
            .arg("--retry-interrupted")
            .env("FROZEN_TEST_KEY", "PRIVATE_SENTINEL_VALUE")
            .output()
            .unwrap(),
    );
    let replay = report(command(&home).args(&args).output().unwrap());
    assert!(replay.replayed && replay.turn == result.turn);
    assert_eq!(server.count(), 1);
}
#[test]
fn frozen_workflow_credentials_keep_original_source_in_json_and_jsonl_diagnostics() {
    use nouli_core::protocol::{EngineEvent, EventKind, validate_events};
    let once = std::sync::atomic::AtomicBool::new(true);
    let server = loopback::Server::with_handler(move |body| {
        if body["state"].as_str().unwrap().contains("NODE:high")
            && once.swap(false, std::sync::atomic::Ordering::SeqCst)
        {
            (503, "{}".into())
        } else {
            harness::typesafe(body)
        }
    });
    let home = Home::create();
    let path = configure_default(&home, &server, "api_key_env='ORIGINAL_FROZEN_KEY'\n");
    let file = home.0.join("workflow.json");
    std::fs::write(&file, serde_json::to_vec(&harness::workflow()).unwrap()).unwrap();
    let args = [
        "workflow",
        "run",
        file.to_str().unwrap(),
        "--request-id",
        "frozen-credentials",
    ];
    let partial = harness::run_report(
        command(&home)
            .args(args)
            .env("ORIGINAL_FROZEN_KEY", "PRIVATE_SENTINEL_VALUE")
            .output()
            .unwrap(),
        4,
    );
    let run = partial.run.unwrap().to_string();
    std::fs::write(
        &path,
        "output='json'\n[providers.changed]\napi_key_env='CHANGED_KEY'\n",
    )
    .unwrap();
    for jsonl in [false, true] {
        let mut call = command(&home);
        call.args(["workflow", "resume", &run]);
        if jsonl {
            call.arg("--jsonl");
        }
        let output = call.output().unwrap();
        assert_eq!(output.status.code(), Some(4));
        assert_eq!(String::from_utf8_lossy(&output.stderr), "");
        let text = String::from_utf8(output.stdout).unwrap();
        assert!(!text.contains("PRIVATE_SENTINEL_VALUE") && !text.contains("CHANGED_KEY"));
        let envelope = if jsonl {
            let mut events = text
                .lines()
                .map(|line| serde_json::from_str::<EngineEvent>(line).unwrap())
                .collect::<Vec<_>>();
            validate_events(&events).unwrap();
            let EventKind::Terminal { result } = events.pop().unwrap().event else {
                panic!("terminal")
            };
            *result
        } else {
            serde_json::from_str::<ResultEnvelope>(&text).unwrap()
        };
        let Report::Run(report) = envelope.result else {
            panic!("run")
        };
        let error = report.nodes[1].error.as_ref().unwrap();
        assert_eq!(error.code, "credential_unavailable");
        assert!(error.message.contains("ORIGINAL_FROZEN_KEY"));
        assert!(error.message.contains(path.to_str().unwrap()));
        assert_eq!(report.nodes[0].turn, partial.nodes[0].turn);
        assert_eq!(server.count(), 2);
    }
    let done = harness::run_report(
        command(&home)
            .args(["workflow", "resume", &run])
            .env("ORIGINAL_FROZEN_KEY", "PRIVATE_SENTINEL_VALUE")
            .output()
            .unwrap(),
        0,
    );
    assert_eq!(done.nodes[0].turn, partial.nodes[0].turn);
    assert_eq!(server.count(), 5);
    // A valid current config without providers still permits completed replay.
    std::fs::write(&path, "output='json'\n").unwrap();
    assert!(harness::run_report(command(&home).args(args).output().unwrap(), 0).replayed);
    assert_eq!(server.count(), 5);
}
