#[path = "support/loopback.rs"]
mod loopback;
use loopback::Server;
use nouli_core::{
    codec,
    context::{ContextKind, ContextRecord, ContextUpdate},
    data::{Data, Input},
    decision::TurnRequest,
    history::{EvaluationRequest, ModelSelection, SessionSelection},
    identity::Id,
    protocol::{Report, ResultEnvelope, TurnReport},
    storage::{Ledger, OpenMode},
};
use std::{
    io::Write,
    path::PathBuf,
    process::{Command, Output, Stdio},
};
struct Home(PathBuf);
impl Home {
    fn new() -> Self {
        let home = Self(std::env::temp_dir().join(format!("nouli-cli-eval-{}", Id::generate())));
        std::fs::create_dir_all(&home.0).unwrap();
        home
    }
    fn store(&self) -> PathBuf {
        self.0.join("ledger")
    }
    fn configure(&self, server: &Server, protocol: &str, credential: bool) {
        let config = format!(
            "default_provider = \"local\"\n[execution]\nmax_attempts = 1\nattempt_timeout_seconds = 5\n[providers.local]\nprotocol = \"{protocol}\"\nbase_url = \"{}\"\nmodel = \"offline\"\ncontext_budget_tokens = 100000\ncontext_window_tokens = 100000\nreserved_output_tokens = 128\n{}",
            server.url,
            if credential {
                "api_key_env = \"NOULI_FIXTURE_KEY\"\n"
            } else {
                ""
            }
        );
        std::fs::write(self.0.join("config.toml"), config).unwrap();
    }
    fn command(&self) -> Command {
        self.command_at(&self.store())
    }
    fn command_at(&self, store: &std::path::Path) -> Command {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_nouli"));
        cmd.env_clear()
            .env("HOME", &self.0)
            .arg("--config")
            .arg(self.0.join("config.toml"))
            .arg("--store")
            .arg(store);
        cmd
    }
    fn run(&self, args: &[&str]) -> Output {
        self.command().args(args).output().unwrap()
    }
    fn request(
        &self,
        path: &str,
        selection: SessionSelection,
        marker: &str,
        updates: Vec<ContextUpdate>,
    ) -> PathBuf {
        let mut turn = turn();
        turn.state.push(Input::Text {
            text: marker.into(),
        });
        let request = EvaluationRequest {
            turn,
            session: selection,
            updates,
            required_records: vec![],
            model: ModelSelection::default(),
            request_id: None,
        };
        let path = self.0.join(path);
        std::fs::write(&path, serde_json::to_vec_pretty(&request).unwrap()).unwrap();
        path
    }
}
impl Drop for Home {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).unwrap();
    }
}
fn turn() -> TurnRequest {
    let mut turn: TurnRequest = serde_json::from_str(include_str!(
        "../../fixtures/contracts/decision-request.json"
    ))
    .unwrap();
    turn.questions.truncate(3);
    turn
}
fn report(output: Output) -> TurnReport {
    assert!(
        output.status.success(),
        "exit={:?},stderr={}",
        output.status.code(),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stderr.is_empty());
    let result: ResultEnvelope = serde_json::from_slice(&output.stdout).unwrap();
    let Report::Turn(turn) = result.result else {
        panic!("turn report")
    };
    *turn
}
fn questions() -> String {
    serde_json::to_string(&turn().questions).unwrap()
}
#[test]
fn persisted_continue_branch_blend_fixed_heads_and_ephemeral_use_the_shared_engine() {
    let server = Server::new(include_str!(
        "../../fixtures/providers/typesafe/response.json"
    ));
    let home = Home::new();
    home.configure(&server, "typesafe", false);
    let questions = questions();
    let seed = report(home.run(&[
        "-s",
        "ORIGINAL",
        "-q",
        &questions,
        "--name",
        "Origin",
        "--request-id",
        "seed",
    ]));
    let first = seed.turn.unwrap();
    let original = seed.session.unwrap();
    let next = report(home.run(&["-s", "LATER", "-q", &questions, "--session", "Origin"]));
    assert_eq!(next.session, Some(original));
    let branch = report(home.run(&[
        "-s",
        "BRANCH",
        "-q",
        &questions,
        "--from",
        &format!("Origin@{first}"),
        "--name",
        "Branch",
    ]));
    assert_ne!(branch.session, Some(original));
    assert_eq!(branch.manifest.sources[0].frontier, vec![first]);
    let blend = report(home.run(&[
        "-s", "BLEND", "-q", &questions, "--from", "Branch", "--from", "Origin", "--name", "Blend",
    ]));
    assert_eq!(blend.manifest.sources.len(), 2);
    let bytes = std::fs::read(home.store().join("ledger.sqlite3")).unwrap();
    let ephemeral = report(home.run(&[
        "-s",
        "TEMPORARY",
        "-q",
        &questions,
        "--from",
        "Blend",
        "--no-session",
    ]));
    assert!(ephemeral.session.is_none());
    assert!(ephemeral.turn.is_none());
    assert_eq!(
        bytes,
        std::fs::read(home.store().join("ledger.sqlite3")).unwrap()
    );
    let fresh_store = home.0.join("never-created");
    let fresh = report(
        home.command_at(&fresh_store)
            .args(["-s", "FRESH", "-q", &questions, "--no-session"])
            .output()
            .unwrap(),
    );
    assert!(fresh.session.is_none());
    assert!(!fresh_store.exists());
    let no_home = report(
        Command::new(env!("CARGO_BIN_EXE_nouli"))
            .env_clear()
            .arg("--config")
            .arg(home.0.join("config.toml"))
            .arg("--store")
            .arg(&fresh_store)
            .args(["-q", &questions, "--no-session"])
            .output()
            .unwrap(),
    );
    assert!(no_home.turn.is_none());
    assert!(!fresh_store.exists());

    assert_eq!(server.count(), 7);
    let calls = server.calls.lock().unwrap();
    assert!(calls[1]["state"].as_str().unwrap().contains("ORIGINAL"));
    assert!(!calls[2]["state"].as_str().unwrap().contains("LATER"));
    assert!(calls[4]["state"].as_str().unwrap().contains("BLEND"));
}
#[test]
fn full_file_updates_json_toon_stdin_and_normalized_replay_preserve_complete_input() {
    let server = Server::new(include_str!(
        "../../fixtures/providers/typesafe/response.json"
    ));
    let home = Home::new();
    home.configure(&server, "typesafe", false);
    let record = ContextRecord {
        id: Id::generate(),
        key: "budget".into(),
        value: Data::new(serde_json::json!(18446744073709551615u64)).unwrap(),
        kind: ContextKind::Commitment,
    };
    let path = home.request(
        "request.json",
        SessionSelection::New { name: None },
        "FULL_STATE",
        vec![ContextUpdate::Append {
            record: record.clone(),
        }],
    );
    let first = report(home.run(&[
        "--input",
        path.to_str().unwrap(),
        "--request-id",
        "full-file",
        "--model",
        "requested-alias",
        "--context-budget",
        "90000",
    ]));
    assert_eq!(first.manifest.budget, 90000);
    assert_eq!(first.outcome.provider.model, "laya-english-fixture");
    assert_eq!(server.calls.lock().unwrap()[0]["model"], "requested-alias");
    let request: EvaluationRequest =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    let toon = home.0.join("same.toon");
    std::fs::write(&toon, codec::encode_toon(&request).unwrap()).unwrap();
    let replay = report(home.run(&[
        "--input",
        toon.to_str().unwrap(),
        "--request-id",
        "full-file",
        "--model",
        "requested-alias",
        "--context-budget",
        "80000",
    ]));
    assert!(replay.replayed);
    assert_eq!(replay.turn, first.turn);
    assert_eq!(replay.manifest.budget, 90000);
    assert_eq!(server.count(), 1);
    let ledger = Ledger::open(&home.store(), OpenMode::ReadOnlyExisting).unwrap();
    let saved = ledger.turn(first.turn.unwrap()).unwrap();
    assert_eq!(saved.request, request.turn);
    assert_eq!(saved.updates, request.updates);
    assert!(
        saved
            .manifest
            .records
            .iter()
            .any(|entry| entry.record == record.id)
    );
    drop(ledger);
    let mut cmd = home.command();
    cmd.args(["--input", "-", "--input-format", "json", "--no-session"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = cmd.spawn().unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(&serde_json::to_vec(&request).unwrap())
        .unwrap();
    let result = report(child.wait_with_output().unwrap());
    assert!(result.turn.is_none());
    assert_eq!(server.count(), 2);
}
#[test]
fn invalid_selection_modes_names_budget_credentials_and_request_conflicts_do_not_dispatch() {
    let server = Server::new(include_str!(
        "../../fixtures/providers/typesafe/response.json"
    ));
    let home = Home::new();
    home.configure(&server, "typesafe", true);
    let questions = questions();
    let missing = home.run(&[
        "-q",
        &questions,
        "--request-id",
        "credentials",
        "--name",
        "Named",
    ]);
    assert_eq!(missing.status.code(), Some(3));
    assert_eq!(server.count(), 0);
    let recovered = report(
        home.command()
            .args([
                "-q",
                &questions,
                "--request-id",
                "credentials",
                "--retry-interrupted",
                "--name",
                "Named",
            ])
            .env("NOULI_FIXTURE_KEY", "SECRET_FIXTURE_TOKEN")
            .output()
            .unwrap(),
    );
    assert!(recovered.turn.is_some());
    assert_eq!(server.count(), 1);
    for (args, code) in [
        (vec!["-q", questions.as_str(), "--session", "Unknown"], 2),
        (vec!["-q", questions.as_str(), "--name", "Named"], 5),
        (
            vec![
                "-s",
                "changed",
                "-q",
                questions.as_str(),
                "--request-id",
                "credentials",
            ],
            5,
        ),
        (
            vec![
                "-q",
                questions.as_str(),
                "--no-session",
                "--request-id",
                "ephemeral",
            ],
            2,
        ),
        (vec!["-q", questions.as_str(), "--retry-interrupted"], 2),
    ] {
        let output = home
            .command()
            .args(args)
            .env("NOULI_FIXTURE_KEY", "SECRET_FIXTURE_TOKEN")
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(code));
        assert_eq!(server.count(), 1);
    }
    assert!(
        !std::fs::read(home.store().join("ledger.sqlite3"))
            .unwrap()
            .windows(b"SECRET_FIXTURE_TOKEN".len())
            .any(|part| part == b"SECRET_FIXTURE_TOKEN")
    );
}

#[test]
fn cli_dispatches_direct_openai_and_router_protocols_without_fallback() {
    for (protocol, response, full_questions, expected) in [
        (
            "openai-decisions",
            include_str!("../../fixtures/providers/openai/response.json"),
            true,
            "gpt-6-luna-fixture",
        ),
        (
            "openrouter-decisions",
            include_str!("../../fixtures/providers/openrouter/response.json"),
            false,
            "openai/gpt-6-luna-decisions-fixture",
        ),
    ] {
        let server = Server::new(response);
        let home = Home::new();
        home.configure(&server, protocol, false);
        let mut request = turn();
        if full_questions {
            request = serde_json::from_str(include_str!(
                "../../fixtures/contracts/decision-request.json"
            ))
            .unwrap();
        }
        let questions = serde_json::to_string(&request.questions).unwrap();
        let result = report(home.run(&["-q", &questions, "--provider", "local", "--no-session"]));
        assert_eq!(result.outcome.provider.model, expected);
        assert_eq!(server.count(), 1);
        assert!(result.session.is_none());
        assert!(!home.store().exists());
        if full_questions {
            assert!(matches!(
                result.outcome.outcomes[3],
                nouli_core::outcome::QuestionOutcome::Refused { .. }
            ));
        }
    }
}
#[test]
fn missing_model_budget_and_competing_file_modes_fail_before_inference() {
    let server = Server::new(include_str!(
        "../../fixtures/providers/typesafe/response.json"
    ));
    let home = Home::new();
    home.configure(&server, "typesafe", false);
    let path = home.0.join("config.toml");
    let config = std::fs::read_to_string(&path)
        .unwrap()
        .replace("context_budget_tokens = 100000\n", "")
        .replace("context_window_tokens = 100000\n", "");
    std::fs::write(&path, config).unwrap();
    let questions = questions();
    let missing = home.run(&["-q", &questions, "--no-session"]);
    assert_eq!(missing.status.code(), Some(2));
    assert_eq!(server.count(), 0);
    assert!(!home.store().exists());
    let valid = report(home.run(&[
        "-q",
        &questions,
        "--no-session",
        "--context-budget",
        "90000",
    ]));
    assert_eq!(valid.manifest.budget, 90000);
    assert_eq!(server.count(), 1);
    let file = home.request(
        "competing.json",
        SessionSelection::New { name: None },
        "unused",
        vec![],
    );
    let output = home.run(&["--input", file.to_str().unwrap(), "--from", "Other"]);
    assert_eq!(output.status.code(), Some(2));
    assert_eq!(server.count(), 1);
    assert!(!home.store().exists());
}

#[test]
fn readonly_diamond_exports_preserve_originals_after_compaction_and_exclude_later_or_unrelated_work()
 {
    use nouli_core::{
        history::{Representation, SourceSelection},
        identity::{SessionName, SessionSelector},
        storage::inspection::{ContextView, SessionExport, TurnInspection},
    };
    let server = Server::new(include_str!(
        "../../fixtures/providers/typesafe/response.json"
    ));
    let home = Home::new();
    home.configure(&server, "typesafe", true);
    let keyed = |args: &[&str]| {
        home.command()
            .args(args)
            .env("NOULI_FIXTURE_KEY", "EXPORT_PRIVATE_TOKEN")
            .output()
            .unwrap()
    };
    let pinned = ContextRecord {
        id: Id::generate(),
        key: "budget".into(),
        value: Data::new(serde_json::json!(8)).unwrap(),
        kind: ContextKind::Commitment,
    };
    let original = "ARCHIVED_EVIDENCE_".repeat(3000);
    let root = home.request(
        "root.json",
        SessionSelection::New {
            name: Some(SessionName::try_from("Root".to_owned()).unwrap()),
        },
        &original,
        vec![ContextUpdate::Pin {
            target: nouli_core::context::PinTarget::New {
                record: pinned.clone(),
            },
        }],
    );
    let root = report(keyed(&["--input", root.to_str().unwrap()]));
    let mut branches = vec![];
    for (name, kind) in [
        ("Left", ContextKind::Evidence),
        ("Right", ContextKind::Commitment),
    ] {
        let record = ContextRecord {
            id: Id::generate(),
            key: name.to_lowercase(),
            value: Data::new(serde_json::json!(name)).unwrap(),
            kind,
        };
        let path = home.request(
            &format!("{name}.json"),
            SessionSelection::From {
                sources: vec![SourceSelection {
                    session: SessionSelector::Id(root.session.unwrap()),
                    frontier: None,
                }],
                name: Some(SessionName::try_from(name.to_owned()).unwrap()),
            },
            name,
            vec![ContextUpdate::Append { record }],
        );
        branches.push(report(keyed(&["--input", path.to_str().unwrap()])));
    }
    let questions = questions();
    let blend = report(keyed(&[
        "-s",
        "BLENDED",
        "-q",
        &questions,
        "--from",
        "Left",
        "--from",
        "Right",
        "--name",
        "Diamond",
        "--context-budget",
        "2500",
    ]));
    assert!(
        blend
            .manifest
            .records
            .iter()
            .any(|record| record.record == root.turn.unwrap()
                && record.representation != Representation::Full)
    );
    report(keyed(&[
        "-s",
        "AFTER_CAPTURE",
        "-q",
        &questions,
        "--session",
        "Root",
    ]));
    report(keyed(&[
        "-s",
        "UNRELATED_PRIVATE_STATE",
        "-q",
        &questions,
        "--name",
        "Unrelated",
    ]));
    let count = server.count();
    let before = std::fs::read(home.store().join("ledger.sqlite3")).unwrap();
    // A changed/invalid provider profile must not block store-only inspection.
    let config = std::fs::read_to_string(home.0.join("config.toml"))
        .unwrap()
        .replace("protocol = \"typesafe\"", "protocol = \"removed-protocol\"");
    std::fs::write(home.0.join("config.toml"), config).unwrap();
    let output = home.run(&["sessions", "export", "Diamond", "--format", "json"]);
    assert!(
        output.status.success(),
        "{:?}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stderr.is_empty());
    let export: SessionExport = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(export.turns.len(), 4);
    assert_eq!(export.provenance.len(), 4);
    assert_eq!(export.session.id, blend.session.unwrap());
    assert_eq!(export.sources.len(), 2);
    assert_eq!(
        export
            .turns
            .iter()
            .filter(|turn| turn.id == root.turn.unwrap())
            .count(),
        1
    );
    let saved = export
        .turns
        .iter()
        .find(|turn| turn.id == root.turn.unwrap())
        .unwrap();
    assert!(
        saved
            .request
            .state
            .iter()
            .any(|input| matches!(input,Input::Text {text} if text==&original))
    );
    assert_eq!(
        saved.request.questions[0].answer_space,
        turn().questions[0].answer_space
    );
    assert_eq!(
        saved.request.questions[2].answer_space,
        turn().questions[2].answer_space
    );
    assert_eq!(saved.outcome.outcomes, root.outcome.outcomes);
    let ContextView::Resolved { records } = &export.context else {
        panic!("resolved context")
    };
    assert_eq!(records.len(), 3);
    assert!(
        records
            .iter()
            .any(|record| record.record.id == pinned.id && record.pinned)
    );
    assert!(
        records
            .iter()
            .any(|record| record.record.kind == ContextKind::Evidence)
    );
    assert!(
        records
            .iter()
            .any(|record| record.record.kind == ContextKind::Commitment)
    );
    let json = String::from_utf8(output.stdout).unwrap();
    assert!(!json.contains("AFTER_CAPTURE"));
    assert!(!json.contains("UNRELATED_PRIVATE_STATE"));
    assert!(json.contains("NOULI_FIXTURE_KEY"));
    assert!(!json.contains("EXPORT_PRIVATE_TOKEN"));
    assert_eq!(export.attempts.len(), 4);
    assert_eq!(export.requests.len(), 4);
    let toon = home.run(&["sessions", "export", "Diamond", "--format", "toon"]);
    assert!(toon.status.success());
    let decoded: SessionExport = codec::decode(&toon.stdout, codec::InputFormat::Toon).unwrap();
    assert_eq!(decoded, export);
    let context = home.run(&[
        "context",
        "Diamond",
        "--turn",
        &blend.turn.unwrap().to_string(),
        "--json",
    ]);
    assert!(
        context.status.success(),
        "exit={:?} stdout={} stderr={}",
        context.status.code(),
        String::from_utf8_lossy(&context.stdout),
        String::from_utf8_lossy(&context.stderr)
    );
    let context: TurnInspection = serde_json::from_slice(&context.stdout).unwrap();
    assert_eq!(context.turn.manifest, blend.manifest);
    assert_eq!(context.attempts.len(), 1);
    assert_eq!(context.turn.outcome.usage, blend.outcome.usage);
    let foreign = home.run(&[
        "context",
        "Diamond",
        "--turn",
        &root.turn.unwrap().to_string(),
        "--json",
    ]);
    assert_eq!(foreign.status.code(), Some(2));
    let list = home.run(&["sessions", "list", "--format", "text"]);
    assert!(list.status.success());
    let text = String::from_utf8(list.stdout).unwrap();
    assert!(text.contains("Diamond"));
    assert!(text.contains("Unrelated"));
    let shown = home.run(&["sessions", "show", "Diamond", "--format", "text"]);
    assert!(shown.status.success());
    assert!(
        String::from_utf8(shown.stdout)
            .unwrap()
            .contains("context_toon")
    );
    assert_eq!(
        before,
        std::fs::read(home.store().join("ledger.sqlite3")).unwrap()
    );
    assert_eq!(server.count(), count);
    let reopened = Ledger::open(&home.store(), OpenMode::ReadOnlyExisting).unwrap();
    assert_eq!(
        reopened
            .export_session(&SessionSelector::Id(blend.session.unwrap()))
            .unwrap(),
        export
    );
}
#[test]
fn inspection_missing_store_never_creates_files_or_requires_provider_settings() {
    let root = std::env::temp_dir().join(format!("nouli-inspect-missing-{}", Id::generate()));
    let output = Command::new(env!("CARGO_BIN_EXE_nouli"))
        .env_clear()
        .env("HOME", &root)
        .arg("--store")
        .arg(root.join("store"))
        .args(["sessions", "list", "--json"])
        .output()
        .unwrap();
    assert_eq!(
        output.status.code(),
        Some(6),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stderr.is_empty());
    let error: ResultEnvelope = serde_json::from_slice(&output.stdout).unwrap();
    assert!(matches!(error.result,Report::Error(error) if error.code=="missing_ledger"));
    assert!(!root.exists());
}

#[test]
fn readonly_inspection_rejects_newer_and_migration_required_stores_without_changes() {
    for (version, code) in [
        (999, "newer_ledger_schema"),
        (1, "ledger_migration_required"),
    ] {
        let home = Home::new();
        std::fs::write(home.0.join("config.toml"), "").unwrap();
        let mut ledger = Ledger::open(&home.store(), OpenMode::ReadWriteCreate).unwrap();
        ledger
            .transaction::<_, nouli_core::storage::StorageError>("set test schema version", |tx| {
                tx.pragma_update(None, "user_version", version).unwrap();
                Ok(())
            })
            .unwrap();
        drop(ledger);
        let before = std::fs::read(home.store().join("ledger.sqlite3")).unwrap();
        let output = home.run(&["sessions", "list", "--json"]);
        assert_eq!(output.status.code(), Some(6));
        let error: ResultEnvelope = serde_json::from_slice(&output.stdout).unwrap();
        assert!(matches!(error.result,Report::Error(error) if error.code==code));
        assert_eq!(
            before,
            std::fs::read(home.store().join("ledger.sqlite3")).unwrap()
        );
    }
}
