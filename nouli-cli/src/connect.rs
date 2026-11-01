//! CLI-owned connection recipes and configuration authoring. Execution never reads recipes.
mod prompt;
mod recipes;
mod startup;
use crate::{
    Args,
    settings::{
        self,
        config_file::{Draft, has_provider, path, read, save},
    },
};
use clap::ValueEnum;
use nouli_core::{
    config::{
        Config, ConnectionSettings, HttpBinding, ModelSettings, Protocol, ProviderDefinition,
    },
    history::ModelSelection,
};
pub(crate) use prompt::read_key;
use prompt::{
    choose, choose_credential, choose_endpoint, choose_model, choose_protocol, prompt_default,
};
use recipes::{Recipe, SERVICES, recipes};
pub(crate) use startup::{pick_model, startup};
use std::{
    io::{self, IsTerminal, Write},
    path::Path,
    process::ExitCode,
};
use thiserror::Error;

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
    #[error("Unknown service {0}. Choose one of: {SERVICES}")]
    UnknownService(String),
    #[error("A connection name cannot be empty")]
    InvalidName,
    #[error("Unknown protocol. Choose system-one or openai-decisions")]
    InvalidProtocol,
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
            Self::UnknownService(_) => (Category::Input, "unknown_service"),
            Self::InvalidName => (Category::Input, "invalid_connection_name"),
            Self::InvalidProtocol => (Category::Input, "invalid_protocol"),
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
#[derive(Debug, Clone, Copy, ValueEnum)]
pub(crate) enum ProtocolArg {
    SystemOne,
    OpenaiDecisions,
}
impl From<ProtocolArg> for Protocol {
    fn from(value: ProtocolArg) -> Self {
        match value {
            ProtocolArg::SystemOne => Self::SystemOne,
            ProtocolArg::OpenaiDecisions => Self::OpenaiDecisions,
        }
    }
}
#[derive(Debug, Clone, Copy, ValueEnum)]
pub(crate) enum BindingArg {
    Standard,
    CloudflareWorkersAi,
}
impl From<BindingArg> for HttpBinding {
    fn from(value: BindingArg) -> Self {
        match value {
            BindingArg::Standard => Self::Standard,
            BindingArg::CloudflareWorkersAi => Self::CloudflareWorkersAi,
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
    #[arg(long, value_enum)]
    pub protocol: Option<ProtocolArg>,
    #[arg(long, value_enum)]
    pub binding: Option<BindingArg>,
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

pub(crate) fn configure(file: &Path, options: &Connect) -> Result<ModelSelection, SetupError> {
    let interactive =
        !options.non_interactive && io::stdin().is_terminal() && io::stdout().is_terminal();
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
    if !options.replace && has_provider(&read(file)?, &name) {
        return Err(SetupError::Exists);
    }
    let protocol = choose_protocol(options, recipe, interactive)?;
    let mut connection = recipe.map(|r| r.connection.clone()).unwrap_or_default();
    if let Some(binding) = options.binding {
        connection.binding = Some(binding.into());
    }
    let base_url = choose_endpoint(options, &service, recipe, interactive)?;
    let key = choose_credential(options, &mut connection, interactive)?;
    let model = choose_model(options, recipe, interactive)?;
    if service == "cloudflare" {
        connection.wire_model = Some(model.rsplit('/').next().unwrap_or(&model).into());
    }
    let definition = provider_definition(connection, protocol, base_url, &model)?;
    let default = options.default || (interactive && prompt_default(file)?);
    save(
        file,
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
        let selection = configure(&file, options)?;
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
                .map_err(crate::render::RenderError::from)
            } else {
                crate::render::labelled(
                    "Connection",
                    &serde_json::json!({"version":1,"configured":true,"provider":selection.provider,"model":selection.model}),
                    &mut sink,
                    format,
                )
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

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use clap::Parser;
    use nouli_core::identity::Id;
    pub(crate) use std::{fs, path::PathBuf};
    pub(crate) struct Directory(pub(crate) PathBuf);
    impl Directory {
        pub(crate) fn new() -> Self {
            let path = std::env::temp_dir().join(format!("nouli-connect-{}", Id::generate()));
            Self(path)
        }
        pub(crate) fn args(&self) -> Args {
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
    pub(crate) fn custom(name: &str) -> Connect {
        Connect {
            service: Some("custom".into()),
            name: Some(name.into()),
            protocol: Some(ProtocolArg::SystemOne),
            base_url: Some("https://arbitrary.example/prefix".into()),
            model: Some("user/model.v2".into()),
            no_auth: true,
            default: true,
            non_interactive: true,
            ..Default::default()
        }
    }
    #[test]
    fn invalid_draft_creates_no_files_and_command_accepts_flags() {
        let dir = Directory::new();
        let mut draft = custom("broken");
        draft.base_url = Some("https://user:secret@example.com".into());
        assert!(configure(&dir.0.join("config.toml"), &draft).is_err());
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
}
