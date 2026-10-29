#[path = "support/harness.rs"]
pub mod harness;
#[path = "support/loopback.rs"]
pub mod loopback;
use harness::{Home, QUESTIONS, report};
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
        let mut response: Value = serde_json::from_str(harness::TYPESAFE).unwrap();
        let asked = body["questions"].as_object().unwrap();
        let answers = response["answers"].as_object_mut().unwrap();
        answers.retain(|id, _| asked.contains_key(id));
        for id in asked.keys() {
            // Any other question is a yes/no one.
            answers
                .entry(id.clone())
                .or_insert_with(|| json!({"type": "noul", "noul": 0.5}));
        }
        if body["state"].as_str().unwrap().contains("CANCELLED") {
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
    assert!(output.stderr.is_empty());
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
fn add_reasks_standing_questions_and_drop_replace_and_whatif_change_only_what_they_name() {
    let server = provider();
    let home = Home::create();
    home.configure(&server, "typesafe", false);
    let first = report(
        home.run(
            [
                &["new", "Trip", "The train costs 40 USD", "--json"][..],
                &QUESTIONS,
            ]
            .concat(),
        ),
    );
    assert_eq!(ids(&first.questions), ["travel", "on_time", "quality"]);
    // A turn with only a fact asks the same three questions again.
    let second = report(home.run(["add", "Trip", "A taxi costs 70 USD"]));
    assert_eq!(ids(&second.questions), ["travel", "on_time", "quality"]);
    assert_eq!(
        server.calls.lock().unwrap()[1]["questions"]
            .as_object()
            .unwrap()
            .len(),
        3
    );
    // Dropping a question stops asking it from then on.
    let third = report(home.run(["ask", "Trip", "--drop", "quality"]));
    assert_eq!(ids(&third.questions), ["travel", "on_time"]);
    let ledger = Ledger::open(&home.store(), OpenMode::ReadOnlyExisting).unwrap();
    let trip = SessionSelector::Name("Trip".to_owned().try_into().unwrap());
    assert_eq!(
        ids(&ledger.standing_questions(&trip).unwrap()),
        ["travel", "on_time"]
    );
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
    let fourth = report(home.run(["add", "Trip", "--replace", "2", "A taxi costs 140 USD"]));
    assert_eq!(ids(&fourth.questions), ["travel", "on_time"]);
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
    // A what-if saves nothing, even with an extra question.
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
    assert_eq!(ids(&what.questions), ["travel", "on_time", "extra"]);
    // A what-if can leave a standing question out for that turn only.
    let narrower = report(home.run(["whatif", "Trip", "--drop", "on_time", "--json"]));
    assert_eq!(ids(&narrower.questions), ["travel"]);
    let ledger = Ledger::open(&home.store(), OpenMode::ReadOnlyExisting).unwrap();
    assert_eq!(ledger.standing_questions(&trip).unwrap().len(), 2);
    drop(ledger);
    assert_eq!(server.count(), calls + 2);
    assert!(before == std::fs::read(home.store().join("ledger.sqlite3")).unwrap());
    // Mistakes are input errors that never reach the provider.
    let calls = server.count();
    let unknown = failure(
        home.run(["add", "Trip", "--drop", "missing"]),
        "invalid_standing_question",
    );
    assert!(unknown.message.contains("missing"));
    let grammar = failure(
        home.run(["add", "Trip", "-q", "no colon in this question"]),
        "invalid_decision_input",
    );
    assert!(grammar.message.contains("question 1"));
    failure(
        home.run(["add", "Trip", "-q", "bad: What? [only one]"]),
        "invalid_decision_input",
    );
    failure(
        home.run(["add", "Trip", "--replace", "9", "no such fact"]),
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
    assert_eq!(server.count(), calls);
    assert!(before == std::fs::read(home.store().join("ledger.sqlite3")).unwrap());
}

#[test]
fn new_without_facts_or_questions_creates_a_session_and_branches_inherit_standing_questions() {
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
    assert!(session.sources.is_empty());
    assert_eq!(server.count(), 0);
    let ledger = Ledger::open(&home.store(), OpenMode::ReadOnlyExisting).unwrap();
    let saved = ledger
        .session(&SessionSelector::Id(session.session))
        .unwrap();
    assert!(saved.frontier.is_empty());
    assert!(ledger.list_attempts(saved.id).unwrap().is_empty());
    drop(ledger);
    // The session is usable: its first turn brings the questions.
    let first =
        report(home.run([&["add", "Empty", "A fact"][..], &QUESTIONS, &["--json"]].concat()));
    assert_eq!(first.session, Some(session.session));
    // A name is taken once.
    let again = home.run(["new", "Empty"]);
    assert_eq!(again.status.code(), Some(5));
    // A branch has its source's questions without repeating them.
    let child = report(home.run(["new", "Child", "A later fact", "--from", "Empty", "--json"]));
    assert_eq!(ids(&child.questions), ["travel", "on_time", "quality"]);
    let branch = home.run(["new", "Sibling", "--from", "Empty", "--format", "text"]);
    assert!(branch.status.success());
    let text = String::from_utf8(branch.stdout).unwrap();
    assert!(
        text.contains("Created session Sibling") && text.contains("From: "),
        "{text}"
    );
    // Asking with no questions anywhere says so.
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
            &QUESTIONS,
        ]
        .concat(),
    );
    assert!(first.status.success());
    let text = String::from_utf8(first.stdout).unwrap();
    assert!(
        text.starts_with("  turn 1 · 1 fact · 3 questions\n"),
        "{text}"
    );
    assert!(text.contains("▶ train") && text.contains("0.70"), "{text}");
    assert!(!text.contains("→ was") && !text.contains('\x1b'));
    let second = home.run(["add", "Trip", "The train is CANCELLED", "--format", "text"]);
    let text = String::from_utf8(second.stdout).unwrap();
    assert!(
        text.starts_with("  turn 2 · 1 fact · 3 questions · re-asked 3\n"),
        "{text}"
    );
    assert!(text.contains("▶ flight"), "{text}");
    assert!(text.contains("→ was train 0.70"), "{text}");
    assert!(text.contains("expected 1.43 of 2"), "{text}");
    // A what-if says it was not saved.
    let what = home.run(["whatif", "Trip", "--format", "text"]);
    let text = String::from_utf8(what.stdout).unwrap();
    assert!(
        text.starts_with("  (not saved) · 0 facts · 3 questions · re-asked 3\n"),
        "{text}"
    );
    // A replayed request keeps its record IDs, so the same command replays.
    let args = [
        "add",
        "Trip",
        "Replay me",
        "--request-id",
        "same-command",
        "--json",
    ];
    let saved = report(home.run(args));
    let replay = report(home.run(args));
    assert!(replay.replayed && replay.turn == saved.turn);
}

