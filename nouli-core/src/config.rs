//! Explicit configuration inputs. No process environment or provider calls.

pub mod resolve;

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};
use thiserror::Error;
use url::Url;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Protocol {
    Typesafe,
    OpenaiDecisions,
    OpenrouterDecisions,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OutputFormat {
    #[default]
    Auto,
    Text,
    Json,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ExecutionSettings {
    pub max_parallel: usize,
    pub max_attempts: u32,
    pub attempt_timeout_seconds: u64,
    pub max_retry_delay_seconds: u64,
    pub max_response_bytes: usize,
}

impl Default for ExecutionSettings {
    fn default() -> Self {
        Self {
            max_parallel: 4,
            max_attempts: 3,
            attempt_timeout_seconds: 120,
            max_retry_delay_seconds: 30,
            max_response_bytes: 16 * 1024 * 1024,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderProfile {
    pub protocol: Protocol,
    pub base_url: String,
    pub model: String,
    pub api_key_env: Option<String>,
    pub context_budget_tokens: Option<u64>,
    pub context_window_tokens: Option<u64>,
    pub reserved_output_tokens: Option<u64>,
    pub max_attempts: Option<u32>,
    pub attempt_timeout_seconds: Option<u64>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub default_provider: Option<String>,
    pub output: OutputFormat,
    pub store: Option<PathBuf>,
    pub execution: ExecutionSettings,
    pub providers: BTreeMap<String, ProviderProfile>,
}

/// Store/presentation discovery for inspection and saved-run recovery. Provider
/// definitions are deliberately outside this read-only configuration boundary.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct StoreSettings {
    pub output: OutputFormat,
    pub store: Option<PathBuf>,
}
impl StoreSettings {
    pub fn load(path: &Path, optional: bool) -> Result<Self, ConfigError> {
        let input = match std::fs::read_to_string(path) {
            Ok(input) => input,
            Err(error) if optional && error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Self::default());
            }
            Err(source) => {
                return Err(ConfigError::Read {
                    path: path.to_owned(),
                    source,
                });
            }
        };
        let settings: Self = toml::from_str(&input).map_err(|_| ConfigError::Syntax)?;
        if settings
            .store
            .as_ref()
            .is_some_and(|path| path.as_os_str().is_empty())
        {
            return Err(ConfigError::Invalid("store"));
        }
        Ok(settings)
    }
}

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("cannot read configuration at {path}: {source}")]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("configuration syntax or fields are invalid")]
    Syntax,
    #[error("invalid configuration setting: {0}")]
    Invalid(&'static str),
    #[error("no provider was selected")]
    MissingProvider,
    #[error("selected provider profile does not exist")]
    UnknownProvider,
    #[error("selected credential environment variable is unavailable or empty")]
    MissingCredential,
    #[error("selected credential contains invalid header characters")]
    InvalidCredential,
}

fn env_name(name: &str) -> bool {
    let mut chars = name.bytes();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == b'_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == b'_')
}

impl ExecutionSettings {
    pub fn validate(&self) -> Result<(), ConfigError> {
        if self.max_parallel == 0
            || self.max_attempts == 0
            || self.attempt_timeout_seconds == 0
            || self.max_retry_delay_seconds == 0
            || self.max_response_bytes == 0
        {
            return Err(ConfigError::Invalid("execution limits"));
        }
        Ok(())
    }
}

impl ProviderProfile {
    pub fn validate(&self) -> Result<(), ConfigError> {
        let url = Url::parse(&self.base_url).map_err(|_| ConfigError::Invalid("base_url"))?;
        let local = match url.host() {
            Some(url::Host::Domain(host)) => host == "localhost",
            Some(url::Host::Ipv4(ip)) => ip.is_loopback() || ip.is_private() || ip.is_link_local(),
            Some(url::Host::Ipv6(ip)) => {
                ip.is_loopback() || ip.is_unique_local() || ip.is_unicast_link_local()
            }
            None => false,
        };
        if (url.scheme() != "https" && !(url.scheme() == "http" && local))
            || url.host().is_none()
            || !url.username().is_empty()
            || url.password().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
        {
            return Err(ConfigError::Invalid("base_url"));
        }
        if self.model.trim().is_empty() {
            return Err(ConfigError::Invalid("model"));
        }
        if self
            .api_key_env
            .as_ref()
            .is_some_and(|name| !env_name(name))
        {
            return Err(ConfigError::Invalid("api_key_env"));
        }
        if [
            self.context_budget_tokens,
            self.context_window_tokens,
            self.reserved_output_tokens,
            self.attempt_timeout_seconds,
        ]
        .into_iter()
        .flatten()
        .any(|n| n == 0)
            || self.max_attempts == Some(0)
        {
            return Err(ConfigError::Invalid("profile limits"));
        }
        Ok(())
    }
}

impl Config {
    pub fn parse(input: &str) -> Result<Self, ConfigError> {
        let config: Self = toml::from_str(input).map_err(|_| ConfigError::Syntax)?;
        config.validate()?;
        Ok(config)
    }

    /// The caller supplies the path; only a missing optional default is ignored.
    /// Loading never creates directories, a ledger, or a provider connection.
    pub fn load(path: &Path, optional: bool) -> Result<Self, ConfigError> {
        match std::fs::read_to_string(path) {
            Ok(input) => Self::parse(&input),
            Err(error) if optional && error.kind() == std::io::ErrorKind::NotFound => {
                Ok(Self::default())
            }
            Err(source) => Err(ConfigError::Read {
                path: path.to_owned(),
                source,
            }),
        }
    }

    pub fn validate(&self) -> Result<(), ConfigError> {
        self.execution.validate()?;
        for (name, profile) in &self.providers {
            if name.trim().is_empty() {
                return Err(ConfigError::Invalid("profile name"));
            }
            profile.validate()?;
        }
        if self
            .default_provider
            .as_ref()
            .is_some_and(|name| !self.providers.contains_key(name))
        {
            return Err(ConfigError::UnknownProvider);
        }
        if self
            .store
            .as_ref()
            .is_some_and(|path| path.as_os_str().is_empty())
        {
            return Err(ConfigError::Invalid("store"));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_config_has_defaults_and_no_files_are_created() {
        let home =
            std::env::temp_dir().join(format!("nouli-config-{}", crate::identity::Id::generate()));
        let config = Config::load(&home.join(".nouli/config.toml"), true).unwrap();
        assert_eq!(config.execution, ExecutionSettings::default());
        assert!(!home.exists());
        assert!(Config::load(&home.join("missing"), false).is_err());
    }

    #[test]
    fn validates_unknown_fields_protocols_urls_and_limits() {
        for input in [
            "unknown = true",
            "[execution]\nmax_parallel = 0",
            "[execution]\nmax_attempts = 0",
        ] {
            assert!(Config::parse(input).is_err());
        }
        let template = |url: &str| {
            format!("[providers.local]\nprotocol='typesafe'\nbase_url='{url}'\nmodel='local'")
        };
        for url in [
            "not a URL",
            "file:///tmp/model",
            "http://public.example",
            "https://user:secret@example.org",
            "https://example.org?api_key=secret",
            "https://example.org#secret",
        ] {
            assert!(Config::parse(&template(url)).is_err());
        }
        for url in [
            "http://localhost:8000/base",
            "http://127.0.0.1:8000",
            "http://[::1]:8000",
            "https://example.org/prefix",
        ] {
            Config::parse(&template(url)).unwrap();
        }
        assert!(
            Config::parse(&template("https://example.org").replace("typesafe", "unknown")).is_err()
        );
    }
}
