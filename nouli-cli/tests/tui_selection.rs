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
use pty::Terminal;
use std::time::Duration;

const COLUMNS: u16 = 160;
const MOUSE_ON: &[u8] = b"\x1b[?1006h";
const MOUSE_OFF: &[u8] = b"\x1b[?1006l";
const F6: &[u8] = b"\x1b[17~";

fn contains(output: &[u8], bytes: &[u8]) -> bool {
    output.windows(bytes.len()).any(|window| window == bytes)
}

const DRAFT: &[u8] = b"selection-draft-proof";

fn check_selection(entry: &[u8]) {
    let home = Home::create();
    let mut terminal = launch(&home);
    paste_draft(&mut terminal);
    let held = enter_selection(&mut terminal, entry);
    assert_selection_holds_the_view(&mut terminal, held);
    leave_selection_for_native_dragging(&mut terminal, held);
    assert_resize_keeps_draft_and_native_selection(&mut terminal);
    assert_exit_restores_terminal(&mut terminal);
}

/// Opens the TUI in a PTY and checks the terminal owns dragging at startup.
fn launch(home: &Home) -> Terminal {
    std::fs::write(
        home.0.join("config.toml"),
        "default_provider = \"local\"\ndefault_model = \"offline\"\n\
         [providers.local]\nprotocol = \"system-one\"\n\
         base_url = \"http://127.0.0.1:9\"\n",
    )
    .unwrap();
    let mut command = home.command();
    command
        .env("TERM", "xterm-256color")
        .env("COLORTERM", "truecolor")
        .args(["open", "copy-check"]);
    let mut terminal = Terminal::spawn(command, COLUMNS, 32).expect("launch CLI in a PTY");
    terminal.wait_for(b"Ready", 0, "TUI readiness").unwrap();
    terminal
        .wait_for(MOUSE_OFF, 0, "native selection at startup")
        .unwrap();
    assert!(
        !contains(terminal.output_since(0), MOUSE_ON),
        "ordinary dragging must belong to the terminal at startup"
    );
    terminal
}

fn paste_draft(terminal: &mut Terminal) {
    terminal
        .send(b"\x1b[200~selection-draft-proof\x1b[201~")
        .unwrap();
    terminal.wait_for(DRAFT, 0, "pasted draft").unwrap();
    assert!(
        !contains(terminal.output_since(0), MOUSE_ON),
        "editing must preserve native dragging"
    );
}

/// Focuses the graph, then enters selection with `entry`; returns the output
/// mark once the held view is drawn.
fn enter_selection(terminal: &mut Terminal, entry: &[u8]) -> usize {
    let start = terminal.mark();
    terminal.send(b"\x07").unwrap(); // Ctrl-G enables graph mouse controls.
    terminal
        .wait_for(MOUSE_ON, start, "graph mouse reporting")
        .unwrap();
    let start = terminal.mark();
    terminal.send(entry).unwrap();
    terminal
        .wait_for(MOUSE_OFF, start, "selection mouse release")
        .unwrap();
    terminal
        .wait_for(b"Esc/F6 resume", start, "selection instructions")
        .unwrap();
    terminal.pump(Duration::from_millis(100)).unwrap();
    terminal.mark()
}

fn assert_selection_holds_the_view(terminal: &mut Terminal, held: usize) {
    terminal
        .send(b"z\t\r\x1b[200~do-not-paste\x1b[201~")
        .unwrap();
    terminal.pump(Duration::from_millis(250)).unwrap();
    assert_eq!(terminal.mark(), held, "selection must hold the view");
}

fn leave_selection_for_native_dragging(terminal: &mut Terminal, held: usize) {
    terminal.send(b"\x1b").unwrap();
    terminal
        .wait_for(MOUSE_ON, held, "mouse reporting after Escape")
        .unwrap();
    let start = terminal.mark();
    terminal.send(b"\x1b").unwrap(); // Leave graph focus for native selection.
    terminal
        .wait_for(
            MOUSE_OFF,
            start,
            "native selection after leaving graph focus",
        )
        .unwrap();
}

fn assert_resize_keeps_draft_and_native_selection(terminal: &mut Terminal) {
    let start = terminal.mark();
    terminal.send(F6).unwrap();
    terminal
        .wait_for(b"Esc/F6 resume", start, "held view before resize")
        .unwrap();
    let start = terminal.mark();
    terminal.resize(100, 24).unwrap();
    terminal
        .wait_for(DRAFT, start, "preserved draft after resize")
        .unwrap();
    assert!(
        !contains(terminal.output_since(start), MOUSE_ON),
        "resize must keep native selection outside graph focus"
    );
    assert!(
        !contains(terminal.output_since(start), b"do-not-paste"),
        "selection must ignore paste and preserve the original draft"
    );
}

fn assert_exit_restores_terminal(terminal: &mut Terminal) {
    terminal.send(b"\x03\x03").unwrap();
    assert!(
        terminal.wait_for_exit().unwrap().success(),
        "CLI must exit cleanly"
    );
    let output = terminal.output_since(0);
    let last = |bytes: &[u8]| {
        output
            .windows(bytes.len())
            .rposition(|window| window == bytes)
    };
    assert!(
        last(MOUSE_OFF) > last(MOUSE_ON),
        "exit must release mouse reporting"
    );
    assert!(
        contains(output, b"\x1b[?1049l"),
        "exit must restore the normal screen"
    );
    assert!(
        contains(output, b"\x1b[?2004l"),
        "exit must disable bracketed paste"
    );
    assert!(
        !contains(output, b"\x1b]52;"),
        "clipboard writes belong to the terminal"
    );
}

#[test]
fn f6_selection_preserves_draft_and_restores_terminal() {
    check_selection(F6);
}

#[test]
fn header_selection_preserves_draft_and_restores_terminal() {
    // The selection control occupies the right edge of the first row. Click
    // inside it relative to the PTY width (SGR mouse coordinates are one-based).
    check_selection(format!("\x1b[<0;{};1M", COLUMNS - 7).as_bytes());
}
