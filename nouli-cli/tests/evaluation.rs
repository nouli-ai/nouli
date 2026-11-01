#[path = "support/harness.rs"]
#[allow(dead_code, reason = "each test uses a subset of the helpers")]
mod harness;
#[path = "support/loopback.rs"]
#[allow(dead_code, reason = "each test uses a subset of the helpers")]
mod loopback;
use harness::{
    Home, QUESTION, context_record, jsonl_events, ledger_bytes, report, run_report, turn, workflow,
};
use loopback::Server;
use nouli_core::{
    codec,
    context::{ContextKind, ContextRecord, ContextUpdate},
    data::{Data, Input},
    history::{EvaluationRequest, ModelSelection, SessionSelection, StandingChanges},
    identity::Id,
    protocol::{Report, ResultEnvelope},
    storage::{Ledger, OpenMode},
};
use std::fmt::Write as _;
use std::{
    io::Write,
    process::{Command, Stdio},
};
#[test]
fn persisted_continue_branch_blend_fixed_heads_and_ephemeral_use_the_shared_engine() {
    let server = Server::with_handler(harness::typesafe);
    let home = Home::create();
    home.configure(&server, "typesafe", false);
    let seed = report(
        &home.run(
            [
                &["new", "Origin", "ORIGINAL"][..],
                &QUESTION,
                &["--request-id", "seed"],
            ]
            .concat(),
        ),
    );
    let first = seed.turn.unwrap();
    let original = seed.session.unwrap();
    // A later turn asks the question it is given, here the same one again.
    let next = report(&home.run([&["add", "Origin", "LATER"][..], &QUESTION].concat()));
    assert_eq!(next.session, Some(original));
    assert_eq!(next.questions, seed.questions);
    let source = format!("Origin@{first}");
    let branch = report(
        &home.run(
            [
                &["new", "Branch", "BRANCH", "--from", &source][..],
                &QUESTION,
            ]
            .concat(),
        ),
    );
    assert_ne!(branch.session, Some(original));
    assert_eq!(branch.manifest.sources[0].frontier, vec![first]);
    assert_eq!(branch.questions, seed.questions);
    let blend = report(
        &home.run(
            [
                &[
                    "new", "Blend", "BLEND", "--from", "Branch", "--from", "Origin",
                ][..],
                &QUESTION,
            ]
            .concat(),
        ),
    );
    assert_eq!(blend.manifest.sources.len(), 2);
    let bytes = ledger_bytes(&home);
    let ephemeral = report(&home.run([&["whatif", "Blend", "TEMPORARY"][..], &QUESTION].concat()));
    assert!(ephemeral.session.is_none());
    assert!(ephemeral.turn.is_none());
    assert!(
        bytes == ledger_bytes(&home),
        "Read-only operations changed ledger bytes"
    );
    // A what-if needs a saved session, and never creates a store to look for one.
    let fresh_store = home.0.join("never-created");
    let fresh = home
        .command_at(&fresh_store)
        .args([&["whatif", "Blend", "FRESH"][..], &QUESTION].concat())
        .output()
        .unwrap();
    assert_eq!(fresh.status.code(), Some(6));
    assert!(!fresh_store.exists());
    let no_home = Command::new(env!("CARGO_BIN_EXE_nouli"))
        .env_clear()
        .arg("--config")
        .arg(home.0.join("config.toml"))
        .arg("--store")
        .arg(&fresh_store)
        .args([&["whatif", "Blend", "FRESH"][..], &QUESTION].concat())
        .output()
        .unwrap();
    assert_eq!(no_home.status.code(), Some(6));
    assert!(!fresh_store.exists());

    assert_eq!(server.count(), 5);
    let calls = server.calls.lock().unwrap();
    assert!(calls[1]["state"].as_str().unwrap().contains("ORIGINAL"));
    assert!(!calls[2]["state"].as_str().unwrap().contains("LATER"));
    assert!(calls[4]["state"].as_str().unwrap().contains("BLEND"));
}
#[test]
fn full_file_updates_json_toon_stdin_and_normalized_replay_preserve_complete_input() {
    let server = Server::with_handler(harness::typesafe);
    let home = Home::create();
    home.configure(&server, "typesafe", false);
    let record = ContextRecord {
        id: Id::generate(),
        key: "budget".into(),
        value: Data::new(serde_json::json!(18_446_744_073_709_551_615_u64)).unwrap(),
        kind: ContextKind::Commitment,
    };
    let path = home.request(
        "request.json",
        "FULL_STATE",
        vec![ContextUpdate::Append {
            record: record.clone(),
        }],
    );
    let first = report(&home.run([
        "new",
        "Full",
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
    let mut file: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert!(file.get("session").is_none());
    let request = EvaluationRequest {
        standing: StandingChanges::default(),
        turn: serde_json::from_value(file["turn"].clone()).unwrap(),
        session: SessionSelection::New { name: None },
        updates: serde_json::from_value(file["updates"].clone()).unwrap(),
        required_records: vec![],
        model: ModelSelection::default(),
        request_id: None,
    };
    let toon = home.0.join("same.toon");
    std::fs::write(&toon, codec::encode_toon(&file).unwrap()).unwrap();
    let replay = report(&home.run([
        "new",
        "Full",
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
    // A what-if starts from the saved session, so its update needs a new record.
    file["updates"][0]["record"]["id"] = serde_json::json!(Id::generate());
    file["updates"][0]["record"]["key"] = serde_json::json!("whatif-only");
    cmd.args(["whatif", "Full", "--input", "-", "--input-format", "json"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = cmd.spawn().unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(&serde_json::to_vec(&file).unwrap())
        .unwrap();
    let result = report(&child.wait_with_output().unwrap());
    assert!(result.turn.is_none());
    assert_eq!(server.count(), 2);
    // A request file that names a session is refused: the verb supplies it.
    file["session"] = serde_json::json!({"mode": "new", "name": null});
    let named = home.0.join("named.json");
    std::fs::write(&named, serde_json::to_vec(&file).unwrap()).unwrap();
    let refused = home.run(["add", "Full", "--input", named.to_str().unwrap()]);
    assert_eq!(refused.status.code(), Some(2));
    assert_eq!(server.count(), 2);
}
#[test]
fn invalid_selection_modes_names_budget_credentials_and_request_conflicts_do_not_dispatch() {
    let server = Server::with_handler(harness::typesafe);
    let home = Home::create();
    home.configure(&server, "typesafe", true);
    let named = [
        &["new", "Named"][..],
        &QUESTION,
        &["--request-id", "credentials"],
    ]
    .concat();
    let missing = home.run(&named);
    assert_eq!(missing.status.code(), Some(3));
    assert_eq!(server.count(), 0);
    assert!(!home.store().exists());
    let recovered = report(
        &home
            .command()
            .args(&named)
            .env("NOULI_FIXTURE_KEY", "SECRET_FIXTURE_TOKEN")
            .output()
            .unwrap(),
    );
    assert!(recovered.turn.is_some());
    assert_eq!(server.count(), 1);
    for (args, code) in [
        ([&["add", "Unknown"][..], &QUESTION].concat(), 2),
        ([&["new", "Named"][..], &QUESTION].concat(), 5),
        (
            [
                &["new", "Named", "changed"][..],
                &QUESTION,
                &["--request-id", "credentials"],
            ]
            .concat(),
            5,
        ),
        // A what-if has no request identity to replay.
        (vec!["whatif", "Named", "--request-id", "ephemeral"], 2),
        (vec!["add", "Named", "--retry-interrupted"], 2),
        // A request file cannot be combined with facts or questions.
        (
            [&["add", "Named", "--input", "unused.json"][..], &QUESTION].concat(),
            2,
        ),
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
        // `harness::turn()` writes plain score labels, as the compact grammar
        // would, so the router's legend must match them.
        let mut body: serde_json::Value = serde_json::from_str(response).unwrap();
        if !full_questions {
            body["answers"]["quality"]["legend"]["1"] = serde_json::json!("Adequate");
        }
        let server = Server::new(Box::leak(body.to_string().into_boxed_str()));
        let home = Home::create();
        home.configure(&server, protocol, false);
        // Several questions in one call come from a request file.
        let mut turn = turn();
        if full_questions {
            turn.questions.push(
                nouli_core::decision::grammar::parse("private: Is private information available?")
                    .unwrap(),
            );
        }
        let path = home.0.join("request.json");
        std::fs::write(&path, serde_json::to_vec(&turn).unwrap()).unwrap();
        let result = report(&home.run([
            "new",
            "Dispatch",
            "--input",
            path.to_str().unwrap(),
            "--provider",
            "local",
        ]));
        assert_eq!(result.outcome.provider.model, expected);
        assert_eq!(server.count(), 1);
        assert!(result.session.is_some());
        if full_questions {
            assert!(matches!(
                result.outcome.outcomes[3],
                nouli_core::outcome::QuestionOutcome::Refused { .. }
            ));
        }
    }
}
#[test]
fn default_model_budget_works_and_competing_file_modes_fail_before_inference() {
    let server = Server::with_handler(harness::typesafe);
    let home = Home::create();
    home.configure(&server, "typesafe", false);
    let path = home.0.join("config.toml");
    let config = std::fs::read_to_string(&path)
        .unwrap()
        .replace("context_budget_tokens = 100000\n", "")
        .replace("context_window_tokens = 100000\n", "");
    std::fs::write(&path, config).unwrap();
    let create = [&["new", "Budgeted"][..], &QUESTION].concat();
    let defaulted = report(&home.run(&create));
    assert_eq!(defaulted.manifest.budget, 8192);
    assert_eq!(server.count(), 1);
    let valid = report(
        &home.run(
            [
                &["new", "ExplicitBudget"][..],
                &QUESTION,
                &["--context-budget", "90000"],
            ]
            .concat(),
        ),
    );
    assert_eq!(valid.manifest.budget, 90000);
    assert_eq!(server.count(), 2);
    let file = home.request("competing.json", "unused", vec![]);
    // A request file that names its own session competes with the verb.
    let mut value: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();
    value["session"] = serde_json::json!({"mode": "new", "name": null});
    let competing = home.0.join("session-named.json");
    std::fs::write(&competing, serde_json::to_vec(&value).unwrap()).unwrap();
    let output = home.run(["new", "Other", "--input", competing.to_str().unwrap()]);
    assert_eq!(output.status.code(), Some(2));
    assert_eq!(server.count(), 2);
    assert_eq!(before_count(&home), 2);
}

/// Runs a command with the provider credential in its environment.
fn keyed(home: &Home, args: &[&str]) -> std::process::Output {
    home.command()
        .args(args)
        .env("NOULI_FIXTURE_KEY", "EXPORT_PRIVATE_TOKEN")
        .output()
        .unwrap()
}
/// The sessions of a diamond: a `Root` with a pinned budget and a large state,
/// `Left` and `Right` branches from it, and their compacted `Diamond` blend.
struct Diamond {
    pinned: ContextRecord,
    original: String,
    root: nouli_core::protocol::TurnReport,
    blend: nouli_core::protocol::TurnReport,
}
fn diamond(home: &Home) -> Diamond {
    use nouli_core::history::Representation;
    let pinned = context_record("budget", serde_json::json!(8), ContextKind::Commitment);
    let original = "ARCHIVED_EVIDENCE_".repeat(3000);
    let root = home.request(
        "root.json",
        &original,
        vec![ContextUpdate::Pin {
            target: nouli_core::context::PinTarget::New {
                record: pinned.clone(),
            },
        }],
    );
    let root = report(&keyed(
        home,
        &["new", "Root", "--input", root.to_str().unwrap()],
    ));
    for (name, kind) in [
        ("Left", ContextKind::Evidence),
        ("Right", ContextKind::Commitment),
    ] {
        let record = context_record(&name.to_lowercase(), serde_json::json!(name), kind);
        let path = home.request(
            &format!("{name}.json"),
            name,
            vec![ContextUpdate::Append { record }],
        );
        report(&keyed(
            home,
            &[
                "new",
                name,
                "--from",
                &root.session.unwrap().to_string(),
                "--input",
                path.to_str().unwrap(),
            ],
        ));
    }
    let blend = report(&keyed(
        home,
        &[
            &[
                "new", "Diamond", "BLENDED", "--from", "Left", "--from", "Right",
            ][..],
            &QUESTION,
            &["--context-budget", "2500"],
        ]
        .concat(),
    ));
    assert!(
        blend
            .manifest
            .records
            .iter()
            .any(|record| record.record == root.turn.unwrap()
                && record.representation != Representation::Full)
    );
    Diamond {
        pinned,
        original,
        root,
        blend,
    }
}
#[test]
fn readonly_diamond_exports_preserve_originals_after_compaction_and_exclude_later_or_unrelated_work()
 {
    use nouli_core::{identity::SessionSelector, storage::inspection::SessionExport};
    let server = Server::with_handler(harness::typesafe);
    let home = Home::create();
    home.configure(&server, "typesafe", true);
    let diamond = diamond(&home);
    report(&keyed(
        &home,
        &[&["add", "Root", "AFTER_CAPTURE"][..], &QUESTION].concat(),
    ));
    report(&keyed(
        &home,
        &[
            &["new", "Unrelated", "UNRELATED_PRIVATE_STATE"][..],
            &QUESTION,
        ]
        .concat(),
    ));
    let count = server.count();
    let before = ledger_bytes(&home);
    // A changed/invalid provider profile must not block store-only inspection.
    let config = std::fs::read_to_string(home.0.join("config.toml"))
        .unwrap()
        .replace(
            "protocol = \"system-one\"",
            "protocol = \"removed-protocol\"",
        );
    std::fs::write(home.0.join("config.toml"), config).unwrap();
    let output = home.run(["sessions", "export", "Diamond", "--format", "json"]);
    assert!(
        output.status.success(),
        "{:?}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&output.stderr), "");
    let export: SessionExport = serde_json::from_slice(&output.stdout).unwrap();
    assert_export_keeps_captured_originals(&export, &diamond);
    let json = String::from_utf8(output.stdout).unwrap();
    assert!(!json.contains("AFTER_CAPTURE"));
    assert!(!json.contains("UNRELATED_PRIVATE_STATE"));
    assert!(json.contains("NOULI_FIXTURE_KEY"));
    assert!(!json.contains("EXPORT_PRIVATE_TOKEN"));
    let toon = home.run(["sessions", "export", "Diamond", "--format", "toon"]);
    assert!(toon.status.success());
    let decoded: SessionExport = codec::decode(&toon.stdout, codec::InputFormat::Toon).unwrap();
    assert_eq!(decoded, export);
    assert_turn_context_is_the_blend_own(&home, &diamond);
    let list = home.run(["sessions", "list", "--format", "text"]);
    assert!(list.status.success());
    let text = String::from_utf8(list.stdout).unwrap();
    assert!(text.contains("Diamond"));
    assert!(text.contains("Unrelated"));
    let shown = home.run(["sessions", "show", "Diamond", "--format", "text"]);
    assert!(shown.status.success());
    assert!(
        String::from_utf8(shown.stdout)
            .unwrap()
            .contains("context_toon")
    );
    assert!(
        before == ledger_bytes(&home),
        "Read-only operations changed ledger bytes"
    );
    assert_eq!(server.count(), count);
    let reopened = Ledger::open(&home.store(), OpenMode::ReadOnlyExisting).unwrap();
    assert_eq!(
        reopened
            .export_session(&SessionSelector::Id(diamond.blend.session.unwrap()))
            .unwrap(),
        export
    );
}
/// The export holds each captured turn once, the root turn with its original
/// uncompacted request, and the blend's resolved context.
fn assert_export_keeps_captured_originals(
    export: &nouli_core::storage::inspection::SessionExport,
    diamond: &Diamond,
) {
    use nouli_core::storage::inspection::ContextView;
    let root = diamond.root.turn.unwrap();
    assert_eq!(export.turns.len(), 4);
    assert_eq!(export.provenance.len(), 4);
    assert_eq!(export.session.id, diamond.blend.session.unwrap());
    assert_eq!(export.sources.len(), 2);
    assert_eq!(
        export.turns.iter().filter(|turn| turn.id == root).count(),
        1
    );
    let saved = export.turns.iter().find(|turn| turn.id == root).unwrap();
    assert!(
        saved
            .request
            .state
            .iter()
            .any(|input| matches!(input,Input::Text {text} if text==&diamond.original))
    );
    assert_eq!(
        saved.request.questions[0].answer_space,
        turn().questions[0].answer_space
    );
    assert_eq!(
        saved.request.questions[2].answer_space,
        turn().questions[2].answer_space
    );
    assert_eq!(saved.outcome.outcomes, diamond.root.outcome.outcomes);
    let ContextView::Resolved { records } = &export.context else {
        panic!("resolved context")
    };
    // The pinned budget, both branches' records, and the blend's own fact.
    assert_eq!(records.len(), 4);
    assert!(
        records
            .iter()
            .any(|record| record.record.id == diamond.pinned.id && record.pinned)
    );
    assert_eq!(
        records
            .iter()
            .filter(|record| record.record.key.starts_with("fact:"))
            .count(),
        1
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
    assert_eq!(export.attempts.len(), 4);
    assert_eq!(export.requests.len(), 4);
}
/// `context` shows the blend turn as dispatched, and refuses a turn the
/// session only inherited.
fn assert_turn_context_is_the_blend_own(home: &Home, diamond: &Diamond) {
    let context = home.run([
        "context",
        "Diamond",
        "--turn",
        &diamond.blend.turn.unwrap().to_string(),
        "--json",
    ]);
    assert!(
        context.status.success(),
        "exit={:?} stdout={} stderr={}",
        context.status.code(),
        String::from_utf8_lossy(&context.stdout),
        String::from_utf8_lossy(&context.stderr)
    );
    let context: nouli_core::storage::inspection::TurnInspection =
        serde_json::from_slice(&context.stdout).unwrap();
    assert_eq!(context.turn.manifest, diamond.blend.manifest);
    assert_eq!(context.attempts.len(), 1);
    assert_eq!(context.turn.outcome.usage, diamond.blend.outcome.usage);
    let foreign = home.run([
        "context",
        "Diamond",
        "--turn",
        &diamond.root.turn.unwrap().to_string(),
        "--json",
    ]);
    assert_eq!(foreign.status.code(), Some(2));
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
    assert_eq!(String::from_utf8_lossy(&output.stderr), "");
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
        let home = Home::create();
        std::fs::write(home.0.join("config.toml"), "").unwrap();
        let mut ledger = Ledger::open(&home.store(), OpenMode::ReadWriteCreate).unwrap();
        ledger
            .transaction::<_, nouli_core::storage::StorageError>("set test schema version", |tx| {
                tx.pragma_update(None, "user_version", version).unwrap();
                Ok(())
            })
            .unwrap();
        drop(ledger);
        let before = ledger_bytes(&home);
        let output = home.run(["sessions", "list", "--json"]);
        assert_eq!(output.status.code(), Some(6));
        let error: ResultEnvelope = serde_json::from_slice(&output.stdout).unwrap();
        assert!(matches!(error.result,Report::Error(error) if error.code==code));
        assert_eq!(before, ledger_bytes(&home));
    }
}

#[test]
fn workflow_cli_conditional_join_streaming_and_ephemeral_paths_keep_typed_outputs() {
    use nouli_core::workflow::{NodeStatus, RunStatus};
    let server = Server::with_handler(harness::typesafe);
    let home = Home::create();
    home.configure(&server, "typesafe", false);
    let file = home.0.join("workflow.json");
    std::fs::write(&file, serde_json::to_vec(&workflow()).unwrap()).unwrap();
    let result = run_report(
        &home.run([
            "workflow",
            "run",
            file.to_str().unwrap(),
            "--name",
            "Travel",
            "--parallel",
            "2",
            "--request-id",
            "conditional",
        ]),
        0,
    );
    assert_eq!(result.status, RunStatus::Completed);
    assert_eq!(result.nodes[2].status, NodeStatus::Skipped);
    assert_eq!(
        result.outputs[0].value,
        nouli_core::workflow::reference::ReferenceValue::Present {
            value: Data::new(serde_json::json!(1.43)).unwrap()
        }
    );
    assert!(result.nodes[4].turn.is_some());
    assert_eq!(server.count(), 4);
    let bytes = ledger_bytes(&home);
    let run = result.run.unwrap();
    assert_show_keeps_frozen_settings_and_skipped_node(&home, run);
    assert_resume_of_a_completed_run_replays(&home, run);
    assert_eq!(server.count(), 4);
    assert_ephemeral_run_from_a_session_saves_nothing(&home, &file);
    assert!(
        bytes == ledger_bytes(&home),
        "Read-only operations changed ledger bytes"
    );
    let fresh = home.0.join("fresh-workflow-store");
    let result = run_report(
        &home
            .command_at(&fresh)
            .args(["workflow", "run", file.to_str().unwrap(), "--no-session"])
            .output()
            .unwrap(),
        0,
    );
    assert!(!result.persisted);
    assert!(result.run.is_none());
    assert!(!fresh.exists());
}
fn assert_show_keeps_frozen_settings_and_skipped_node(home: &Home, run: Id) {
    let output = home.run(["workflow", "show", &run.to_string(), "--format", "toon"]);
    assert!(output.status.success());
    let shown: nouli_core::storage::workflows::recovery::WorkflowInspection =
        codec::decode(&output.stdout, codec::InputFormat::Toon).unwrap();
    assert_eq!(shown.rounds.len(), 1);
    assert!(
        shown
            .inspection
            .run
            .frozen
            .settings
            .iter()
            .all(|settings| settings.execution.max_parallel == 2)
    );
    assert_eq!(
        shown.inspection.run.nodes[2].status,
        nouli_core::workflow::NodeStatus::Skipped
    );
    assert_eq!(before_count(home), 1);
}
fn assert_resume_of_a_completed_run_replays(home: &Home, run: Id) {
    use nouli_core::protocol::EventKind;
    let replay = home.run(["workflow", "resume", &run.to_string(), "--jsonl"]);
    assert!(replay.status.success());
    let events = jsonl_events(&replay);
    assert_eq!(events.len(), 2);
    assert!(
        matches!(&events.last().unwrap().event,EventKind::Terminal {result} if matches!(&result.result,Report::Run(report) if report.replayed))
    );
}
/// A `--no-session` run from `Travel` reports no run and no node turns.
fn assert_ephemeral_run_from_a_session_saves_nothing(home: &Home, file: &std::path::Path) {
    use nouli_core::protocol::EventKind;
    let ephemeral = home.run([
        "workflow",
        "run",
        file.to_str().unwrap(),
        "--from",
        "Travel",
        "--no-session",
        "--jsonl",
    ]);
    assert!(ephemeral.status.success());
    let events = jsonl_events(&ephemeral);
    assert!(events.iter().all(|event| event.run.is_none()));
    assert!(
        events
            .iter()
            .filter_map(|event| match &event.event {
                EventKind::NodeState { turn, .. } => Some(turn),
                _ => None,
            })
            .all(Option::is_none)
    );
}
fn before_count(home: &Home) -> usize {
    Ledger::open(&home.store(), OpenMode::ReadOnlyExisting)
        .unwrap()
        .list_sessions()
        .unwrap()
        .len()
}
/// A provider whose first request for the `high` node fails with a 503.
fn high_fails_once() -> Server {
    let fail = std::sync::atomic::AtomicBool::new(true);
    Server::with_handler(move |body| {
        if body["state"].as_str().unwrap().contains("NODE:high")
            && fail.swap(false, std::sync::atomic::Ordering::SeqCst)
        {
            (503, "{}".into())
        } else {
            harness::typesafe(body)
        }
    })
}
/// Adds the model aliases that the partial run freezes to the local provider.
fn add_frozen_model_aliases(home: &Home) {
    let config_path = home.0.join("config.toml");
    let mut config = std::fs::read_to_string(&config_path).unwrap();
    for model in ["high-frozen-alias", "run-frozen-alias"] {
        write!(
            config,
            "\n[providers.local.models.'{model}']\ncontext_budget_tokens=100000\nreserved_output_tokens=128\n"
        )
        .unwrap();
    }
    std::fs::write(config_path, config).unwrap();
}
/// After the run is captured: supersedes the source's budget, breaks the
/// workflow file, and removes the provider and model the run used. Returns the
/// replacement budget.
fn change_everything_the_run_froze(
    home: &Home,
    source: &nouli_core::protocol::TurnReport,
    budget: &ContextRecord,
    file: &std::path::Path,
) -> ContextRecord {
    let replacement = context_record("budget", serde_json::json!(12), ContextKind::Commitment);
    let change = home.request(
        "source-change.json",
        "SOURCE_AFTER",
        vec![ContextUpdate::Supersede {
            targets: vec![budget.id],
            replacement: replacement.clone(),
            pinned: None,
        }],
    );
    report(&home.run([
        "add",
        &source.session.unwrap().to_string(),
        "--input",
        change.to_str().unwrap(),
    ]));
    std::fs::write(file, "edited invalid workflow").unwrap();
    let config = std::fs::read_to_string(home.0.join("config.toml"))
        .unwrap()
        .replace("typesafe", "removed-protocol")
        .replace("offline", "changed-model");
    std::fs::write(home.0.join("config.toml"), config).unwrap();
    replacement
}
#[test]
fn workflow_cli_partial_resume_freezes_sources_settings_and_graph_and_retains_completed_siblings() {
    use nouli_core::{
        context::PinTarget,
        protocol::EventKind,
        workflow::{NodeStatus, RunStatus},
    };
    let server = high_fails_once();
    let home = Home::create();
    home.configure(&server, "typesafe", false);
    let budget = context_record("budget", serde_json::json!(8), ContextKind::Commitment);
    let seed = home.request(
        "seed.json",
        "SOURCE_BEFORE",
        vec![ContextUpdate::Pin {
            target: PinTarget::New {
                record: budget.clone(),
            },
        }],
    );
    let source = report(&home.run(["new", "Source", "--input", seed.to_str().unwrap()]));
    let mut definition = workflow();
    definition.nodes[1].when = None;
    definition.nodes[2].when = None;
    definition.nodes[1].model.model = Some("high-frozen-alias".into());
    add_frozen_model_aliases(&home);
    let file = home.0.join("parallel.json");
    std::fs::write(&file, serde_json::to_vec(&definition).unwrap()).unwrap();
    let partial = run_report(
        &home.run([
            "workflow",
            "run",
            file.to_str().unwrap(),
            "--from",
            "Source",
            "--name",
            "Partial",
            "--parallel",
            "2",
            "--model",
            "run-frozen-alias",
        ]),
        4,
    );
    assert_eq!(partial.status, RunStatus::Partial);
    assert_eq!(partial.nodes[1].status, NodeStatus::Failed);
    assert_eq!(partial.nodes[2].status, NodeStatus::Completed);
    let run = partial.run.unwrap();
    let replacement = change_everything_the_run_froze(&home, &source, &budget, &file);
    let before = server.count();
    let resumed = home.run(["workflow", "resume", &run.to_string(), "--jsonl"]);
    assert!(
        resumed.status.success(),
        "{}",
        String::from_utf8_lossy(&resumed.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&resumed.stderr), "");
    let events = jsonl_events(&resumed);
    let EventKind::Terminal { result } = &events.last().unwrap().event else {
        panic!("terminal")
    };
    let Report::Run(done) = &result.result else {
        panic!("run")
    };
    assert_eq!(done.status, RunStatus::Completed);
    assert_eq!(done.nodes[0].turn, partial.nodes[0].turn);
    assert_eq!(done.nodes[2].turn, partial.nodes[2].turn);
    assert_eq!(server.count() - before, 3);
    assert_eq!(done.sources[0].frontier, vec![source.turn.unwrap()]);
    let calls = server.calls.lock().unwrap();
    let high = &calls[before];
    assert_eq!(high["model"], "high-frozen-alias");
    assert_eq!(calls[1]["model"], "run-frozen-alias");
    assert_eq!(
        done.nodes[1].provider.as_ref().unwrap().model,
        "laya-english-fixture"
    );
    let context = high["state"].as_str().unwrap();
    assert!(context.contains(&budget.id.to_string()));
    assert!(!context.contains(&replacement.id.to_string()));
    assert!(!context.contains("SOURCE_AFTER"));
    drop(calls);
    let show = home.run(["workflow", "show", &run.to_string(), "--json"]);
    assert!(show.status.success());
    let inspection: nouli_core::storage::workflows::recovery::WorkflowInspection =
        serde_json::from_slice(&show.stdout).unwrap();
    assert_eq!(inspection.rounds.len(), 2);
    assert_eq!(inspection.inspection.nodes[1].attempts.len(), 2);
}

#[test]
fn workflow_resume_rejects_advanced_head_and_invalid_modes_before_dispatch() {
    use nouli_core::workflow::NodeStatus;
    let server = high_fails_once();
    let home = Home::create();
    home.configure(&server, "typesafe", false);
    let file = home.0.join("workflow.json");
    std::fs::write(&file, serde_json::to_vec(&workflow()).unwrap()).unwrap();
    let partial = run_report(&home.run(["workflow", "run", file.to_str().unwrap()]), 4);
    assert_eq!(partial.nodes[1].status, NodeStatus::Failed);
    let run = partial.run.unwrap();
    report(
        &home.run(
            [
                &["add", &partial.session.unwrap().to_string()][..],
                &QUESTION,
            ]
            .concat(),
        ),
    );
    let count = server.count();
    let bytes = ledger_bytes(&home);
    let conflict = home.run(["workflow", "resume", &run.to_string()]);
    assert_eq!(conflict.status.code(), Some(5));
    let error: ResultEnvelope = serde_json::from_slice(&conflict.stdout).unwrap();
    assert!(matches!(error.result,Report::Error(error) if error.code=="advanced_session_head"));
    assert_eq!(error.recovery.unwrap().run, Some(run));
    assert_eq!(server.count(), count);
    assert_eq!(bytes, ledger_bytes(&home));
    for args in [
        vec![
            "workflow",
            "run",
            file.to_str().unwrap(),
            "--no-session",
            "--name",
            "Invalid",
        ],
        vec![
            "workflow",
            "run",
            file.to_str().unwrap(),
            "--no-session",
            "--request-id",
            "invalid",
        ],
        vec!["workflow", "run", file.to_str().unwrap(), "--parallel", "0"],
    ] {
        let output = home.run(&args);
        assert_eq!(output.status.code(), Some(2));
        assert_eq!(server.count(), count);
    }
    let mut definition = workflow();
    definition.nodes[0].dependencies = vec!["final".into()];
    std::fs::write(&file, serde_json::to_vec(&definition).unwrap()).unwrap();
    let output = home.run(["workflow", "run", file.to_str().unwrap(), "--jsonl"]);
    assert_eq!(output.status.code(), Some(2));
    jsonl_events(&output);
    assert_eq!(server.count(), count);
}
#[test]
fn workflow_files_may_mix_compact_and_object_questions_and_name_the_node_on_errors() {
    use nouli_core::workflow::RunStatus;
    let server = Server::with_handler(harness::typesafe);
    let home = Home::create();
    home.configure(&server, "typesafe", false);
    let mut graph = serde_json::to_value(workflow()).unwrap();
    // The first node is written in the grammar; the rest stay full objects.
    graph["nodes"][0]["turn"]["questions"] = serde_json::json!([
        "travel: Which route? [train, flight]",
        "on_time: Will the train arrive on time?",
        {
            "id": "quality",
            "prompt": "Rate the route quality",
            "answer_space": {"type": "score", "levels": ["Poor", "Adequate", "Excellent"]}
        }
    ]);
    let file = home.0.join("mixed.json");
    std::fs::write(&file, serde_json::to_vec(&graph).unwrap()).unwrap();
    let done = run_report(
        &home.run(["workflow", "run", file.to_str().unwrap(), "--no-session"]),
        0,
    );
    assert_eq!(done.status, RunStatus::Completed);
    let calls = server.calls.lock().unwrap();
    assert_eq!(
        calls[0]["questions"]["travel"]["instructions"],
        "Which route?"
    );
    drop(calls);
    // The same file in TOON works the same way.
    let toon = home.0.join("mixed.toon");
    std::fs::write(&toon, codec::encode_toon(&graph).unwrap()).unwrap();
    run_report(
        &home.run(["workflow", "run", toon.to_str().unwrap(), "--no-session"]),
        0,
    );
    let count = server.count();
    graph["nodes"][1]["turn"]["questions"][1] = serde_json::json!("SECRET without an id");
    std::fs::write(&file, serde_json::to_vec(&graph).unwrap()).unwrap();
    let output = home.run(["workflow", "run", file.to_str().unwrap(), "--no-session"]);
    assert_eq!(output.status.code(), Some(2));
    let envelope: ResultEnvelope = serde_json::from_slice(&output.stdout).unwrap();
    let Report::Error(error) = envelope.result else {
        panic!("input error")
    };
    let id = graph["nodes"][1]["id"].as_str().unwrap();
    assert!(
        error.message.contains(&format!("node {id}, question 1")),
        "{error:?}"
    );
    assert!(!error.message.contains("SECRET"));
    assert_eq!(server.count(), count);
}
#[test]
fn workflow_toon_specs_and_complete_stdin_requests_share_the_same_boundary() {
    use nouli_core::{
        protocol::{EngineEvent, EventKind, validate_events},
        workflow::WorkflowRequest,
    };
    let server = Server::with_handler(harness::typesafe);
    let home = Home::create();
    home.configure(&server, "typesafe", false);
    let spec = workflow();
    let file = home.0.join("workflow.toon");
    std::fs::write(&file, codec::encode_toon(&spec).unwrap()).unwrap();
    let report = run_report(
        &home.run(["workflow", "run", file.to_str().unwrap(), "--no-session"]),
        0,
    );
    assert!(report.run.is_none());
    let request = WorkflowRequest {
        workflow: spec,
        session: SessionSelection::New { name: None },
        updates: vec![],
        required_records: vec![],
        request_id: None,
    };
    let mut command = home.command();
    command
        .args([
            "workflow",
            "run",
            "-",
            "--input-format",
            "json",
            "--no-session",
            "--jsonl",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command.spawn().unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(&serde_json::to_vec(&request).unwrap())
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success());
    assert_eq!(String::from_utf8_lossy(&output.stderr), "");
    let events = String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str::<EngineEvent>(line).unwrap())
        .collect::<Vec<_>>();
    validate_events(&events).unwrap();
    assert!(
        matches!(&events.last().unwrap().event,EventKind::Terminal {result} if matches!(&result.result,Report::Run(report) if !report.persisted))
    );
    assert!(!home.store().exists());
    assert_eq!(server.count(), 8);
}

#[cfg(unix)]
#[path = "support/process.rs"]
#[allow(dead_code, reason = "each test uses a subset of the helpers")]
mod process;
#[cfg(unix)]
#[test]
fn workflow_ctrl_c_streams_recoverable_interruption_and_releases_process_guards() {
    use nouli_core::{
        protocol::{EngineEvent, EventKind, validate_events},
        workflow::{NodeStatus, RunStatus},
    };
    use std::{sync::mpsc, time::Duration};
    let (started, waiting) = mpsc::sync_channel(1);
    let (release, held) = mpsc::sync_channel(1);
    let server = Server::with_handler(move |body| {
        started.send(()).unwrap();
        held.recv_timeout(Duration::from_secs(5)).unwrap();
        harness::typesafe(body)
    });
    let home = Home::create();
    home.configure(&server, "typesafe", false);
    let file = home.0.join("workflow.json");
    std::fs::write(&file, serde_json::to_vec(&workflow()).unwrap()).unwrap();
    let mut command = home.command();
    command
        .args(["workflow", "run", file.to_str().unwrap(), "--jsonl"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let child = process::OwnedChild::new(command.spawn().unwrap());
    waiting.recv_timeout(Duration::from_secs(5)).unwrap();
    assert!(
        Command::new("kill")
            .args(["-INT", &child.id().to_string()])
            .status()
            .unwrap()
            .success()
    );
    let output = child.output(Duration::from_secs(5));
    release.send(()).unwrap();
    assert_eq!(output.status.code(), Some(130));
    assert_eq!(String::from_utf8_lossy(&output.stderr), "");
    let events = String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str::<EngineEvent>(line).unwrap())
        .collect::<Vec<_>>();
    validate_events(&events).unwrap();
    let EventKind::Terminal { result } = &events.last().unwrap().event else {
        panic!("terminal")
    };
    let Report::Run(report) = &result.result else {
        panic!("saved run")
    };
    assert_eq!(report.status, RunStatus::Interrupted);
    assert!(
        report
            .nodes
            .iter()
            .all(|node| node.status == NodeStatus::Interrupted)
    );
    assert_eq!(server.count(), 1);
    let ledger = Ledger::open(&home.store(), OpenMode::ReadOnlyExisting).unwrap();
    let run = ledger.inspect_workflow_full(report.run.unwrap()).unwrap();
    assert!(!run.inspection.live_owner);
    assert_eq!(run.inspection.nodes[0].uncertain_prior_calls, 1);
    drop(ledger);
    let mut ledger = Ledger::open(&home.store(), OpenMode::ReadWriteCreate).unwrap();
    let decision = ledger.resume_workflow(report.run.unwrap()).unwrap();
    assert!(matches!(
        decision,
        nouli_core::storage::workflows::WorkflowDecision::Accepted(_)
    ));
}

#[cfg(unix)]
#[test]
fn workflow_backpressure_ctrl_c_exits_without_the_reader_draining_or_closing() {
    use nouli_core::protocol::EngineEvent;
    use std::{
        io::{BufRead, BufReader, Read},
        time::Duration,
    };
    let server = Server::with_handler(harness::typesafe);
    let home = Home::create();
    home.configure(&server, "typesafe", false);
    let mut graph = workflow();
    graph.nodes.truncate(1);
    graph.nodes[0].id = "x".repeat(200_000);
    graph.outputs.clear();
    let file = home.0.join("backpressure.json");
    std::fs::write(&file, serde_json::to_vec(&graph).unwrap()).unwrap();
    let mut command = home.command();
    command
        .args(["workflow", "run", file.to_str().unwrap(), "--jsonl"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = process::OwnedChild::new(command.spawn().unwrap());
    let mut reader = BufReader::new(child.stdout());
    let mut started = String::new();
    reader.read_line(&mut started).unwrap();
    let started_event: EngineEvent = serde_json::from_str(&started).unwrap();
    let run = started_event.run.unwrap();
    // Observing the next frame proves delivery has begun. Leave this reader open
    // without draining the oversized NodeState while cancellation is requested.
    let mut first = [0u8; 1];
    reader.read_exact(&mut first).unwrap();
    assert!(
        Command::new("kill")
            .args(["-INT", &child.id().to_string()])
            .status()
            .unwrap()
            .success()
    );
    let output = child.output(Duration::from_secs(5));
    assert_eq!(output.status.code(), Some(130));
    assert!(
        String::from_utf8(output.stderr)
            .unwrap()
            .contains(&run.to_string())
    );
    let mut remaining = String::new();
    reader.read_to_string(&mut remaining).unwrap();
    let stream = format!("{started}{}{remaining}", char::from(first[0]));
    assert_eq!(stream.matches(r#""type":"terminal""#).count(), 0);
    assert_eq!(server.count(), 0);
    let ledger = Ledger::open(&home.store(), OpenMode::ReadOnlyExisting).unwrap();
    let inspection = ledger.inspect_workflow_full(run).unwrap();
    assert!(!inspection.inspection.live_owner);
    assert_eq!(
        inspection.inspection.nodes[0].attempts,
        [] as [nouli_core::storage::turns::AttemptSnapshot; 0]
    );
    assert_eq!(inspection.inspection.nodes[0].uncertain_prior_calls, 0);
    assert_eq!(
        inspection.inspection.run.status,
        nouli_core::workflow::RunStatus::Interrupted
    );
    assert_eq!(
        inspection
            .events
            .iter()
            .filter(|event| event.event.kind
                == nouli_core::storage::workflows::WorkflowEventKind::RunFinished)
            .count(),
        1
    );
}

#[test]
fn large_jsonl_frames_resume_on_writable_readiness_and_redirect_cleanly_to_files() {
    use nouli_core::protocol::{EngineEvent, validate_events};
    let server = Server::with_handler(harness::typesafe);
    let home = Home::create();
    home.configure(&server, "typesafe", false);
    let mut graph = workflow();
    graph.nodes.truncate(1);
    graph.nodes[0].id = "x".repeat(200_000);
    graph.outputs.clear();
    let file = home.0.join("large.json");
    std::fs::write(&file, serde_json::to_vec(&graph).unwrap()).unwrap();
    let output = home.run([
        "workflow",
        "run",
        file.to_str().unwrap(),
        "--no-session",
        "--jsonl",
    ]);
    assert!(output.status.success());
    assert_eq!(String::from_utf8_lossy(&output.stderr), "");
    let events = String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str::<EngineEvent>(line).unwrap())
        .collect::<Vec<_>>();
    validate_events(&events).unwrap();
    assert_eq!(server.count(), 1);
    let path = home.0.join("redirected.jsonl");
    let output = home
        .command()
        .args([
            "workflow",
            "run",
            file.to_str().unwrap(),
            "--no-session",
            "--jsonl",
        ])
        .stdout(std::fs::File::create(&path).unwrap())
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_eq!(String::from_utf8_lossy(&output.stderr), "");
    let events = std::fs::read_to_string(path)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str::<EngineEvent>(line).unwrap())
        .collect::<Vec<_>>();
    validate_events(&events).unwrap();
    assert_eq!(server.count(), 2);
    let output = home
        .command()
        .args([
            "workflow",
            "run",
            file.to_str().unwrap(),
            "--no-session",
            "--jsonl",
        ])
        .stdout(Stdio::null())
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_eq!(String::from_utf8_lossy(&output.stderr), "");
    assert_eq!(server.count(), 3);
    assert!(!home.store().exists());
}

#[test]
fn workflow_continuing_a_session_runs_only_its_providers_credential_command() {
    let server = Server::with_handler(harness::typesafe);
    let home = Home::create();
    let provider = |name: &str| {
        format!(
            "[providers.{name}]\nprotocol = \"system-one\"\nbase_url = \"{}\"\n[providers.{name}.models.offline]\ncontext_budget_tokens = 100000\ncontext_window_tokens = 100000\nreserved_output_tokens = 128\n",
            server.url
        )
    };
    std::fs::write(
        home.0.join("config.toml"),
        format!(
            "default_provider = \"alpha\"\ndefault_model = \"offline\"\n[execution]\nmax_attempts = 1\nattempt_timeout_seconds = 5\n{}{}",
            provider("alpha"),
            provider("beta")
        ),
    )
    .unwrap();
    let runs_file = |name: &str| home.0.join(format!("{name}-runs"));
    // Each credential command records a line per run, then prints its key.
    for name in ["alpha", "beta"] {
        let script = format!(
            "echo run >> '{}'; printf KEY_{name}",
            runs_file(name).display()
        );
        let login = home.run(["auth", "login", name, "--command", "sh", "-c", &script]);
        assert!(login.status.success());
    }
    let runs = |name: &str| {
        std::fs::read_to_string(runs_file(name)).map_or(0, |text| text.lines().count())
    };
    // The new session's model defaults name beta, not the configured alpha. First-run
    // setup checks beta's credential too, so only the workflow's runs are counted.
    report(
        &home.run(
            [
                &["new", "Trip", "A fact", "--provider", "beta"][..],
                &QUESTION,
            ]
            .concat(),
        ),
    );
    assert_eq!(runs("alpha"), 0);
    std::fs::remove_file(runs_file("beta")).unwrap();
    let file = home.0.join("workflow.json");
    std::fs::write(&file, serde_json::to_vec(&workflow()).unwrap()).unwrap();
    let before = server.count();
    let run = run_report(
        &home.run([
            "workflow",
            "run",
            file.to_str().unwrap(),
            "--session",
            "Trip",
        ]),
        0,
    );
    assert!(server.count() - before > 1);
    assert!(run.session.is_some());
    // The prefetch ran beta's command once for every node; alpha's never ran.
    assert_eq!((runs("alpha"), runs("beta")), (0, 1));
    assert!(
        server
            .authorizations
            .lock()
            .unwrap()
            .iter()
            .all(|authorization| authorization.as_deref() == Some("Bearer KEY_beta"))
    );
}
