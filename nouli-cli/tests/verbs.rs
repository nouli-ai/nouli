#[path = "support/harness.rs"]
#[allow(dead_code, reason = "each test uses a subset of the helpers")]
mod harness;
#[path = "support/loopback.rs"]
#[allow(dead_code, reason = "each test uses a subset of the helpers")]
mod loopback;
use harness::{Home, QUESTION, report};
use loopback::Server;
use nouli_core::{
    identity::SessionSelector,
    protocol::{ErrorReport, Report, ResultEnvelope},
    storage::{Ledger, OpenMode},
};
use serde_json::{Value, json};
use std::process::Output;

/// A provider that answers exactly the questions it is asked, and flips its
/// choice once the evidence says the train is cancelled.
fn provider() -> Server {
    Server::with_handler(|body| {
        let mut response: Value = serde_json::from_str(&harness::typesafe(body).1).unwrap();
        if body["questions"].get("travel").is_some()
            && body["state"].as_str().unwrap().contains("CANCELLED")
        {
            response["answers"]["travel"] = json!({
                "type": "choice",
                "choice": "flight",
                "probabilities": {"train": 0.1, "flight": 0.9},
                "confidence": 0.35,
                "answer_confidence": 0.9
            });
        }
        (200, response.to_string())
    })
}

fn failure(output: Output, code: &str) -> ErrorReport {
    assert_eq!(output.status.code(), Some(2), "{code}");
    assert_eq!(String::from_utf8_lossy(&output.stderr), "");
    let envelope: ResultEnvelope = serde_json::from_slice(&output.stdout).unwrap();
    let Report::Error(error) = envelope.result else {
        panic!("expected an error report")
    };
    assert_eq!(error.code, code);
    error
}

fn ids(questions: &[nouli_core::decision::Question]) -> Vec<&str> {
    questions
        .iter()
        .map(|question| question.id.as_str())
        .collect()
}

