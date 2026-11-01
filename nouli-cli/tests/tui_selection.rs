#![cfg(unix)]

#[path = "support/harness.rs"]
pub mod harness;
#[path = "support/loopback.rs"]
pub mod loopback;
#[path = "support/pty.rs"]
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

fn check_selection(entry: &[u8]) {
    let home = Home::create();
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

    let draft = b"selection-draft-proof";
    terminal
        .send(b"\x1b[200~selection-draft-proof\x1b[201~")
        .unwrap();
    terminal.wait_for(draft, 0, "pasted draft").unwrap();
    assert!(
        !contains(terminal.output_since(0), MOUSE_ON),
        "editing must preserve native dragging"
    );
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
    let held = terminal.mark();
    terminal
        .send(b"z\t\r\x1b[200~do-not-paste\x1b[201~")
        .unwrap();
    terminal.pump(Duration::from_millis(250)).unwrap();
    assert_eq!(terminal.mark(), held, "selection must hold the view");

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
    let start = terminal.mark();
    terminal.send(F6).unwrap();
    terminal
        .wait_for(b"Esc/F6 resume", start, "held view before resize")
        .unwrap();
    let start = terminal.mark();
    terminal.resize(100, 24).unwrap();
    terminal
        .wait_for(draft, start, "preserved draft after resize")
        .unwrap();
    assert!(
        !contains(terminal.output_since(start), MOUSE_ON),
        "resize must keep native selection outside graph focus"
    );
    assert!(
        !contains(terminal.output_since(start), b"do-not-paste"),
        "selection must ignore paste and preserve the original draft"
    );

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
