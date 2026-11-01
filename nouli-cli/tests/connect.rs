#[path = "support/harness.rs"]
#[allow(dead_code, reason = "each test uses a subset of the helpers")]
mod harness;
#[path = "support/loopback.rs"]
#[allow(dead_code, reason = "each test uses a subset of the helpers")]
mod loopback;
use harness::Home;

#[test]
fn connection_setup_respects_output_formats_and_rejects_jsonl_before_writing() {
    for format in ["json", "toon", "text"] {
        let home = Home::create();
        let output = home.run([
            "--format",
            format,
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
            "--no-auth",
            "--default",
            "--non-interactive",
        ]);
        assert!(output.status.success(), "{output:?}");
        let text = String::from_utf8(output.stdout).unwrap();
        if format == "json" {
            let value: serde_json::Value = serde_json::from_str(&text).unwrap();
            assert_eq!(value["configured"], true);
            assert_eq!(value["provider"], "gateway");
        } else {
            assert!(text.contains("gateway"));
        }
    }
    let home = Home::create();
    let output = home.run([
        "--jsonl",
        "connect",
        "ollama",
        "--model",
        "clef",
        "--non-interactive",
    ]);
    assert!(!output.status.success());
    assert!(!home.0.join("config.toml").exists());
    assert!(!home.0.join("auth.toml").exists());
}

#[test]
fn setup_records_environment_source_separately_and_auth_disabled_is_explicit() {
    let home = Home::create();
    let output = home.run([
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
        "--env",
        "UNSET_PROVIDER_KEY",
        "--non-interactive",
    ]);
    assert!(output.status.success(), "{output:?}");
    let config = std::fs::read_to_string(home.0.join("config.toml")).unwrap();
    assert!(!config.contains("UNSET_PROVIDER_KEY"));
    let auth = std::fs::read_to_string(home.0.join("auth.toml")).unwrap();
    assert!(auth.contains("UNSET_PROVIDER_KEY"));
    let output = home.run(["connect", "ollama", "--model", "clef", "--non-interactive"]);
    assert!(output.status.success(), "{output:?}");
    let listed = home.run(["auth", "list", "--json"]);
    let value: serde_json::Value = serde_json::from_slice(&listed.stdout).unwrap();
    let local = value["providers"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["provider"] == "ollama")
        .unwrap();
    assert_eq!(local["source"], "disabled");
    let login = home.run(["auth", "login", "ollama", "--env", "NEW_KEY", "--json"]);
    assert!(login.status.success(), "{login:?}");
    let value: serde_json::Value = serde_json::from_slice(&login.stdout).unwrap();
    assert_eq!(value["authentication_disabled_for_default_model"], true);
    let updated = std::fs::read_to_string(home.0.join("auth.toml")).unwrap();
    assert!(updated.contains("UNSET_PROVIDER_KEY"));
    assert!(updated.contains("NEW_KEY"));
}

#[test]
fn authentication_utilities_follow_the_same_model_overlays_as_execution() {
    let home = Home::create();
    std::fs::write(
        home.0.join("config.toml"),
        r#"
default_provider = "gateway"
default_model = "enabled"
[defaults.connection]
no_auth = true
[providers.gateway]
protocol = "system-one"
base_url = "https://arbitrary.example/api"
[providers.gateway.models.enabled.connection]
no_auth = false
[providers.gateway.models.disabled.connection]
no_auth = true
"#,
    )
    .unwrap();
    let login = home.run(["auth", "login", "gateway", "--env", "NEW_KEY", "--json"]);
    assert!(login.status.success(), "{login:?}");
    let value: serde_json::Value = serde_json::from_slice(&login.stdout).unwrap();
    assert_eq!(value["authentication_disabled_for_default_model"], false);
    let config = std::fs::read_to_string(home.0.join("config.toml"))
        .unwrap()
        .replace(
            "default_model = \"enabled\"",
            "default_model = \"disabled\"",
        );
    std::fs::write(home.0.join("config.toml"), config).unwrap();
    let list = home.run(["auth", "list", "--json"]);
    let value: serde_json::Value = serde_json::from_slice(&list.stdout).unwrap();
    assert_eq!(value["providers"][0]["source"], "disabled");
}

#[test]
fn relative_config_path_is_written_in_the_current_directory() {
    let home = Home::create();
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_nouli"))
        .env_clear()
        .current_dir(&home.0)
        .env("HOME", &home.0)
        .args([
            "--config",
            "config.toml",
            "--json",
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
            "--env",
            "UNSET_PROVIDER_KEY",
            "--default",
            "--non-interactive",
        ])
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    let config = std::fs::read_to_string(home.0.join("config.toml")).unwrap();
    assert!(config.contains("gateway"));
    assert!(home.0.join("auth.toml").exists());
    let leftovers = std::fs::read_dir(&home.0)
        .unwrap()
        .filter_map(|entry| entry.unwrap().file_name().into_string().ok())
        .filter(|name| name.starts_with('.'))
        .count();
    assert_eq!(leftovers, 0);
}

/// The error code a failed `--json` command reported.
fn error_code(output: &std::process::Output) -> String {
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["result"]["kind"], "error", "{output:?}");
    value["result"]["report"]["code"]
        .as_str()
        .unwrap()
        .to_owned()
}

/// A noninteractive custom connection setup with `extra` arguments.
fn custom<'a>(extra: &[&'a str]) -> Vec<&'a str> {
    [
        &[
            "--json",
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
            "--non-interactive",
        ][..],
        extra,
    ]
    .concat()
}

