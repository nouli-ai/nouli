#![cfg(unix)]

#[path = "support/harness.rs"]
#[allow(dead_code, reason = "each test uses a subset of the helpers")]
mod harness;
#[path = "support/loopback.rs"]
#[allow(dead_code, reason = "each test uses a subset of the helpers")]
mod loopback;
#[path = "support/pty.rs"]
#[allow(dead_code, reason = "each test uses a subset of the helpers")]
mod pty;

use harness::Home;
use loopback::Server;
use pty::Terminal;
use serde_json::{Value, json};
use std::time::{Duration, Instant};

const READY: &[u8] = b"Tab to add a question";

const SHIFT_TAB: &[u8] = b"\x1b[Z";
const STEP: Duration = Duration::from_millis(250);
const ANSWERED: Duration = Duration::from_millis(400);
const PROVIDER_WAIT: Duration = Duration::from_secs(5);

/// Text the terminal received, without its escape sequences.
fn plain(output: &[u8]) -> String {
    let text = String::from_utf8_lossy(output);
    let mut shown = String::new();
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            for next in chars.by_ref() {
                if next.is_ascii_alphabetic() || next == '~' {
                    break;
                }
            }
        } else {
            shown.push(c);
        }
    }
    shown
}

fn launch(home: &Home, session: &str) -> Terminal {
    let mut command = home.command();
    command
        .env("TERM", "xterm-256color")
        .env("COLORTERM", "truecolor")
        .current_dir(&home.0)
        .args(["open", session]);
    let mut terminal = Terminal::spawn(command, 160, 40).expect("launch CLI in a PTY");
    terminal.wait_for(READY, 0, "TUI readiness").unwrap();
    terminal
}

/// Types into the focused field, one chunk the way a person would.
fn typed(terminal: &mut Terminal, text: &str) {
    for byte in text.bytes() {
        terminal.send(&[byte]).unwrap();
        terminal.pump(Duration::from_millis(15)).unwrap();
    }
}

/// A provider that is certain: the first option gets 1.0 and the rest 0.0.
fn answer(body: &Value) -> (u16, String) {
    let mut answers = serde_json::Map::new();
    for (id, question) in body["questions"].as_object().unwrap() {
        let reply = if question["type"] == "choice" {
            let options: Vec<_> = question["criteria"].as_object().unwrap().keys().collect();
            let probabilities: serde_json::Map<String, Value> = options
                .iter()
                .enumerate()
                .map(|(index, option)| {
                    let share = if index == 0 { 1.0 } else { 0.0 };
                    ((*option).clone(), json!(share))
                })
                .collect();
            json!({"type":"choice","choice":options[0],"probabilities":probabilities,
                   "confidence":0.5,"answer_confidence":0.9})
        } else {
            json!({"type":"noul","noul":1.0})
        };
        answers.insert(id.clone(), reply);
    }
    (200, json!({"model":"m","answers":answers}).to_string())
}

fn launch_with_notes(home: &Home, session: &str) -> Terminal {
    std::fs::write(home.0.join("notes.txt"), "the-note-contents").unwrap();
    launch(home, session)
}

/// Asks in a fresh form, with `answers` for a choice.
fn ask(terminal: &mut Terminal, server: &Server, question: &str, answers: &[&str]) {
    if !answers.is_empty() {
        terminal.send(SHIFT_TAB).unwrap();
        terminal.pump(STEP).unwrap();
    }
    terminal.send(b"\t").unwrap();
    typed(terminal, question);
    for (index, answer) in answers.iter().enumerate() {
        terminal
            .send(if index == 0 { b"\t" } else { b"\r" })
            .unwrap();
        typed(terminal, answer);
    }
    if !answers.is_empty() {
        terminal.send(b"\r").unwrap();
    }
    terminal.pump(STEP).unwrap();
    let sent = server.count();
    terminal.send(b"\x13").unwrap(); // Ctrl-S submits from any section.
    settle(terminal, server, sent);
}

/// Waits for the provider to get one more call and the view to show the answer.
fn settle(terminal: &mut Terminal, server: &Server, sent: usize) {
    let deadline = Instant::now() + PROVIDER_WAIT;
    while server.count() == sent && Instant::now() < deadline {
        terminal.pump(STEP).unwrap();
    }
    terminal.pump(ANSWERED).unwrap();
}

