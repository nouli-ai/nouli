#[path = "support/harness.rs"]
#[allow(dead_code, reason = "each test uses a subset of the helpers")]
mod harness;
#[path = "support/loopback.rs"]
#[allow(dead_code, reason = "each test uses a subset of the helpers")]
mod loopback;
use harness::{Home, QUESTION, context_record, ledger_bytes, report, turn};
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
use std::process::Command;
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
    // A what-if starts from the saved session, so its update needs a new record.
    file["updates"][0]["record"]["id"] = serde_json::json!(Id::generate());
    file["updates"][0]["record"]["key"] = serde_json::json!("whatif-only");
    let result = report(&home.run_with_stdin(
        ["whatif", "Full", "--input", "-", "--input-format", "json"],
        &serde_json::to_vec(&file).unwrap(),
    ));
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
        !std::fs::read(home.ledger_path())
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
    assert_eq!(harness::session_count(&home), 2);
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
