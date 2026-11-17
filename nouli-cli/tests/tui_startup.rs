#[path = "support/harness.rs"]
#[allow(dead_code, reason = "startup tests use a subset of fixture helpers")]
mod harness;
#[path = "support/loopback.rs"]
#[allow(dead_code, reason = "startup tests use a subset of fixture helpers")]
mod loopback;
#[path = "support/pty.rs"]
#[allow(dead_code, reason = "startup tests use a subset of fixture helpers")]
mod pty;

use harness::Home;
use pty::{Terminal, fixture as child};
use std::{fs, process::Command};

const READY: &[u8] = b"Tab to add a question";
const DRAFT: &[u8] = b"draft-before-setup";

fn launch(mut command: Command) -> Terminal {
    command.env("TERM", "xterm-256color");
    let mut terminal = Terminal::spawn(command, 120, 32).unwrap();
    terminal.wait_for(READY, 0, "the interactive form").unwrap();
    terminal
        .wait_for(b"\x1b[?1049h", 0, "the alternate screen")
        .unwrap();
    let output = String::from_utf8_lossy(terminal.output_since(0));
    assert!(!output.contains("Connect a decision service."), "{output}");
    assert!(!output.contains("Authenticate connection"), "{output}");
    assert!(!output.contains("API key:"), "{output}");
    terminal
}

fn close(mut terminal: Terminal) {
    terminal.send(b"\x03\x03\x03").unwrap();
    assert!(terminal.wait_for_exit().unwrap().success());
}

#[test]
fn first_launch_opens_the_view_and_explicit_setup_keeps_the_draft() {
    let home = Home::create();
    let mut terminal = launch(home.bare_command(false));
    terminal
        .wait_for(b"No model configured.", 0, "missing-model guidance")
        .unwrap();
    terminal
        .wait_for(b"/connect", 0, "the connection command")
        .unwrap();
    terminal
        .send(b"\x1b[200~draft-before-setup\x1b[201~\r")
        .unwrap();
    terminal.wait_for(DRAFT, 0, "the queued draft").unwrap();
    let config = home.0.join(".nouli/config.toml");
    let before = fs::read(&config).unwrap();
    terminal.send(b"/connect\r").unwrap();
    terminal
        .wait_for(b"Service:", 0, "explicit connection setup")
        .unwrap();
    terminal
        .wait_for(
            b"typesafe, openrouter, cloudflare, openai",
            0,
            "the requested service order",
        )
        .unwrap();
    let returning = terminal.mark();
    terminal.send(b"/cancel\r").unwrap();
    terminal
        .wait_for(b"\x1b[?1049h", returning, "the restored interactive view")
        .unwrap();
    terminal
        .wait_for(DRAFT, returning, "the retained draft")
        .unwrap();
    assert_eq!(fs::read(config).unwrap(), before);
    assert!(!home.0.join(".nouli/auth.toml").exists());
    close(terminal);
}

#[test]
fn a_connection_without_a_default_model_opens_the_view_with_model_guidance() {
    let home = Home::create();
    let server = loopback::Server::new(r#"{"models":[]}"#);
    home.configure(&server, "system-one", false);
    let file = home.0.join("config.toml");
    let config = fs::read_to_string(&file)
        .unwrap()
        .replace("default_provider = \"local\"\n", "")
        .replace("default_model = \"offline\"\n", "");
    fs::write(file, config).unwrap();
    let mut terminal = launch(home.command());
    terminal
        .wait_for(b"No model selected.", 0, "model-selection guidance")
        .unwrap();
    terminal
        .wait_for(b"/model", 0, "the model command")
        .unwrap();
    assert_eq!(server.count(), 0, "startup does not contact a provider");
    terminal.send(b"/model\r").unwrap();
    terminal
        .wait_for(b"Add connection", 0, "the model picker")
        .unwrap();
    close(terminal);
}

#[test]
fn missing_credentials_do_not_prompt_before_the_view_or_contact_the_provider() {
    let home = Home::create();
    let server = loopback::Server::new(r#"{"models":[]}"#);
    home.configure(&server, "system-one", true);
    let terminal = launch(home.command());
    assert_eq!(server.count(), 0);
    assert!(!home.0.join("auth.toml").exists());
    close(terminal);
    assert_eq!(server.count(), 0);
}

#[test]
fn opening_the_view_does_not_execute_a_configured_credential_command() {
    let home = Home::create();
    let server = loopback::Server::new(r#"{"models":[]}"#);
    home.configure(&server, "system-one", false);
    let counter = home.0.join("credential-runs");
    let helper = child::executable();
    assert!(
        home.run([
            "auth",
            "login",
            "local",
            "--command",
            helper.to_str().unwrap(),
            "count",
            counter.to_str().unwrap(),
        ])
        .status
        .success()
    );
    assert!(!counter.exists(), "storing a command does not execute it");
    close(launch(home.command()));
    assert!(!counter.exists(), "opening the view does not execute it");
    assert_eq!(server.count(), 0);
    assert!(
        child::command("count")
            .arg(&counter)
            .output()
            .unwrap()
            .status
            .success()
    );
    assert_eq!(
        fs::read(counter).unwrap(),
        b"x\n",
        "the helper records executions"
    );
}
