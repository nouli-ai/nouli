//! CLI-owned filesystem/environment discovery. Loading has no ledger side effects.

mod bootstrap;
mod environment;
pub(crate) mod preflight;

use std::{path::PathBuf, sync::Arc};

use nouli_core::config::{Config, ConfigError};

#[derive(Clone)]
pub(crate) struct Settings {
    pub config: Config,
    pub home: PathBuf,
    pub store: PathBuf,
    pub config_path: PathBuf,
    pub environment: Arc<environment::Environment>,
    env_file: Option<PathBuf>,
    initialize_default: bool,
    store_override: Option<PathBuf>,
}
pub(crate) fn load(args: &super::Args) -> Result<Settings, ConfigError> {
    discover(args, Config::load)
}
pub(crate) fn load_inspection(args: &super::Args) -> Result<Settings, ConfigError> {
    discover(args, |path, optional| {
        nouli_core::config::StoreSettings::load(path, optional).map(|settings| Config {
            output: settings.output,
            store: settings.store,
            ..Default::default()
        })
    })
}
fn discover(
    args: &super::Args,
    loader: impl FnOnce(&std::path::Path, bool) -> Result<Config, ConfigError>,
) -> Result<Settings, ConfigError> {
    let home = std::env::var_os("HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from);
    let path = args
        .config
        .clone()
        .or_else(|| home.as_ref().map(|home| home.join(".nouli/config.toml")))
        .ok_or(ConfigError::Invalid("HOME or explicit --config"))?;
    let config = loader(&path, args.config.is_none())?;
    if home.is_none() && args.store.is_none() && config.store.is_none() {
        return Err(ConfigError::Invalid("HOME or explicit store"));
    }
    let home = home.unwrap_or_default();
    let store = config.store_path(&home, args.store.as_deref())?;
    Ok(Settings {
        config,
        home,
        store,
        config_path: path,
        environment: Arc::default(),
        env_file: args.env_file.clone(),
        initialize_default: args.config.is_none(),
        store_override: args.store.clone(),
    })
}

impl Settings {
    pub(crate) fn load_environment(&mut self) -> Result<(), nouli_core::protocol::ErrorReport> {
        self.environment = Arc::new(
            environment::Environment::load(self.env_file.as_deref())
                .map_err(|error| error.report())?,
        );
        Ok(())
    }
    pub(crate) fn initialize(&mut self) -> Result<(), nouli_core::protocol::ErrorReport> {
        if self.initialize_default {
            self.config = bootstrap::initialize(&self.config_path).map_err(|error| match error {
                bootstrap::BootstrapError::Config(error) => self.config_error(error),
                bootstrap::BootstrapError::Io(_) => nouli_core::protocol::ErrorReport {
                    category: nouli_core::protocol::ErrorCategory::Configuration,
                    code: "configuration_create_failed".into(),
                    message: format!("Cannot create starter configuration at {}. Check directory permissions or use --config FILE", self.config_path.display()),
                    retryable: false,
                },
            })?;
            self.store = self
                .config
                .store_path(&self.home, self.store_override.as_deref())
                .map_err(|error| self.config_error(error))?;
        }
        Ok(())
    }
    pub(crate) fn config_error(&self, error: ConfigError) -> nouli_core::protocol::ErrorReport {
        let mut report = nouli_core::engine::EngineError::Config(error).report();
        self.annotate(&mut report);
        report
    }
    pub(crate) fn annotate(&self, report: &mut nouli_core::protocol::ErrorReport) {
        annotate_report(report, &self.config_path);
    }
}

pub(crate) fn error(args: &super::Args, error: ConfigError) -> nouli_core::protocol::ErrorReport {
    let mut report = nouli_core::engine::EngineError::Config(error).report();
    let path = args.config.clone().or_else(|| {
        std::env::var_os("HOME")
            .filter(|value| !value.is_empty())
            .map(|home| PathBuf::from(home).join(".nouli/config.toml"))
    });
    if let Some(path) = path {
        report
            .message
            .push_str(&format!(". Configuration: {}", path.display()));
    }
    report
}

pub(crate) fn annotate_report(
    report: &mut nouli_core::protocol::ErrorReport,
    path: &std::path::Path,
) {
    use nouli_core::protocol::ErrorCategory;
    if report.category == ErrorCategory::Configuration
        || (report.category == ErrorCategory::Authentication
            && report.code == "credential_unavailable")
    {
        let suffix = format!(". Configuration: {}", path.display());
        if !report.message.ends_with(&suffix) {
            report.message.push_str(&suffix);
        }
    }
}
pub(crate) fn annotate_envelope(
    envelope: &mut nouli_core::protocol::ResultEnvelope,
    path: &std::path::Path,
) {
    use nouli_core::protocol::Report;
    match &mut envelope.result {
        Report::Error(report) => annotate_report(report, path),
        Report::Run(report) => {
            for node in &mut report.nodes {
                if let Some(error) = &mut node.error {
                    annotate_report(error, path);
                }
            }
        }
        Report::Turn(_) | Report::Session(_) => {}
    }
}
