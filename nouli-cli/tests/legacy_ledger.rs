//! Ledgers written by released binaries. 0.1.0: three sessions' worth of
//! history with two completed `--request-id` turns and one request interrupted
//! in flight. 0.2.0: a session whose two turns saved the standing questions
//! `transport` and `on_time`, and a branch of it with no turns of its own.
#[path = "support/harness.rs"]
pub mod harness;
#[path = "support/loopback.rs"]
pub mod loopback;
use harness::{Home, report};
use loopback::Server;
use serde_json::{Value, json};
use std::path::Path;

/// Where the 0.1.0 run's provider listened when it was interrupted.
const INTERRUPTED_ADDRESS: &str = "127.0.0.1:38817";

fn install(home: &Home) {
    install_version(home, "0.1.0");
}

fn install_version(home: &Home, version: &str) {
    let store = home.store();
    std::fs::create_dir_all(&store).unwrap();
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join(format!(
        "../fixtures/contracts/ledger-v{version}/ledger.sqlite3"
    ));
    std::fs::copy(fixture, store.join("ledger.sqlite3")).unwrap();
}

fn write(home: &Home, name: &str, value: &Value) -> String {
    let path = home.0.join(name);
    std::fs::write(&path, value.to_string()).unwrap();
    path.display().to_string()
}

fn first_request() -> Value {
    serde_json::to_value(harness::turn()).unwrap()
}

fn second_request() -> Value {
    let mut request = first_request();
    request["state"]
        .as_array_mut()
        .unwrap()
        .push(json!({"type": "text", "text": "Second turn evidence."}));
    request
}

fn answering() -> Server {
    Server::new(harness::TYPESAFE)
}

#[test]
fn a_ledger_from_0_1_0_opens_lists_views_and_replays_its_completed_requests() {
    let server = answering();
    let home = Home::create();
    home.configure(&server, "typesafe", false);
    install(&home);
    let before = std::fs::read(home.store().join("ledger.sqlite3")).unwrap();
    // Inspection reads the old schema without migrating it.
    let listed = home.run(["sessions", "list", "--json"]);
    assert!(listed.status.success(), "{listed:?}");
    let listed = String::from_utf8(listed.stdout).unwrap();
    assert!(
        listed.contains("Trip") && listed.contains("Stuck"),
        "{listed}"
    );
    let viewed = home.run(["view", "Trip", "--format", "text"]);
    assert!(viewed.status.success(), "{viewed:?}");
    let facts = home.run(["facts", "Trip", "--format", "text"]);
    assert!(facts.status.success(), "{facts:?}");
    assert_eq!(
        String::from_utf8(facts.stdout).unwrap().trim(),
        "no facts yet"
    );
    assert!(before == std::fs::read(home.store().join("ledger.sqlite3")).unwrap());
    // The turns that 0.1.0 completed replay without calling the provider.
    let first = write(&home, "first.json", &first_request());
    let replay = report(home.run([
        "new",
        "Trip",
        "--input",
        &first,
        "--request-id",
        "done-key",
        "--json",
    ]));
    assert!(replay.replayed);
    let second = write(&home, "second.json", &second_request());
    let again = report(home.run([
        "add",
        "Trip",
        "--input",
        &second,
        "--request-id",
        "second-key",
        "--json",
    ]));
    assert!(again.replayed && again.turn != replay.turn);
    assert_eq!(server.count(), 0);
    // The old session takes a new turn in the new format.
    let added = report(
        home.run(
            [
                &["add", "Trip", "A fact from 0.2.0", "--json"][..],
                &harness::QUESTIONS,
            ]
            .concat(),
        ),
    );
    assert!(!added.replayed);
    let facts = home.run(["facts", "Trip", "--format", "text"]);
    assert!(
        String::from_utf8(facts.stdout)
            .unwrap()
            .contains("A fact from 0.2.0")
    );
}

#[test]
fn an_attempt_interrupted_under_0_1_0_is_reported_and_retried_with_its_saved_settings() {
    let server = answering();
    let home = Home::create();
    home.configure(&server, "typesafe", false);
    install(&home);
    let request = write(&home, "stuck.json", &first_request());
    let args = [
        "new",
        "Stuck",
        "--input",
        &request,
        "--request-id",
        "stuck-key",
        "--json",
    ];
    // Repeating the request does not dispatch it again.
    let repeated = home.run(args);
    assert_eq!(repeated.status.code(), Some(130), "{repeated:?}");
    assert_eq!(server.count(), 0);
    let text = String::from_utf8(repeated.stdout).unwrap();
    assert!(text.contains("request_requires_explicit_retry"), "{text}");
    // An explicit retry uses the provider address 0.1.0 saved, if it can be had.
    let Ok(saved) = Server::bind(INTERRUPTED_ADDRESS, |_| (200, harness::TYPESAFE.to_owned()))
    else {
        return;
    };
    let retried =
        report(home.run([&args[..args.len() - 1], &["--retry-interrupted", "--json"]].concat()));
    assert!(!retried.replayed);
    assert_eq!(saved.count(), 1);
    assert_eq!(server.count(), 0);
}

#[test]
fn a_ledger_from_0_2_0_reads_and_never_asks_its_standing_questions_again() {
    let server = answering();
    let home = Home::create();
    home.configure(&server, "typesafe", false);
    install_version(&home, "0.2.0");
    let before = std::fs::read(home.store().join("ledger.sqlite3")).unwrap();
    for args in [
        &["view", "Trip", "--format", "text"][..],
        &["facts", "Branch", "--format", "text"],
        &["sessions", "export", "Trip", "--json"],
    ] {
        let output = home.run(args);
        assert!(output.status.success(), "{args:?}: {output:?}");
    }
    assert!(before == std::fs::read(home.store().join("ledger.sqlite3")).unwrap());
    // Each call asks only the questions given, never the saved `transport`, and
    // `on_time` is a new question rather than a clash with the saved one.
    let asked = |server: &Server| {
        let calls = server.calls.lock().unwrap();
        let mut ids: Vec<String> = calls.last().unwrap()["questions"]
            .as_object()
            .unwrap()
            .keys()
            .cloned()
            .collect();
        ids.sort();
        ids
    };
    for args in [
        &["add", "Trip", "A taxi is waiting."][..],
        &["add", "Branch", "The branch has its own fact."],
        &["whatif", "Trip", "The taxi is gone."],
    ] {
        report(home.run([args, &harness::QUESTIONS[..], &["--json"]].concat()));
        assert_eq!(asked(&server), ["on_time", "quality", "travel"], "{args:?}");
    }
    assert_eq!(server.count(), 3);
    // A request ID that 0.2.0 used is taken: a new command under it conflicts.
    let used = "01a188ca-9c22-7292-8946-686ab75b22b8";
    for extra in [&[][..], &["--replace", "1", "The train costs 45 USD."]] {
        let args = [
            &[
                "add",
                "Trip",
                "-q",
                "x: Is it?",
                "--request-id",
                used,
                "--json",
            ][..],
            extra,
        ]
        .concat();
        let output = home.run(&args);
        assert_eq!(output.status.code(), Some(5), "{output:?}");
        assert!(
            String::from_utf8_lossy(&output.stdout).contains("request_conflict"),
            "{output:?}"
        );
    }
    assert_eq!(server.count(), 3);
}
