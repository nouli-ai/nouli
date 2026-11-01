#![cfg(unix)]
#[path = "support/harness.rs"]
#[allow(dead_code, reason = "each test uses a subset of the helpers")]
mod harness;
#[path = "support/loopback.rs"]
#[allow(dead_code, reason = "each test uses a subset of the helpers")]
mod loopback;
#[path = "support/process.rs"]
#[allow(dead_code, reason = "each test uses a subset of the helpers")]
mod process;
use harness::{Home, QUESTION, report, run_report, turn, workflow};
use loopback::Server;
use nouli_core::{
    context::{ContextKind, ContextRecord, ContextUpdate, PinTarget},
    data::{Data, Input},
    history::{EvaluationRequest, ModelSelection, SessionSelection},
    identity::{Id, SessionSelector},
    protocol::{Report, ResultEnvelope},
    storage::{Ledger, OpenMode, inspection::ContextView, turns::AttemptStatus},
    workflow::RunStatus,
};
use process::OwnedChild;
use std::{
    process::{Command, Stdio},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::Duration,
};
fn sql(home: &Home, statement: &str) {
    let connection = rusqlite::Connection::open(home.store().join("ledger.sqlite3")).unwrap();
    connection.busy_timeout(Duration::from_secs(5)).unwrap();
    connection.execute_batch(statement).unwrap();
}
fn scalar(home: &Home, query: &str) -> i64 {
    let connection = rusqlite::Connection::open_with_flags(
        home.store().join("ledger.sqlite3"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap();
    connection.query_row(query, [], |row| row.get(0)).unwrap()
}
fn key_request(
    home: &Home,
    name: &str,
    key: &str,
    updates: Vec<ContextUpdate>,
) -> std::path::PathBuf {
    let mut turn = turn();
    turn.state.push(Input::Text { text: name.into() });
    let request = EvaluationRequest {
        standing: Default::default(),
        turn,
        session: SessionSelection::New { name: None },
        updates,
        required_records: vec![],
        model: ModelSelection::default(),
        request_id: Some(key.into()),
    };
    // The verb supplies the session, so the request file names none.
    let mut file = serde_json::to_value(&request).unwrap();
    file.as_object_mut().unwrap().remove("session");
    let path = home.0.join(format!("{name}.json"));
    std::fs::write(&path, serde_json::to_vec(&file).unwrap()).unwrap();
    path
}
fn error(output: std::process::Output, code: i32) -> nouli_core::protocol::ErrorReport {
    assert_eq!(output.status.code(), Some(code));
    assert!(output.stderr.is_empty());
    let envelope: ResultEnvelope = serde_json::from_slice(&output.stdout).unwrap();
    let Report::Error(error) = envelope.result else {
        panic!("error report")
    };
    error
}
#[test]
fn competing_writers_duplicate_ids_owner_termination_and_restart_preserve_each_session() {
    let (started, waiting) = mpsc::sync_channel(1);
    let (release, held) = mpsc::sync_channel(1);
    let once = AtomicBool::new(true);
    let primary = Server::with_handler(move |body| {
        if body["state"].as_str().unwrap().contains("CRASHING")
            && once.swap(false, Ordering::SeqCst)
        {
            started.send(()).unwrap();
            match held.recv_timeout(Duration::from_secs(30)) {
                Ok(()) | Err(mpsc::RecvTimeoutError::Disconnected) => {}
                Err(mpsc::RecvTimeoutError::Timeout) => panic!("test provider barrier timed out"),
            }
        }
        harness::typesafe(body)
    });
    let other = Server::with_handler(harness::typesafe);
    let home = Home::create();
    home.configure(&primary, "typesafe", false);
    let config = home.0.join("config.toml");
    let mut text = std::fs::read_to_string(&config).unwrap().replace(
        "attempt_timeout_seconds = 5",
        "attempt_timeout_seconds = 30",
    );
    text += &format!(
        "[providers.other]\nprotocol=\"system-one\"\nbase_url=\"{}\"\n[providers.other.models.offline]\ncontext_budget_tokens=100000\nreserved_output_tokens=128\n",
        other.url
    );
    std::fs::write(config, text).unwrap();
    let base = report(home.run([&["new", "Main", "BASE"][..], &QUESTION].concat()));
    let arguments = [
        &["add", "Main", "CRASHING", "--request-id", "crash-request"][..],
        &QUESTION,
    ]
    .concat();
    let child = OwnedChild::new(
        home.command()
            .args(&arguments)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap(),
    );
    waiting.recv_timeout(Duration::from_secs(5)).unwrap();
    error(home.run(&arguments), 5);
    error(
        home.run(
            [
                &[
                    "add",
                    "Main",
                    "OTHER_INTENT",
                    "--request-id",
                    "different-request",
                ][..],
                &QUESTION,
            ]
            .concat(),
        ),
        5,
    );
    error(
        home.run(
            [
                &["add", "Main", "CHANGED", "--request-id", "crash-request"][..],
                &QUESTION,
            ]
            .concat(),
        ),
        5,
    );
    assert_eq!(primary.count(), 2);
    let independent = report(
        home.run(
            [
                &["new", "Other", "INDEPENDENT"][..],
                &QUESTION,
                &["--provider", "other"],
            ]
            .concat(),
        ),
    );
    assert_ne!(independent.session, base.session);
    assert_eq!(other.count(), 1);
    assert!(
        Command::new("kill")
            .args(["-KILL", &child.id().to_string()])
            .status()
            .unwrap()
            .success()
    );
    let output = child.output(Duration::from_secs(5));
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    release.send(()).unwrap();
    let ledger = Ledger::open(&home.store(), OpenMode::ReadWriteCreate).unwrap();
    let main = base.session.unwrap();
    assert!(ledger.session_guard(main).is_ok());
    assert_eq!(
        ledger.session(&SessionSelector::Id(main)).unwrap().frontier,
        vec![base.turn.unwrap()]
    );
    let attempts = ledger.list_attempts(main).unwrap();
    assert_eq!(attempts.len(), 2);
    assert_eq!(attempts[1].progress.state, AttemptStatus::Running);
    drop(ledger);
    let interrupted = error(home.run(&arguments), 130);
    assert_eq!(
        interrupted.category,
        nouli_core::protocol::ErrorCategory::Interrupted
    );
    assert_eq!(primary.count(), 2);
    let mut retry = arguments.to_vec();
    retry.push("--retry-interrupted");
    let recovered = report(home.run(&retry));
    assert_eq!(recovered.session, base.session);
    assert_eq!(
        recovered.manifest.sources[0].frontier,
        vec![base.turn.unwrap()]
    );
    assert_eq!(primary.count(), 3);
    let replay = report(home.run(&arguments));
    assert!(replay.replayed);
    assert_eq!(replay.turn, recovered.turn);
    assert_eq!(primary.count(), 3);
    let ledger = Ledger::open(&home.store(), OpenMode::ReadOnlyExisting).unwrap();
    let attempts = ledger.list_attempts(main).unwrap();
    assert_eq!(attempts.len(), 3);
    assert_eq!(attempts[1].progress.state, AttemptStatus::Interrupted);
    assert_eq!(
        attempts[1].progress.error.as_ref().unwrap().code,
        "provider_outcome_unknown"
    );
    assert_eq!(attempts[2].progress.state, AttemptStatus::Completed);
    assert_eq!(scalar(&home, "SELECT count(*) FROM turns"), 3);
}
#[test]
fn before_dispatch_and_transactional_finalization_faults_never_publish_false_success() {
    for mode in ["before", "finalize", "commit"] {
        let server = Server::with_handler(harness::typesafe);
        let home = Home::create();
        home.configure(&server, "typesafe", false);
        let base = report(home.run([&["new", "Main", "BASE"][..], &QUESTION].concat()));
        let record = ContextRecord {
            id: Id::generate(),
            key: "atomic".into(),
            value: Data::new(serde_json::json!("caller commitment")).unwrap(),
            kind: ContextKind::Commitment,
        };
        let path = key_request(
            &home,
            "ATOMIC",
            "atomic-request",
            vec![ContextUpdate::Append {
                record: record.clone(),
            }],
        );
        if mode == "before" {
            sql(
                &home,
                "CREATE TRIGGER inject_fault BEFORE INSERT ON attempts WHEN NEW.run_id IS NULL BEGIN SELECT RAISE(ABORT,'fault'); END;",
            );
        } else if mode == "finalize" {
            sql(
                &home,
                "CREATE TRIGGER inject_fault BEFORE UPDATE OF state ON request_reservations WHEN NEW.request_id='atomic-request' AND NEW.state='completed' BEGIN SELECT RAISE(ABORT,'fault'); END;",
            );
        } else {
            // Every statement succeeds. The invalid deferred reference is checked
            // only by SQLite COMMIT, after all publication mutations are staged.
            sql(
                &home,
                "CREATE TABLE deferred_fault(parent_id TEXT REFERENCES sessions(id) DEFERRABLE INITIALLY DEFERRED); CREATE TRIGGER inject_fault AFTER UPDATE OF state ON request_reservations WHEN NEW.request_id='atomic-request' AND NEW.state='completed' BEGIN INSERT INTO deferred_fault(parent_id) VALUES('missing-session'); END;",
            );
        }
        error(
            home.run(["add", "Main", "--input", path.to_str().unwrap()]),
            6,
        );
        assert_eq!(server.count(), if mode == "before" { 1 } else { 2 });
        assert_eq!(scalar(&home, "SELECT count(*) FROM turns"), 1);
        assert_eq!(scalar(&home, "SELECT count(*) FROM manifests"), 1);
        // Only the base turn's fact is saved; the failed turn's commitment is not.
        assert_eq!(scalar(&home, "SELECT count(*) FROM context_records"), 1);
        assert_eq!(
            scalar(&home, "SELECT count(*) FROM attempts"),
            if mode == "before" { 1 } else { 2 }
        );
        let ledger = Ledger::open(&home.store(), OpenMode::ReadOnlyExisting).unwrap();
        assert_eq!(
            ledger
                .session(&SessionSelector::Id(base.session.unwrap()))
                .unwrap()
                .frontier,
            vec![base.turn.unwrap()]
        );
        drop(ledger);
        if mode == "commit" {
            assert_eq!(scalar(&home, "SELECT count(*) FROM deferred_fault"), 0);
        }
        assert!(ledger_guard_released(&home, base.session.unwrap()));
        sql(&home, "DROP TRIGGER inject_fault;");
        let retry = report(home.run([
            "add",
            "Main",
            "--input",
            path.to_str().unwrap(),
            "--retry-interrupted",
        ]));
        assert_eq!(retry.session, base.session);
        assert_eq!(scalar(&home, "SELECT count(*) FROM turns"), 2);
        assert_eq!(scalar(&home, "SELECT count(*) FROM context_records"), 2);
        assert_eq!(scalar(&home, "SELECT count(*) FROM manifests"), 2);
        let ledger = Ledger::open(&home.store(), OpenMode::ReadOnlyExisting).unwrap();
        let attempts = ledger.list_attempts(base.session.unwrap()).unwrap();
        if mode != "before" {
            assert_eq!(
                attempts[1].progress.error.as_ref().unwrap().code,
                "provider_outcome_unknown"
            );
        }
    }
}

#[test]
fn retrying_an_interrupted_request_asks_its_own_question_once() {
    let server = Server::with_handler(harness::typesafe);
    let home = Home::create();
    home.configure(&server, "typesafe", false);
    report(home.run([&["new", "Main", "BASE"][..], &QUESTION].concat()));
    // The answer comes back, but saving the turn fails after the provider call.
    sql(
        &home,
        "CREATE TRIGGER inject_fault BEFORE UPDATE OF state ON request_reservations WHEN NEW.request_id='retry-questions' AND NEW.state='completed' BEGIN SELECT RAISE(ABORT,'fault'); END;",
    );
    let arguments = [
        "add",
        "Main",
        "A new fact",
        "-q",
        "extra: Is it cheap?",
        "--request-id",
        "retry-questions",
        "--json",
    ];
    error(home.run(arguments), 6);
    assert_eq!(server.count(), 2);
    sql(&home, "DROP TRIGGER inject_fault;");
    // Without the flag the request is interrupted and nothing is sent.
    let interrupted = error(home.run(arguments), 130);
    assert_eq!(interrupted.code, "request_requires_explicit_retry");
    assert_eq!(server.count(), 2);
    let recovered = report(home.run([&arguments[..], &["--retry-interrupted"]].concat()));
    assert!(!recovered.replayed);
    let asked: Vec<_> = recovered
        .questions
        .iter()
        .map(|question| question.id.as_str())
        .collect();
    assert_eq!(asked, ["extra"]);
    let sent = server.calls.lock().unwrap().last().unwrap().clone();
    let sent: Vec<_> = sent["questions"]
        .as_object()
        .unwrap()
        .keys()
        .cloned()
        .collect();
    assert_eq!(sent, ["extra"]);
    assert_eq!(server.count(), 3);
    let replay = report(home.run(arguments));
    assert!(replay.replayed && replay.turn == recovered.turn);
    assert_eq!(server.count(), 3);
}

fn record(value: i64) -> ContextRecord {
    ContextRecord {
        id: Id::generate(),
        key: "budget".into(),
        value: Data::new(serde_json::json!(value)).unwrap(),
        kind: ContextKind::Commitment,
    }
}
#[test]
fn workflow_state_faults_rollback_node_publication_and_resume_only_unfinished_nodes() {
    use nouli_core::storage::workflows::recovery::{Disposition, WorkflowInspection};
    for mode in ["running", "completed"] {
        let home = Home::create();
        drop(Ledger::open(&home.store(), OpenMode::ReadWriteCreate).unwrap());
        let database = home.store().join("ledger.sqlite3");
        let once = AtomicBool::new(mode == "completed");
        let server = Server::with_handler(move |body| {
            if body["state"].as_str().unwrap().contains("NODE:high")
                && once.swap(false, Ordering::SeqCst)
            {
                let connection = rusqlite::Connection::open(&database).unwrap();
                connection.busy_timeout(Duration::from_secs(5)).unwrap();
                connection.execute_batch("CREATE TRIGGER inject_fault BEFORE UPDATE OF state ON workflow_nodes WHEN NEW.node_id='high' AND NEW.state='completed' BEGIN SELECT RAISE(ABORT,'fault'); END;").unwrap();
            }
            harness::typesafe(body)
        });
        home.configure(&server, "typesafe", false);
        if mode == "running" {
            sql(
                &home,
                "CREATE TRIGGER inject_fault BEFORE UPDATE OF state ON workflow_nodes WHEN NEW.node_id='high' AND NEW.state='running' BEGIN SELECT RAISE(ABORT,'fault'); END;",
            );
        }
        let mut high = record(12);
        high.key = "high-fact".into();
        let mut low = record(16);
        low.key = "low-fact".into();
        let mut graph = workflow();
        graph.nodes[1].when = None;
        graph.nodes[2].when = None;
        graph.nodes[1].updates = vec![ContextUpdate::Append {
            record: high.clone(),
        }];
        graph.nodes[2].updates = vec![ContextUpdate::Append {
            record: low.clone(),
        }];
        let file = home.0.join("workflow.json");
        std::fs::write(&file, serde_json::to_vec(&graph).unwrap()).unwrap();
        let output = home.run(["workflow", "run", file.to_str().unwrap(), "--parallel", "1"]);
        let (run, preserved) = if mode == "running" {
            assert_eq!(output.status.code(), Some(6));
            let failure: ResultEnvelope = serde_json::from_slice(&output.stdout).unwrap();
            assert!(matches!(failure.result, Report::Error(_)));
            (failure.recovery.unwrap().run.unwrap(), None)
        } else {
            let partial = run_report(output, 4);
            assert_eq!(partial.status, RunStatus::Partial);
            (partial.run.unwrap(), partial.nodes[2].turn)
        };
        assert_eq!(
            scalar(
                &home,
                &format!(
                    "SELECT count(*) FROM context_records WHERE id='{}'",
                    high.id
                )
            ),
            0
        );
        let ledger = Ledger::open(&home.store(), OpenMode::ReadOnlyExisting).unwrap();
        let before = ledger.inspect_workflow_full(run).unwrap();
        assert!(!before.inspection.live_owner);
        assert!(before.inspection.session_head_compatible);
        if mode == "completed" {
            assert_eq!(
                before.inspection.nodes[1].disposition,
                Disposition::ProviderOutcomeUnknown
            );
            assert_eq!(before.inspection.nodes[1].uncertain_prior_calls, 1);
            assert_eq!(
                before.inspection.nodes[2].disposition,
                Disposition::Completed
            );
            assert_eq!(scalar(&home, "SELECT count(*) FROM context_records"), 1);
        } else {
            assert!(before.inspection.nodes[1].attempts.is_empty());
        }
        let initial = before.inspection.run.nodes[0]
            .output
            .as_ref()
            .and_then(|output| output.source.as_ref())
            .map(|source| source.frontier[0]);
        drop(ledger);
        sql(&home, "DROP TRIGGER inject_fault;");
        let calls = server.count();
        let done = run_report(home.run(["workflow", "resume", &run.to_string()]), 0);
        assert_eq!(done.status, RunStatus::Completed);
        assert_eq!(done.nodes[0].turn, initial);
        if let Some(preserved) = preserved {
            assert_eq!(done.nodes[2].turn, Some(preserved));
        }
        assert_eq!(
            server.count() - calls,
            if mode == "running" { 4 } else { 3 }
        );
        assert_eq!(scalar(&home, "SELECT count(*) FROM turns"), 5);
        assert_eq!(scalar(&home, "SELECT count(*) FROM context_records"), 2);
        let shown = home.run(["workflow", "show", &run.to_string()]);
        assert!(shown.status.success());
        let inspection: WorkflowInspection = serde_json::from_slice(&shown.stdout).unwrap();
        assert_eq!(inspection.rounds.len(), 2);
        assert_eq!(
            inspection.inspection.nodes[1].uncertain_prior_calls,
            usize::from(mode == "completed")
        );
    }
}
#[test]
fn failed_migration_and_readonly_old_schema_preserve_original_bytes() {
    let server = Server::with_handler(harness::typesafe);
    let home = Home::create();
    home.configure(&server, "typesafe", false);
    std::fs::create_dir_all(home.store()).unwrap();
    let database = home.store().join("ledger.sqlite3");
    let connection = rusqlite::Connection::open(&database).unwrap();
    for migration in [
        include_str!("../../nouli-core/src/storage/schema/0001.sql"),
        include_str!("../../nouli-core/src/storage/schema/0002.sql"),
        include_str!("../../nouli-core/src/storage/schema/0003.sql"),
        include_str!("../../nouli-core/src/storage/schema/0004.sql"),
    ] {
        connection.execute_batch(migration).unwrap();
    }
    connection.execute_batch("PRAGMA application_id=1313822028; PRAGMA user_version=4; CREATE TABLE workflow_round_results(collision);").unwrap();
    drop(connection);
    let before = std::fs::read(&database).unwrap();
    error(
        home.run([&["new", "Migrate", "MIGRATE"][..], &QUESTION].concat()),
        6,
    );
    assert_eq!(server.count(), 0);
    assert_eq!(scalar(&home, "PRAGMA user_version"), 4);
    assert!(
        before == std::fs::read(&database).unwrap(),
        "failed migration changed original database"
    );
    let failure = error(home.run(["sessions", "list"]), 6);
    assert_eq!(failure.code, "ledger_migration_required");
    assert!(
        before == std::fs::read(&database).unwrap(),
        "read-only command changed original database"
    );
    assert!(!home.store().join("ledger.sqlite3-journal").exists());
}
#[test]
fn conflicting_blend_requires_explicit_resolution_and_never_mutates_sources() {
    let server = Server::with_handler(harness::typesafe);
    let home = Home::create();
    home.configure(&server, "typesafe", false);
    let original = record(8);
    let seed = home.request(
        "base.json",
        "BASE",
        vec![ContextUpdate::Pin {
            target: PinTarget::New {
                record: original.clone(),
            },
        }],
    );
    let base = report(home.run(["new", "Base", "--input", seed.to_str().unwrap()]));
    let left = record(12);
    let right = record(16);
    let mut sources = vec![];
    for (name, replacement) in [("Left", &left), ("Right", &right)] {
        let path = home.request(
            &format!("{name}.json"),
            name,
            vec![ContextUpdate::Supersede {
                targets: vec![original.id],
                replacement: replacement.clone(),
                pinned: None,
            }],
        );
        sources.push(report(home.run([
            "new",
            name,
            "--from",
            &base.session.unwrap().to_string(),
            "--input",
            path.to_str().unwrap(),
        ])));
    }
    let ledger = Ledger::open(&home.store(), OpenMode::ReadOnlyExisting).unwrap();
    let before = sources
        .iter()
        .map(|source| {
            ledger
                .export_session(&SessionSelector::Id(source.session.unwrap()))
                .unwrap()
        })
        .collect::<Vec<_>>();
    drop(ledger);
    let from: Vec<String> = sources
        .iter()
        .flat_map(|source| ["--from".to_owned(), source.session.unwrap().to_string()])
        .collect();
    let path = home.request("conflict.json", "CONFLICT", vec![]);
    error(
        home.run(
            [
                &["new", "Conflict", "--input", path.to_str().unwrap()][..],
                &from.iter().map(String::as_str).collect::<Vec<_>>(),
            ]
            .concat(),
        ),
        5,
    );
    assert_eq!(server.count(), 3);
    let resolved = record(11);
    let path = home.request(
        "resolved.json",
        "RESOLVE",
        vec![ContextUpdate::Supersede {
            targets: vec![left.id, right.id],
            replacement: resolved.clone(),
            pinned: None,
        }],
    );
    let blend = report(
        home.run(
            [
                &["new", "Resolved", "--input", path.to_str().unwrap()][..],
                &from.iter().map(String::as_str).collect::<Vec<_>>(),
            ]
            .concat(),
        ),
    );
    assert_eq!(server.count(), 4);
    let ledger = Ledger::open(&home.store(), OpenMode::ReadOnlyExisting).unwrap();
    for (source, expected) in sources.iter().zip(before) {
        assert_eq!(
            ledger
                .export_session(&SessionSelector::Id(source.session.unwrap()))
                .unwrap(),
            expected
        );
    }
    let export = ledger
        .export_session(&SessionSelector::Id(blend.session.unwrap()))
        .unwrap();
    assert_eq!(export.turns.len(), 4);
    let ContextView::Resolved { records } = export.context else {
        panic!("explicitly resolved context")
    };
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].record, resolved);
    assert!(records[0].pinned);
}

fn ledger_guard_released(home: &Home, session: Id) -> bool {
    Ledger::open(&home.store(), OpenMode::ReadWriteCreate)
        .unwrap()
        .session_guard(session)
        .is_ok()
}
