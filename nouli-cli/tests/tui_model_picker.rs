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
use serde_json::json;
use std::time::Duration;

const READY: &[u8] = b"Tab to add a question";
const CTRL_L: &[u8] = b"\x0c";
const ALTERNATE_SCREEN_OFF: &[u8] = b"\x1b[?1049l";

fn listing_provider() -> Server {
    Server::with_handler(|body| {
        if body["get"] == "/v1/models" {
            let models = json!({"models": [
                {"name": "listed-model"},
                {"name": "listed-tts"},
            ]});
            (200, models.to_string())
        } else {
            (500, "{}".to_owned())
        }
    })
}

fn launch(home: &Home) -> Terminal {
    let mut command = home.command();
    command
        .env("TERM", "xterm-256color")
        .env("COLORTERM", "truecolor")
        .env("NOULI_FIXTURE_KEY", "fixture-key")
        .args(["open", "models"]);
    let mut terminal = Terminal::spawn(command, 120, 32).expect("launch CLI in a PTY");
    terminal.wait_for(READY, 0, "TUI readiness").unwrap();
    terminal
}

#[test]
fn listed_models_can_be_chosen_without_leaving_the_view() {
    let home = Home::create();
    let server = listing_provider();
    home.configure(&server, "system-one", true);
    let mut terminal = launch(&home);
    let start = terminal.mark();
    terminal.send(CTRL_L).unwrap();
    terminal
        .wait_for(b"listed-model", start, "the provider's model")
        .unwrap();
    assert!(
        !terminal
            .output_since(start)
            .windows(ALTERNATE_SCREEN_OFF.len())
            .any(|window| window == ALTERNATE_SCREEN_OFF),
        "the picker stays in the full-screen view"
    );
    assert!(
        !terminal
            .output_since(start)
            .windows(10)
            .any(|window| window == b"listed-tts"),
        "models that cannot answer in text are left out"
    );
    for byte in b"listed" {
        terminal.send(&[*byte]).unwrap();
        terminal.pump(Duration::from_millis(20)).unwrap();
    }
    terminal.send(b"\r").unwrap();
    terminal.pump(Duration::from_millis(400)).unwrap();
    terminal.send(b"\x03\x03").unwrap();
    assert!(terminal.wait_for_exit().unwrap().success());
    let saved = std::fs::read_to_string(home.0.join("config.toml")).unwrap();
    assert!(
        saved.contains("default_model = \"listed-model\""),
        "{saved}"
    );
}