#[test]
fn setup_failures_report_specific_codes_and_exit_codes() {
    let home = Home::create();
    for (arguments, code) in [
        (
            vec!["--json", "connect", "nowhere", "--non-interactive"],
            "unknown_service",
        ),
        (
            vec![
                "--json",
                "connect",
                "llama.cpp",
                "--protocol",
                "smoke-signals",
                "--model",
                "any-model",
                "--non-interactive",
            ],
            "invalid_protocol",
        ),
        (
            vec![
                "--json",
                "connect",
                "ollama",
                "--name",
                " ",
                "--non-interactive",
            ],
            "invalid_connection_name",
        ),
        (custom(&["--binding", "pigeon"]), "invalid_binding"),
        (custom(&["--env", "1 bad"]), "invalid_environment_variable"),
        (custom(&["--stdin"]), "credential_required"),
        (custom(&[]), "terminal_required"),
    ] {
        let output = home.run(&arguments);
        assert_eq!(output.status.code(), Some(2), "{arguments:?}: {output:?}");
        if matches!(code, "invalid_protocol" | "invalid_binding") {
            assert_eq!(output.stdout, [] as [u8; 0]);
            assert!(String::from_utf8_lossy(&output.stderr).contains("invalid value"));
        } else {
            assert_eq!(error_code(&output), code, "{arguments:?}");
        }
    }
    assert!(!home.0.join("config.toml").exists());
    let output = home.run(custom(&["--no-auth"]));
    assert!(output.status.success(), "{output:?}");
    let again = home.run(custom(&["--no-auth"]));
    assert_eq!(again.status.code(), Some(2));
    assert_eq!(error_code(&again), "connection_exists");
}

#[test]
fn an_unwritable_credential_store_is_a_storage_failure() {
    use std::os::unix::fs::PermissionsExt;
    let home = Home::create();
    // Without its lock file, the configuration itself cannot be written.
    std::fs::set_permissions(&home.0, std::fs::Permissions::from_mode(0o500)).unwrap();
    let output = home.run(custom(&["--env", "PROVIDER_KEY"]));
    std::fs::set_permissions(&home.0, std::fs::Permissions::from_mode(0o700)).unwrap();
    assert_eq!(output.status.code(), Some(6), "{output:?}");
    assert_eq!(error_code(&output), "configuration_write_failed");
    for lock in ["config.toml.lock", "auth.toml.lock"] {
        std::fs::write(home.0.join(lock), "").unwrap();
    }
    std::fs::set_permissions(&home.0, std::fs::Permissions::from_mode(0o500)).unwrap();
    let output = home.run(custom(&["--env", "PROVIDER_KEY"]));
    std::fs::set_permissions(&home.0, std::fs::Permissions::from_mode(0o700)).unwrap();
    assert_eq!(output.status.code(), Some(6), "{output:?}");
    assert_eq!(error_code(&output), "auth_store_write_failed");
    assert!(!home.0.join("config.toml").exists());
    assert!(!home.0.join("auth.toml").exists());
}

#[test]
fn existing_configuration_and_store_problems_keep_their_codes_and_name_the_file() {
    use std::os::unix::fs::PermissionsExt;
    let home = Home::create();
    let config = home.0.join("config.toml");
    std::fs::write(&config, "x = [").unwrap();
    let output = home.run(custom(&["--no-auth"]));
    assert_eq!(output.status.code(), Some(2), "{output:?}");
    assert_eq!(error_code(&output), "invalid_configuration_syntax");
    assert!(String::from_utf8_lossy(&output.stdout).contains("Configuration: "));
    std::fs::write(&config, "").unwrap();
    std::fs::set_permissions(&config, std::fs::Permissions::from_mode(0o000)).unwrap();
    let output = home.run(custom(&["--no-auth"]));
    std::fs::set_permissions(&config, std::fs::Permissions::from_mode(0o600)).unwrap();
    assert_eq!(output.status.code(), Some(2), "{output:?}");
    assert_eq!(error_code(&output), "configuration_unreadable");
    let auth = home.0.join("auth.toml");
    std::fs::write(&auth, "").unwrap();
    std::fs::set_permissions(&auth, std::fs::Permissions::from_mode(0o644)).unwrap();
    let output = home.run(custom(&["--env", "PROVIDER_KEY"]));
    assert_eq!(output.status.code(), Some(2), "{output:?}");
    assert_eq!(error_code(&output), "auth_store_insecure");
    let output = home.run([
        "--json",
        "connect",
        "custom",
        "--name",
        "gateway",
        "--protocol",
        "system-one",
        "--base-url",
        "http://someone:hunter2@example.com",
        "--model",
        "any-model",
        "--no-auth",
        "--non-interactive",
    ]);
    assert_eq!(output.status.code(), Some(2), "{output:?}");
    assert_eq!(error_code(&output), "invalid_connection");
    assert!(!String::from_utf8_lossy(&output.stdout).contains("hunter2"));
}

#[test]
fn invalid_protocol_and_binding_values_list_valid_values_before_setup() {
    for (flag, valid) in [
        ("--protocol", ["system-one", "openai-decisions"]),
        ("--binding", ["standard", "cloudflare-workers-ai"]),
    ] {
        let home = Home::create();
        let output = home.run([
            "connect",
            "custom",
            flag,
            "not-a-value",
            "--non-interactive",
        ]);
        assert_eq!(output.status.code(), Some(2));
        assert_eq!(output.stdout, [] as [u8; 0]);
        let error = String::from_utf8(output.stderr).unwrap();
        assert!(
            error.contains("invalid value") && error.contains(flag),
            "{error}"
        );
        for value in valid {
            assert!(error.contains(value), "{error}");
        }
        assert!(!home.0.join("config.toml").exists());
        assert!(!home.0.join("auth.toml").exists());
    }
}
