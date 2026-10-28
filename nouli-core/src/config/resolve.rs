//! Resolve explicit overrides and selected credentials without process globals.

use std::{
    fmt,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

use super::{Config, ConfigError, ExecutionSettings, OutputFormat, ProviderProfile};
use crate::history::ModelSelection;

/// Intentionally not serializable. Credential source names belong in snapshots;
/// credential values are re-resolved when a saved run resumes.
pub struct Credential(String);

impl fmt::Debug for Credential {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Credential([REDACTED])")
    }
}

impl Credential {
    pub fn new(value: String) -> Result<Self, ConfigError> {
        if value.is_empty() {
            return Err(ConfigError::MissingCredential);
        }
        if !value.bytes().all(|b| b.is_ascii_graphic()) {
            return Err(ConfigError::InvalidCredential);
        }
        Ok(Self(value))
    }
    pub fn expose(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, Default)]
pub struct RuntimeOverrides {
    pub selection: ModelSelection,
    pub output: Option<OutputFormat>,
    pub store: Option<PathBuf>,
    pub max_parallel: Option<usize>,
    pub max_attempts: Option<u32>,
    pub attempt_timeout_seconds: Option<u64>,
}

/// Frozen, serializable settings contain no credential values.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SettingsSnapshot {
    pub profile_name: String,
    pub profile: ProviderProfile,
    pub execution: ExecutionSettings,
    pub output: OutputFormat,
    pub store: PathBuf,
}

#[derive(Debug)]
pub struct ResolvedSettings {
    pub snapshot: SettingsSnapshot,
    pub credential: Option<Credential>,
}

/// Layers are ordered highest first. Provider and model overrides are independent
/// fields. An unspecified model falls back to the chosen profile's model.
pub fn select_model(layers: &[&ModelSelection]) -> ModelSelection {
    ModelSelection {
        provider: layers.iter().find_map(|layer| layer.provider.clone()),
        model: layers.iter().find_map(|layer| layer.model.clone()),
    }
}

impl SettingsSnapshot {
    /// Re-resolve only the saved source name, preserving every saved setting.
    pub fn with_credentials(
        self,
        lookup: impl Fn(&str) -> Option<String>,
    ) -> Result<ResolvedSettings, ConfigError> {
        self.profile.validate()?;
        self.execution.validate()?;
        let credential = self
            .profile
            .api_key_env
            .as_ref()
            .map(|name| {
                let value = lookup(name).ok_or(ConfigError::MissingCredential)?;
                Credential::new(value)
            })
            .transpose()?;
        Ok(ResolvedSettings {
            snapshot: self,
            credential,
        })
    }
}

impl Config {
    pub fn store_path(&self, home: &Path, explicit: Option<&Path>) -> Result<PathBuf, ConfigError> {
        let store = explicit
            .or(self.store.as_deref())
            .map(Path::to_owned)
            .unwrap_or_else(|| home.join(".nouli"));
        if store.as_os_str().is_empty() {
            return Err(ConfigError::Invalid("store"));
        }
        Ok(store)
    }
    /// CLI selection precedes request then session defaults. For workflow nodes,
    /// pass node selection as the override and the frozen run selection as layer 1.
    pub fn resolve(
        &self,
        overrides: &RuntimeOverrides,
        request: &ModelSelection,
        session: &ModelSelection,
        home: &Path,
        lookup: impl Fn(&str) -> Option<String>,
    ) -> Result<ResolvedSettings, ConfigError> {
        self.resolve_snapshot(overrides, request, session, home)?
            .with_credentials(lookup)
    }