/// Queues the first result for `query` as a Context row, the usual place for
/// a reference.
fn refer_in_context(terminal: &mut Terminal, query: &str) {
    typed(terminal, "See ");
    typed(terminal, query);
    terminal.pump(STEP).unwrap();
    terminal.send(b"\r").unwrap(); // Takes the result.
    terminal.pump(STEP).unwrap();
    terminal.send(b"\r").unwrap(); // Queues the row.
    terminal.pump(STEP).unwrap();
}

/// Sends a follow-up that changes the question and reports what the view drew.
fn follow_up(terminal: &mut Terminal, server: &Server, question: &str) -> String {
    terminal.send(b"\t").unwrap();
    terminal.pump(STEP).unwrap();
    typed(terminal, question);
    terminal.pump(STEP).unwrap();
    let start = terminal.mark();
    let sent = server.count();
    terminal.send(b"\r").unwrap();
    settle(terminal, server, sent);
    plain(terminal.output_since(start))
}

fn calls(server: &Server) -> Vec<Value> {
    server.calls.lock().unwrap().clone()
}

fn assert_reached_provider(shown: &str, server: &Server, expected: usize, evidence: &str) {
    assert!(
        !shown.to_lowercase().contains("conflict"),
        "the request must not conflict: {shown}"
    );
    let calls = calls(server);
    assert_eq!(calls.len(), expected, "{shown}");
    let last = calls.last().unwrap().to_string();
    assert!(last.contains(evidence), "{evidence} missing: {last}");
}

fn provider_and_home() -> (Home, Server) {
    let home = Home::create();
    let server = Server::with_handler(answer);
    home.configure(&server, "system-one", false);
    (home, server)
}

fn quit(terminal: &mut Terminal) {
    terminal.send(b"\x03\x03\x03").unwrap();
    assert!(terminal.wait_for_exit().unwrap().success());
}

#[test]
fn context_can_reference_a_choice_answered_with_certainty() {
    // The answer's 1.0 and 0.0 come back from storage as integers.
    let (home, server) = provider_and_home();
    let mut terminal = launch(&home, "refs");
    ask(
        &mut terminal,
        &server,
        "Where should we go?",
        &["North", "South"],
    );
    refer_in_context(&mut terminal, "@where");
    let shown = follow_up(&mut terminal, &server, "Is that wise?");
    assert_reached_provider(&shown, &server, 2, "[decision:");
    quit(&mut terminal);
}

#[test]
fn context_can_reference_a_yes_or_no_answered_with_certainty() {
    let (home, server) = provider_and_home();
    let mut terminal = launch(&home, "refs");
    ask(&mut terminal, &server, "Will it rain?", &[]);
    refer_in_context(&mut terminal, "@rain");
    let shown = follow_up(&mut terminal, &server, "Is that wise?");
    assert_reached_provider(&shown, &server, 2, "[decision:");
    quit(&mut terminal);
}

#[test]
fn context_can_reference_an_earlier_answer_after_later_ones() {
    let (home, server) = provider_and_home();
    let mut terminal = launch(&home, "refs");
    ask(
        &mut terminal,
        &server,
        "Where should we go?",
        &["North", "South"],
    );
    ask(&mut terminal, &server, "Which season suits?", &[]);
    terminal.pump(STEP).unwrap();
    refer_in_context(&mut terminal, "@where");
    let shown = follow_up(&mut terminal, &server, "Is that wise?");
    assert_reached_provider(&shown, &server, 3, "[decision:");
    quit(&mut terminal);
}

#[test]
fn context_can_reference_a_certain_answer_from_another_session() {
    let (home, server) = provider_and_home();
    let mut terminal = launch(&home, "first");
    ask(
        &mut terminal,
        &server,
        "Where should we go?",
        &["North", "South"],
    );
    quit(&mut terminal);
    let mut terminal = launch(&home, "second");
    refer_in_context(&mut terminal, "@where");
    let shown = follow_up(&mut terminal, &server, "Is that wise?");
    assert_reached_provider(&shown, &server, 2, "[decision:");
    quit(&mut terminal);
}

#[test]
fn context_can_reference_a_file_after_a_certain_answer() {
    let (home, server) = provider_and_home();
    let mut terminal = launch_with_notes(&home, "refs");
    ask(&mut terminal, &server, "Will it rain?", &[]);
    refer_in_context(&mut terminal, "@notes");
    let shown = follow_up(&mut terminal, &server, "Is that wise?");
    assert_reached_provider(&shown, &server, 2, "the-note-contents");
    quit(&mut terminal);
}
