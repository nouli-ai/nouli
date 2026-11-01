//! CLI-owned filesystem/environment discovery. Loading has no ledger side effects.

pub(crate) mod auth;
mod bootstrap;
mod environment;
pub(crate) mod preflight;
pub(crate) mod private_file;

use std::fmt::Write as _;
use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use nouli_core::config::{Config, ConfigError};

#[derive(Clone)]
pub(crate) struct Settings {
    pub config: Config,
    pub home: PathBuf,
    pub store: PathBuf,
    pub config_path: PathBuf,
    pub credentials: Arc<auth::Credentials>,
    pub(crate) env_file: Option<PathBuf>,
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
    discover_paths(
        args.config.as_deref(),
        args.env_file.as_deref(),
        args.store.as_deref(),
        loader,
    )
}

fn discover_paths(
    config_path: Option<&Path>,
    env_file: Option<&Path>,
    store_override: Option<&Path>,
    loader: impl FnOnce(&Path, bool) -> Result<Config, ConfigError>,
) -> Result<Settings, ConfigError> {
    let home = std::env::var_os("HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from);
    let path = config_path
        .map(Path::to_owned)
        .or_else(|| home.as_ref().map(|home| home.join(".nouli/config.toml")))
        .ok_or(ConfigError::Invalid("HOME or explicit --config"))?;
    let config = loader(&path, config_path.is_none())?;
    if home.is_none() && store_override.is_none() && config.store.is_none() {
        return Err(ConfigError::Invalid("HOME or explicit store"));
    }
    let home = home.unwrap_or_default();
    let store = config.store_path(&home, store_override)?;
    Ok(Settings {
        config,
        home,
        store,
        config_path: path,
        credentials: Arc::default(),
        env_file: env_file.map(Path::to_owned),
        initialize_default: config_path.is_none(),
        store_override: store_override.map(Path::to_owned),
    })
}

impl Settings {
    pub(crate) fn reload(&self) -> Result<Self, ConfigError> {
        discover_paths(
            Some(&self.config_path),
            self.env_file.as_deref(),
            Some(&self.store),
            Config::load,
        )
    }

    pub(crate) fn load_environment(&mut self) -> Result<(), nouli_core::protocol::ErrorReport> {
        let environment = environment::Environment::load(self.env_file.as_deref())
            .map_err(|error| error.report())?;
        let store =
            auth::read(&auth::path_for(&self.config_path)).map_err(|error| error.report())?;
        self.credentials = Arc::new(auth::Credentials::new(environment, store));
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
        let _ = write!(report.message, ". Configuration: {}", path.display());
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

#[cfg(test)]
impl Settings {
    /// Settings over a store path, for tests that never load a file.
    pub(crate) fn for_test(store: PathBuf) -> Self {
        Self {
            config: Config::default(),
            home: PathBuf::new(),
            store,
            config_path: PathBuf::new(),
            credentials: Arc::default(),
            env_file: None,
            initialize_default: false,
            store_override: None,
        }
    }
}
