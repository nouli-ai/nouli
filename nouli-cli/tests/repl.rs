#[path = "support/harness.rs"]
pub mod harness;
#[path = "support/loopback.rs"]
pub mod loopback;
use harness::Home;
use loopback::Server;
use nouli_core::{
    decision::Question,
    identity::SessionSelector,
    storage::{Ledger, OpenMode},
};
use serde_json::{Value, json};
use std::{
    io::Write,
    process::{Output, Stdio},
};

/// A provider that answers whatever it is asked: the first choice leads, unless
/// the evidence says CANCELLED, when the last one does.
fn provider() -> Server {
    Server::with_handler(|body| {
        let cancelled = body["state"].as_str().unwrap().contains("CANCELLED");
        let mut answers = serde_json::Map::new();
        for (id, question) in body["questions"].as_object().unwrap() {
            let answer = match question["type"].as_str().unwrap() {
                "choice" => {
                    let options: Vec<_> =
                        question["criteria"].as_object().unwrap().keys().collect();
                    let pick = if cancelled { options.len() - 1 } else { 0 };
                    let probabilities: serde_json::Map<String, Value> = options
                        .iter()
                        .enumerate()
                        .map(|(index, option)| {
                            let leading = index == pick;
                            let rest = 0.1 / (options.len() - 1) as f64;
                            ((*option).clone(), json!(if leading { 0.9 } else { rest }))
                        })
                        .collect();
                    json!({"type":"choice","choice":options[pick],"probabilities":probabilities,
                           "confidence":0.5,"answer_confidence":0.9})
                }
                "noul" => json!({"type":"noul","noul":if cancelled {0.13} else {0.98}}),
                _ => {
                    let levels = question["criteria"].as_array().unwrap().len();
                    let probabilities: serde_json::Map<String, Value> = (0..levels)
                        .map(|level| (level.to_string(), json!(1.0 / levels as f64)))
                        .collect();
                    json!({"type":"score","score":(levels - 1) as f64 / 2.0,"probabilities":probabilities,
                           "confidence":0.5})
                }
            };
            answers.insert(id.clone(), answer);
        }
        (
            200,
            json!({"model":"repl-fixture","answers":answers,"usage":{"input_tokens":10,"output_tokens":2}})
                .to_string(),
        )
    })
}

fn open(home: &Home, arguments: &[&str], script: &str) -> Output {
    let mut child = home
        .command()
        .args(arguments)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(script.as_bytes())
        .unwrap();
    child.wait_with_output().unwrap()
}

