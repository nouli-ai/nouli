//! Idle connection and credential setup commands.
use super::App;

impl App<'_> {
    /// Runs a setup command; returns the status line to show on success.
    pub(super) fn setup_command(
        &mut self,
        command: &str,
    ) -> Result<&'static str, crate::connect::SetupError> {
        let (verb, argument) = command.split_once(' ').unwrap_or((command, ""));
        let mut note = "connection settings updated";
        match verb {
            "/connect" => {
                let selection = crate::connect::configure(
                    &self.repl.settings.config_path,
                    &crate::connect::Connect {
                        service: if argument.is_empty() {
                            None
                        } else {
                            Some(argument.into())
                        },
                        ..Default::default()
                    },
                )?;
                self.repl.model = selection;
            }
            "/login" | "/logout" => {
                let provider = if argument.is_empty() {
                    self.repl
                        .model
                        .provider
                        .clone()
                        .or(self.repl.settings.config.default_provider.clone())
                        .ok_or(crate::connect::SetupError::ConnectionRequired)?
                } else {
                    argument.into()
                };
                let auth = crate::settings::auth::path_for(&self.repl.settings.config_path);
                let report = crate::connect::SetupError::Report;
                if verb == "/logout" {
                    let outcome = crate::auth::log_out(&auth, &provider).map_err(report)?;
                    note = if outcome.removed {
                        "credential removed"
                    } else {
                        "no stored credential to remove"
                    };
                } else {
                    let disabled =
                        crate::auth::check_login(&self.repl.settings, &provider).map_err(report)?;
                    let key = crate::connect::read_key("API key: ")?;
                    let source = nouli_core::config::auth::KeySource::Key(key);
                    crate::auth::store_key(&auth, &provider, &source, disabled).map_err(report)?;
                    note = if disabled {
                        "credential stored; authentication is disabled for the default model"
                    } else {
                        "credential stored"
                    };
                }
            }
            _ => return Err(crate::connect::SetupError::UnknownCommand),
        }
        self.reload_settings()?;
        Ok(note)
    }

    /// Reads the configuration and credentials again after a change.
    pub(super) fn reload_settings(&mut self) -> Result<(), crate::connect::SetupError> {
        let mut settings = self.repl.settings.reload()?;
        settings
            .load_environment()
            .map_err(crate::connect::SetupError::Report)?;
        self.repl.settings = std::borrow::Cow::Owned(settings);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use crate::repl::tui::tests::*;

    #[test]
    fn logout_removes_a_stored_credential_for_an_unconfigured_provider_like_auth_logout() {
        use std::os::unix::fs::PermissionsExt;
        let root = std::env::temp_dir().join(format!("nouli-tui-logout-{}", Id::generate()));
        std::fs::create_dir_all(&root).unwrap();
        let config = root.join("config.toml");
        std::fs::write(
            &config,
            "default_provider = \"local\"\ndefault_model = \"offline\"\n[providers.local]\nprotocol = \"system-one\"\nbase_url = \"http://127.0.0.1:9\"\n[providers.local.models.offline]\ncontext_budget_tokens = 100000\nreserved_output_tokens = 128\n",
        )
        .unwrap();
        let auth = crate::settings::auth::path_for(&config);
        std::fs::write(
            &auth,
            "[providers.ghost]\ntype = \"api_key\"\nkey = \"secret\"\n",
        )
        .unwrap();
        std::fs::set_permissions(&auth, std::fs::Permissions::from_mode(0o600)).unwrap();
        let mut settings = Settings::for_test(root.join("store"));
        settings.config_path.clone_from(&config);
        let mut app = app(&settings);
        assert_eq!(
            app.setup_command("/logout ghost").unwrap(),
            "credential removed"
        );
        assert_eq!(
            app.setup_command("/logout ghost").unwrap(),
            "no stored credential to remove"
        );
        let remaining = crate::settings::auth::update(&auth, |store| {
            let present = store.remove("ghost");
            (present, false)
        })
        .unwrap();
        assert!(!remaining, "ghost's credential was not removed");
        std::fs::remove_dir_all(root).unwrap();
    }
}
