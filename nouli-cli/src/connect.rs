//! CLI-owned connection recipes and configuration authoring. Execution never reads recipes.
use crate::{Args, settings};
use nouli_core::{
    config::{
        Config, ConnectionSettings, HttpBinding, ImageEncoding, ModelSettings, Protocol,
        ProviderDefinition,
        auth::{AuthEntry, KeySource},
    },
    history::ModelSelection,
};
use std::{
    fs,
    io::{self, IsTerminal, Write},
    path::{Path, PathBuf},
    process::ExitCode,
};
use thiserror::Error;
use toml_edit::{DocumentMut, Item, value};
/// The services `nouli connect` has recipes for, besides `custom`.
const SERVICES: &str = "openai, typesafe, openrouter, cloudflare, ollama, llama.cpp, custom";
#[derive(Debug, Error)]
pub(crate) enum SetupError {
    #[error(transparent)]
    Configuration(#[from] nouli_core::config::ConfigError),
    #[error("connection setup canceled")]
    Cancelled,
    #[error("connection setup needs a terminal or complete noninteractive arguments")]
    Terminal,
    #[error("Set HOME or pass --config FILE to choose a configuration file")]
    HomeUnavailable,
    #[error("The configuration path is not valid UTF-8")]
    ConfigPath,
    #[error("Unknown service {0}. Choose one of: {SERVICES}")]
    UnknownService(String),
    #[error("A connection name cannot be empty")]
    InvalidName,
    #[error("Unknown protocol. Choose system-one or openai-decisions")]
    InvalidProtocol,
    #[error("Unknown binding. Choose standard or cloudflare-workers-ai")]
    InvalidBinding,
    #[error("A Cloudflare account ID must be letters and digits only")]
    InvalidAccountId,
    #[error("Unknown authentication method. Choose one of the listed methods")]
    InvalidCredentialSource,
    #[error("That is not a valid environment variable name")]
    InvalidEnvironmentVariable,
    #[error("A command source needs a program name and no empty words")]
    InvalidCommand,
    #[error("A key cannot be empty or contain control characters")]
    InvalidKey,
    #[error("A model ID cannot be empty")]
    InvalidModel,
    #[error("Name a connection, or select a model first")]
    ConnectionRequired,
    #[error("Unknown setup command")]
    UnknownCommand,
    #[error("invalid connection settings: {0}")]
    InvalidConnection(#[source] nouli_core::config::ConfigError),
    #[error("connection already exists; choose another name or use --replace")]
    Exists,
    #[error("cannot update configuration; any stored credential can be reused by repeating setup")]
    Write,
    #[error("cannot encode the connection settings")]
    Encode,
    #[error(transparent)]
    Credential(#[from] settings::auth::AuthFileError),
    #[error(transparent)]
    Key(#[from] crate::auth::KeyLineError),
    #[error(transparent)]
    Io(#[from] io::Error),
    #[error("{}", .0.message)]
    Report(nouli_core::protocol::ErrorReport),
}
impl SetupError {
    /// The error report for this failure; its category sets the exit code.
    pub(crate) fn report(self) -> nouli_core::protocol::ErrorReport {
        use nouli_core::protocol::{ErrorCategory as Category, ErrorReport};
        let (category, code) = match self {
            Self::Configuration(error) => {
                return nouli_core::engine::EngineError::Config(error).report();
            }
            Self::Credential(error) => return error.report(),
            Self::Key(error) => return error.report(),
            Self::Report(report) => return report,
            Self::Cancelled => (Category::Interrupted, "setup_canceled"),
            Self::Terminal => (Category::Input, "terminal_required"),
            Self::HomeUnavailable => (Category::Configuration, "home_unavailable"),
            Self::ConfigPath => (Category::Configuration, "invalid_configuration_path"),
            Self::UnknownService(_) => (Category::Input, "unknown_service"),
            Self::InvalidName => (Category::Input, "invalid_connection_name"),
            Self::InvalidProtocol => (Category::Input, "invalid_protocol"),
            Self::InvalidBinding => (Category::Input, "invalid_binding"),
            Self::InvalidAccountId => (Category::Input, "invalid_account_id"),
            Self::InvalidCredentialSource => (Category::Input, "invalid_credential_source"),
            Self::InvalidEnvironmentVariable => (Category::Input, "invalid_environment_variable"),
            Self::InvalidCommand => (Category::Input, "invalid_command"),
            Self::InvalidKey => (Category::Input, "invalid_credential"),
            Self::InvalidModel => (Category::Input, "invalid_model"),
            Self::ConnectionRequired => (Category::Input, "connection_required"),
            Self::UnknownCommand => (Category::Input, "unknown_command"),
            Self::InvalidConnection(_) => (Category::Input, "invalid_connection"),
            Self::Exists => (Category::Input, "connection_exists"),
            Self::Write => (Category::Storage, "configuration_write_failed"),
            Self::Encode => (Category::Internal, "connection_encode_failed"),
            Self::Io(_) => (Category::Storage, "setup_io_failed"),
        };
        ErrorReport {
            category,
            code: code.into(),
            message: self.to_string(),
            retryable: false,
        }
    }
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
    nouli_core::config::auth::Secret::new(value).map_err(|_| SetupError::InvalidKey)
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
        .ok_or(SetupError::HomeUnavailable)
}
fn read(path: &Path) -> Result<DocumentMut, SetupError> {
    match fs::read_to_string(path) {
        Ok(s) => {
            Config::parse(&s)?;
            s.parse()
                .map_err(|_| SetupError::Configuration(nouli_core::config::ConfigError::Syntax))
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(DocumentMut::new()),
        Err(source) => Err(nouli_core::config::ConfigError::Read {
            path: path.to_owned(),
            source,
        }
        .into()),
    }
}
/// Replaces the configuration atomically.
fn publish(path: &Path, doc: &DocumentMut) -> Result<(), SetupError> {
    settings::private_file::publish(path, doc.to_string().as_bytes()).map_err(|_| SetupError::Write)
}
/// A validated connection, ready to be written to the configuration.
struct Draft {
    name: String,
    model: String,
    definition: ProviderDefinition,
    key: Option<KeySource>,
}
pub(crate) fn configure(args: &Args, options: &Connect) -> Result<ModelSelection, SetupError> {
    let interactive =
        !options.non_interactive && io::stdin().is_terminal() && io::stdout().is_terminal();
    let file = path(args)?;
    if interactive {
        let _ = writeln!(
            io::stdout(),
            "Connect a decision service. Enter /cancel to cancel.\nServices: {SERVICES}"
        );
    }
    let service = choose(options.service.clone(), "Service", "", interactive)?;
    let recipe = recipes().into_iter().find(|r| r.name == service);
    if recipe.is_none() && service != "custom" {
        return Err(SetupError::UnknownService(service));
    }
    let recipe = recipe.as_ref();
    let name = choose(
        options
            .name
            .clone()
            .or_else(|| recipe.map(|_| service.clone())),
        "Connection name",
        &service,
        interactive,
    )?;
    if name.trim().is_empty() {
        return Err(SetupError::InvalidName);
    }
    if !options.replace && has_provider(&read(&file)?, &name) {
        return Err(SetupError::Exists);
    }
    let protocol = choose_protocol(options, recipe, interactive)?;
    let mut connection = recipe.map(|r| r.connection.clone()).unwrap_or_default();
    if let Some(binding) = &options.binding {
        connection.binding = Some(parse_binding(binding)?);
    }
    let base_url = choose_endpoint(options, &service, recipe, interactive)?;
    let key = choose_credential(options, &mut connection, interactive)?;
    let model = choose_model(options, recipe, interactive)?;
    if service == "cloudflare" {
        connection.wire_model = Some(model.rsplit('/').next().unwrap_or(&model).into());
    }
    let definition = provider_definition(connection, protocol, base_url, &model)?;
    let default = options.default || (interactive && prompt_default(&file)?);
    save(
        &file,
        Draft {
            name,
            model,
            definition,
            key,
        },
        default,
        options.replace,
    )
}
fn has_provider(doc: &DocumentMut, name: &str) -> bool {
    doc.get("providers")
        .and_then(|providers| providers.get(name))
        .is_some()
}
fn protocol_name(protocol: Protocol) -> &'static str {
    if protocol == Protocol::SystemOne {
        "system-one"
    } else {
        "openai-decisions"
    }
}
fn choose_protocol(
    options: &Connect,
    recipe: Option<&Recipe>,
    interactive: bool,
) -> Result<Protocol, SetupError> {
    let suggested = recipe.map(|r| protocol_name(r.protocol));
    let chosen = choose(
        options
            .protocol
            .clone()
            .or_else(|| suggested.map(Into::into)),
        "Protocol (system-one or openai-decisions)",
        suggested.unwrap_or(""),
        interactive,
    )?;
    match chosen.as_str() {
        "system-one" => Ok(Protocol::SystemOne),
        "openai-decisions" => Ok(Protocol::OpenaiDecisions),
        _ => Err(SetupError::InvalidProtocol),
    }
}
fn parse_binding(binding: &str) -> Result<HttpBinding, SetupError> {
    match binding {
        "standard" => Ok(HttpBinding::Standard),
        "cloudflare-workers-ai" => Ok(HttpBinding::CloudflareWorkersAi),
        _ => Err(SetupError::InvalidBinding),
    }
}
/// The endpoint from the options or recipe. Interactive Cloudflare setup
/// builds it from the account ID, since its recipe has no fixed endpoint.
fn choose_endpoint(
    options: &Connect,
    service: &str,
    recipe: Option<&Recipe>,
    interactive: bool,
) -> Result<String, SetupError> {
    let mut url = options
        .base_url
        .clone()
        .unwrap_or_else(|| recipe.map_or("", |r| r.url).into());
    if url.is_empty() && service == "cloudflare" && interactive {
        let account = prompt("Cloudflare account ID", "")?;
        if !account.bytes().all(|b| b.is_ascii_alphanumeric()) || account.is_empty() {
            return Err(SetupError::InvalidAccountId);
        }
        url = format!("https://api.cloudflare.com/client/v4/accounts/{account}/ai");
    }
    choose(
        if url.is_empty() { None } else { Some(url) },
        "Endpoint",
        "",
        interactive,
    )
}
/// The credential source from the options, or from a prompt when the
/// connection needs one. Records the authentication choice on `connection`.
fn choose_credential(
    options: &Connect,
    connection: &mut ConnectionSettings,
    interactive: bool,
) -> Result<Option<KeySource>, SetupError> {
    if options.no_auth {
        connection.no_auth = Some(true);
        return Ok(None);
    }
    let key = if let Some(env) = &options.env {
        KeySource::env(env.clone()).map_err(|_| SetupError::InvalidEnvironmentVariable)?
    } else if !options.command.is_empty() {
        KeySource::command(options.command.clone()).map_err(|_| SetupError::InvalidCommand)?
    } else if options.stdin {
        let key = crate::auth::key_line(&mut io::stdin().lock())?;
        KeySource::Key(
            nouli_core::config::auth::Secret::new(key).map_err(|_| SetupError::InvalidKey)?,
        )
    } else if connection.no_auth != Some(true) {
        if !interactive {
            return Err(SetupError::Terminal);
        }
        return prompt_credential(connection);
    } else {
        return Ok(None);
    };
    connection.no_auth = Some(false);
    Ok(Some(key))
}
fn prompt_credential(connection: &mut ConnectionSettings) -> Result<Option<KeySource>, SetupError> {
    let method = prompt("Authentication (key, env, command, none)", "key")?;
    if method == "none" {
        connection.no_auth = Some(true);
        return Ok(None);
    }
    prompted_source(&method).map(Some)
}
/// Prompts for the credential source `method` names.
fn prompted_source(method: &str) -> Result<KeySource, SetupError> {
    match method {
        "key" => Ok(KeySource::Key(read_key("API key: ")?)),
        "env" => KeySource::env(prompt("Environment variable", "")?)
            .map_err(|_| SetupError::InvalidEnvironmentVariable),
        "command" => KeySource::command(
            shlex::split(&prompt("Credential program and arguments", "")?)
                .ok_or(SetupError::InvalidCommand)?,
        )
        .map_err(|_| SetupError::InvalidCommand),
        _ => Err(SetupError::InvalidCredentialSource),
    }
}
fn choose_model(
    options: &Connect,
    recipe: Option<&Recipe>,
    interactive: bool,
) -> Result<String, SetupError> {
    let suggestions = recipe.map_or(&[][..], |r| r.models);
    if interactive && !suggestions.is_empty() {
        let _ = writeln!(
            io::stdout(),
            "Suggested models: {} (any model ID is accepted)",
            suggestions.join(", ")
        );
    }
    choose(
        options.model.clone(),
        "Model ID",
        suggestions.first().copied().unwrap_or(""),
        interactive,
    )
}
/// A provider with one model. The connection's wire model moves to that
/// model's settings so it can be edited per model.
fn provider_definition(
    mut connection: ConnectionSettings,
    protocol: Protocol,
    base_url: String,
    model: &str,
) -> Result<ProviderDefinition, SetupError> {
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
        models: std::collections::BTreeMap::from([(model.to_owned(), defaults)]),
    };
    definition
        .validate()
        .map_err(SetupError::InvalidConnection)?;
    Ok(definition)
}
fn prompt_default(file: &Path) -> Result<bool, SetupError> {
    let suggested = if read(file)?.get("default_provider").is_none() {
        "yes"
    } else {
        "no"
    };
    Ok(prompt("Make startup default? (yes/no)", suggested)? == "yes")
}
/// Opens and locks the configuration's lock file, creating its directory.
fn lock_config(file: &Path) -> Result<fs::File, SetupError> {
    settings::private_file::lock_beside(file).map_err(|_| SetupError::Write)
}
/// Writes the draft under the configuration lock, storing its credential
/// before publishing the configuration.
fn save(
    file: &Path,
    draft: Draft,
    default: bool,
    replace: bool,
) -> Result<ModelSelection, SetupError> {
    let _lock = lock_config(file)?;
    let mut doc = read(file)?;
    if has_provider(&doc, &draft.name) && !replace {
        return Err(SetupError::Exists);
    }
    let provider =
        toml_edit::ser::to_document(&draft.definition).map_err(|_| SetupError::Encode)?;
    doc["providers"][&draft.name] = Item::Table(provider.as_table().clone());
    if default {
        doc["default_provider"] = value(&draft.name);
        doc["default_model"] = value(&draft.model);
    }
    Config::parse(&doc.to_string())?;
    if let Some(key) = draft.key {
        settings::auth::update(&settings::auth::path_for(file), |store| {
            store.insert(draft.name.clone(), AuthEntry::ApiKey(key));
            ((), true)
        })?;
    }
    publish(file, &doc)?;
    Ok(ModelSelection {
        provider: Some(draft.name),
        model: Some(draft.model),
    })
}
pub(crate) fn run(args: &Args, options: &Connect) -> ExitCode {
    let output = (|| -> Result<_, SetupError> {
        let file = path(args)?;
        let config = Config::parse(&read(&file)?.to_string())?;
        let format = crate::render::resolve_report_format(
            &args.output,
            config.output,
            io::stdout().is_terminal(),
        )
        .map_err(SetupError::Report)?;
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
        Err(error) => {
            let mut report = error.report();
            if let Ok(file) = path(args) {
                settings::annotate_report(&mut report, &file);
            }
            crate::render_error(args, nouli_core::config::OutputFormat::Auto, report)
        }
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
        .ok_or(nouli_core::config::ConfigError::UnknownProvider)?;
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
        return Err(SetupError::InvalidModel);
    }
    if prompt("Save as startup default? (yes/no)", "no")? == "yes" {
        let file = &settings.config_path;
        let _lock = lock_config(file)?;
        let mut doc = read(file)?;
        doc["default_provider"] = value(&provider);
        doc["default_model"] = value(&model);
        Config::parse(&doc.to_string())?;
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
    let snapshot = settings.config.resolve_snapshot(
        &overrides,
        &ModelSelection::default(),
        &ModelSelection::default(),
        &settings.home,
    )?;
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
        let key = prompted_source(&prompt("Authentication (key, env, command)", "key")?)?;
        settings::auth::update(&settings::auth::path_for(&settings.config_path), |store| {
            store.insert(snapshot.profile_name, AuthEntry::ApiKey(key));
            ((), true)
        })?;
        settings.load_environment().map_err(SetupError::Report)?;
        let overrides = RuntimeOverrides {
            stored_credentials: settings.credentials.stored(),
            ..overrides
        };
        settings.config.resolve(
            &overrides,
            &ModelSelection::default(),
            &ModelSelection::default(),
            &settings.home,
            |source| settings.credentials.lookup(source),
        )?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;
    use nouli_core::identity::Id;
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
    #[expect(clippy::too_many_lines, reason = "one table row per setup error")]
    fn setup_errors_report_specific_codes_and_categories() {
        use nouli_core::{
            config::ConfigError,
            protocol::{ErrorCategory as Category, ErrorReport},
        };
        let carried = ErrorReport {
            category: Category::Authentication,
            code: "carried".into(),
            message: "carried report".into(),
            retryable: false,
        };
        for (error, category, code) in [
            (
                SetupError::Configuration(ConfigError::UnknownProvider),
                Category::Configuration,
                "unknown_provider",
            ),
            (
                SetupError::Cancelled,
                Category::Interrupted,
                "setup_canceled",
            ),
            (SetupError::Terminal, Category::Input, "terminal_required"),
            (
                SetupError::HomeUnavailable,
                Category::Configuration,
                "home_unavailable",
            ),
            (
                SetupError::ConfigPath,
                Category::Configuration,
                "invalid_configuration_path",
            ),
            (
                SetupError::UnknownService("nowhere".into()),
                Category::Input,
                "unknown_service",
            ),
            (
                SetupError::InvalidName,
                Category::Input,
                "invalid_connection_name",
            ),
            (
                SetupError::InvalidProtocol,
                Category::Input,
                "invalid_protocol",
            ),
            (
                SetupError::InvalidBinding,
                Category::Input,
                "invalid_binding",
            ),
            (
                SetupError::InvalidAccountId,
                Category::Input,
                "invalid_account_id",
            ),
            (
                SetupError::InvalidCredentialSource,
                Category::Input,
                "invalid_credential_source",
            ),
            (
                SetupError::InvalidEnvironmentVariable,
                Category::Input,
                "invalid_environment_variable",
            ),
            (
                SetupError::InvalidCommand,
                Category::Input,
                "invalid_command",
            ),
            (
                SetupError::InvalidKey,
                Category::Input,
                "invalid_credential",
            ),
            (SetupError::InvalidModel, Category::Input, "invalid_model"),
            (
                SetupError::ConnectionRequired,
                Category::Input,
                "connection_required",
            ),
            (
                SetupError::UnknownCommand,
                Category::Input,
                "unknown_command",
            ),
            (
                SetupError::InvalidConnection(ConfigError::Invalid("base_url")),
                Category::Input,
                "invalid_connection",
            ),
            (SetupError::Exists, Category::Input, "connection_exists"),
            (
                SetupError::Write,
                Category::Storage,
                "configuration_write_failed",
            ),
            (
                SetupError::Encode,
                Category::Internal,
                "connection_encode_failed",
            ),
            (
                SetupError::Credential(settings::auth::AuthFileError::Write {
                    path: "auth.toml".into(),
                }),
                Category::Storage,
                "auth_store_write_failed",
            ),
            (
                SetupError::Key(crate::auth::KeyLineError::TooLong),
                Category::Input,
                "credential_too_long",
            ),
            (
                SetupError::Io(io::Error::other("closed")),
                Category::Storage,
                "setup_io_failed",
            ),
            (
                SetupError::Report(carried.clone()),
                carried.category,
                "carried",
            ),
        ] {
            let message = error.to_string();
            let report = error.report();
            assert_eq!((report.category, report.code.as_str()), (category, code));
            assert!(!report.retryable);
            if code != "unknown_provider" {
                assert_eq!(report.message, message);
            }
        }
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
