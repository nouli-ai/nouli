#[path = "support/harness.rs"]
#[allow(dead_code, reason = "each test uses a subset of the helpers")]
mod harness;
#[path = "support/loopback.rs"]
#[allow(dead_code, reason = "each test uses a subset of the helpers")]
mod loopback;
use harness::{Home, context_record, jsonl_events, ledger_bytes, report, run_report};
use loopback::Server;
use nouli_core::{
    codec,
    context::{ContextKind, ContextRecord, ContextUpdate, PinTarget},
    data::{Data, Input},
    decision::TurnRequest,
    history::{
        EvaluationRequest, ModelSelection, Representation, SessionSelection, StandingChanges,
    },
    identity::{Id, SessionName, SessionSelector},
    outcome::{Answer, QuestionOutcome},
    protocol::{Report, ResultEnvelope},
    storage::{
        Ledger, OpenMode,
        inspection::{ContextView, SessionExport},
    },
};
use serde_json::Value;
use std::fmt::Write as _;
fn fixture(name: &str) -> TurnRequest {
    let bytes = match name {
        "food" => include_bytes!("../../fixtures/journeys/food.json").as_slice(),
        "budget" => include_bytes!("../../fixtures/journeys/budget.json").as_slice(),
        "refusal" => include_bytes!("../../fixtures/journeys/food-with-refusal.json").as_slice(),
        _ => panic!("unknown fixture"),
    };
    let codec::Request::Turn(turn) =
        codec::decode_request(bytes, codec::InputFormat::Json).unwrap()
    else {
        panic!("turn")
    };
    turn
}
fn body_context(body: &Value) -> &str {
    body.get("state")
        .or_else(|| body.get("input"))
        .unwrap()
        .as_str()
        .unwrap()
}
fn budget_question(body: &Value) -> bool {
    if let Some(questions) = body["questions"].as_array() {
        questions
            .iter()
            .find(|question| question["name"] == "quality")
            .unwrap()["levels"][0]["label"]
            == "Over budget"
    } else {
        body["questions"]["quality"]["criteria"][0] == "Over budget"
    }
}
fn response(protocol: &str, body: &Value) -> &'static str {
    match (protocol, budget_question(body)) {
        ("typesafe", false) => include_str!("../../fixtures/journeys/typesafe-food-response.json"),
        ("typesafe", true) => include_str!("../../fixtures/journeys/typesafe-budget-response.json"),
        ("openrouter-decisions", false) => {
            include_str!("../../fixtures/journeys/openrouter-food-response.json")
        }
        ("openrouter-decisions", true) => {
            include_str!("../../fixtures/journeys/openrouter-budget-response.json")
        }
        ("openai-decisions", true) => {
            include_str!("../../fixtures/journeys/openai-budget-response.json")
        }
        ("openai-decisions", false)
            if body["questions"]
                .as_array()
                .unwrap()
                .iter()
                .any(|question| question["name"] == "private") =>
        {
            include_str!("../../fixtures/journeys/openai-food-refusal-response.json")
        }
        ("openai-decisions", false) => {
            include_str!("../../fixtures/journeys/openai-food-response.json")
        }
        _ => panic!("unknown protocol"),
    }
}
fn server(protocol: &'static str) -> Server {
    Server::with_handler(move |body| (200, response(protocol, body).into()))
}
fn configure(home: &Home, jev: &Server, openai: &Server, router: &Server, attempts: u32) {
    let mut text = format!(
        "default_provider = \"jev\"\ndefault_model = \"fixture-requested-alias\"\n[execution]\nmax_attempts = {attempts}\nattempt_timeout_seconds = 10\n"
    );
    for (name, protocol, endpoint) in [
        ("jev", "system-one", &jev.url),
        ("openai", "openai-decisions", &openai.url),
        ("router", "system-one", &router.url),
    ] {
        write!(
            text,
            "[providers.{name}]\nprotocol = \"{protocol}\"\nbase_url = \"{endpoint}\"\n[providers.{name}.models.\"fixture-requested-alias\"]\ncontext_budget_tokens = 100000\ncontext_window_tokens = 100000\nreserved_output_tokens = 128\n"
        )
        .unwrap();
    }
    std::fs::write(home.0.join("config.toml"), text).unwrap();
}
/// Writes an `--input` request file. It names no session: the verb supplies it.
fn write_request(
    home: &Home,
    name: &str,
    turn: TurnRequest,
    updates: Vec<ContextUpdate>,
    key: Option<&str>,
) -> std::path::PathBuf {
    let request = EvaluationRequest {
        standing: StandingChanges::default(),
        turn,
        session: SessionSelection::New { name: None },
        updates,
        required_records: vec![],
        model: ModelSelection::default(),
        request_id: key.map(str::to_owned),
    };
    let mut file = serde_json::to_value(&request).unwrap();
    file.as_object_mut().unwrap().remove("session");
    let path = home.0.join(name);
    std::fs::write(&path, serde_json::to_vec_pretty(&file).unwrap()).unwrap();
    path
}
fn named(name: &str) -> nouli_core::identity::SessionName {
    SessionName::try_from(name.to_owned()).unwrap()
}
fn score(outcome: &nouli_core::outcome::TurnOutcome) -> f64 {
    let QuestionOutcome::Answered {
        answer: Answer::Score { value, .. },
        ..
    } = &outcome.outcomes[2]
    else {
        panic!("score")
    };
    *value
}
fn has_record(
    manifest: &nouli_core::history::EvaluationManifest,
    record: Id,
    representation: Representation,
) -> bool {
    manifest
        .records
        .iter()
        .any(|entry| entry.record == record && entry.representation == representation)
}
/// The record with `id` in a dispatched TOON context.
fn dispatched_record(context: &str, id: Id) -> Value {
    let decoded = codec::decode_value(context.as_bytes(), codec::InputFormat::Toon).unwrap();
    decoded["records"]
        .as_array()
        .unwrap()
        .iter()
        .find(|record| record["id"] == id.to_string())
        .unwrap()
        .clone()
}
#[test]
fn meal_budget_branch_blend_and_ephemeral_journey_asserts_dispatched_context_and_full_history() {
    let jev = server("typesafe");
    let openai = server("openai-decisions");
    let router = server("openrouter-decisions");
    let home = Home::create();
    configure(&home, &jev, &openai, &router, 1);
    let budget = context_record("budget", serde_json::json!(8), ContextKind::Commitment);
    let optional = context_record(
        "old_evidence",
        serde_json::json!("OPTIONAL_EVIDENCE_".repeat(1500)),
        ContextKind::Evidence,
    );
    let meal = seed_dinner_and_replay_it_from_toon(&home, &jev, &budget, &optional);
    let follow = follow_dinner_with_compacted_history(&home, &jev, &meal, &optional);
    branch_relaxed_without_optional_evidence(&home, &router, &budget, &optional, &follow);
    let file = home.0.join("plain-food.json");
    std::fs::write(&file, serde_json::to_vec(&fixture("food")).unwrap()).unwrap();
    let blend = report(&home.run([
        "new",
        "Blend",
        "--input",
        file.to_str().unwrap(),
        "--from",
        "Dinner",
        "--from",
        "Relaxed",
        "--provider",
        "openai",
    ]));
    assert_eq!(score(&blend.outcome), 1.43);
    assert_ne!(score(&blend.outcome), f64::midpoint(0.6, 1.43));
    assert_blend_history_and_source_pins(&home, &blend, &meal, &budget);
    continue_blend_from_its_captured_sources(&home, &openai, &budget, &file, &blend);
    let bytes = ledger_bytes(&home);
    let ephemeral = report(&home.run([
        "whatif",
        "Blend",
        "--input",
        file.to_str().unwrap(),
        "--provider",
        "router",
    ]));
    assert!(ephemeral.turn.is_none());
    assert_eq!(bytes, ledger_bytes(&home));
    let exported = home.run(["sessions", "export", "Blend", "--json"]);
    assert!(exported.status.success());
    let export: SessionExport = serde_json::from_slice(&exported.stdout).unwrap();
    assert_eq!(export.turns.len(), 5);
    let list = home.run(["sessions", "list", "--json"]);
    let list: Value = serde_json::from_slice(&list.stdout).unwrap();
    assert_eq!(list["sessions"].as_array().unwrap().len(), 3);
    assert_eq!((jev.count(), openai.count(), router.count()), (3, 2, 2));
}
/// Starts `Dinner` with a large state, optional evidence, and a pinned budget,
/// then replays the same request from an equivalent TOON file.
fn seed_dinner_and_replay_it_from_toon(
    home: &Home,
    jev: &Server,
    budget: &ContextRecord,
    optional: &ContextRecord,
) -> nouli_core::protocol::TurnReport {
    let mut first = fixture("food");
    first.state.push(Input::Text {
        text: "OLD_DECISION_STATE_".repeat(1500),
    });
    let path = write_request(
        home,
        "food.json",
        first,
        vec![
            ContextUpdate::Append {
                record: optional.clone(),
            },
            ContextUpdate::Pin {
                target: PinTarget::New {
                    record: budget.clone(),
                },
            },
        ],
        Some("meal-seed"),
    );
    let meal = report(&home.run(["new", "Dinner", "--input", path.to_str().unwrap()]));
    assert_eq!(score(&meal.outcome), 1.43);
    assert_eq!(jev.count(), 1);
    let request: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    let toon = home.0.join("equivalent.toon");
    std::fs::write(&toon, codec::encode_toon(&request).unwrap()).unwrap();
    let replay = report(&home.run(["new", "Dinner", "--input", toon.to_str().unwrap()]));
    assert!(replay.replayed);
    assert_eq!(replay.turn, meal.turn);
    assert_eq!(jev.count(), 1);
    meal
}
/// Adds a budget turn under a tight context budget: the meal turn is sent
/// compacted with its native answers, and the optional evidence in full.
fn follow_dinner_with_compacted_history(
    home: &Home,
    jev: &Server,
    meal: &nouli_core::protocol::TurnReport,
    optional: &ContextRecord,
) -> nouli_core::protocol::TurnReport {
    let commitment = context_record(
        "selected_meal",
        serde_json::json!({"meal":"lentils","completed":true}),
        ContextKind::Commitment,
    );
    let path = write_request(
        home,
        "budget.json",
        fixture("budget"),
        vec![ContextUpdate::Pin {
            target: PinTarget::New { record: commitment },
        }],
        None,
    );
    let follow = report(&home.run([
        "add",
        "Dinner",
        "--input",
        path.to_str().unwrap(),
        "--context-budget",
        "9000",
    ]));
    assert_eq!(score(&follow.outcome), 0.6);
    assert!(has_record(
        &follow.manifest,
        meal.turn.unwrap(),
        Representation::Compacted
    ));
    assert!(has_record(
        &follow.manifest,
        optional.id,
        Representation::Full
    ));
    let old = dispatched_record(
        body_context(&jev.calls.lock().unwrap()[1]),
        meal.turn.unwrap(),
    );
    let results = &old["value"]["results"];
    assert_eq!(results[0]["answer"]["selected"], "lentils");
    assert_eq!(results[0]["answer"]["description"], "Lentil stew");
    assert_eq!(results[0]["answer"]["probability"], 0.7);
    assert!(results[0]["answer"].get("distribution").is_none());
    assert_eq!(results[2]["answer"]["value"], 1.43);
    assert_eq!(results[2]["answer"]["scale"][2]["label"], "Excellent");
    assert_eq!(results[0]["confidence"]["value"], 0.35);
    assert!(
        results[0]["confidence"]["meaning"]
            .as_str()
            .unwrap()
            .contains("jev")
    );
    assert_eq!(old["value"]["provider"]["model"], "laya-food-fixture");
    assert!(!follow.manifest.input_count.exact);
    assert!(follow.outcome.usage.exact);
    follow
}
/// Branches `Relaxed` from `Dinner` on the router, unpinning the budget under
/// a budget so tight that the optional evidence and old state are left out.
fn branch_relaxed_without_optional_evidence(
    home: &Home,
    router: &Server,
    budget: &ContextRecord,
    optional: &ContextRecord,
    follow: &nouli_core::protocol::TurnReport,
) {
    let branch_request = EvaluationRequest {
        standing: StandingChanges::default(),
        turn: fixture("food"),
        session: SessionSelection::New { name: None },
        updates: vec![ContextUpdate::Unpin { target: budget.id }],
        required_records: vec![],
        model: ModelSelection::default(),
        request_id: None,
    };
    let mut value = codec::to_value(&branch_request).unwrap();
    value.as_object_mut().unwrap().remove("session");
    let path = home.0.join("unpin.json");
    std::fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
    let branch = report(&home.run([
        "new",
        "Relaxed",
        "--input",
        path.to_str().unwrap(),
        "--from",
        "Dinner",
        "--provider",
        "router",
        "--context-budget",
        "1800",
    ]));
    assert!(has_record(
        &branch.manifest,
        optional.id,
        Representation::Omitted
    ));
    assert!(has_record(
        &branch.manifest,
        follow.turn.unwrap(),
        Representation::Compacted
    ));
    let captured = router.calls.lock().unwrap();
    let context = body_context(&captured[0]);
    assert!(!context.contains("OPTIONAL_EVIDENCE_"));
    assert!(!context.contains("OLD_DECISION_STATE_"));
    let compact = dispatched_record(context, follow.turn.unwrap());
    assert_eq!(compact["value"]["results"][2]["answer"]["value"], 0.6);
    assert_eq!(
        compact["value"]["results"][2]["answer"]["scale"][0]["label"],
        "Over budget"
    );
    assert_eq!(
        captured[0]["questions"]["meal"]["criteria"]["pasta"],
        "Pasta with tomato sauce"
    );
}
/// The blend holds every source turn once, with its full original request, and
/// each source keeps its own pin on the budget.
fn assert_blend_history_and_source_pins(
    home: &Home,
    blend: &nouli_core::protocol::TurnReport,
    meal: &nouli_core::protocol::TurnReport,
    budget: &ContextRecord,
) {
    let ledger = Ledger::open(&home.store(), OpenMode::ReadOnlyExisting).unwrap();
    let exported = ledger
        .export_session(&SessionSelector::Id(blend.session.unwrap()))
        .unwrap();
    assert_eq!(exported.turns.len(), 4);
    assert_eq!(
        exported
            .turns
            .iter()
            .filter(|turn| Some(turn.id) == meal.turn)
            .count(),
        1
    );
    assert!(
        exported.turns[0].request.state.iter().any(
            |input| matches!(input,Input::Text {text} if text.contains("OLD_DECISION_STATE_"))
        )
    );
    let budget_pinned = |name: &str| {
        let export = ledger
            .export_session(&SessionSelector::Name(named(name)))
            .unwrap();
        let ContextView::Resolved { records } = export.context else {
            panic!("context")
        };
        records
            .iter()
            .find(|record| record.record.id == budget.id)
            .unwrap()
            .pinned
    };
    assert!(budget_pinned("Dinner"));
    assert!(!budget_pinned("Relaxed"));
}
/// Supersedes the budget in `Dinner`, then continues `Blend`: it still sees the
/// sources as they were when it captured them.
fn continue_blend_from_its_captured_sources(
    home: &Home,
    openai: &Server,
    budget: &ContextRecord,
    file: &std::path::Path,
    blend: &nouli_core::protocol::TurnReport,
) {
    let replacement = context_record("budget", serde_json::json!(12), ContextKind::Commitment);
    let mut changed = fixture("budget");
    changed.state.push(Input::Text {
        text: "SOURCE_AFTER_CAPTURE".into(),
    });
    let path = write_request(
        home,
        "changed.json",
        changed,
        vec![ContextUpdate::Supersede {
            targets: vec![budget.id],
            replacement: replacement.clone(),
            pinned: None,
        }],
        None,
    );
    report(&home.run(["add", "Dinner", "--input", path.to_str().unwrap()]));
    let continued = report(&home.run(["add", "Blend", "--input", file.to_str().unwrap()]));
    assert_eq!(continued.session, blend.session);
    let calls = openai.calls.lock().unwrap();
    let actual = body_context(calls.last().unwrap());
    assert!(!actual.contains("SOURCE_AFTER_CAPTURE"));
    assert!(!actual.contains(&replacement.id.to_string()));
    assert!(actual.contains(&budget.id.to_string()));
    assert!(actual.contains("Over budget"));
    assert!(actual.contains("Excellent"));
}

