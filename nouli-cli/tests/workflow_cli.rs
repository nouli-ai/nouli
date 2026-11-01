#[path = "support/harness.rs"]
#[allow(dead_code, reason = "each test uses a subset of the helpers")]
mod harness;
#[path = "support/loopback.rs"]
#[allow(dead_code, reason = "each test uses a subset of the helpers")]
mod loopback;
use harness::{
    Home, QUESTION, context_record, jsonl_events, ledger_bytes, report, run_report, workflow,
};
use loopback::Server;
use nouli_core::{
    codec,
    context::{ContextKind, ContextRecord, ContextUpdate},
    data::Data,
    history::SessionSelection,
    identity::Id,
    protocol::{Report, ResultEnvelope},
    storage::{Ledger, OpenMode},
};
use std::fmt::Write as _;
use std::process::{Command, Stdio};
#[test]
fn a_node_input_that_is_a_whole_number_keeps_the_run_going() {
    use nouli_core::workflow::RunStatus;
    let server = Server::with_handler(harness::typesafe);
    let home = Home::create();
    home.configure(&server, "typesafe", false);
    let mut definition = serde_json::to_value(workflow()).unwrap();
    // The fixture provider answers on_time with exactly 1.0.
    definition["nodes"][4]["inputs"] = serde_json::json!([{
        "name": "certain",
        "reference": {
            "node": "initial",
            "question": "on_time",
            "field": "probability",
            "pointer": null,
            "on_missing": {"policy": "required"}
        }
    }]);
    let file = home.0.join("workflow.json");
    std::fs::write(&file, serde_json::to_vec(&definition).unwrap()).unwrap();
    let output = home.run([
        "workflow",
        "run",
        file.to_str().unwrap(),
        "--name",
        "Travel",
    ]);
    assert_eq!(run_report(&output, 0).status, RunStatus::Completed);
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
    assert_eq!(harness::session_count(home), 1);
}
fn assert_resume_of_a_completed_run_replays(home: &Home, run: Id) {
    let replay = home.run(["workflow", "resume", &run.to_string(), "--jsonl"]);
    assert!(replay.status.success());
    let events = jsonl_events(&replay);
    assert_eq!(events.len(), 2);
    assert!(matches!(&harness::terminal(&events).result, Report::Run(report) if report.replayed));
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
    let result = harness::terminal(&events);
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
    use nouli_core::workflow::WorkflowRequest;
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
    let output = home.run_with_stdin(
        [
            "workflow",
            "run",
            "-",
            "--input-format",
            "json",
            "--no-session",
            "--jsonl",
        ],
        &serde_json::to_vec(&request).unwrap(),
    );
    assert!(output.status.success());
    assert_eq!(String::from_utf8_lossy(&output.stderr), "");
    let events = harness::jsonl_events(&output);

    assert!(matches!(&harness::terminal(&events).result, Report::Run(report) if !report.persisted));
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
    use nouli_core::workflow::{NodeStatus, RunStatus};
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
    let events = harness::jsonl_events(&output);

    let result = harness::terminal(&events);
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
    harness::jsonl_events(&output);

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
    harness::jsonl_events_from(&std::fs::read(path).unwrap());

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