#[test]
fn each_turn_asks_only_its_own_questions_and_replace_and_whatif_change_only_what_they_name() {
    let server = provider();
    let home = Home::create();
    home.configure(&server, "typesafe", false);
    let first = report(
        home.run(
            [
                &["new", "Trip", "The train costs 40 USD", "--json"][..],
                &QUESTION,
            ]
            .concat(),
        ),
    );
    assert_eq!(ids(&first.questions), ["travel"]);
    // A fact needs a question to go with it; nothing is asked again.
    let calls = server.count();
    let nothing = failure(
        home.run(["add", "Trip", "A taxi costs 70 USD", "--json"]),
        "questions_required",
    );
    assert!(nothing.message.contains("-q"));
    assert_eq!(server.count(), calls);
    let second = report(home.run([
        "add",
        "Trip",
        "A taxi costs 70 USD",
        "-q",
        "late: Is the taxi late?",
        "--json",
    ]));
    assert_eq!(ids(&second.questions), ["late"]);
    assert_eq!(
        server.calls.lock().unwrap()[1]["questions"]
            .as_object()
            .unwrap()
            .len(),
        1
    );
    let ledger = Ledger::open(&home.store(), OpenMode::ReadOnlyExisting).unwrap();
    let trip = SessionSelector::Name("Trip".to_owned().try_into().unwrap());
    let numbered = ledger.facts(&trip).unwrap();
    assert_eq!(
        numbered
            .iter()
            .map(|fact| fact.text.as_str())
            .collect::<Vec<_>>(),
        ["The train costs 40 USD", "A taxi costs 70 USD"]
    );
    drop(ledger);
    // Replacing fact 2 shows the model only the new text, and keeps its number.
    let fourth = report(home.run([
        "add",
        "Trip",
        "--replace",
        "2",
        "A taxi costs 140 USD",
        "-q",
        "cheap: Is the taxi cheap?",
        "--json",
    ]));
    assert_eq!(ids(&fourth.questions), ["cheap"]);
    let seen = server.calls.lock().unwrap().last().unwrap()["state"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(seen.contains("140 USD") && !seen.contains("70 USD"));
    let ledger = Ledger::open(&home.store(), OpenMode::ReadOnlyExisting).unwrap();
    let numbered = ledger.facts(&trip).unwrap();
    assert_eq!(numbered.len(), 2);
    assert_eq!(numbered[1].text, "A taxi costs 140 USD");
    drop(ledger);
    // A what-if asks its own question and saves nothing.
    let before = std::fs::read(home.store().join("ledger.sqlite3")).unwrap();
    let calls = server.count();
    let what = report(home.run([
        "whatif",
        "Trip",
        "The train is CANCELLED",
        "-q",
        "extra: Is there another way?",
        "--json",
    ]));
    assert!(what.session.is_none() && what.turn.is_none());
    assert_eq!(ids(&what.questions), ["extra"]);
    assert_eq!(server.count(), calls + 1);
    assert_eq!(
        before,
        std::fs::read(home.store().join("ledger.sqlite3")).unwrap()
    );
    // Mistakes are input errors that never reach the provider.
    let calls = server.count();
    failure(home.run(["whatif", "Trip", "--json"]), "questions_required");
    let removed = home.run(["add", "Trip", "--drop", "travel"]);
    assert_eq!(removed.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&removed.stderr).contains("--drop"));
    let grammar = failure(
        home.run(["add", "Trip", "-q", "no colon in this question"]),
        "invalid_decision_input",
    );
    assert!(grammar.message.starts_with("grammar error: "));
    // Each call asks one question, so a second -q is a usage error.
    let twice = home.run(["add", "Trip", "-q", "a: A?", "-q", "b: B?"]);
    assert_eq!(twice.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&twice.stderr).contains("--question"));
    failure(
        home.run(["add", "Trip", "-q", "bad: What? [only one]"]),
        "invalid_decision_input",
    );
    failure(
        home.run([
            "add",
            "Trip",
            "--replace",
            "9",
            "no such fact",
            "-q",
            "a: b",
        ]),
        "invalid_decision_input",
    );
    failure(
        home.run(["add", "Trip", "--replace", "one", "text"]),
        "invalid_decision_input",
    );
    failure(
        home.run(["add", "Trip", "--input", "request.json", "-q", "a: b"]),
        "invalid_decision_input",
    );
    failure(
        home.run(["add", "Trip", "--input", "request.json", "A fact"]),
        "invalid_decision_input",
    );
    // A 0.2.0 request file with standing questions says what to change.
    let question = serde_json::to_value(harness::turn()).unwrap()["questions"][0].clone();
    for (name, questions) in [
        ("standing-only", json!([])),
        ("standing-and-own", json!([question])),
    ] {
        let path = home.0.join(format!("{name}.json"));
        let file =
            json!({"turn": {"state": [], "questions": questions}, "standing": {"set": [question]}});
        std::fs::write(&path, file.to_string()).unwrap();
        let standing = failure(
            home.run(["add", "Trip", "--input", path.to_str().unwrap(), "--json"]),
            "invalid_standing_question",
        );
        assert!(standing.message.contains("turn.questions"), "{name}");
    }
    let empty = home.0.join("empty.json");
    std::fs::write(
        &empty,
        json!({"turn": {"state": [], "questions": []}}).to_string(),
    )
    .unwrap();
    let nothing = failure(
        home.run(["add", "Trip", "--input", empty.to_str().unwrap(), "--json"]),
        "questions_required",
    );
    assert!(nothing.message.contains("turn.questions"));
    assert_eq!(server.count(), calls);
    assert_eq!(
        before,
        std::fs::read(home.store().join("ledger.sqlite3")).unwrap()
    );
}

#[test]
fn new_without_facts_or_questions_creates_a_session_and_branches_ask_only_their_own() {
    let server = provider();
    let home = Home::create();
    home.configure(&server, "typesafe", false);
    let created = home.run(["new", "Empty", "--json"]);
    assert!(created.status.success());
    let envelope: ResultEnvelope = serde_json::from_slice(&created.stdout).unwrap();
    let Report::Session(session) = envelope.result else {
        panic!("a session report")
    };
    assert_eq!(session.name.as_ref().unwrap().as_str(), "Empty");
    assert_eq!(session.sources, [] as [nouli_core::history::SourcePoint; 0]);
    assert_eq!(server.count(), 0);
    let ledger = Ledger::open(&home.store(), OpenMode::ReadOnlyExisting).unwrap();
    let saved = ledger
        .session(&SessionSelector::Id(session.session))
        .unwrap();
    assert_eq!(saved.frontier, [] as [nouli_core::identity::Id; 0]);
    assert_eq!(
        ledger.list_attempts(saved.id).unwrap(),
        [] as [nouli_core::storage::turns::AttemptSnapshot; 0]
    );
    drop(ledger);
    // The session is usable: its first turn brings the question.
    let first =
        report(home.run([&["add", "Empty", "A fact"][..], &QUESTION, &["--json"]].concat()));
    assert_eq!(first.session, Some(session.session));
    // A name is taken once.
    let again = home.run(["new", "Empty"]);
    assert_eq!(again.status.code(), Some(5));
    // A branch asks its own question, not its source's.
    failure(
        home.run(["new", "Child", "A later fact", "--from", "Empty", "--json"]),
        "questions_required",
    );
    let child = report(home.run([
        "new",
        "Child",
        "A later fact",
        "--from",
        "Empty",
        "-q",
        "later: Is it later?",
        "--json",
    ]));
    assert_eq!(ids(&child.questions), ["later"]);
    let branch = home.run(["new", "Sibling", "--from", "Empty", "--format", "text"]);
    assert!(branch.status.success());
    let text = String::from_utf8(branch.stdout).unwrap();
    assert!(
        text.contains("Created session Sibling") && text.contains("From: "),
        "{text}"
    );
    // A fact with no question says so.
    let blend = home.run(["new", "Blend", "--from", "Empty", "--from", "Child"]);
    assert!(blend.status.success());
    let nothing = failure(home.run(["add", "Blend", "A fact"]), "questions_required");
    assert!(nothing.message.contains("-q"));
}

