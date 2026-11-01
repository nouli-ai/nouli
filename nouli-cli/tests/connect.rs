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
