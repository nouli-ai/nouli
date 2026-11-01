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
    history::{EvaluationRequest, ModelSelection, SessionSelection, StandingChanges},
    identity::{Id, SessionSelector},
    protocol::{Report, ResultEnvelope},
    storage::{Ledger, OpenMode, inspection::ContextView, turns::AttemptStatus},
    workflow::RunStatus,
};
use process::OwnedChild;
use std::fmt::Write as _;
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
        standing: StandingChanges::default(),
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
fn error(output: &std::process::Output, code: i32) -> nouli_core::protocol::ErrorReport {
    assert_eq!(output.status.code(), Some(code));
    assert_eq!(String::from_utf8_lossy(&output.stderr), "");
    let envelope: ResultEnvelope = serde_json::from_slice(&output.stdout).unwrap();
    let Report::Error(error) = envelope.result else {
        panic!("error report")
    };
    error
}
/// `arguments` followed by the travel question.
fn asking<'a>(arguments: &[&'a str]) -> Vec<&'a str> {
    [arguments, &QUESTION].concat()
}
/// A provider that holds the first `CRASHING` request until released: it
/// signals `started` once the request arrives, then waits on `release`.
fn holding_provider() -> (Server, mpsc::Receiver<()>, mpsc::SyncSender<()>) {
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
    (primary, waiting, release)
}
/// Configures `primary` as the default provider, with a timeout long enough to
/// outlast the held request, and `other` as a second provider.
fn configure_two_providers(home: &Home, primary: &Server, other: &Server) {
    home.configure(primary, "typesafe", false);
    let config = home.0.join("config.toml");
    let mut text = std::fs::read_to_string(&config).unwrap().replace(
        "attempt_timeout_seconds = 5",
        "attempt_timeout_seconds = 30",
    );
    write!(
        text,
        "[providers.other]\nprotocol=\"system-one\"\nbase_url=\"{}\"\n[providers.other.models.offline]\ncontext_budget_tokens=100000\nreserved_output_tokens=128\n",
        other.url
    )
    .unwrap();
    std::fs::write(config, text).unwrap();
}
#[test]
fn competing_writers_duplicate_ids_owner_termination_and_restart_preserve_each_session() {
    let (primary, waiting, release) = holding_provider();
    let other = Server::with_handler(harness::typesafe);
    let home = Home::create();
    configure_two_providers(&home, &primary, &other);
    let base = report(&home.run(asking(&["new", "Main", "BASE"])));
    let main = base.session.unwrap();
    let arguments = asking(&["add", "Main", "CRASHING", "--request-id", "crash-request"]);
    let child = OwnedChild::new(
        home.command()
            .args(&arguments)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap(),
    );
    waiting.recv_timeout(Duration::from_secs(5)).unwrap();
    // While the owner holds Main, every other writer to it is turned away.
    error(&home.run(&arguments), 5);
    error(
        &home.run(asking(&[
            "add",
            "Main",
            "OTHER_INTENT",
            "--request-id",
            "different-request",
        ])),
        5,
    );
    error(
        &home.run(asking(&[
            "add",
            "Main",
            "CHANGED",
            "--request-id",
            "crash-request",
        ])),
        5,
    );
    assert_eq!(primary.count(), 2);
    // Another session is independent of the held one.
    let independent = report(
        &home.run(
            [
                &asking(&["new", "Other", "INDEPENDENT"])[..],
                &["--provider", "other"],
            ]
            .concat(),
        ),
    );
    assert_ne!(independent.session, base.session);
    assert_eq!(other.count(), 1);
    kill_owner(child);
    release.send(()).unwrap();
    assert_owner_left_a_running_attempt(&home, main, base.turn.unwrap());
    let interrupted = error(&home.run(&arguments), 130);
    assert_eq!(
        interrupted.category,
        nouli_core::protocol::ErrorCategory::Interrupted
    );
    assert_eq!(primary.count(), 2);
    let mut retry = arguments.clone();
    retry.push("--retry-interrupted");
    let recovered = report(&home.run(&retry));
    assert_eq!(recovered.session, base.session);
    assert_eq!(
        recovered.manifest.sources[0].frontier,
        vec![base.turn.unwrap()]
    );
    assert_eq!(primary.count(), 3);
    let replay = report(&home.run(&arguments));
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
/// Kills the owning process; it exits unsuccessfully and prints no result.
fn kill_owner(child: OwnedChild) {
    assert!(
        Command::new("kill")
            .args(["-KILL", &child.id().to_string()])
            .status()
            .unwrap()
            .success()
    );
    let output = child.output(Duration::from_secs(5));
    assert!(!output.status.success());
    assert_eq!(String::from_utf8_lossy(&output.stdout), "");
}
/// After the owner dies, its session is free again, still at `head`, with the
/// killed attempt left running.
fn assert_owner_left_a_running_attempt(home: &Home, main: Id, head: Id) {
    let ledger = Ledger::open(&home.store(), OpenMode::ReadWriteCreate).unwrap();
    assert!(ledger.session_guard(main).is_ok());
    assert_eq!(
        ledger.session(&SessionSelector::Id(main)).unwrap().frontier,
        vec![head]
    );
    let attempts = ledger.list_attempts(main).unwrap();
    assert_eq!(attempts.len(), 2);
    assert_eq!(attempts[1].progress.state, AttemptStatus::Running);
}
#[test]
fn before_dispatch_and_transactional_finalization_faults_never_publish_false_success() {
    for mode in ["before", "finalize", "commit"] {
        let server = Server::with_handler(harness::typesafe);
        let home = Home::create();
        home.configure(&server, "typesafe", false);
        let base = report(&home.run([&["new", "Main", "BASE"][..], &QUESTION].concat()));
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
            &home.run(["add", "Main", "--input", path.to_str().unwrap()]),
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
        let retry = report(&home.run([
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
    report(&home.run([&["new", "Main", "BASE"][..], &QUESTION].concat()));
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
    error(&home.run(arguments), 6);
    assert_eq!(server.count(), 2);
    sql(&home, "DROP TRIGGER inject_fault;");
    // Without the flag the request is interrupted and nothing is sent.
    let interrupted = error(&home.run(arguments), 130);
    assert_eq!(interrupted.code, "request_requires_explicit_retry");
    assert_eq!(server.count(), 2);
    let recovered = report(&home.run([&arguments[..], &["--retry-interrupted"]].concat()));
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
    let replay = report(&home.run(arguments));
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
/// Where a workflow node's publication fails: as it starts `running`, or as it
/// is marked `completed` after its provider call.
#[derive(Clone, Copy, PartialEq)]
enum NodeFault {
    Running,
    Completed,
}
#[test]
fn workflow_fault_before_a_node_runs_rolls_back_and_resume_runs_only_unfinished_nodes() {
    workflow_state_fault_rolls_back_node_publication_and_resumes(NodeFault::Running);
}
#[test]
fn workflow_fault_after_a_node_answers_rolls_back_and_resume_runs_only_unfinished_nodes() {
    workflow_state_fault_rolls_back_node_publication_and_resumes(NodeFault::Completed);
}
/// A provider that, on a `Completed` fault, installs a trigger failing the
/// `high` node's completion as soon as that node's request arrives.
fn node_fault_provider(home: &Home, fault: NodeFault) -> Server {
    let database = home.store().join("ledger.sqlite3");
    let once = AtomicBool::new(fault == NodeFault::Completed);
    Server::with_handler(move |body| {
        if body["state"].as_str().unwrap().contains("NODE:high")
            && once.swap(false, Ordering::SeqCst)
        {
            let connection = rusqlite::Connection::open(&database).unwrap();
            connection.busy_timeout(Duration::from_secs(5)).unwrap();
            connection.execute_batch("CREATE TRIGGER inject_fault BEFORE UPDATE OF state ON workflow_nodes WHEN NEW.node_id='high' AND NEW.state='completed' BEGIN SELECT RAISE(ABORT,'fault'); END;").unwrap();
        }
        harness::typesafe(body)
    })
}
/// Writes the fixture workflow with both branches active, each appending its
/// own fact, and returns its path and the `high` node's fact.
fn fact_appending_workflow(home: &Home) -> (std::path::PathBuf, ContextRecord) {
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
    graph.nodes[2].updates = vec![ContextUpdate::Append { record: low }];
    let file = home.0.join("workflow.json");
    std::fs::write(&file, serde_json::to_vec(&graph).unwrap()).unwrap();
    (file, high)
}
fn workflow_state_fault_rolls_back_node_publication_and_resumes(fault: NodeFault) {
    let home = Home::create();
    drop(Ledger::open(&home.store(), OpenMode::ReadWriteCreate).unwrap());
    let server = node_fault_provider(&home, fault);
    home.configure(&server, "typesafe", false);
    if fault == NodeFault::Running {
        sql(
            &home,
            "CREATE TRIGGER inject_fault BEFORE UPDATE OF state ON workflow_nodes WHEN NEW.node_id='high' AND NEW.state='running' BEGIN SELECT RAISE(ABORT,'fault'); END;",
        );
    }
    let (file, high) = fact_appending_workflow(&home);
    let output = home.run(["workflow", "run", file.to_str().unwrap(), "--parallel", "1"]);
    let (run, preserved) = match fault {
        NodeFault::Running => {
            assert_eq!(output.status.code(), Some(6));
            let failure: ResultEnvelope = serde_json::from_slice(&output.stdout).unwrap();
            assert!(matches!(failure.result, Report::Error(_)));
            (failure.recovery.unwrap().run.unwrap(), None)
        }
        NodeFault::Completed => {
            let partial = run_report(&output, 4);
            assert_eq!(partial.status, RunStatus::Partial);
            (partial.run.unwrap(), partial.nodes[2].turn)
        }
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
    let initial = inspect_interrupted_run(&home, run, fault);
    sql(&home, "DROP TRIGGER inject_fault;");
    let calls = server.count();
    let done = run_report(&home.run(["workflow", "resume", &run.to_string()]), 0);
    assert_eq!(done.status, RunStatus::Completed);
    assert_eq!(done.nodes[0].turn, initial);
    if let Some(preserved) = preserved {
        assert_eq!(done.nodes[2].turn, Some(preserved));
    }
    assert_eq!(
        server.count() - calls,
        match fault {
            NodeFault::Running => 4,
            NodeFault::Completed => 3,
        }
    );
    assert_eq!(scalar(&home, "SELECT count(*) FROM turns"), 5);
    assert_eq!(scalar(&home, "SELECT count(*) FROM context_records"), 2);
    let shown = home.run(["workflow", "show", &run.to_string()]);
    assert!(shown.status.success());
    let inspection: nouli_core::storage::workflows::recovery::WorkflowInspection =
        serde_json::from_slice(&shown.stdout).unwrap();
    assert_eq!(inspection.rounds.len(), 2);
    assert_eq!(
        inspection.inspection.nodes[1].uncertain_prior_calls,
        usize::from(fault == NodeFault::Completed)
    );
}
/// Checks what the interrupted run records before it is resumed, and returns
/// the initial node's turn.
fn inspect_interrupted_run(home: &Home, run: Id, fault: NodeFault) -> Option<Id> {
    use nouli_core::storage::workflows::recovery::Disposition;
    let ledger = Ledger::open(&home.store(), OpenMode::ReadOnlyExisting).unwrap();
    let before = ledger.inspect_workflow_full(run).unwrap();
    assert!(!before.inspection.live_owner);
    assert!(before.inspection.session_head_compatible);
    match fault {
        NodeFault::Completed => {
            assert_eq!(
                before.inspection.nodes[1].disposition,
                Disposition::ProviderOutcomeUnknown
            );
            assert_eq!(before.inspection.nodes[1].uncertain_prior_calls, 1);
            assert_eq!(
                before.inspection.nodes[2].disposition,
                Disposition::Completed
            );
            assert_eq!(scalar(home, "SELECT count(*) FROM context_records"), 1);
        }
        NodeFault::Running => {
            assert_eq!(
                before.inspection.nodes[1].attempts,
                [] as [nouli_core::storage::turns::AttemptSnapshot; 0]
            );
        }
    }
    before.inspection.run.nodes[0]
        .output
        .as_ref()
        .and_then(|output| output.source.as_ref())
        .map(|source| source.frontier[0])
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
        &home.run([&["new", "Migrate", "MIGRATE"][..], &QUESTION].concat()),
        6,
    );
    assert_eq!(server.count(), 0);
    assert_eq!(scalar(&home, "PRAGMA user_version"), 4);
    assert!(
        before == std::fs::read(&database).unwrap(),
        "failed migration changed original database"
    );
    let failure = error(&home.run(["sessions", "list"]), 6);
    assert_eq!(failure.code, "ledger_migration_required");
    assert!(
        before == std::fs::read(&database).unwrap(),
        "read-only command changed original database"
    );
    assert!(!home.store().join("ledger.sqlite3-journal").exists());
}
/// Branches `Left` and `Right` from `base`, each superseding `original` with
/// its own replacement.
fn conflicting_branches(
    home: &Home,
    base: Id,
    original: &ContextRecord,
    left: &ContextRecord,
    right: &ContextRecord,
) -> Vec<nouli_core::protocol::TurnReport> {
    [("Left", left), ("Right", right)]
        .into_iter()
        .map(|(name, replacement)| {
            let path = home.request(
                &format!("{name}.json"),
                name,
                vec![ContextUpdate::Supersede {
                    targets: vec![original.id],
                    replacement: replacement.clone(),
                    pinned: None,
                }],
            );
            report(&home.run([
                "new",
                name,
                "--from",
                &base.to_string(),
                "--input",
                path.to_str().unwrap(),
            ]))
        })
        .collect()
}
fn export_session(home: &Home, session: Id) -> nouli_core::storage::inspection::SessionExport {
    Ledger::open(&home.store(), OpenMode::ReadOnlyExisting)
        .unwrap()
        .export_session(&SessionSelector::Id(session))
        .unwrap()
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
    let base = report(&home.run(["new", "Base", "--input", seed.to_str().unwrap()]));
    let left = record(12);
    let right = record(16);
    let sources = conflicting_branches(&home, base.session.unwrap(), &original, &left, &right);
    let export_sources = || {
        sources
            .iter()
            .map(|source| export_session(&home, source.session.unwrap()))
            .collect::<Vec<_>>()
    };
    let before = export_sources();
    let run_blend = |name: &str, path: &std::path::Path| {
        let mut arguments = vec![
            "new".to_owned(),
            name.to_owned(),
            "--input".to_owned(),
            path.to_str().unwrap().to_owned(),
        ];
        for source in &sources {
            arguments.extend(["--from".to_owned(), source.session.unwrap().to_string()]);
        }
        home.run(arguments)
    };
    let path = home.request("conflict.json", "CONFLICT", vec![]);
    error(&run_blend("Conflict", &path), 5);
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
    let blend = report(&run_blend("Resolved", &path));
    assert_eq!(server.count(), 4);
    assert_eq!(export_sources(), before);
    let export = export_session(&home, blend.session.unwrap());
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
