//! CLI-owned connection recipes and configuration authoring. Execution never reads recipes.
use crate::{Args, settings};
use nouli_core::{
    config::{
        Config, ConnectionSettings, HttpBinding, ImageEncoding, ModelSettings, Protocol,
        ProviderDefinition,
        auth::{AuthEntry, KeySource},
    },
    history::ModelSelection,
    identity::Id,
};
use std::{
    fs,
    io::{self, IsTerminal, Write},
    path::{Path, PathBuf},
    process::ExitCode,
};
use thiserror::Error;
use toml_edit::{DocumentMut, Item, value};
#[derive(Debug, Error)]
pub(crate) enum SetupError {
    #[error(transparent)]
    Configuration(#[from] nouli_core::config::ConfigError),
    #[error("connection setup cancelled")]
    Cancelled,
    #[error("connection setup needs a terminal or complete noninteractive arguments")]
    Terminal,
    #[error(
        "authentication is disabled in this connection; set connection.no_auth = false in config.toml before logging in"
    )]
    AuthDisabled,
    #[error("invalid connection settings")]
    Invalid,
    #[error("connection already exists; choose another name or use --replace")]
    Exists,
    #[error("cannot update configuration; any stored credential can be reused by repeating setup")]
    Write,
    #[error(transparent)]
    Io(#[from] io::Error),
}
#[derive(Debug, Default, Clone, clap::Args)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "each bool is an independent command-line flag"
)]
pub(crate) struct Connect {
    /// Setup shortcut, or custom.
    pub service: Option<String>,
    #[arg(long)]
    pub name: Option<String>,
    #[arg(long)]
    pub protocol: Option<String>,
    #[arg(long)]
    pub binding: Option<String>,
    #[arg(long)]
    pub base_url: Option<String>,
    #[arg(long)]
    pub model: Option<String>,
    #[arg(long)]
    pub env: Option<String>,
    #[arg(long, conflicts_with_all=["env","command","no_auth"])]
    pub stdin: bool,
    #[arg(long,num_args=1.., allow_hyphen_values=true, conflicts_with_all=["env","stdin","no_auth"])]
    pub command: Vec<String>,
    #[arg(long, conflicts_with_all=["env","command","stdin"])]
    pub no_auth: bool,
    #[arg(long)]
    pub default: bool,
    #[arg(long)]
    pub replace: bool,
    #[arg(long)]
    pub non_interactive: bool,
}
#[derive(Clone)]
struct Recipe {
    name: &'static str,
    url: &'static str,
    protocol: Protocol,
    models: &'static [&'static str],
    connection: ConnectionSettings,
}
// Setup-only suggestions, verified 2026-10-30. They are written as ordinary
// settings and never consulted during resolution or execution.
// https://docs.typesafe.ai/api/system-one and https://docs.typesafe.ai/images
// https://openrouter.ai/docs/guides/community/typesafe-sdk
// https://developers.cloudflare.com/workers-ai/models/clef/
// https://docs.ollama.com/api/systemone
fn recipes() -> Vec<Recipe> {
    let plain = ConnectionSettings::default();
    let assets = ConnectionSettings {
        image_encoding: Some(ImageEncoding::Assets),
        headers: std::collections::BTreeMap::from([(
            "TypeSafe-Danger-Zone-Images".into(),
            "true".into(),
        )]),
        min_image_dimension: Some(280),
        max_image_dimension: Some(1568),
        pixel_patch_size: Some(28),
        ..Default::default()
    };
    vec![
        Recipe {
            name: "openai",
            url: "https://api.openai.com",
            protocol: Protocol::OpenaiDecisions,
            models: &["gpt-6-luna"],
            connection: ConnectionSettings {
                image_encoding: Some(ImageEncoding::OpenaiContent),
                pixel_patch_size: Some(32),
                ..plain.clone()
            },
        },
        Recipe {
            name: "typesafe",
            url: "https://api.typesafe.ai",
            protocol: Protocol::SystemOne,
            models: &["jev-latest"],
            connection: assets,
        },
        Recipe {
            name: "openrouter",
            url: "https://openrouter.ai/api",
            protocol: Protocol::SystemOne,
            models: &["typesafe/jev-1.14", "cloudflare/clef"],
            connection: ConnectionSettings {
                image_encoding: Some(ImageEncoding::StateImageUrls),
                extra_body: std::collections::BTreeMap::from([(
                    "provider".into(),
                    serde_json::json!({"allow_fallbacks":false}),
                )]),
                ..plain.clone()
            },
        },
        Recipe {
            name: "cloudflare",
            url: "",
            protocol: Protocol::SystemOne,
            models: &["@cf/cloudflare/clef", "@cf/cloudflare/clef-flash"],
            connection: ConnectionSettings {
                binding: Some(HttpBinding::CloudflareWorkersAi),
                image_encoding: Some(ImageEncoding::ImagesDataUrls),
                ..plain.clone()
            },
        },
        Recipe {
            name: "ollama",
            url: "http://localhost:11434",
            protocol: Protocol::SystemOne,
            models: &["clef", "nimble"],
            connection: ConnectionSettings {
                no_auth: Some(true),
                image_encoding: Some(ImageEncoding::ImagesBase64),
                ..plain.clone()
            },
        },
        Recipe {
            name: "llama.cpp",
            url: "http://localhost:8080",
            protocol: Protocol::SystemOne,
            models: &[],
            connection: ConnectionSettings {
                no_auth: Some(true),
                image_encoding: Some(ImageEncoding::ImagesDataUrls),
                ..plain
            },
        },
    ]
}
pub(crate) fn read_key(label: &str) -> Result<nouli_core::config::auth::Secret, SetupError> {
    let value = rpassword::prompt_password(label)?;
    if value == "/cancel" {
        return Err(SetupError::Cancelled);
    }
    nouli_core::config::auth::Secret::new(value).map_err(|_| SetupError::Invalid)
}
pub(crate) fn prompt(label: &str, default: &str) -> Result<String, SetupError> {
    if !io::stdin().is_terminal() {
        return Err(SetupError::Terminal);
    }
    write!(
        io::stdout(),
        "{label}{}: ",
        if default.is_empty() {
            String::new()
        } else {
            format!(" [{default}]")
        }
    )?;
    io::stdout().flush()?;
    let mut line = String::new();
    if io::stdin().read_line(&mut line)? == 0 {
        return Err(SetupError::Cancelled);
    }
    let line = line.trim();
    if line == "/cancel" {
        return Err(SetupError::Cancelled);
    }
    Ok(if line.is_empty() {
        default.into()
    } else {
        line.into()
    })
}
fn choose(
    value: Option<String>,
    label: &str,
    default: &str,
    interactive: bool,
) -> Result<String, SetupError> {
    match value {
        Some(v) => Ok(v),
        None if interactive => prompt(label, default),
        None if !default.is_empty() => Ok(default.into()),
        None => Err(SetupError::Terminal),
    }
}
fn path(args: &Args) -> Result<PathBuf, SetupError> {
    args.config
        .clone()
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".nouli/config.toml")))
        .ok_or(SetupError::Invalid)
}
fn read(path: &Path) -> Result<DocumentMut, SetupError> {
    match fs::read_to_string(path) {
        Ok(s) => {
            Config::parse(&s).map_err(|_| SetupError::Invalid)?;
            s.parse().map_err(|_| SetupError::Invalid)
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(DocumentMut::new()),
        Err(e) => Err(e.into()),
    }
}
fn publish(path: &Path, doc: &DocumentMut) -> Result<(), SetupError> {
    use std::os::unix::fs::OpenOptionsExt;
    let temporary = path.with_file_name(format!(".config-{}.tmp", Id::generate()));
    let result = (|| {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&temporary)?;
        file.write_all(doc.to_string().as_bytes())?;
        file.sync_all()?;
        fs::rename(&temporary, path)?;
        fs::File::open(path.parent().unwrap())?.sync_all()
    })();
    if result.is_err() {
        let _ = fs::remove_file(temporary);
    }
    result.map_err(|_| SetupError::Write)
}
#[expect(
    clippy::too_many_lines,
    reason = "pending refactor into smaller functions"
)]
pub(crate) fn configure(args: &Args, options: &Connect) -> Result<ModelSelection, SetupError> {
    use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
    let interactive =
        !options.non_interactive && io::stdin().is_terminal() && io::stdout().is_terminal();
    let file = path(args)?;
    if interactive {
        let _ = writeln!(
            io::stdout(),
            "Connect a decision service. Enter /cancel to cancel.\nServices: openai, typesafe, openrouter, cloudflare, ollama, llama.cpp, custom"
        );
    }
    let service = choose(options.service.clone(), "Service", "", interactive)?;
    let recipe = recipes().into_iter().find(|r| r.name == service);
    if recipe.is_none() && service != "custom" {
        return Err(SetupError::Invalid);
    }
    let name = choose(
        options
            .name
            .clone()
            .or_else(|| recipe.as_ref().map(|_| service.clone())),
        "Connection name",
        &service,
        interactive,
    )?;
    if name.trim().is_empty() {
        return Err(SetupError::Invalid);
    }
    if !options.replace
        && read(&file)?
            .get("providers")
            .and_then(|providers| providers.get(&name))
            .is_some()
    {
        return Err(SetupError::Exists);
    }
    let proto = choose(
        options.protocol.clone().or_else(|| {
            recipe.as_ref().map(|r| {
                if r.protocol == Protocol::SystemOne {
                    "system-one"
                } else {
                    "openai-decisions"
                }
                .into()
            })
        }),
        "Protocol (system-one or openai-decisions)",
        recipe.as_ref().map_or("", |r| {
            if r.protocol == Protocol::SystemOne {
                "system-one"
            } else {
                "openai-decisions"
            }
        }),
        interactive,
    )?;
    let protocol = match proto.as_str() {
        "system-one" => Protocol::SystemOne,
        "openai-decisions" => Protocol::OpenaiDecisions,
        _ => return Err(SetupError::Invalid),
    };
    let mut connection = recipe
        .as_ref()
        .map(|r| r.connection.clone())
        .unwrap_or_default();
    if let Some(binding) = &options.binding {
        connection.binding = Some(match binding.as_str() {
            "standard" => HttpBinding::Standard,
            "cloudflare-workers-ai" => HttpBinding::CloudflareWorkersAi,
            _ => return Err(SetupError::Invalid),
        });
    }
    let mut url = options
        .base_url
        .clone()
        .unwrap_or_else(|| recipe.as_ref().map_or("", |r| r.url).into());
    if url.is_empty() && service == "cloudflare" && interactive {
        let account = prompt("Cloudflare account ID", "")?;
        if !account.bytes().all(|b| b.is_ascii_alphanumeric()) || account.is_empty() {
            return Err(SetupError::Invalid);
        }
        url = format!("https://api.cloudflare.com/client/v4/accounts/{account}/ai");
    }
    let base_url = choose(
        if url.is_empty() { None } else { Some(url) },
        "Endpoint",
        "",
        interactive,
    )?;
    let mut key = None;
    if options.no_auth {
        connection.no_auth = Some(true);
    } else if let Some(env) = &options.env {
        connection.no_auth = Some(false);
        key = Some(KeySource::env(env.clone()).map_err(|_| SetupError::Invalid)?);
    } else if !options.command.is_empty() {
        connection.no_auth = Some(false);
        key = Some(KeySource::command(options.command.clone()).map_err(|_| SetupError::Invalid)?);
    } else if options.stdin {
        connection.no_auth = Some(false);
        let mut s = String::new();
        io::stdin().read_line(&mut s)?;
        key = Some(KeySource::Key(
            nouli_core::config::auth::Secret::new(s.trim_end().into())
                .map_err(|_| SetupError::Invalid)?,
        ));
    } else if connection.no_auth != Some(true) {
        if !interactive {
            return Err(SetupError::Terminal);
        }
        let method = prompt("Authentication (key, env, command, none)", "key")?;
        key = match method.as_str() {
            "key" => Some(KeySource::Key(read_key("API key: ")?)),
            "env" => Some(
                KeySource::env(prompt("Environment variable", "")?)
                    .map_err(|_| SetupError::Invalid)?,
            ),
            "command" => Some(
                KeySource::command(
                    shlex::split(&prompt("Credential program and arguments", "")?)
                        .ok_or(SetupError::Invalid)?,
                )
                .map_err(|_| SetupError::Invalid)?,
            ),
            "none" => {
                connection.no_auth = Some(true);
                None
            }
            _ => return Err(SetupError::Invalid),
        };
    }
    let suggestions = recipe.as_ref().map_or(&[][..], |r| r.models);
    if interactive && !suggestions.is_empty() {
        let _ = writeln!(
            io::stdout(),
            "Suggested models: {} (any model ID is accepted)",
            suggestions.join(", ")
        );
    }
    let model = choose(
        options.model.clone(),
        "Model ID",
        suggestions.first().copied().unwrap_or(""),
        interactive,
    )?;
    if service == "cloudflare" {
        connection.wire_model = Some(model.rsplit('/').next().unwrap_or(&model).into());
    }
    let defaults = ModelSettings {
        connection: ConnectionSettings {
            wire_model: connection.wire_model.take(),
            ..Default::default()
        },
        ..Default::default()
    };
    let definition = ProviderDefinition {
        connection,
        defaults: ModelSettings::default(),
        protocol,
        base_url,
        api_key_env: None,
        max_attempts: None,
        attempt_timeout_seconds: None,
        models: std::collections::BTreeMap::from([(model.clone(), defaults)]),
    };
    definition.validate().map_err(|_| SetupError::Invalid)?;
    let default = options.default
        || (interactive
            && prompt(
                "Make startup default? (yes/no)",
                if read(&file)?.get("default_provider").is_none() {
                    "yes"
                } else {
                    "no"
                },
            )? == "yes");
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(file.parent().ok_or(SetupError::Invalid)?)?;
    let lock = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(nix::libc::O_NOFOLLOW)
        .open(file.with_extension("toml.lock"))?;
    lock.lock()?;
    let mut doc = read(&file)?;
    if doc
        .get("providers")
        .and_then(|providers| providers.get(&name))
        .is_some()
        && !options.replace
    {
        return Err(SetupError::Exists);
    }
    let provider = toml_edit::ser::to_document(&definition).map_err(|_| SetupError::Invalid)?;
    doc["providers"][&name] = Item::Table(provider.as_table().clone());
    if default {
        doc["default_provider"] = value(&name);
        doc["default_model"] = value(&model);
    }
    Config::parse(&doc.to_string()).map_err(|_| SetupError::Invalid)?;
    if let Some(key) = key {
        settings::auth::update(&settings::auth::path_for(&file), |store| {
            store.insert(name.clone(), AuthEntry::ApiKey(key));
            ((), true)
        })
        .map_err(|_| SetupError::Write)?;
    }
    publish(&file, &doc)?;
    Ok(ModelSelection {
        provider: Some(name),
        model: Some(model),
    })
}
pub(crate) fn run(args: &Args, options: &Connect) -> ExitCode {
    let output = (|| {
        let file = path(args)?;
        let config = Config::parse(&read(&file)?.to_string())?;
        let format =
            crate::render::resolve(&args.output, config.output, io::stdout().is_terminal())
                .map_err(|_| SetupError::Invalid)?;
        if format == crate::render::Format::Jsonl {
            return Err(SetupError::Invalid);
        }
        let selection = configure(args, options)?;
        Ok((selection, format))
    })();
    match output {
        Ok((selection, format)) => {
            let mut sink = io::stdout().lock();
            let written = if format == crate::render::Format::Text {
                writeln!(
                    sink,
                    "Configured {} / {}",
                    selection.provider.as_deref().unwrap_or_default(),
                    selection.model.as_deref().unwrap_or_default()
                )
            } else {
                crate::inspect::write("Connection", &serde_json::json!({"version":1,"configured":true,"provider":selection.provider,"model":selection.model}), &mut sink, format)
                    .map_err(|_| io::Error::other("output"))
            };
            if written.is_ok() {
                ExitCode::SUCCESS
            } else {
                ExitCode::from(6)
            }
        }
        Err(error) => crate::render_error(
            args,
            nouli_core::config::OutputFormat::Auto,
            nouli_core::protocol::ErrorReport {
                category: nouli_core::protocol::ErrorCategory::Configuration,
                code: "connection_setup_failed".into(),
                message: error.to_string(),
                retryable: false,
            },
        ),
    }
}

