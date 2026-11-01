#[path = "support/harness.rs"]
#[allow(dead_code, reason = "each test uses a subset of the helpers")]
mod harness;
#[path = "support/loopback.rs"]
#[allow(dead_code, reason = "each test uses a subset of the helpers")]
mod loopback;
use harness::Home;
use loopback::Server;
use nouli_core::{
    identity::SessionSelector,
    storage::{Ledger, OpenMode},
};
use serde_json::{Value, json};
use std::process::Output;

/// A provider that answers whatever it is asked: the first choice leads, unless
/// the evidence says CANCELLED, when the last one does.
fn provider() -> Server {
    Server::with_handler(answer)
}

#[expect(
    clippy::cast_precision_loss,
    reason = "fixture questions have a few options"
)]
fn answer(body: &Value) -> (u16, String) {
    {
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
    }
}

fn open(home: &Home, arguments: &[&str], script: &str) -> Output {
    home.run_with_stdin(arguments, script.as_bytes())
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

/// The question IDs each provider call asked, in order.
fn asked(server: &Server) -> Vec<Vec<String>> {
    server
        .calls
        .lock()
        .unwrap()
        .iter()
        .map(|call| {
            call["questions"]
                .as_object()
                .unwrap()
                .keys()
                .cloned()
                .collect()
        })
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
fn each_question_is_one_call_that_asks_only_that_question() {
    let server = provider();
    let home = Home::create();
    home.configure(&server, "typesafe", false);
    let script = "The train costs 40 USD.\nA taxi costs 70 USD.\n\
        ?transport: How should I get there? [train, taxi]\n?on_time: Will the train arrive on time?\n\
        ?broken question\nThe train is CANCELLED\n\n\
        ?transport: How should I get there now? [train, taxi]\n/quit\n";
    let transcript = text(&open(&home, &["open", "travel"], script));
    let expected = [
        "new session \"travel\" · /help for commands",
        "travel › ?transport: How should I get there? [train, taxi]",
        "  turn 1 · 2 facts · 1 question",
        "  turn 2 · 0 facts · 1 question",
        "  grammar error: a question starts with an ID and a colon",
        "  1 queued; ask a question with ?id: prompt to send them",
        "  turn 3 · 1 fact · 1 question",
        // Asked again by name, the new answer is shown against the earlier one.
        "→ was taxi 0.90",
        "  bye",
    ];
    for line in expected {
        assert!(transcript.contains(line), "{line}\n{transcript}");
    }
    assert!(!transcript.contains("re-asked"), "{transcript}");
    assert!(!transcript.contains('\x1b'));
    // Each call asked only the question typed for it.
    assert_eq!(asked(&server), [["transport"], ["on_time"], ["transport"]]);
    let ledger = ledger(&home);
    assert_eq!(turns(&ledger, "travel"), 3);
    let facts = ledger.facts(&session("travel")).unwrap();
    assert_eq!(facts.len(), 3);
    assert_eq!(facts[2].text, "The train is CANCELLED");
    // An empty line with nothing queued does nothing, and nothing re-asks.
    let quiet = text(&open(
        &home,
        &["open", "Quiet"],
        "\n\n/ask\n/questions\n/quit\n",
    ));
    assert!(quiet.contains("unknown command /ask"), "{quiet}");
    assert!(quiet.contains("unknown command /questions"), "{quiet}");
    assert_eq!(server.count(), 3);
}

#[test]
fn a_failed_turn_keeps_its_question_and_an_empty_line_sends_it_again() {
    // The second call fails; every other call is answered.
    let calls = std::sync::atomic::AtomicUsize::new(0);
    let server = Server::with_handler(move |body| {
        if calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 1 {
            (400, json!({"error": "rejected"}).to_string())
        } else {
            answer(body)
        }
    });
    let home = Home::create();
    home.configure(&server, "typesafe", false);
    // An unknown command neither sends nor replaces the waiting question.
    let script =
        "?first: Is it first?\nA fact\n?q: Is it?\n/whatif ?other: Is it other?\n\n/quit\n";
    let transcript = text(&open(&home, &["open", "travel"], script));
    assert!(transcript.contains("  error: "), "{transcript}");
    assert!(
        transcript.contains("unknown command /whatif"),
        "{transcript}"
    );
    assert!(
        transcript.contains("  turn 2 · 1 fact · 1 question"),
        "{transcript}"
    );
    assert_eq!(asked(&server), [["first"], ["q"], ["q"]]);
    assert_eq!(turns(&ledger(&home), "travel"), 2);
}

#[test]
fn removed_whatif_command_keeps_the_queue_and_sends_nothing() {
    let server = provider();
    let home = Home::create();
    home.configure(&server, "typesafe", false);
    let script = "The train costs 40 USD.\n?transport: How? [train, taxi]\nThe train is CANCELLED\n\
        /whatif ?late: Will I be late?\n/whatif The fare doubled.\nA queued fact\n/facts\n/quit\n";
    let transcript = text(&open(&home, &["open", "travel"], script));
    for line in [
        "unknown command /whatif",
        "   +  The train is CANCELLED   (queued)",
        "   +  A queued fact   (queued)",
    ] {
        assert!(transcript.contains(line), "{line}\n{transcript}");
    }
    let ledger = ledger(&home);
    assert_eq!(turns(&ledger, "travel"), 1);
    assert_eq!(ledger.facts(&session("travel")).unwrap().len(), 1);
    let id = ledger.session(&session("travel")).unwrap().id;
    assert_eq!(ledger.list_attempts(id).unwrap().len(), 1);
    assert_eq!(asked(&server), [["transport"]]);
    // Before any turn there is nothing to start from.
    let early = text(&open(
        &home,
        &["open", "Fresh"],
        "/whatif ?q: Is it?\n/branch Nope\n/quit\n",
    ));
    assert!(early.contains("unknown command /whatif"));
    assert!(early.contains("send a turn first: a branch starts from the last one"));
}

#[test]
fn replace_and_clear_change_the_next_turn_only() {
    let server = provider();
    let home = Home::create();
    home.configure(&server, "typesafe", false);
    let script = "The taxi costs 70 USD.\n?transport: How? [train, taxi]\n\
        /replace 1 The taxi costs 140 USD.\n?fare: Is the taxi affordable?\n\
        A stray fact\n/clear\n/facts\n/drop fare\n/quit\n";
    let transcript = text(&open(&home, &["open", "travel"], script));
    for line in [
        "  queued: replace fact 1",
        "  turn 2 · 1 fact (1 replaced, kept in history) · 1 question",
        "  queue cleared",
        "   1  The taxi costs 140 USD.   (replaced; was: The taxi costs 70 USD.)",
        "unknown command /drop",
    ] {
        assert!(transcript.contains(line), "{line}\n{transcript}");
    }
    let ledger = ledger(&home);
    let facts = ledger.facts(&session("travel")).unwrap();
    assert_eq!(facts.len(), 1);
    assert_eq!(facts[0].previous.as_deref(), Some("The taxi costs 70 USD."));
    // The model saw the replacement and not the original, with only the new question.
    assert_eq!(asked(&server), [["transport"], ["fare"]]);
    let state = server.calls.lock().unwrap()[1]["state"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(state.contains("140 USD") && !state.contains("70 USD"));
    assert!(!state.contains("A stray fact"));
    assert_eq!(turns(&ledger, "travel"), 2);
}

#[test]
fn branch_blend_use_new_and_view_create_and_show_the_expected_sessions() {
    let server = provider();
    let home = Home::create();
    home.configure(&server, "typesafe", false);
    let script = "The train costs 40 USD.\n?transport: How? [train, taxi]\n\
        /branch Alt\nThe taxi is cheaper.\n?cheaper: Is the taxi cheaper?\n\
        /use travel\n/new Budget\nThe budget is 50 USD.\n?approval: Is approval needed?\n\
        /blend travel Budget as Plan\n/view\n/use nowhere\n/new bad name\n/quit\n";
    let transcript = text(&open(&home, &["open", "travel"], script));
    for line in [
        "  new session \"Alt\" from travel @ turn 1\n",
        "  travel\n",
        "  new session \"Budget\"",
        "  new session \"Plan\" from travel + Budget\n",
        "  ○ travel  1 turn",
        "  ○ Alt  1 turn  ← travel",
        "  ○ Budget  1 turn",
        "  ● Plan  0 turns  ← travel + Budget",
        "error: no session \"nowhere\"",
        "usage: /new [NAME]",
    ] {
        assert!(transcript.contains(line), "{line}\n{transcript}");
    }
    // A branch asks only its own question, not its source's.
    assert_eq!(asked(&server), [["transport"], ["cheaper"], ["approval"]]);
    let ledger = ledger(&home);
    let alt = ledger.session(&session("Alt")).unwrap();
    assert_eq!(ledger.session_sources(alt.id).unwrap().len(), 1);
    let plan = ledger.session(&session("Plan")).unwrap();
    assert_eq!(ledger.session_sources(plan.id).unwrap().len(), 2);
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
        "/context\nA fact\n?q: Is it?\n/context\n/help\n/nonsense\n/replace x\nUnsent fact\n",
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