#[test]
fn text_output_shows_bars_and_what_changed_since_the_last_answer() {
    let server = provider();
    let home = Home::create();
    home.configure(&server, "typesafe", false);
    let first = home.run(
        [
            &["new", "Trip", "The train is on time", "--format", "text"][..],
            &QUESTION,
        ]
        .concat(),
    );
    assert!(first.status.success());
    let text = String::from_utf8(first.stdout).unwrap();
    assert!(
        text.starts_with("  turn 1 · 1 fact · 1 question\n"),
        "{text}"
    );
    assert!(text.contains("▶ train") && text.contains("0.70"), "{text}");
    assert!(!text.contains("→ was") && !text.contains('\x1b'));
    // Asking the same question again by name compares with its last answer.
    let second = home.run(
        [
            &["add", "Trip", "The train is CANCELLED", "--format", "text"][..],
            &QUESTION,
        ]
        .concat(),
    );
    let text = String::from_utf8(second.stdout).unwrap();
    assert!(
        text.starts_with("  turn 2 · 1 fact · 1 question\n"),
        "{text}"
    );
    assert!(text.contains("▶ flight"), "{text}");
    assert!(text.contains("→ was train 0.70"), "{text}");
    // A what-if says it was not saved.
    let what = home.run(
        [
            &["whatif", "Trip", "--format", "text"][..],
            &harness::QUESTIONS[2],
        ]
        .concat(),
    );
    let text = String::from_utf8(what.stdout).unwrap();
    assert!(
        text.starts_with("  (not saved) · 0 facts · 1 question\n"),
        "{text}"
    );
    assert!(text.contains("expected 1.43 of 2"), "{text}");
    // A replayed request keeps its record IDs, so the same command replays.
    let args = [
        "add",
        "Trip",
        "Replay me",
        "-q",
        "replay: Is this replayed?",
        "--request-id",
        "same-command",
        "--json",
    ];
    let saved = report(home.run(args));
    let replay = report(home.run(args));
    assert!(replay.replayed && replay.turn == saved.turn);
    // A replacement replays too, though its fact number now names the
    // replacement; a different replacement under the same ID conflicts.
    let args = [
        "add",
        "Trip",
        "--replace",
        "1",
        "The train is delayed",
        "-q",
        "delayed: Is the train delayed?",
        "--request-id",
        "replace-command",
        "--json",
    ];
    let saved = report(home.run(args));
    assert!(!saved.replayed);
    let replay = report(home.run(args));
    assert!(replay.replayed && replay.turn == saved.turn);
    let other = home.run([
        "add",
        "Trip",
        "--replace",
        "1",
        "The train is early",
        "-q",
        "delayed: Is the train delayed?",
        "--request-id",
        "replace-command",
        "--json",
    ]);
    assert!(
        String::from_utf8_lossy(&other.stdout).contains("request_conflict"),
        "{other:?}"
    );
}