pub(crate) fn pick_model(
    settings: &settings::Settings,
    argument: &str,
) -> Result<ModelSelection, SetupError> {
    writeln!(
        io::stdout(),
        "Connections: {}",
        settings
            .config
            .providers
            .keys()
            .cloned()
            .collect::<Vec<_>>()
            .join(", ")
    )?;
    let provider = if argument.is_empty() {
        prompt(
            "Connection",
            settings.config.default_provider.as_deref().unwrap_or(""),
        )?
    } else {
        argument.into()
    };
    let definition = settings
        .config
        .providers
        .get(&provider)
        .ok_or(SetupError::Invalid)?;
    writeln!(
        io::stdout(),
        "Configured models: {}",
        definition
            .models
            .keys()
            .cloned()
            .collect::<Vec<_>>()
            .join(", ")
    )?;
    let suggestions = recipes()
        .into_iter()
        .filter(|recipe| recipe.protocol == definition.protocol)
        .flat_map(|recipe| recipe.models.iter().copied())
        .collect::<Vec<_>>();
    if !suggestions.is_empty() {
        writeln!(
            io::stdout(),
            "Examples for this protocol: {} (availability depends on your endpoint; any model ID is accepted)",
            suggestions.join(", ")
        )?;
    }
    let model = prompt(
        "Model ID",
        if settings.config.default_provider.as_deref() == Some(&provider) {
            settings.config.default_model.as_deref().unwrap_or("")
        } else {
            definition.models.keys().next().map_or("", String::as_str)
        },
    )?;
    if model.trim().is_empty() {
        return Err(SetupError::Invalid);
    }
    if prompt("Save as startup default? (yes/no)", "no")? == "yes" {
        use std::os::unix::fs::OpenOptionsExt;
        let file = &settings.config_path;
        let lock = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .custom_flags(nix::libc::O_NOFOLLOW)
            .open(file.with_extension("toml.lock"))?;
        lock.lock()?;
        let mut doc = read(file)?;
        doc["default_provider"] = value(&provider);
        doc["default_model"] = value(&model);
        Config::parse(&doc.to_string()).map_err(|_| SetupError::Invalid)?;
        publish(file, &doc)?;
    }
    Ok(ModelSelection {
        provider: Some(provider),
        model: Some(model),
    })
}