#[test]
fn protected_context_overflow_precedes_native_dispatch_for_every_protocol() {
    let jev = server("typesafe");
    let openai = server("openai-decisions");
    let router = server("openrouter-decisions");
    let home = Home::create();
    configure(&home, &jev, &openai, &router, 1);
    for profile in ["jev", "openai", "router"] {
        let record = context_record(
            "mandatory",
            serde_json::json!("PROTECTED_".repeat(2000)),
            ContextKind::Commitment,
        );
        let file = write_request(
            &home,
            &format!("{profile}.json"),
            fixture("food"),
            vec![ContextUpdate::Pin {
                target: PinTarget::New { record },
            }],
            None,
        );
        let first = report(&home.run([
            "new",
            &format!("Pinned_{profile}"),
            "--input",
            file.to_str().unwrap(),
            "--provider",
            profile,
        ]));
        let count = jev.count() + openai.count() + router.count();
        let next = home.0.join("next.json");
        std::fs::write(&next, serde_json::to_vec(&fixture("food")).unwrap()).unwrap();
        let output = home.run([
            "add",
            &first.session.unwrap().to_string(),
            "--input",
            next.to_str().unwrap(),
            "--context-budget",
            "1000",
        ]);
        assert_eq!(output.status.code(), Some(3));
        let result: ResultEnvelope = serde_json::from_slice(&output.stdout).unwrap();
        assert!(
            matches!(result.result,Report::Error(error) if error.category==nouli_core::protocol::ErrorCategory::Capacity)
        );
        assert_eq!(jev.count() + openai.count() + router.count(), count);
        let ledger = Ledger::open(&home.store(), OpenMode::ReadOnlyExisting).unwrap();
        assert_eq!(
            ledger
                .session(&SessionSelector::Id(first.session.unwrap()))
                .unwrap()
                .frontier,
            vec![first.turn.unwrap()]
        );
        assert_eq!(
            ledger.list_attempts(first.session.unwrap()).unwrap().len(),
            1
        );
    }
}
#[test]
fn checked_in_json_toon_journey_payloads_are_equivalent_and_keep_native_numeric_meaning() {
    for (json, toon) in [
        (
            include_bytes!("../../fixtures/journeys/food.json").as_slice(),
            include_bytes!("../../fixtures/journeys/food.toon").as_slice(),
        ),
        (
            include_bytes!("../../fixtures/journeys/budget.json").as_slice(),
            include_bytes!("../../fixtures/journeys/budget.toon").as_slice(),
        ),
        (
            include_bytes!("../../fixtures/journeys/food-with-refusal.json").as_slice(),
            include_bytes!("../../fixtures/journeys/food-with-refusal.toon").as_slice(),
        ),
    ] {
        assert_eq!(
            codec::decode_request(json, codec::InputFormat::Json).unwrap(),
            codec::decode_request(toon, codec::InputFormat::Toon).unwrap()
        );
    }
}
/// A condition on the initial node's `available` probability against zero.
fn available_probability(
    comparison: nouli_core::workflow::reference::Comparison,
) -> nouli_core::workflow::reference::Condition {
    use nouli_core::workflow::reference::{
        Condition, MissingPolicy, Operand, ResultField, ResultReference,
    };
    Condition::Compare {
        left: Operand::Reference {
            reference: ResultReference {
                node: "initial".into(),
                question: "available".into(),
                field: ResultField::Probability,
                pointer: None,
                on_missing: MissingPolicy::Required,
            },
        },
        comparison,
        right: Operand::Literal {
            value: Data::new(serde_json::json!(0.0)).unwrap(),
        },
    }
}
/// The fixture workflow over food turns: a refusing initial node on `openai`,
/// router and openai nodes, a node that always runs and one that never does,
/// and an output for the refused private question.
fn mixed_provider_graph() -> nouli_core::workflow::WorkflowSpec {
    use nouli_core::workflow::{
        OutputBinding, WorkflowNode,
        reference::{Comparison, MissingPolicy, ResultField, ResultReference},
    };
    let mut graph = harness::workflow();
    for node in &mut graph.nodes {
        node.turn = fixture("food");
        node.turn.state.push(Input::Text {
            text: format!("NODE:{}", node.id),
        });
    }
    graph.nodes[0].turn = fixture("refusal");
    graph.nodes[0].turn.state.push(Input::Text {
        text: "NODE:initial".into(),
    });
    graph.nodes[0].model.provider = Some("openai".into());
    graph.nodes[2].model.provider = Some("router".into());
    graph.nodes[3].model.provider = Some("router".into());
    graph.nodes[4].model.provider = Some("openai".into());
    graph.nodes[2].when = Some(available_probability(Comparison::Ge));
    graph.nodes.push(WorkflowNode {
        id: "inactive".into(),
        dependencies: vec!["initial".into()],
        turn: fixture("food"),
        inputs: vec![],
        when: Some(available_probability(Comparison::Lt)),
        updates: vec![],
        required_records: vec![],
        model: ModelSelection::default(),
    });
    graph.nodes[3].dependencies.push("inactive".into());
    graph.outputs.push(OutputBinding {
        name: "private_receipt".into(),
        reference: ResultReference {
            node: "initial".into(),
            question: "private".into(),
            field: ResultField::Outcome,
            pointer: None,
            on_missing: MissingPolicy::Optional,
        },
    });
    graph
}
fn provider_counts(jev: &Server, openai: &Server, router: &Server) -> (usize, usize, usize) {
    (jev.count(), openai.count(), router.count())
}
#[test]
fn parallel_mixed_provider_workflow_preserves_refusal_skips_retries_resume_and_continuation() {
    use nouli_core::workflow::{NodeStatus, RunStatus};
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    let failures = Arc::new(AtomicUsize::new(2));
    let remaining = failures.clone();
    let jev = Server::with_handler(move |body| {
        if body_context(body).contains("NODE:high")
            && remaining
                .try_update(Ordering::SeqCst, Ordering::SeqCst, |count| {
                    count.checked_sub(1)
                })
                .is_ok()
        {
            (503, "{}".into())
        } else {
            (200, response("typesafe", body).into())
        }
    });
    let openai = server("openai-decisions");
    let router = server("openrouter-decisions");
    let home = Home::create();
    configure(&home, &jev, &openai, &router, 2);
    let path = home.0.join("parallel.json");
    std::fs::write(&path, serde_json::to_vec(&mixed_provider_graph()).unwrap()).unwrap();
    let partial = run_report(
        &home.run([
            "workflow",
            "run",
            path.to_str().unwrap(),
            "--parallel",
            "2",
            "--request-id",
            "mixed-food",
        ]),
        4,
    );
    assert_eq!(partial.status, RunStatus::Partial);
    assert_eq!(partial.nodes[1].status, NodeStatus::Failed);
    assert_eq!(partial.nodes[2].status, NodeStatus::Completed);
    assert_eq!(partial.nodes[5].status, NodeStatus::Skipped);
    assert_eq!(provider_counts(&jev, &openai, &router), (2, 1, 1));
    let nouli_core::workflow::reference::ReferenceValue::Present { value } =
        &partial.outputs[2].value
    else {
        panic!("whole refusal")
    };
    assert_eq!(value.value()["status"], "refused");
    let done = resume_completes_only_unfinished_nodes(&home, &partial);
    assert_eq!(provider_counts(&jev, &openai, &router), (3, 2, 2));
    assert_native_providers_and_refusal(&home, &done);
    continue_from_the_final_head(&home, &jev, &done);
    // A run that keeps no session leaves the ledger untouched.
    let bytes = ledger_bytes(&home);
    let output = home.run([
        "workflow",
        "run",
        path.to_str().unwrap(),
        "--from",
        &done.session.unwrap().to_string(),
        "--no-session",
        "--jsonl",
    ]);
    assert!(output.status.success());
    let events = jsonl_events(&output);
    assert!(events.iter().all(|event| event.run.is_none()));
    assert_eq!(bytes, ledger_bytes(&home));
    assert_eq!(provider_counts(&jev, &openai, &router), (5, 4, 4));
}
/// Resumes the partial run and checks it completes while keeping finished turns.
fn resume_completes_only_unfinished_nodes(
    home: &Home,
    partial: &nouli_core::protocol::RunReport,
) -> nouli_core::protocol::RunReport {
    use nouli_core::workflow::{NodeStatus, RunStatus};
    let output = home.run([
        "workflow",
        "resume",
        &partial.run.unwrap().to_string(),
        "--jsonl",
    ]);
    assert!(output.status.success());
    assert_eq!(String::from_utf8_lossy(&output.stderr), "");
    let events = jsonl_events(&output);
    let result = harness::terminal(&events).clone();
    let Report::Run(done) = result.result else {
        panic!("run")
    };
    assert_eq!(done.status, RunStatus::Completed);
    assert_eq!(done.nodes[0].turn, partial.nodes[0].turn);
    assert_eq!(done.nodes[2].turn, partial.nodes[2].turn);
    assert_eq!(done.nodes[5].status, NodeStatus::Skipped);
    done
}
/// Each node kept its own provider's model, native confidence, and refusal.
fn assert_native_providers_and_refusal(home: &Home, done: &nouli_core::protocol::RunReport) {
    for (node, model) in [
        (0, "openai-food-fixture"),
        (1, "laya-food-fixture"),
        (2, "router/food-fixture"),
    ] {
        assert_eq!(done.nodes[node].provider.as_ref().unwrap().model, model);
    }
    let inspected = Ledger::open(&home.store(), OpenMode::ReadOnlyExisting)
        .unwrap()
        .inspect_turn(done.nodes[0].turn.unwrap())
        .unwrap();
    let QuestionOutcome::Answered {
        confidence: Some(confidence),
        ..
    } = &inspected.turn.outcome.outcomes[0]
    else {
        panic!("native confidence")
    };
    assert_eq!(confidence.value, Some(0.8));
    assert!(confidence.meaning.contains("openai-food-fixture"));
    assert!(matches!(
        inspected.turn.outcome.outcomes[3],
        QuestionOutcome::Refused { .. }
    ));
}
/// A turn added to the run's session starts from the final head and sees each
/// completed node's turn, including the refusal, exactly once.
fn continue_from_the_final_head(home: &Home, jev: &Server, done: &nouli_core::protocol::RunReport) {
    let final_head = done.nodes[4].turn.unwrap();
    let input = home.0.join("continue.json");
    std::fs::write(&input, serde_json::to_vec(&fixture("food")).unwrap()).unwrap();
    let continued = report(&home.run([
        "add",
        &done.session.unwrap().to_string(),
        "--input",
        input.to_str().unwrap(),
    ]));
    assert_eq!(continued.manifest.sources[0].frontier, vec![final_head]);
    let calls = jev.calls.lock().unwrap();
    let context = codec::decode_value(
        body_context(calls.last().unwrap()).as_bytes(),
        codec::InputFormat::Toon,
    )
    .unwrap();
    let records = context["records"].as_array().unwrap();
    for record in [
        done.nodes[0].turn.unwrap(),
        done.nodes[1].turn.unwrap(),
        done.nodes[2].turn.unwrap(),
        final_head,
    ] {
        assert_eq!(
            records
                .iter()
                .filter(|entry| entry["id"] == record.to_string())
                .count(),
            1
        );
    }
    assert!(body_context(calls.last().unwrap()).contains("refused"));
}