#[test]
fn facts_list_inherited_and_replaced_facts() {
    let server = provider();
    let home = Home::create();
    home.configure(&server, "typesafe", false);
    report(
        home.run(
            [
                &["new", "Trip", "Fact one", "Fact two", "--json"][..],
                &QUESTION,
            ]
            .concat(),
        ),
    );
    report(home.run([
        "add",
        "Trip",
        "--replace",
        "2",
        "Fact two, corrected",
        "-q",
        "corrected: Is fact two corrected?",
        "--json",
    ]));
    report(home.run([
        "new",
        "Child",
        "A fact of its own",
        "--from",
        "Trip",
        "-q",
        "own: Does it have a fact of its own?",
        "--json",
    ]));
    let facts = |session: &str, format: &str| {
        let output = home.run(["facts", session, "--format", format]);
        assert!(output.status.success(), "{output:?}");
        assert_eq!(String::from_utf8_lossy(&output.stderr), "");
        String::from_utf8(output.stdout).unwrap()
    };
    assert_eq!(
        facts("Trip", "text"),
        "   1  Fact one\n   2  Fact two, corrected   (replaced; was: Fact two)\n"
    );
    let child = facts("Child", "text");
    let lines: Vec<_> = child.lines().collect();
    assert_eq!(lines.len(), 3, "{child}");
    assert_eq!(lines[0], "   1  Fact one   (from Trip)");
    assert!(lines[1].starts_with("   2  Fact two, corrected   (from Trip)"));
    assert_eq!(lines[2], "   3  A fact of its own");
    let json: Value = serde_json::from_str(&facts("Child", "json")).unwrap();
    let listed = json["facts"].as_array().unwrap();
    assert_eq!(listed.len(), 3);
    assert_eq!(listed[1]["number"], 2);
    assert_eq!(listed[1]["previous"], "Fact two");
    assert!(listed[0]["source_session"].is_string() && listed[2]["source_session"].is_null());
    // A session without facts says so; an unknown one is an input error.
    home.run(["new", "Empty"]);
    assert_eq!(facts("Empty", "text"), "  no facts yet\n");
    assert_eq!(home.run(["facts", "Missing"]).status.code(), Some(2));
    // Sessions keep no questions to list.
    assert_eq!(home.run(["questions", "Trip"]).status.code(), Some(2));
}

#[test]
fn blending_branches_that_replaced_the_same_fact_lists_it_once_and_a_replacement_settles_it() {
    let server = provider();
    let home = Home::create();
    home.configure(&server, "typesafe", false);
    report(
        home.run(
            [
                &["new", "Trip", "Fact one", "Fact two", "--json"][..],
                &QUESTION,
            ]
            .concat(),
        ),
    );
    for (name, text) in [("Cheap", "Cheap fact"), ("Dear", "Dear fact")] {
        assert!(home.run(["new", name, "--from", "Trip"]).status.success());
        report(home.run([
            "add",
            name,
            "--replace",
            "1",
            text,
            "-q",
            "replaced: Was fact 1 replaced?",
            "--json",
        ]));
    }
    assert!(
        home.run(["new", "Both", "--from", "Cheap", "--from", "Dear"])
            .status
            .success()
    );
    let facts = |format: &str| {
        let output = home.run(["facts", "Both", "--format", format]);
        assert!(output.status.success(), "{output:?}");
        String::from_utf8(output.stdout).unwrap()
    };
    let text = facts("text");
    let lines: Vec<_> = text.lines().collect();
    assert_eq!(lines.len(), 2, "{text}");
    assert!(
        lines[0].contains("\"Cheap fact\" | \"Dear fact\"")
            || lines[0].contains("\"Dear fact\" | \"Cheap fact\""),
        "{text}"
    );
    assert!(lines[0].contains("--replace 1"), "{text}");
    let json: Value = serde_json::from_str(&facts("json")).unwrap();
    let listed = json["facts"].as_array().unwrap();
    assert_eq!(listed.len(), 2);
    assert_eq!(listed[0]["records"].as_array().unwrap().len(), 2);
    assert_eq!(listed[0]["alternatives"].as_array().unwrap().len(), 2);
    assert_eq!(listed[1]["records"].as_array().unwrap().len(), 1);
    // Both alternatives reach the model, and a plain add still works.
    report(home.run([&["add", "Both", "A new fact", "--json"][..], &QUESTION].concat()));
    let state = server.calls.lock().unwrap().last().unwrap().to_string();
    assert!(
        state.contains("Cheap fact") && state.contains("Dear fact"),
        "{state}"
    );
    // A replacement supersedes every alternative.
    report(home.run([
        "add",
        "Both",
        "--replace",
        "1",
        "Settled",
        "-q",
        "settled: Is it settled?",
        "--json",
    ]));
    let json: Value = serde_json::from_str(&facts("json")).unwrap();
    let listed = json["facts"].as_array().unwrap();
    assert_eq!(listed.len(), 3);
    assert_eq!(listed[0]["text"], "Settled");
    assert_eq!(listed[0]["records"].as_array().unwrap().len(), 1);
    assert!(listed[0].get("alternatives").is_none());
}