/// Complete terminal startup before a new session is allocated.
pub(crate) fn startup(
    settings: &mut settings::Settings,
    selection: &mut ModelSelection,
) -> Result<(), SetupError> {
    use nouli_core::config::resolve::RuntimeOverrides;
    if selection
        .provider
        .as_ref()
        .or(settings.config.default_provider.as_ref())
        .is_none()
        || selection
            .model
            .as_ref()
            .or(settings.config.default_model.as_ref())
            .is_none()
    {
        *selection = pick_model(settings, selection.provider.as_deref().unwrap_or(""))?;
    }
    let overrides = RuntimeOverrides {
        selection: selection.clone(),
        stored_credentials: settings.credentials.stored(),
        ..Default::default()
    };
    let snapshot = settings
        .config
        .resolve_snapshot(
            &overrides,
            &ModelSelection::default(),
            &ModelSelection::default(),
            &settings.home,
        )
        .map_err(|_| SetupError::Invalid)?;
    let needs_key = match snapshot
        .clone()
        .with_credentials(|source| settings.credentials.lookup(source))
    {
        Ok(_) => false,
        Err(nouli_core::config::ConfigError::CredentialSource {
            ref variable,
            ref source,
            ..
        }) if matches!(**source, nouli_core::config::ConfigError::MissingCredential)
            && settings
                .credentials
                .lookup(&nouli_core::config::auth::CredentialSource::Env(
                    variable.clone(),
                ))
                .is_none() =>
        {
            true
        }
        Err(nouli_core::config::ConfigError::StoredCredential { ref source, .. })
            if matches!(**source, nouli_core::config::ConfigError::MissingCredential) =>
        {
            true
        }
        Err(error) => return Err(error.into()),
    };
    if needs_key {
        writeln!(
            io::stdout(),
            "Authenticate connection {}",
            snapshot.profile_name
        )?;
        let key = match prompt("Authentication (key, env, command)", "key")?.as_str() {
            "key" => KeySource::Key(read_key("API key: ")?),
            "env" => KeySource::env(prompt("Environment variable", "")?)
                .map_err(|_| SetupError::Invalid)?,
            "command" => KeySource::command(
                shlex::split(&prompt("Credential program and arguments", "")?)
                    .ok_or(SetupError::Invalid)?,
            )
            .map_err(|_| SetupError::Invalid)?,
            _ => return Err(SetupError::Invalid),
        };
        settings::auth::update(&settings::auth::path_for(&settings.config_path), |store| {
            store.insert(snapshot.profile_name, AuthEntry::ApiKey(key));
            ((), true)
        })
        .map_err(|_| SetupError::Write)?;
        settings
            .load_environment()
            .map_err(|_| SetupError::Invalid)?;
        let overrides = RuntimeOverrides {
            stored_credentials: settings.credentials.stored(),
            ..overrides
        };
        settings
            .config
            .resolve(
                &overrides,
                &ModelSelection::default(),
                &ModelSelection::default(),
                &settings.home,
                |source| settings.credentials.lookup(source),
            )
            .map_err(|_| SetupError::Invalid)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;
    struct Directory(PathBuf);
    impl Directory {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!("nouli-connect-{}", Id::generate()));
            Self(path)
        }
        fn args(&self) -> Args {
            Args::try_parse_from([
                "nouli",
                "--config",
                self.0.join("config.toml").to_str().unwrap(),
            ])
            .unwrap()
        }
    }
    impl Drop for Directory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    fn custom(name: &str) -> Connect {
        Connect {
            service: Some("custom".into()),
            name: Some(name.into()),
            protocol: Some("system-one".into()),
            base_url: Some("https://arbitrary.example/prefix".into()),
            model: Some("user/model.v2".into()),
            no_auth: true,
            default: true,
            non_interactive: true,
            ..Default::default()
        }
    }
    #[test]
    fn missing_header_credentials_do_not_trigger_primary_key_setup_or_writes() {
        let dir = Directory::new();
        configure(&dir.args(), &custom("gateway")).unwrap();
        let mut settings = settings::Settings::for_test(dir.0.clone());
        settings.config_path = dir.0.join("config.toml");
        settings.config =
            Config::parse(&fs::read_to_string(&settings.config_path).unwrap()).unwrap();
        let definition = settings.config.providers.get_mut("gateway").unwrap();
        definition.api_key_env = Some("PRIMARY_MISSING".into());
        definition.connection.no_auth = Some(false);
        definition
            .connection
            .headers_env
            .insert("X-Gateway-Key".into(), "HEADER_MISSING".into());
        assert!(
            matches!(startup(&mut settings, &mut ModelSelection::default()),
            Err(SetupError::Configuration(nouli_core::config::ConfigError::HeaderCredential { variable })) if variable == "HEADER_MISSING")
        );
        assert!(!dir.0.join("auth.toml").exists());
    }
    #[test]
    fn generated_config_is_ordinary_config_and_preserves_unrelated_edits() {
        let dir = Directory::new();
        fs::create_dir_all(&dir.0).unwrap();
        fs::write(
            dir.0.join("config.toml"),
            "# Keep my comment\noutput = 'json'\n[execution]\nmax_parallel = 7 # keep this too\n",
        )
        .unwrap();
        configure(&dir.args(), &custom("my gateway")).unwrap();
        let text = fs::read_to_string(dir.0.join("config.toml")).unwrap();
        assert!(text.contains("# Keep my comment"));
        assert!(text.contains("# keep this too"));
        let generated = Config::parse(&text).unwrap();
        let hand = Config::parse(
            r#"
default_provider = "my gateway"
default_model = "user/model.v2"
output = "json"
[execution]
max_parallel = 7
[providers."my gateway"]
protocol = "system-one"
base_url = "https://arbitrary.example/prefix"
[providers."my gateway".connection]
no_auth = true
[providers."my gateway".models."user/model.v2"]
"#,
        )
        .unwrap();
        assert_eq!(generated, hand);
        let before = text;
        assert!(matches!(
            configure(&dir.args(), &custom("my gateway")),
            Err(SetupError::Exists)
        ));
        assert_eq!(
            fs::read_to_string(dir.0.join("config.toml")).unwrap(),
            before
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(dir.0.join("config.toml"))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
    }
    #[test]
    fn invalid_draft_creates_no_files_and_command_accepts_flags() {
        let dir = Directory::new();
        let mut draft = custom("broken");
        draft.base_url = Some("https://user:secret@example.com".into());
        assert!(configure(&dir.args(), &draft).is_err());
        assert!(!dir.0.exists());
        let parsed = Args::try_parse_from([
            "nouli",
            "connect",
            "custom",
            "--command",
            "op",
            "read",
            "--no-newline",
            "op://key",
        ])
        .unwrap();
        let crate::commands::Command::Connect(connect) = parsed.command.unwrap() else {
            panic!("connect")
        };
        assert_eq!(connect.command, ["op", "read", "--no-newline", "op://key"]);
    }
    #[test]
    fn concurrent_setup_retains_both_connections() {
        let dir = Directory::new();
        std::thread::scope(|scope| {
            for name in ["one", "two"] {
                let args = dir.args();
                scope.spawn(move || configure(&args, &custom(name)).unwrap());
            }
        });
        let config =
            Config::parse(&fs::read_to_string(dir.0.join("config.toml")).unwrap()).unwrap();
        assert_eq!(config.providers.len(), 2);
    }
    #[test]
    fn cloudflare_wire_model_is_an_editable_exact_model_setting() {
        let dir = Directory::new();
        let draft = Connect {
            service: Some("cloudflare".into()),
            name: Some("edge".into()),
            base_url: Some("https://edge.example/accounts/demo/ai".into()),
            model: Some("@cf/cloudflare/clef".into()),
            env: Some("CF_TOKEN".into()),
            non_interactive: true,
            ..Default::default()
        };
        configure(&dir.args(), &draft).unwrap();
        let config =
            Config::parse(&fs::read_to_string(dir.0.join("config.toml")).unwrap()).unwrap();
        let definition = &config.providers["edge"];
        assert_eq!(definition.connection.wire_model, None);
        assert_eq!(
            definition.models["@cf/cloudflare/clef"]
                .connection
                .wire_model
                .as_deref(),
            Some("clef")
        );
        assert!(
            !fs::read_to_string(dir.0.join("config.toml"))
                .unwrap()
                .contains("CF_TOKEN")
        );
    }
}
