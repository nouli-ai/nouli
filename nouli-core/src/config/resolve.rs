//! Resolve explicit overrides and selected credentials without process globals.

use std::{
    fmt,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

use super::{
    Config, ConfigError, ExecutionSettings, OutputFormat, ProviderProfile, auth::CredentialSource,
};
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
    pub context_budget_tokens: Option<u64>,
    pub max_attempts: Option<u32>,
    pub attempt_timeout_seconds: Option<u64>,
    /// Providers that have an entry in the credential store. A provider with an
    /// entry and no `api_key_env` takes its key from the store.
    pub stored_credentials: std::collections::BTreeSet<String>,
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
/// fields, including the final configuration defaults layer.
pub fn select_model(layers: &[&ModelSelection]) -> ModelSelection {
    ModelSelection {
        provider: layers.iter().find_map(|layer| layer.provider.clone()),
        model: layers.iter().find_map(|layer| layer.model.clone()),
    }
}

impl SettingsSnapshot {
    /// Re-resolve only the saved source, preserving every saved setting. An
    /// environment variable comes first, and the provider's stored entry is the
    /// fallback when the variable is not set at all.
    pub fn with_credentials(
        self,
        lookup: impl Fn(&CredentialSource) -> Option<String>,
    ) -> Result<ResolvedSettings, ConfigError> {
        self.profile.validate()?;
        self.execution.validate()?;
        let provider = self.profile_name.clone();
        let credential = if let Some(name) = &self.profile.api_key_env {
            lookup(&CredentialSource::Env(name.clone()))
                .or_else(|| lookup(&CredentialSource::Auth(provider.clone())))
                .ok_or(ConfigError::MissingCredential)
                .and_then(Credential::new)
                .map_err(|source| ConfigError::CredentialSource {
                    variable: name.clone(),
                    provider: provider.clone(),
                    source: Box::new(source),
                })
                .map(Some)?
        } else if self.profile.stored_credential {
            lookup(&CredentialSource::Auth(provider.clone()))
                .ok_or(ConfigError::MissingCredential)
                .and_then(Credential::new)
                .map_err(|source| ConfigError::StoredCredential {
                    provider: provider.clone(),
                    source: Box::new(source),
                })
                .map(Some)?
        } else {
            None
        };
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
        lookup: impl Fn(&CredentialSource) -> Option<String>,
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
        if self.providers.is_empty() {
            return Err(ConfigError::NoProviders);
        }
        let default = ModelSelection {
            provider: self.default_provider.clone(),
            model: self.default_model.clone(),
        };
        let selection = select_model(&[&overrides.selection, request, session, &default]);
        let name = selection.provider.ok_or(ConfigError::MissingProvider)?;
        let provider = self
            .providers
            .get(&name)
            .ok_or(ConfigError::UnknownProvider)?;
        let model = selection.model.ok_or(ConfigError::MissingModel)?;
        let settings = provider.models.get(&model).cloned().unwrap_or_default();
        let mut profile = ProviderProfile {
            protocol: provider.protocol,
            base_url: provider.base_url.clone(),
            model,
            api_key_env: provider.api_key_env.clone(),
            stored_credential: provider.api_key_env.is_none()
                && overrides.stored_credentials.contains(&name),
            context_budget_tokens: settings.context_budget_tokens,
            context_window_tokens: settings.context_window_tokens,
            reserved_output_tokens: settings.reserved_output_tokens,
            max_attempts: provider.max_attempts,
            attempt_timeout_seconds: provider.attempt_timeout_seconds,
        };
        if let Some(budget) = overrides.context_budget_tokens {
            profile.context_budget_tokens = Some(budget);
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
        let lookup = |source: &CredentialSource| {
            reads.borrow_mut().push(source.to_string());
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
        assert_eq!(*reads.borrow(), vec!["env:TYPESAFE_API_KEY"]);
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
        assert!(
            matches!(error, ConfigError::CredentialSource { variable, source, .. } if variable=="TYPESAFE_API_KEY" && matches!(*source,ConfigError::MissingCredential))
        );
        let local = RuntimeOverrides {
            selection: ModelSelection {
                provider: Some("local".into()),
                model: Some("english".into()),
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
            context_budget_tokens: Some(4096),
            max_attempts: Some(5),
            attempt_timeout_seconds: Some(7),
            ..RuntimeOverrides::default()
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
        assert_eq!(resolved.snapshot.profile.context_budget_tokens, Some(4096));
        assert_eq!(
            config.providers["custom"].models["custom-decision-model"].context_budget_tokens,
            Some(8192)
        );
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
    fn a_missing_variable_falls_back_to_the_stored_entry_but_an_empty_one_does_not() {
        let config = config();
        let resolve = |lookup: &dyn Fn(&CredentialSource) -> Option<String>| {
            config.resolve(
                &RuntimeOverrides::default(),
                &ModelSelection::default(),
                &ModelSelection::default(),
                Path::new("/fake-home"),
                lookup,
            )
        };
        // The provider is named in the stored lookup.
        let stored = resolve(&|source| match source {
            CredentialSource::Auth(provider) => Some(format!("stored-for-{provider}")),
            CredentialSource::Env(_) => None,
        })
        .unwrap();
        let key = stored.credential.unwrap();
        assert!(key.expose().starts_with("stored-for-"));
        let shell = resolve(&|source| match source {
            CredentialSource::Env(_) => Some("from-env".into()),
            CredentialSource::Auth(_) => Some("from-store".into()),
        })
        .unwrap();
        assert_eq!(shell.credential.unwrap().expose(), "from-env");
        // An exported empty value is a real value, so it must not fall through.
        let empty = resolve(&|source| match source {
            CredentialSource::Env(_) => Some(String::new()),
            CredentialSource::Auth(_) => Some("from-store".into()),
        })
        .unwrap_err();
        assert!(
            matches!(empty, ConfigError::CredentialSource { provider, .. } if !provider.is_empty())
        );
        let neither = resolve(&|_| None).unwrap_err();
        assert!(matches!(neither, ConfigError::CredentialSource { .. }));
    }

    #[test]
    fn a_provider_with_a_stored_entry_and_no_variable_freezes_the_stored_source() {
        let config = config();
        let overrides = RuntimeOverrides {
            selection: ModelSelection {
                provider: Some("local".into()),
                model: Some("english".into()),
            },
            ..RuntimeOverrides::default()
        };
        let none = config
            .resolve_snapshot(
                &overrides,
                &ModelSelection::default(),
                &ModelSelection::default(),
                Path::new("/fake-home"),
            )
            .unwrap();
        assert!(!none.profile.stored_credential && none.profile.api_key_env.is_none());
        // An unauthenticated provider needs no credential at all.
        assert!(
            none.clone()
                .with_credentials(|_| None)
                .unwrap()
                .credential
                .is_none()
        );
        let stored = RuntimeOverrides {
            stored_credentials: ["local".to_owned()].into(),
            ..overrides
        };
        let snapshot = config
            .resolve_snapshot(
                &stored,
                &ModelSelection::default(),
                &ModelSelection::default(),
                Path::new("/fake-home"),
            )
            .unwrap();
        assert!(snapshot.profile.stored_credential);
        let json = serde_json::to_string(&snapshot).unwrap();
        assert!(json.contains("stored_credential"));
        let restored: SettingsSnapshot = serde_json::from_str(&json).unwrap();
        let asked = RefCell::new(Vec::new());
        let resolved = restored
            .with_credentials(|source| {
                asked.borrow_mut().push(source.to_string());
                Some("current-key".into())
            })
            .unwrap();
        assert_eq!(*asked.borrow(), ["auth:local"]);
        assert_eq!(resolved.credential.unwrap().expose(), "current-key");
        let error = snapshot.with_credentials(|_| None).unwrap_err();
        assert!(
            matches!(error, ConfigError::StoredCredential { provider, .. } if provider == "local")
        );
        // Runs saved before the store have no such field and still read as env:NAME.
        let mut legacy: serde_json::Value = serde_json::from_str(&json).unwrap();
        legacy["profile"]
            .as_object_mut()
            .unwrap()
            .remove("stored_credential");
        legacy["profile"]["api_key_env"] = serde_json::json!("OLD_NAME");
        let legacy: SettingsSnapshot = serde_json::from_value(legacy).unwrap();
        let asked = RefCell::new(Vec::new());
        legacy
            .with_credentials(|source| {
                asked.borrow_mut().push(source.to_string());
                Some("key".into())
            })
            .unwrap();
        assert_eq!(*asked.borrow(), ["env:OLD_NAME"]);
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

    #[test]
    fn models_share_connections_but_never_inherit_each_others_limits() {
        use crate::limits::{LimitError, ResolvedLimits};
        let config = Config::parse(
            r#"
default_provider = "gateway"
default_model = "a"
[providers.gateway]
protocol = "typesafe"
base_url = "https://gateway.example.com"
max_attempts = 5
attempt_timeout_seconds = 10
[providers.gateway.models.a]
context_window_tokens = 8192
context_budget_tokens = 4096
reserved_output_tokens = 128
[providers.gateway.models.b]
context_window_tokens = 32768
context_budget_tokens = 16384
reserved_output_tokens = 256
"#,
        )
        .unwrap();
        let resolve = |model: Option<&str>, budget| {
            config.resolve_snapshot(
                &RuntimeOverrides {
                    selection: ModelSelection {
                        provider: None,
                        model: model.map(str::to_owned),
                    },
                    context_budget_tokens: budget,
                    ..Default::default()
                },
                &Default::default(),
                &Default::default(),
                Path::new("/home"),
            )
        };
        let a = resolve(None, None).unwrap();
        let b = resolve(Some("b"), None).unwrap();
        assert_eq!(a.profile.model, "a");
        assert_eq!(a.profile.base_url, b.profile.base_url);
        assert_eq!(a.profile_name, b.profile_name);
        assert_eq!(
            ResolvedLimits::for_profile(&a.profile).unwrap().budget,
            4096
        );
        let limits = ResolvedLimits::for_profile(&b.profile).unwrap();
        assert_eq!(limits.budget, 16384);
        assert_eq!(limits.reserved_output_tokens, 256);
        assert_eq!(b.execution.max_attempts, 5);
        assert_eq!(b.execution.attempt_timeout_seconds, 10);
        let unlisted = resolve(Some("unlisted"), None).unwrap();
        assert!(unlisted.profile.context_window_tokens.is_none());
        assert!(unlisted.profile.reserved_output_tokens.is_none());
        assert_eq!(
            ResolvedLimits::for_profile(&unlisted.profile),
            Err(LimitError::MissingBudget)
        );
        assert_eq!(
            ResolvedLimits::for_profile(&resolve(Some("unlisted"), Some(2048)).unwrap().profile)
                .unwrap()
                .budget,
            2048
        );
        assert_eq!(
            ResolvedLimits::for_profile(&resolve(Some("a"), Some(8193)).unwrap().profile),
            Err(LimitError::InvalidBudget)
        );
    }

    #[test]
    fn missing_models_require_selection_and_known_unlisted_models_use_registry_limits() {
        let mut config = config();
        config.default_model = None;
        let snapshot = |config: &Config, selection: ModelSelection| {
            config.resolve_snapshot(
                &RuntimeOverrides {
                    selection,
                    ..Default::default()
                },
                &Default::default(),
                &Default::default(),
                Path::new("/home"),
            )
        };
        assert!(matches!(
            snapshot(&config, ModelSelection::default()),
            Err(ConfigError::MissingModel)
        ));
        assert_eq!(
            snapshot(
                &config,
                ModelSelection {
                    provider: None,
                    model: Some("jev-latest".into())
                }
            )
            .unwrap()
            .profile
            .model,
            "jev-latest"
        );
        config.default_model = Some("gpt-6-luna".into());
        config.providers.get_mut("openai").unwrap().models.clear();
        let selected = snapshot(
            &config,
            ModelSelection {
                provider: Some("openai".into()),
                model: None,
            },
        )
        .unwrap();
        assert!(
            crate::limits::ResolvedLimits::for_profile(&selected.profile)
                .unwrap()
                .capabilities
                .is_some()
        );
        for (request, session, expected) in [
            (None, None, "gpt-6-luna"),
            (None, Some("session-model"), "session-model"),
            (
                Some("request-model"),
                Some("session-model"),
                "request-model",
            ),
        ] {
            let selected = config
                .resolve_snapshot(
                    &Default::default(),
                    &ModelSelection {
                        provider: None,
                        model: request.map(str::to_owned),
                    },
                    &ModelSelection {
                        provider: None,
                        model: session.map(str::to_owned),
                    },
                    Path::new("/home"),
                )
                .unwrap();
            assert_eq!(selected.profile.model, expected);
        }
    }
}