#[test]
fn facts_and_questions_list_inherited_and_replaced_facts_and_standing_questions() {
    let server = provider();
    let home = Home::create();
    home.configure(&server, "typesafe", false);
    report(
        home.run(
            [
                &["new", "Trip", "Fact one", "Fact two", "--json"][..],
                &QUESTIONS,
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
        "--json",
    ]));
    report(home.run([
        "new",
        "Child",
        "A fact of its own",
        "--from",
        "Trip",
        "--json",
    ]));
    let facts = |session: &str, format: &str| {
        let output = home.run(["facts", session, "--format", format]);
        assert!(output.status.success(), "{output:?}");
        assert!(output.stderr.is_empty());
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
    // Standing questions print in the grammar, one per line.
    let questions = home.run(["questions", "Child", "--format", "text"]);
    assert_eq!(
        String::from_utf8(questions.stdout).unwrap(),
        "travel: Which route? [train, flight]\non_time: Will the train arrive on time?\nquality: Rate the route quality <Poor, Adequate, Excellent>\n"
    );
    let json = home.run(["questions", "Trip", "--json"]);
    let json: Value = serde_json::from_slice(&json.stdout).unwrap();
    assert_eq!(json["questions"][0]["id"], "travel");
    assert_eq!(
        json["questions"][2]["line"],
        "quality: Rate the route quality <Poor, Adequate, Excellent>"
    );
    assert_eq!(
        json["questions"][1]["question"]["answer_space"]["type"],
        "noul"
    );
    let none = home.run(["questions", "Empty", "--format", "text"]);
    assert!(none.status.success() && none.stdout.is_empty());
}
