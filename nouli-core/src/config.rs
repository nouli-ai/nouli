//! Explicit configuration inputs. No process environment or provider calls.

pub mod auth;
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
    SystemOne,
    OpenaiDecisions,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum HttpBinding {
    #[default]
    Standard,
    CloudflareWorkersAi,
}
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ImageEncoding {
    #[default]
    Assets,
    ImagesBase64,
    ImagesDataUrls,
    StateImageUrls,
    OpenaiContent,
}
/// Explicit connection behavior. Every field overlays independently; no vendor detection.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ConnectionSettings {
    pub binding: Option<HttpBinding>,
    pub request_path: Option<String>,
    pub wire_model: Option<String>,
    pub no_auth: Option<bool>,
    pub auth_header: Option<String>,
    pub auth_scheme: Option<String>,
    pub headers: BTreeMap<String, String>,
    pub headers_env: BTreeMap<String, String>,
    pub extra_body: BTreeMap<String, serde_json::Value>,
    pub image_encoding: Option<ImageEncoding>,
    pub min_image_dimension: Option<u32>,
    pub max_image_dimension: Option<u32>,
    pub pixel_patch_size: Option<u32>,
    pub pixel_multiplier: Option<u64>,
    pub pixel_overhead: Option<u64>,
}
impl ConnectionSettings {
    pub fn overlay(&self, other: &Self) -> Self {
        let mut result = self.clone();
        macro_rules! overlay { ($($field:ident),*) => { $(if other.$field.is_some() {result.$field.clone_from(&other.$field);})* }; }
        overlay!(
            binding,
            request_path,
            wire_model,
            no_auth,
            auth_header,
            auth_scheme,
            image_encoding,
            min_image_dimension,
            max_image_dimension,
            pixel_patch_size,
            pixel_multiplier,
            pixel_overhead
        );
        // HTTP header names are case insensitive across all settings layers.
        for (name, value) in &other.headers {
            result
                .headers
                .retain(|key, _| !key.eq_ignore_ascii_case(name));
            result
                .headers_env
                .retain(|key, _| !key.eq_ignore_ascii_case(name));
            result
                .headers
                .insert(name.to_ascii_lowercase(), value.clone());
        }
        for (name, value) in &other.headers_env {
            result
                .headers
                .retain(|key, _| !key.eq_ignore_ascii_case(name));
            result
                .headers_env
                .retain(|key, _| !key.eq_ignore_ascii_case(name));
            result
                .headers_env
                .insert(name.to_ascii_lowercase(), value.clone());
        }
        result.extra_body.extend(other.extra_body.clone());
        result
    }
    pub fn validate_fields(&self) -> Result<(), ConfigError> {
        use reqwest::header::{HeaderName, HeaderValue};
        for (name, value) in &self.headers {
            HeaderName::from_bytes(name.as_bytes()).map_err(|_| ConfigError::Invalid("header"))?;
            HeaderValue::from_str(value).map_err(|_| ConfigError::Invalid("header"))?;
        }
        for (name, value) in &self.headers_env {
            HeaderName::from_bytes(name.as_bytes()).map_err(|_| ConfigError::Invalid("header"))?;
            if !env_name(value) {
                return Err(ConfigError::Invalid("header environment"));
            }
        }
        if let Some(name) = &self.auth_header {
            HeaderName::from_bytes(name.as_bytes())
                .map_err(|_| ConfigError::Invalid("auth header"))?;
        }
        if self
            .auth_scheme
            .as_ref()
            .is_some_and(|v| !v.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-'))
        {
            return Err(ConfigError::Invalid("auth scheme"));
        }
        if self.extra_body.keys().any(|k| {
            matches!(
                k.as_str(),
                "model" | "state" | "input" | "questions" | "assets" | "images"
            )
        }) {
            return Err(ConfigError::Invalid("reserved request field"));
        }
        if [
            self.min_image_dimension,
            self.max_image_dimension,
            self.pixel_patch_size,
        ]
        .contains(&Some(0))
            || self
                .min_image_dimension
                .zip(self.max_image_dimension)
                .is_some_and(|(a, b)| a > b)
        {
            return Err(ConfigError::Invalid("image settings"));
        }
        if self.request_path.as_ref().is_some_and(|p| {
            p.is_empty()
                || p.split('/').any(|s| s.is_empty() || s == "." || s == "..")
                || p.contains(['?', '#', '\\'])
        }) {
            return Err(ConfigError::Invalid("request path"));
        }
        Ok(())
    }
    pub fn validate(&self, protocol: Protocol) -> Result<(), ConfigError> {
        self.validate_fields()?;
        if protocol == Protocol::OpenaiDecisions
            && (self.binding == Some(HttpBinding::CloudflareWorkersAi)
                || self
                    .image_encoding
                    .is_some_and(|e| e != ImageEncoding::OpenaiContent))
        {
            return Err(ConfigError::Invalid("protocol compatibility"));
        }
        if protocol == Protocol::SystemOne
            && self.image_encoding == Some(ImageEncoding::OpenaiContent)
        {
            return Err(ConfigError::Invalid("protocol compatibility"));
        }
        Ok(())
    }
}
impl ModelSettings {
    pub fn overlay(&self, other: &Self) -> Self {
        let mut r = self.clone();
        r.connection = self.connection.overlay(&other.connection);
        macro_rules! overlay {($($f:ident),*)=>{$(if other.$f.is_some(){r.$f.clone_from(&other.$f);})*};}
        overlay!(
            context_budget_tokens,
            context_window_tokens,
            reserved_output_tokens,
            per_question_context_tokens,
            max_questions,
            max_choices,
            max_score_levels,
            max_request_bytes,
            input_modalities
        );
        r
    }
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

/// Connection and execution policy shared by every model at this provider.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderDefinition {
    #[serde(default)]
    pub connection: ConnectionSettings,
    #[serde(default)]
    pub defaults: ModelSettings,
    pub protocol: Protocol,
    pub base_url: String,
    pub api_key_env: Option<String>,
    pub max_attempts: Option<u32>,
    pub attempt_timeout_seconds: Option<u64>,
    #[serde(default)]
    pub models: BTreeMap<String, ModelSettings>,
}

/// Optional limits for one exact model ID, not an allowlist of selectable models.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ModelSettings {
    pub max_request_bytes: Option<usize>,
    pub input_modalities: Option<Vec<crate::limits::Modality>>,
    #[serde(default)]
    pub connection: ConnectionSettings,
    pub per_question_context_tokens: Option<u64>,
    pub max_questions: Option<usize>,
    pub max_choices: Option<usize>,
    pub max_score_levels: Option<usize>,
    pub context_budget_tokens: Option<u64>,
    pub context_window_tokens: Option<u64>,
    pub reserved_output_tokens: Option<u64>,
}