#[test]
fn successful_output_configuration_auto_overrides_and_jsonl_keep_protocol_stdout_clean() {
    use nouli_core::protocol::EventKind;
    let jev = server("typesafe");
    let openai = server("openai-decisions");
    let router = server("openrouter-decisions");
    let home = Home::create();
    configure(&home, &jev, &openai, &router, 1);
    let config = home.0.join("config.toml");
    let original = std::fs::read_to_string(&config).unwrap();
    std::fs::write(&config, format!("output = \"text\"\n{original}")).unwrap();
    let file = home.0.join("food.json");
    std::fs::write(&file, serde_json::to_vec(&fixture("food")).unwrap()).unwrap();
    // A what-if needs a saved session to start from.
    let file_arg = file.to_str().unwrap();
    report(&home.run(["new", "Meal", "--input", file_arg, "--json"]));
    let output = home.run(["whatif", "Meal", "--input", file_arg]);
    assert!(output.status.success());
    assert_eq!(String::from_utf8_lossy(&output.stderr), "");
    let text = String::from_utf8(output.stdout).unwrap();
    for expected in [
        "(not saved) · 0 facts · 3 questions",
        "Lentil stew",
        "0.70",
        "yes",
        "1.00",
        "expected 1.43 of 2",
        "Poor 0.00 · Adequate 0.57 · Excellent 0.43",
        "laya-food-fixture",
    ] {
        assert!(text.contains(expected), "{expected}: {text}");
    }
    assert!(!text.contains('\x1b'));
    report(&home.run(["whatif", "Meal", "--input", file_arg, "--json"]));
    report(&home.run(["whatif", "Meal", "--input", file_arg, "--format", "auto"]));
    let output = home.run(["whatif", "Meal", "--input", file_arg, "--jsonl"]);
    assert!(output.status.success());
    let events = jsonl_events(&output);
    assert_eq!(String::from_utf8_lossy(&output.stderr), "");
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event.event, EventKind::Terminal { .. }))
            .count(),
        1
    );
    assert_eq!(jev.count(), 5);
    let file = home.0.join("refusal.json");
    std::fs::write(&file, serde_json::to_vec(&fixture("refusal")).unwrap()).unwrap();
    let output = home.run([
        "whatif",
        "Meal",
        "--input",
        file.to_str().unwrap(),
        "--provider",
        "openai",
    ]);
    assert!(output.status.success());
    assert_eq!(String::from_utf8_lossy(&output.stderr), "");
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(text.contains("refused:"), "{text}");
    assert!(text.contains("private"));
    assert!(text.contains("expected 1.43 of 2"));
    // Only the session's own turn was saved; every what-if left no trace.
    let ledger = Ledger::open(&home.store(), OpenMode::ReadOnlyExisting).unwrap();
    assert_eq!(ledger.list_sessions().unwrap().len(), 1);
    let session = ledger.list_sessions().unwrap().remove(0);
    assert_eq!(session.frontier.len(), 1);
    assert_eq!(ledger.list_attempts(session.id).unwrap().len(), 1);
}