    pub fn resolve_snapshot(
        &self,
        overrides: &RuntimeOverrides,
        request: &ModelSelection,
        session: &ModelSelection,
        home: &Path,
    ) -> Result<SettingsSnapshot, ConfigError> {
        self.validate()?;
        let default = ModelSelection {
            provider: self.default_provider.clone(),
            model: None,
        };
        let selection = select_model(&[&overrides.selection, request, session, &default]);
        let name = selection.provider.ok_or(ConfigError::MissingProvider)?;
        let mut profile = self
            .providers
            .get(&name)
            .ok_or(ConfigError::UnknownProvider)?
            .clone();
        if let Some(model) = selection.model {
            profile.model = model;
        }
        profile.validate()?;
        let mut execution = self.execution.clone();
        execution.max_attempts = overrides
            .max_attempts
            .or(profile.max_attempts)
            .unwrap_or(execution.max_attempts);
        execution.attempt_timeout_seconds = overrides
            .attempt_timeout_seconds
            .or(profile.attempt_timeout_seconds)
            .unwrap_or(execution.attempt_timeout_seconds);
        if let Some(parallel) = overrides.max_parallel {
            execution.max_parallel = parallel;
        }
        execution.validate()?;
        let store = self.store_path(home, overrides.store.as_deref())?;
        Ok(SettingsSnapshot {
            profile_name: name,
            profile,
            execution,
            output: overrides.output.unwrap_or(self.output),
            store,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    fn config() -> Config {
        Config::parse(include_str!("../../../fixtures/config/providers.toml")).unwrap()
    }

    #[test]
    fn only_selected_credentials_are_read_and_never_serialized_or_debugged() {
        let config = config();
        let reads = RefCell::new(Vec::new());
        let lookup = |name: &str| {
            reads.borrow_mut().push(name.to_owned());
            Some("SECRET_SENTINEL".to_owned())
        };
        let resolved = config
            .resolve(
                &RuntimeOverrides::default(),
                &ModelSelection::default(),
                &ModelSelection::default(),
                Path::new("/fake-home"),
                lookup,
            )
            .unwrap();
        assert_eq!(*reads.borrow(), vec!["TYPESAFE_API_KEY"]);
        assert_eq!(
            resolved.credential.as_ref().unwrap().expose(),
            "SECRET_SENTINEL"
        );
        for text in [
            format!("{resolved:?}"),
            serde_json::to_string(&resolved.snapshot).unwrap(),
        ] {
            assert!(!text.contains("SECRET_SENTINEL"));
        }
        let error = config
            .resolve(
                &RuntimeOverrides::default(),
                &ModelSelection::default(),
                &ModelSelection::default(),
                Path::new("/fake-home"),
                |_| None,
            )
            .unwrap_err();
        assert!(matches!(error, ConfigError::MissingCredential));
        let local = RuntimeOverrides {
            selection: ModelSelection {
                provider: Some("local".into()),
                model: None,
            },
            ..Default::default()
        };
        let resolved = config
            .resolve(
                &local,
                &ModelSelection::default(),
                &ModelSelection::default(),
                Path::new("/fake-home"),
                |_| panic!("unused keys must not be read"),
            )
            .unwrap();
        assert!(resolved.credential.is_none());
    }

    #[test]
    fn override_precedence_is_explicit_and_does_not_mutate_session_defaults() {
        let config = config();
        let session = ModelSelection {
            provider: Some("local".into()),
            model: Some("session-model".into()),
        };
        let request = ModelSelection {
            provider: None,
            model: Some("request-model".into()),
        };
        let overrides = RuntimeOverrides {
            selection: ModelSelection {
                provider: Some("custom".into()),
                model: Some("cli-model".into()),
            },
            output: Some(OutputFormat::Json),
            store: Some(PathBuf::from("/alternate-store")),
            max_parallel: Some(2),
            max_attempts: Some(5),
            attempt_timeout_seconds: Some(7),
        };
        let resolved = config
            .resolve(&overrides, &request, &session, Path::new("/home"), |_| {
                Some("key".into())
            })
            .unwrap();
        assert_eq!(resolved.snapshot.profile_name, "custom");
        assert_eq!(resolved.snapshot.profile.model, "cli-model");
        assert_eq!(resolved.snapshot.output, OutputFormat::Json);
        assert_eq!(resolved.snapshot.store, PathBuf::from("/alternate-store"));
        assert_eq!(resolved.snapshot.execution.max_parallel, 2);
        assert_eq!(resolved.snapshot.execution.max_attempts, 5);
        assert_eq!(resolved.snapshot.execution.attempt_timeout_seconds, 7);
        assert_eq!(session.model.as_deref(), Some("session-model"));
        assert_eq!(
            select_model(&[&request, &session]).model.as_deref(),
            Some("request-model")
        );
        let node = ModelSelection {
            provider: None,
            model: Some("node-model".into()),
        };
        assert_eq!(
            select_model(&[&node, &overrides.selection])
                .model
                .as_deref(),
            Some("node-model")
        );
    }

    #[test]
    fn frozen_settings_re_resolve_credentials_without_changing_models() {
        let settings = config()
            .resolve(
                &RuntimeOverrides::default(),
                &ModelSelection::default(),
                &ModelSelection::default(),
                Path::new("/home"),
                |_| Some("first-key".into()),
            )
            .unwrap()
            .snapshot;
        let decoded: SettingsSnapshot =
            serde_json::from_str(&serde_json::to_string(&settings).unwrap()).unwrap();
        let restored = decoded
            .with_credentials(|_| Some("second-key".into()))
            .unwrap();
        assert_eq!(restored.snapshot, settings);
        assert_eq!(restored.credential.unwrap().expose(), "second-key");
        assert!(Credential::new("secret\nheader".into()).is_err());
        assert!(Credential::new(" ".into()).is_err());
    }
}