/// Complete selected provider/model settings, also used by frozen snapshots.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderProfile {
    pub connection: ConnectionSettings,
    pub limits: ModelSettings,
    pub protocol: Protocol,
    pub base_url: String,
    pub model: String,
    /// The environment variable named by the provider's `api_key_env`. Saved
    /// runs written before the credential store read as the source `env:NAME`.
    pub api_key_env: Option<String>,
    /// With no `api_key_env`, the provider's entry in the credential store
    /// supplies the key (the source `auth:PROVIDER`).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub stored_credential: bool,
    pub context_budget_tokens: Option<u64>,
    pub context_window_tokens: Option<u64>,
    pub reserved_output_tokens: Option<u64>,
    pub max_attempts: Option<u32>,
    pub attempt_timeout_seconds: Option<u64>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub defaults: ModelSettings,
    pub default_provider: Option<String>,
    pub default_model: Option<String>,
    pub output: OutputFormat,
    pub store: Option<PathBuf>,
    pub execution: ExecutionSettings,
    pub providers: BTreeMap<String, ProviderDefinition>,
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
    #[error("no provider profiles are defined")]
    NoProviders,
    #[error("no provider was selected")]
    MissingProvider,
    #[error("no model was selected")]
    MissingModel,
    #[error("selected provider does not exist")]
    UnknownProvider,
    #[error("selected credential environment variable is unavailable or empty")]
    MissingCredential,
    #[error("selected credential contains invalid header characters")]
    InvalidCredential,
    #[error("header credential environment variable {variable} is unavailable or invalid")]
    HeaderCredential { variable: String },
    #[error("credential environment variable {variable} is unavailable or invalid")]
    CredentialSource {
        variable: String,
        /// The provider, for the suggestion to log in.
        provider: String,
        #[source]
        source: Box<ConfigError>,
    },
    #[error("the stored credential for provider {provider} is unavailable or invalid")]
    StoredCredential {
        provider: String,
        #[source]
        source: Box<ConfigError>,
    },
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

fn validate_connection(
    base_url: &str,
    api_key_env: Option<&str>,
    max_attempts: Option<u32>,
    attempt_timeout_seconds: Option<u64>,
) -> Result<(), ConfigError> {
    let url = Url::parse(base_url).map_err(|_| ConfigError::Invalid("base_url"))?;
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
    if api_key_env.is_some_and(|name| !env_name(name)) {
        return Err(ConfigError::Invalid("api_key_env"));
    }
    if max_attempts == Some(0) || attempt_timeout_seconds == Some(0) {
        return Err(ConfigError::Invalid("provider limits"));
    }
    Ok(())
}

