//! Startup model selection and credential preparation before a session opens.
use super::{
    SetupError,
    prompt::{prompt, prompted_source},
    recipes,
};
use crate::settings::{self, config_file};
use nouli_core::{config::auth::AuthEntry, history::ModelSelection};
use std::io::{self, Write};

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
        config_file::set_default(&settings.config_path, &provider, &model)?;
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
    let selected = crate::selection::model(Some(&settings.config), &[selection]);
    if selected.provider.is_none() || selected.model.is_none() {
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
    use crate::connect::{
        configure,
        tests::{Directory, custom},
    };
    use nouli_core::config::Config;
    use std::fs;

    #[test]
    fn missing_header_credentials_do_not_trigger_primary_key_setup_or_writes() {
        let dir = Directory::new();
        configure(&dir.0.join("config.toml"), &custom("gateway")).unwrap();
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
}
