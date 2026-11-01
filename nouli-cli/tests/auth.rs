#[path = "support/harness.rs"]
#[allow(dead_code, reason = "each test uses a subset of the helpers")]
mod harness;
#[path = "support/loopback.rs"]
#[allow(dead_code, reason = "each test uses a subset of the helpers")]
mod loopback;
use harness::{Home, QUESTION, report};
use loopback::Server;
use nouli_core::protocol::Report;
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    process::Output,
    sync::atomic::{AtomicBool, Ordering},
};

const KEY: &str = "SENTINEL_AUTH_KEY_VALUE";

fn with_stdin(home: &Home, arguments: &[&str], input: &str) -> Output {
    home.run_with_stdin(arguments, input.as_bytes())
}

fn login(home: &Home, key: &str) -> Output {
    with_stdin(
        home,
        &["auth", "login", "local", "--stdin"],
        &format!("{key}\n"),
    )
}

fn auth_file(home: &Home) -> std::path::PathBuf {
    home.0.join("auth.toml")
}

fn mode(path: &std::path::Path) -> u32 {
    fs::metadata(path).unwrap().permissions().mode() & 0o777
}

fn error_code(output: &Output, exit: i32) -> String {
    harness::error_report(output, exit, None).code
}

fn everything_visible(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

#[test]
fn login_creates_a_private_file_that_is_never_wider_and_the_key_is_then_used() {
    let server = Server::with_handler(harness::typesafe);
    let home = Home::create();
    // The provider names no variable: the stored entry is its only source.
    home.configure(&server, "typesafe", false);
    let watching = std::sync::Arc::new(AtomicBool::new(true));
    let widest = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
    let watcher = {
        let (watching, widest, directory) = (watching.clone(), widest.clone(), home.0.clone());
        std::thread::spawn(move || {
            while watching.load(Ordering::SeqCst) {
                for entry in fs::read_dir(&directory).unwrap().flatten() {
                    let name = entry.file_name().to_string_lossy().into_owned();
                    if (name.starts_with(".auth-") || name == "auth.toml")
                        && let Ok(metadata) = entry.metadata()
                    {
                        widest.fetch_max(metadata.permissions().mode() & 0o077, Ordering::SeqCst);
                    }
                }
            }
        })
    };
    // A permissive umask must not widen anything.
    let script = format!(
        "umask 0; printf '%s\\n' '{KEY}' | exec '{}' --config '{}' --store '{}' auth login local --stdin",
        env!("CARGO_BIN_EXE_nouli"),
        home.0.join("config.toml").display(),
        home.store().display()
    );
    let output = Command::new("sh")
        .args(["-c", &script])
        .env_clear()
        .env("HOME", &home.0)
        .current_dir(&home.0)
        .output()
        .unwrap();
    watching.store(false, Ordering::SeqCst);
    watcher.join().unwrap();
    assert!(output.status.success(), "{}", everything_visible(&output));
    assert_eq!(mode(&auth_file(&home)), 0o600);
    assert_eq!(
        widest.load(Ordering::SeqCst),
        0,
        "the store was never wider than 0600"
    );
    assert!(!everything_visible(&output).contains(KEY));
    // The key reaches the provider, and appears nowhere it should not.
    let turn = report(&home.run([&["new", "Trip", "A fact"][..], &QUESTION].concat()));
    assert_eq!(
        server.authorizations.lock().unwrap()[0].as_deref(),
        Some(&*format!("Bearer {KEY}"))
    );
    let listing = home.run(["auth", "list"]);
    assert!(listing.status.success());
    let export = home.run(["sessions", "export", "Trip", "--format", "json"]);
    assert!(export.status.success());
    let ledger = fs::read(home.ledger_path()).unwrap();
    assert!(
        !ledger
            .windows(KEY.len())
            .any(|window| window == KEY.as_bytes())
    );
    for visible in [&listing, &export] {
        assert!(!everything_visible(visible).contains(KEY));
    }
    assert!(turn.turn.is_some());
    // Logging in again replaces the key.
    assert!(login(&home, "SECOND_KEY").status.success());
    report(&home.run([&["add", "Trip", "More"][..], &QUESTION].concat()));
    assert_eq!(
        server.authorizations.lock().unwrap()[1].as_deref(),
        Some("Bearer SECOND_KEY")
    );
}

use std::process::Command;

#[test]
fn a_store_others_can_read_is_refused_by_every_command_with_the_fix() {
    let server = Server::with_handler(harness::typesafe);
    let home = Home::create();
    home.configure(&server, "typesafe", false);
    assert!(login(&home, KEY).status.success());
    fs::set_permissions(auth_file(&home), fs::Permissions::from_mode(0o640)).unwrap();
    for arguments in [
        vec!["auth", "list"],
        vec!["auth", "logout", "local"],
        vec!["new", "Trip", "A fact", "-q", "q: Why?"],
    ] {
        let output = home.run(&arguments);
        assert_eq!(output.status.code(), Some(2), "{arguments:?}");
        assert_eq!(error_code(&output, 2), "auth_store_insecure");
        let visible = everything_visible(&output);
        assert!(visible.contains("chmod 600") && visible.contains("auth.toml"));
        assert!(!visible.contains(KEY));
    }
    assert_eq!(login(&home, "other").status.code(), Some(2));
    assert_eq!(server.count(), 0);
    fs::set_permissions(auth_file(&home), fs::Permissions::from_mode(0o600)).unwrap();
    assert!(home.run(["auth", "list"]).status.success());
}

#[test]
fn the_shell_beats_the_env_file_and_the_env_file_beats_the_store() {
    let server = Server::with_handler(harness::typesafe);
    let home = Home::create();
    // The provider names a variable, so all three sources can apply.
    home.configure(&server, "typesafe", true);
    assert!(login(&home, "STORE_KEY").status.success());
    fs::write(home.0.join(".env"), "NOULI_FIXTURE_KEY=FILE_KEY\n").unwrap();
    let ask = |name: &str, shell: Option<&str>| {
        let mut command = home.command();
        command.args([&["new", name, "A fact"][..], &QUESTION].concat());
        if let Some(value) = shell {
            command.env("NOULI_FIXTURE_KEY", value);
        }
        report(&command.output().unwrap());
    };
    ask("One", Some("SHELL_KEY"));
    ask("Two", None);
    fs::remove_file(home.0.join(".env")).unwrap();
    ask("Three", None);
    assert_eq!(
        *server.authorizations.lock().unwrap(),
        [
            Some("Bearer SHELL_KEY".to_owned()),
            Some("Bearer FILE_KEY".to_owned()),
            Some("Bearer STORE_KEY".to_owned())
        ]
    );
    // The listing says which source wins, without a value.
    let winner = |shell: Option<&str>, file: bool| {
        if file {
            fs::write(home.0.join(".env"), "NOULI_FIXTURE_KEY=FILE_KEY\n").unwrap();
        }
        let mut command = home.command();
        command.args(["auth", "list", "--format", "text"]);
        if let Some(value) = shell {
            command.env("NOULI_FIXTURE_KEY", value);
        }
        let output = command.output().unwrap();
        assert!(output.status.success());
        let text = everything_visible(&output);
        assert!(
            !text.contains("STORE_KEY")
                && !text.contains("SHELL_KEY")
                && !text.contains("FILE_KEY")
        );
        text
    };
    assert!(
        winner(None, false).contains("stored: key") && winner(None, false).contains("using: store")
    );
    assert!(winner(None, true).contains("using: env file"));
    assert!(winner(Some("x"), true).contains("using: shell"));
}

#[test]
fn env_and_command_sources_are_followed_and_failures_are_credential_unavailable() {
    fn create(name: &str) -> Vec<&str> {
        [&["new", name, "A fact"][..], &QUESTION].concat()
    }
    let server = Server::with_handler(harness::typesafe);
    let home = Home::create();
    home.configure(&server, "typesafe", false);
    // An env source names a variable, and the variable's value is the key.
    let stored = home.run(["auth", "login", "local", "--env", "CUSTOM_SOURCE_VARIABLE"]);
    assert!(stored.status.success());
    assert!(
        fs::read_to_string(auth_file(&home))
            .unwrap()
            .contains("CUSTOM_SOURCE_VARIABLE")
    );
    let missing = home.run(create("A"));
    assert_eq!(missing.status.code(), Some(3));
    assert_eq!(error_code(&missing, 3), "credential_unavailable");
    assert!(everything_visible(&missing).contains("nouli auth login local"));
    let mut command = home.command();
    command
        .args(create("B"))
        .env("CUSTOM_SOURCE_VARIABLE", "VARIABLE_KEY");
    report(&command.output().unwrap());
    // A command source runs without a shell and its output is the key.
    assert!(
        home.run([
            "auth",
            "login",
            "local",
            "--command",
            "printf",
            "%s",
            "COMMAND_KEY"
        ])
        .status
        .success()
    );
    report(&home.run(create("C")));
    // A command that fails, or prints nothing, is unavailable. Its output is never shown.
    for (name, words) in [
        (
            "D1",
            vec![
                "sh",
                "-c",
                "echo LEAKED_OUTPUT; echo LEAKED_ERROR >&2; exit 3",
            ],
        ),
        ("D2", vec!["true"]),
    ] {
        let mut arguments = vec!["auth", "login", "local", "--command"];
        arguments.extend(words);
        assert!(home.run(&arguments).status.success());
        let failed = home.run(create(name));
        assert_eq!(failed.status.code(), Some(3));
        assert_eq!(error_code(&failed, 3), "credential_unavailable");
        let visible = everything_visible(&failed);
        assert!(!visible.contains("LEAKED"), "{visible}");
    }
    assert_eq!(
        *server.authorizations.lock().unwrap(),
        [
            Some("Bearer VARIABLE_KEY".to_owned()),
            Some("Bearer COMMAND_KEY".to_owned())
        ]
    );
}

#[test]
fn logout_login_errors_and_unknown_types_name_the_problem_without_the_key() {
    let server = Server::with_handler(harness::typesafe);
    let home = Home::create();
    home.configure(&server, "typesafe", false);
    assert!(login(&home, KEY).status.success());
    let removed = home.run(["auth", "logout", "local", "--format", "text"]);
    assert!(String::from_utf8_lossy(&removed.stdout).contains("Removed the stored credential"));
    assert!(!fs::read_to_string(auth_file(&home)).unwrap().contains(KEY));
    let again = home.run(["auth", "logout", "local", "--format", "text"]);
    assert!(String::from_utf8_lossy(&again.stdout).contains("No stored credential"));
    let after = home.run([&["new", "Trip", "A fact"][..], &QUESTION].concat());
    assert_eq!(
        after.status.code(),
        Some(0),
        "an unauthenticated local provider keeps working"
    );
    // Login for a provider the configuration does not define.
    let unknown = with_stdin(
        &home,
        &["auth", "login", "nowhere", "--stdin"],
        &format!("{KEY}\n"),
    );
    assert_eq!(unknown.status.code(), Some(2));
    assert_eq!(error_code(&unknown, 2), "unknown_provider");
    assert!(!everything_visible(&unknown).contains(KEY));
    // Without a terminal, login refuses to prompt.
    let prompt = with_stdin(&home, &["auth", "login", "local"], "");
    assert_eq!(prompt.status.code(), Some(2));
    assert_eq!(error_code(&prompt, 2), "terminal_required");
    // Empty and invalid input are rejected.
    assert_eq!(
        with_stdin(&home, &["auth", "login", "local", "--stdin"], "\n")
            .status
            .code(),
        Some(2)
    );
    assert_eq!(
        home.run(["auth", "login", "local", "--env", "1 bad"])
            .status
            .code(),
        Some(2)
    );
    // An unsupported type, unknown field, or two sources names the provider.
    for (body, needle) in [
        (
            "type = \"oauth\"\nkey = \"SENTINEL_FILE_KEY\"",
            "unsupported credential type",
        ),
        (
            "type = \"api_key\"\nkey = \"SENTINEL_FILE_KEY\"\nextra = 1",
            "unknown field",
        ),
        (
            "type = \"api_key\"\nkey = \"SENTINEL_FILE_KEY\"\nenv = \"X\"",
            "exactly one of",
        ),
        ("key = \"SENTINEL_FILE_KEY\"", "needs `type"),
    ] {
        fs::write(auth_file(&home), format!("[providers.local]\n{body}\n")).unwrap();
        fs::set_permissions(auth_file(&home), fs::Permissions::from_mode(0o600)).unwrap();
        let output = home.run(["auth", "list"]);
        assert_eq!(output.status.code(), Some(2), "{body}");
        assert_eq!(error_code(&output, 2), "invalid_auth_store");
        let visible = everything_visible(&output);
        assert!(
            visible.contains("local") && visible.contains(needle),
            "{visible}"
        );
        assert!(!visible.contains("SENTINEL_FILE_KEY"));
    }
}

#[test]
fn report_commands_refuse_jsonl_with_one_error_code() {
    let server = Server::with_handler(harness::typesafe);
    let home = Home::create();
    home.configure(&server, "typesafe", false);
    let run = nouli_core::identity::Id::generate().to_string();
    for arguments in [
        &["auth", "list"][..],
        &["auth", "logout", "local"][..],
        &["connect", "ollama", "--model", "clef", "--non-interactive"][..],
        &["sessions", "list"][..],
        &["workflow", "show", &run][..],
    ] {
        let output = home.run([arguments, &["--jsonl"]].concat());
        assert_eq!(output.status.code(), Some(2), "{arguments:?}: {output:?}");
        let events = harness::jsonl_events(&output);
        let result = harness::terminal(&events);
        let Report::Error(error) = &result.result else {
            panic!("an error report: {output:?}")
        };
        assert_eq!(error.code, "unsupported_output_format", "{arguments:?}");
        assert_eq!(
            error.message,
            "This command supports text, JSON, or TOON output"
        );
    }
    assert!(!auth_file(&home).exists());
    // Global output flags on both sides of the subcommand pass clap's checks,
    // for report-only and streaming commands alike.
    for arguments in [
        &["--json", "auth", "list", "--jsonl"][..],
        &["--json", "workflow", "run", "-", "--jsonl"][..],
        &["--json", "new", "Trip", "-q", "pick: Which?", "--jsonl"][..],
    ] {
        let conflicting = home.run(arguments);
        assert_eq!(conflicting.status.code(), Some(2), "{arguments:?}");
        assert_eq!(String::from_utf8_lossy(&conflicting.stdout), "");
        assert!(
            String::from_utf8_lossy(&conflicting.stderr).contains("conflicting_output_options"),
            "{conflicting:?}"
        );
    }
}

#[test]
fn login_and_connect_read_standard_input_keys_the_same_way() {
    let server = Server::with_handler(harness::typesafe);
    let home = Home::create();
    home.configure(&server, "typesafe", false);
    let oversized = format!("{}\n", "k".repeat(16 * 1024));
    for arguments in [
        &["auth", "login", "local", "--stdin"][..],
        &[
            "connect",
            "custom",
            "--name",
            "gateway",
            "--protocol",
            "system-one",
            "--base-url",
            "https://arbitrary.example/api",
            "--model",
            "any-model",
            "--stdin",
            "--non-interactive",
        ][..],
    ] {
        let output = with_stdin(&home, arguments, &oversized);
        assert_eq!(output.status.code(), Some(2), "{arguments:?}");
        assert!(
            String::from_utf8_lossy(&output.stdout).contains("longer than 16 KiB"),
            "{output:?}"
        );
        assert!(!auth_file(&home).exists());
    }
    let output = with_stdin(
        &home,
        &["auth", "login", "local", "--stdin"],
        "spaced key  \r\n",
    );
    assert!(output.status.success(), "{output:?}");
    let output = with_stdin(
        &home,
        &[
            "connect",
            "custom",
            "--name",
            "gateway",
            "--protocol",
            "system-one",
            "--base-url",
            "https://arbitrary.example/api",
            "--model",
            "any-model",
            "--stdin",
            "--non-interactive",
        ],
        "connect key  \n",
    );
    assert!(output.status.success(), "{output:?}");
    let stored = fs::read_to_string(auth_file(&home)).unwrap();
    assert!(stored.contains("\"spaced key  \""), "{stored}");
    assert!(stored.contains("\"connect key  \""), "{stored}");
}

#[test]
fn a_resumed_workflow_uses_the_stored_key_as_it_is_now() {
    use nouli_core::workflow::RunStatus;
    let failed_once = AtomicBool::new(true);
    let server = Server::with_handler(move |body| {
        if body["state"].as_str().unwrap().contains("NODE:high")
            && failed_once.swap(false, Ordering::SeqCst)
        {
            (503, "{}".into())
        } else {
            harness::typesafe(body)
        }
    });
    let home = Home::create();
    home.configure(&server, "typesafe", false);
    assert!(login(&home, "KEY_AT_START").status.success());
    let file = home.0.join("workflow.json");
    fs::write(&file, serde_json::to_vec(&harness::workflow()).unwrap()).unwrap();
    let partial = harness::run_report(&home.run(["workflow", "run", file.to_str().unwrap()]), 4);
    assert_eq!(partial.status, RunStatus::Partial);
    let before = server.count();
    assert!(login(&home, "KEY_AT_RESUME").status.success());
    // The saved run froze the source, auth:local, not the key's value.
    let shown = home.run([
        "workflow",
        "show",
        &partial.run.unwrap().to_string(),
        "--json",
    ]);
    let text = everything_visible(&shown);
    assert!(text.contains("stored_credential") && !text.contains("KEY_AT_START"));
    let done = harness::run_report(
        &home.run(["workflow", "resume", &partial.run.unwrap().to_string()]),
        0,
    );
    assert_eq!(done.status, RunStatus::Completed);
    let headers = server.authorizations.lock().unwrap();
    assert!(
        headers[..before]
            .iter()
            .all(|h| h.as_deref() == Some("Bearer KEY_AT_START"))
    );
    assert!(
        headers[before..]
            .iter()
            .all(|h| h.as_deref() == Some("Bearer KEY_AT_RESUME"))
    );
    assert!(headers.len() > before);
}