impl ModelSettings {
    pub fn validate(&self) -> Result<(), ConfigError> {
        self.connection.validate_fields()?;
        if self.max_request_bytes == Some(0) {
            return Err(ConfigError::Invalid("request byte limit"));
        }
        if [
            self.per_question_context_tokens,
            self.context_budget_tokens,
            self.context_window_tokens,
        ]
        .into_iter()
        .flatten()
        .any(|value| value == 0)
        {
            return Err(ConfigError::Invalid("model limits"));
        }
        if [self.max_questions, self.max_choices, self.max_score_levels].contains(&Some(0)) {
            return Err(ConfigError::Invalid("count limits"));
        }
        Ok(())
    }
}

impl ProviderDefinition {
    pub fn validate(&self) -> Result<(), ConfigError> {
        validate_connection(
            &self.base_url,
            self.api_key_env.as_deref(),
            self.max_attempts,
            self.attempt_timeout_seconds,
        )?;
        self.connection.validate(self.protocol)?;
        self.defaults.validate()?;
        for (model, settings) in &self.models {
            if model.trim().is_empty() {
                return Err(ConfigError::Invalid("model name"));
            }
            settings.validate()?;
        }
        Ok(())
    }
}

impl ProviderProfile {
    pub fn validate(&self) -> Result<(), ConfigError> {
        validate_connection(
            &self.base_url,
            self.api_key_env.as_deref(),
            self.max_attempts,
            self.attempt_timeout_seconds,
        )?;
        self.connection.validate(self.protocol)?;
        if self.model.trim().is_empty() {
            return Err(ConfigError::Invalid("model"));
        }
        self.limits.validate()?;
        ModelSettings {
            max_request_bytes: self.limits.max_request_bytes,
            input_modalities: self.limits.input_modalities.clone(),
            connection: self.connection.clone(),
            per_question_context_tokens: None,
            max_questions: None,
            max_choices: None,
            max_score_levels: None,
            context_budget_tokens: self.context_budget_tokens,
            context_window_tokens: self.context_window_tokens,
            reserved_output_tokens: self.reserved_output_tokens,
        }
        .validate()
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
        self.defaults.validate()?;
        self.execution.validate()?;
        for (name, profile) in &self.providers {
            if name.trim().is_empty() {
                return Err(ConfigError::Invalid("provider name"));
            }
            profile.validate()?;
        }
        if self
            .default_model
            .as_ref()
            .is_some_and(|model| model.trim().is_empty())
        {
            return Err(ConfigError::Invalid("default_model"));
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
    fn header_overlays_respect_case_and_source_precedence() {
        let global = ConnectionSettings {
            headers: BTreeMap::from([("x-tenant".into(), "global".into())]),
            ..Default::default()
        };
        let provider = ConnectionSettings {
            headers: BTreeMap::from([("X-Tenant".into(), "provider".into())]),
            ..Default::default()
        };
        let model = ConnectionSettings {
            headers_env: BTreeMap::from([("X-TENANT".into(), "TENANT_KEY".into())]),
            ..Default::default()
        };
        let merged = global.overlay(&provider);
        assert_eq!(merged.headers.len(), 1);
        assert_eq!(merged.headers["x-tenant"], "provider");
        let merged = merged.overlay(&model);
        assert!(merged.headers.is_empty());
        assert_eq!(merged.headers_env["x-tenant"], "TENANT_KEY");
    }

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
        let template =
            |url: &str| format!("[providers.local]\nprotocol='system-one'\nbase_url='{url}'");
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
            Config::parse(&template("https://example.org").replace("system-one", "unknown"))
                .is_err()
        );
    }

    #[test]
    fn definitions_allow_missing_defaults_but_reject_misplaced_fields_and_invalid_models() {
        let provider =
            "[providers.local]\nprotocol='system-one'\nbase_url='http://localhost:8000'\n";
        let config = Config::parse(provider).unwrap();
        assert!(config.default_model.is_none());
        assert!(config.providers["local"].models.is_empty());
        for input in [
            format!("default_model=' '\n{provider}"),
            format!("{provider}model='a'"),
            format!("{provider}context_budget_tokens=1024"),
            format!("{provider}context_window_tokens=1024"),
            format!("{provider}reserved_output_tokens=128"),
            format!("{provider}[providers.local.models.' ']\ncontext_budget_tokens=1024"),
            format!("{provider}[providers.local.models.a]\ncontext_budget_tokens=0"),
            format!("{provider}[providers.local.models.a]\ncontext_window_tokens=0"),
            format!("{provider}[providers.local.models.a]\nunknown=1"),
        ] {
            assert!(Config::parse(&input).is_err(), "accepted {input}");
        }
    }
}