fn text(output: &Output) -> String {
    assert!(output.status.success(), "{output:?}");
    assert!(
        output.stderr.is_empty(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout.clone()).unwrap()
}

fn session(name: &str) -> SessionSelector {
    SessionSelector::Name(name.to_owned().try_into().unwrap())
}

fn ledger(home: &Home) -> Ledger {
    Ledger::open(&home.store(), OpenMode::ReadOnlyExisting).unwrap()
}

fn ids(questions: &[Question]) -> Vec<&str> {
    questions
        .iter()
        .map(|question| question.id.as_str())
        .collect()
}

fn turns(ledger: &Ledger, name: &str) -> usize {
    let session = ledger.session(&session(name)).unwrap();
    ledger
        .ancestry(&[nouli_core::history::SourcePoint {
            session: session.id,
            frontier: session.frontier.clone(),
        }])
        .unwrap()
        .iter()
        .filter(|turn| turn.session == session.id)
        .count()
}

#[test]
fn queued_facts_and_a_standing_question_are_sent_by_an_empty_line_and_asked_again() {
    let server = provider();
    let home = Home::create();
    home.configure(&server, "typesafe", false);
    let script = "The train costs 40 USD.\nA taxi costs 70 USD.\n\
        ?transport: How should I get there? [train, taxi]\n?on_time: Will the train arrive on time?\n\n\
        ?broken question\nThe train is CANCELLED\n\n/quit\n";
    let transcript = text(&open(&home, &["open", "travel"], script));
    let expected = [
        "new session \"travel\" · /help for commands",
        "travel › ?transport: How should I get there? [train, taxi]",
        "  queued: choice question \"transport\" · options train, taxi",
        "  queued: yes/no question \"on_time\"",
        "  turn 1 · 2 facts · 2 questions",
        "  grammar error: a question starts with an ID and a colon",
        "  turn 2 · 1 fact · 2 questions · re-asked 2",
        "→ was taxi 0.90",
        "→ was 0.98",
        "  bye",
    ];
    for line in expected {
        assert!(transcript.contains(line), "{line}\n{transcript}");
    }
    assert!(!transcript.contains('\x1b'));
    // The grammar error queued nothing, and the ledger holds what was shown.
    let ledger = ledger(&home);
    assert_eq!(turns(&ledger, "travel"), 2);
    assert_eq!(
        ids(&ledger.standing_questions(&session("travel")).unwrap()),
        ["transport", "on_time"]
    );
    let facts = ledger.facts(&session("travel")).unwrap();
    assert_eq!(facts.len(), 3);
    assert_eq!(facts[2].text, "The train is CANCELLED");
    assert_eq!(server.count(), 2);
    // An empty line with nothing queued and nothing standing does nothing.
    let quiet = text(&open(&home, &["open", "Quiet"], "\n\n/ask\n/quit\n"));
    assert!(quiet.contains("nothing to ask: define a question with ?id: prompt"));
    assert_eq!(server.count(), 2);
}

#[test]
fn whatif_saves_nothing_and_keeps_what_was_queued() {
    let server = provider();
    let home = Home::create();
    home.configure(&server, "typesafe", false);
    let script = "The train costs 40 USD.\n?transport: How? [train, taxi]\n\n\
        /whatif The train is CANCELLED\nA queued fact\n/whatif Another what-if\n/facts\n/quit\n";
    let transcript = text(&open(&home, &["open", "travel"], script));
    assert!(
        transcript.contains("  (not saved) · 1 fact · 1 question"),
        "{transcript}"
    );
    assert!(transcript.contains("(queued)"), "{transcript}");
    assert!(
        transcript.contains("   +  A queued fact   (queued)"),
        "{transcript}"
    );
    let ledger = ledger(&home);
    assert_eq!(turns(&ledger, "travel"), 1);
    assert_eq!(ledger.facts(&session("travel")).unwrap().len(), 1);
    let id = ledger.session(&session("travel")).unwrap().id;
    assert_eq!(ledger.list_attempts(id).unwrap().len(), 1);
    // The what-if reached the provider but not the ledger.
    assert_eq!(server.count(), 3);
    // Before any turn there is nothing to start from.
    let early = text(&open(
        &home,
        &["open", "Fresh"],
        "/whatif A fact\n/branch Nope\n/quit\n",
    ));
    assert!(early.contains("send a turn first: a what-if starts from the last one"));
    assert!(early.contains("send a turn first: a branch starts from the last one"));
}

#[test]
fn replace_drop_and_clear_change_the_next_turn_only() {
    let server = provider();
    let home = Home::create();
    home.configure(&server, "typesafe", false);
    let script = "The taxi costs 70 USD.\n?transport: How? [train, taxi]\n?on_time: On time?\n\n\
        /replace 1 The taxi costs 140 USD.\n/drop on_time\n/questions\n\n\
        A stray fact\n/drop missing\n/clear\n/facts\n/quit\n";
    let transcript = text(&open(&home, &["open", "travel"], script));
    for line in [
        "  queued: replace fact 1",
        "  queued: drop on_time",
        "  turn 2 · 1 fact (1 replaced, kept in history) · 1 question · re-asked 1",
        "  queue cleared",
        "   1  The taxi costs 140 USD.   (replaced; was: The taxi costs 70 USD.)",
    ] {
        assert!(transcript.contains(line), "{line}\n{transcript}");
    }
    // The queued drop is shown in /questions only after it is sent.
    let ledger = ledger(&home);
    assert_eq!(
        ids(&ledger.standing_questions(&session("travel")).unwrap()),
        ["transport"]
    );
    let facts = ledger.facts(&session("travel")).unwrap();
    assert_eq!(facts.len(), 1);
    assert_eq!(facts[0].previous.as_deref(), Some("The taxi costs 70 USD."));
    // The model saw the replacement and not the original.
    let state = server.calls.lock().unwrap()[1]["state"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(state.contains("140 USD") && !state.contains("70 USD"));
    assert!(!state.contains("A stray fact"));
    assert_eq!(turns(&ledger, "travel"), 2);
    // A drop of an unknown question is an error that keeps the REPL running.
    let failed = text(&open(
        &home,
        &["open", "travel"],
        "/drop missing\n\n/quit\n",
    ));
    assert!(
        failed.contains("error: invalid_standing_question"),
        "{failed}"
    );
    assert!(failed.contains("  bye"));
}

#[test]
fn branch_blend_use_new_and_view_create_and_show_the_expected_sessions() {
    let server = provider();
    let home = Home::create();
    home.configure(&server, "typesafe", false);
    let script = "The train costs 40 USD.\n?transport: How? [train, taxi]\n\n\
        /branch Alt\nThe taxi is cheaper.\n\n\
        /use travel\n/new Budget\nThe budget is 50 USD.\n?approval: Is approval needed?\n\n\
        /blend travel Budget as Plan\n/questions\n/view\n/use nowhere\n/new bad name\n/quit\n";
    let transcript = text(&open(&home, &["open", "travel"], script));
    for line in [
        "  new session \"Alt\" from travel @ turn 1 · 1 standing question",
        "  travel · 1 standing question",
        "  new session \"Budget\"",
        "  new session \"Plan\" from travel + Budget · no standing questions",
        "  no standing questions",
        "  ○ travel  1 turn",
        "  ○ Alt  1 turn  ← travel",
        "  ○ Budget  1 turn",
        "  ● Plan  0 turns  ← travel + Budget",
        "error: no session \"nowhere\"",
        "usage: /new NAME",
    ] {
        assert!(transcript.contains(line), "{line}\n{transcript}");
    }
    let ledger = ledger(&home);
    let alt = ledger.session(&session("Alt")).unwrap();
    assert_eq!(ledger.session_sources(alt.id).unwrap().len(), 1);
    assert_eq!(
        ids(&ledger.standing_questions(&session("Alt")).unwrap()),
        ["transport"]
    );
    let plan = ledger.session(&session("Plan")).unwrap();
    assert_eq!(ledger.session_sources(plan.id).unwrap().len(), 2);
    assert!(
        ledger
            .standing_questions(&session("Plan"))
            .unwrap()
            .is_empty()
    );
    assert_eq!(ledger.list_sessions().unwrap().len(), 4);
    // The blend lists both sources' facts, and not the branch's.
    assert_eq!(ledger.facts(&session("Plan")).unwrap().len(), 2);
}

#[test]
fn help_context_quit_and_end_of_input_with_a_queue_behave_as_documented() {
    let server = provider();
    let home = Home::create();
    home.configure(&server, "typesafe", false);
    let transcript = text(&open(
        &home,
        &["open", "travel"],
        "/context\nA fact\n?q: Is it?\n\n/context\n/help\n/nonsense\n/replace x\nUnsent fact\n",
    ));
    for line in [
        "nothing sent yet: the model has seen no turn",
        "  travel · last turn ",
        "fact      A fact",
        "answer    q",
        "/blend A B as NAME",
        "unknown command /nonsense · /help lists them",
        "usage: /replace N TEXT, where N is a number from /facts",
        "1 queued; press Ctrl-D again to quit and discard them",
        "  bye",
    ] {
        assert!(transcript.contains(line), "{line}\n{transcript}");
    }
    // Nothing was sent after the second turn, so only one request was made.
    assert_eq!(server.count(), 1);
    // A bare invocation without a terminal still prints help.
    let help = home.run(Vec::<&str>::new());
    assert!(help.status.success());
    assert!(String::from_utf8_lossy(&help.stdout).contains("Usage: nouli"));
}
