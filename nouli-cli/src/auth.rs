//! `nouli auth`: store, remove, and list provider credentials. Values are
//! never printed, logged, or put in an error.
use crate::{
    Args, render, render_error,
    settings::{self, Settings, auth as store},
};
use clap::{Args as ClapArgs, Subcommand};
use nouli_core::{
    config::{
        ConfigError,
        auth::{AuthEntry, KeySource, Secret},
    },
    protocol::{ErrorCategory, ErrorReport},
};
use serde::Serialize;
use std::{
    collections::BTreeSet,
    io::{self, BufRead, IsTerminal},
    path::Path,
    process::ExitCode,
};
use thiserror::Error;

#[derive(Debug, Subcommand)]
pub(crate) enum Command {
    /// Store a provider's API key, prompting for it without echo.
    Login(Login),
    /// Remove a provider's stored credential.
    Logout {
        /// The provider, as named in the configuration.
        provider: String,
    },
    /// Show each provider's stored source and which source supplies its key.
    List,
}

#[derive(Debug, ClapArgs)]
pub(crate) struct Login {
    /// The provider, as named in the configuration.
    pub provider: String,
    /// Read the key from one line of standard input instead of prompting.
    #[arg(long, conflicts_with_all = ["env", "command"])]
    pub stdin: bool,
    /// Store the name of an environment variable that holds the key.
    #[arg(long, value_name = "NAME", conflicts_with_all = ["stdin", "command"])]
    pub env: Option<String>,
    /// Store a program that prints the key, run without a shell.
    #[arg(
        long,
        num_args = 1..,
        value_name = "PROG",
        allow_hyphen_values = true,
        conflicts_with_all = ["stdin", "env"]
    )]
    pub command: Vec<String>,
}

fn input(code: &str, message: &str) -> ErrorReport {
    ErrorReport {
        category: ErrorCategory::Input,
        code: code.into(),
        message: message.into(),
        retryable: false,
    }
}

/// The longest line, ending included, accepted as a key on standard input.
const MAX_KEY_LINE: u64 = 16 * 1024;

/// Why no key could be read from standard input. The key itself is never
/// part of the error.
#[derive(Debug, Error)]
pub(crate) enum KeyLineError {
    #[error("No key was read from standard input")]
    Missing,
    #[error("Could not read the key from standard input")]
    Read,
    #[error("The key on standard input is longer than 16 KiB")]
    TooLong,
}

impl KeyLineError {
    pub(crate) fn report(&self) -> ErrorReport {
        let code = match self {
            Self::Missing | Self::Read => "credential_required",
            Self::TooLong => "credential_too_long",
        };
        input(code, &self.to_string())
    }
}

/// One key line from the reader, without its line ending. Only carriage
/// returns and line feeds are removed, so other trailing characters are part
/// of the key. A line over the limit is an error rather than a cut-off key.
pub(crate) fn key_line(reader: &mut impl BufRead) -> Result<String, KeyLineError> {
    let mut bytes = Vec::new();
    let mut limited = io::Read::take(reader, MAX_KEY_LINE + 1);
    let read = limited
        .read_until(b'\n', &mut bytes)
        .map_err(|_| KeyLineError::Read)?;
    if read as u64 > MAX_KEY_LINE {
        return Err(KeyLineError::TooLong);
    }
    if read == 0 {
        return Err(KeyLineError::Missing);
    }
    let text = String::from_utf8(bytes).map_err(|_| KeyLineError::Read)?;
    Ok(text.trim_end_matches(['\n', '\r']).to_owned())
}

fn key_source(login: &Login) -> Result<KeySource, ErrorReport> {
    if let Some(name) = &login.env {
        return KeySource::env(name.clone()).map_err(|_| {
            input(
                "invalid_environment_variable",
                "That is not a valid environment variable name",
            )
        });
    }
    if !login.command.is_empty() {
        return KeySource::command(login.command.clone()).map_err(|_| {
            input(
                "invalid_command",
                "A command source needs a program name and no empty words",
            )
        });
    }
    let key = if login.stdin {
        key_line(&mut io::stdin().lock()).map_err(|error| error.report())?
    } else if io::stdin().is_terminal() {
        rpassword::prompt_password(format!("API key for {}: ", login.provider))
            .map_err(|_| input("credential_required", "Could not read the key"))?
    } else {
        return Err(input(
            "terminal_required",
            "Login prompts for the key, so it needs a terminal. Use --stdin, --env NAME, or --command PROG",
        ));
    };
    Secret::new(key).map(KeySource::Key).map_err(|_| {
        input(
            "invalid_credential",
            "A key cannot be empty or contain control characters",
        )
    })
}

// Field declaration order preserves the historical JSON object order.
#[derive(Serialize)]
pub(crate) struct LoginOutcome {
    version: u32,
    provider: String,
    source: &'static str,
    authentication_disabled_for_default_model: bool,
    path: String,
}
#[derive(Serialize)]
pub(crate) struct LogoutOutcome {
    version: u32,
    provider: String,
    pub(crate) removed: bool,
}
#[derive(Serialize)]
struct CredentialRow {
    provider: String,
    configured: bool,
    stored: Option<&'static str>,
    source: &'static str,
}
#[derive(Serialize)]
struct CredentialsOutcome {
    version: u32,
    providers: Vec<CredentialRow>,
}
#[derive(Serialize)]
#[serde(untagged)]
enum Outcome {
    Login(LoginOutcome),
    Logout(LogoutOutcome),
    List(CredentialsOutcome),
}
impl Outcome {
    fn label(&self) -> &'static str {
        match self {
            Self::Login(_) => "Stored credential",
            Self::Logout(_) => "Removed credential",
            Self::List(_) => "Credentials",
        }
    }
}

pub(crate) fn run(args: &Args, command: &Command) -> ExitCode {
    let mut settings = match settings::load(args) {
        Ok(settings) => settings,
        Err(error) => {
            return render_error(
                args,
                nouli_core::config::OutputFormat::Auto,
                settings::error(args, error),
            );
        }
    };
    let format = match render::resolve_report_format(
        &args.output,
        settings.config.output,
        io::stdout().is_terminal(),
    ) {
        Ok(format) => format,
        Err(error) => return render_error(args, settings.config.output, error),
    };
    let path = store::path_for(&settings.config_path);
    let outcome = match command {
        Command::Login(login) => log_in(&settings, &path, login).map(Outcome::Login),
        Command::Logout { provider } => log_out(&path, provider).map(Outcome::Logout),
        Command::List => list(&mut settings).map(Outcome::List),
    };
    match outcome {
        Ok(outcome) => write_outcome(&outcome, format),
        Err(error) => render_error(args, settings.config.output, error),
    }
}

/// Stores a key source for a configured provider.
fn log_in(settings: &Settings, path: &Path, login: &Login) -> Result<LoginOutcome, ErrorReport> {
    let login_check = check_login(settings, &login.provider)?;
    store_key(path, &login.provider, &key_source(login)?, login_check)
}

/// Whether a credential can be stored for `provider`: it must be configured.
/// Checked before any prompt. Returns whether the default model's connection
/// has authentication disabled, which is reported but does not block login.
pub(crate) fn check_login(settings: &Settings, provider: &str) -> Result<bool, ErrorReport> {
    if settings.config.providers.is_empty() {
        return Err(settings.config_error(ConfigError::NoProviders));
    }
    if !settings.config.providers.contains_key(provider) {
        return Err(settings.config_error(ConfigError::UnknownProvider));
    }
    Ok(settings
        .config
        .connection_settings(provider, settings.config.default_model.as_deref())
        .map_err(|error| settings.config_error(error))?
        .no_auth
        == Some(true))
}

/// Stores `source` as `provider`'s credential.
pub(crate) fn store_key(
    path: &Path,
    provider: &str,
    source: &KeySource,
    authentication_disabled: bool,
) -> Result<LoginOutcome, ErrorReport> {
    store::update(path, |entries| {
        entries.insert(provider.to_owned(), AuthEntry::ApiKey(source.clone()));
        ((), true)
    })
    .map_err(|error| error.report())?;
    Ok(LoginOutcome {
        version: 1,
        provider: provider.to_owned(),
        source: source.kind(),
        authentication_disabled_for_default_model: authentication_disabled,
        path: path.display().to_string(),
    })
}

/// Removes a provider's stored credential, reporting whether one existed.
pub(crate) fn log_out(path: &Path, provider: &str) -> Result<LogoutOutcome, ErrorReport> {
    let removed = store::update(path, |entries| {
        let removed = entries.remove(provider);
        (removed, removed)
    })
    .map_err(|error| error.report())?;
    Ok(LogoutOutcome {
        version: 1,
        provider: provider.to_owned(),
        removed,
    })
}

/// Lists every configured or stored provider with the source that supplies its key.
fn list(settings: &mut Settings) -> Result<CredentialsOutcome, ErrorReport> {
    settings.load_environment()?;
    let mut names: BTreeSet<&str> = settings
        .config
        .providers
        .keys()
        .map(String::as_str)
        .collect();
    names.extend(settings.credentials.store().providers());
    let rows: Vec<_> = names
        .into_iter()
        .map(|name| credential_row(settings, name))
        .collect();
    Ok(CredentialsOutcome {
        version: 1,
        providers: rows,
    })
}

/// One provider's listing: whether it is configured, its stored source, and
/// the source that supplies its key (or `disabled` when the default model
/// turns authentication off).
fn credential_row(settings: &Settings, name: &str) -> CredentialRow {
    let credentials = &settings.credentials;
    let definition = settings.config.providers.get(name);
    let variable = definition.and_then(|d| d.api_key_env.as_deref());
    let authentication_disabled = settings
        .config
        .connection_settings(name, settings.config.default_model.as_deref())
        .is_ok_and(|connection| connection.no_auth == Some(true));
    let source = if authentication_disabled {
        "disabled"
    } else {
        credentials.winner(name, variable)
    };
    CredentialRow {
        provider: name.to_owned(),
        configured: definition.is_some(),
        stored: credentials
            .store()
            .get(name)
            .map(|entry| entry.source().kind()),
        source,
    }
}

fn write_outcome(outcome: &Outcome, format: render::Format) -> ExitCode {
    let mut sink = io::stdout().lock();
    let written = if format == render::Format::Text {
        text(&mut sink, outcome).map_err(render::RenderError::from)
    } else {
        render::labelled(outcome.label(), outcome, &mut sink, format)
    };
    match written {
        Ok(()) => ExitCode::SUCCESS,
        Err(_) => ExitCode::from(6),
    }
}

fn text(sink: &mut impl io::Write, outcome: &Outcome) -> io::Result<()> {
    match outcome {
        Outcome::Login(value) => {
            writeln!(
                sink,
                "Stored the {} credential for {} in {}",
                render::clean(value.source),
                render::clean(&value.provider),
                render::clean(&value.path)
            )?;
            if value.authentication_disabled_for_default_model {
                writeln!(
                    sink,
                    "Primary authentication is disabled for the default model; this source is available to models that enable it"
                )?;
            }
            Ok(())
        }
        Outcome::Logout(value) => {
            if value.removed {
                writeln!(
                    sink,
                    "Removed the stored credential for {}",
                    render::clean(&value.provider)
                )
            } else {
                writeln!(
                    sink,
                    "No stored credential for {}",
                    render::clean(&value.provider)
                )
            }
        }
        Outcome::List(value) => {
            let rows = &value.providers;
            if rows.is_empty() {
                writeln!(sink, "No providers are configured")?;
            }
            for row in rows {
                let name = render::clean(&row.provider);
                let stored = row.stored.unwrap_or("-");
                let note = if row.configured {
                    ""
                } else {
                    "  (not in the configuration)"
                };
                writeln!(
                    sink,
                    "{name:<16} stored: {stored:<8} using: {}{note}",
                    row.source
                )?;
            }
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_line_strips_only_the_line_ending() {
        for (input, key) in [
            ("key  \n", "key  "),
            ("key\r\n", "key"),
            (" key\t", " key\t"),
            ("key\nnext\n", "key"),
        ] {
            assert_eq!(key_line(&mut input.as_bytes()).unwrap(), key, "{input:?}");
        }
    }

    #[test]
    fn key_line_rejects_missing_and_oversized_input() {
        assert!(matches!(
            key_line(&mut &b""[..]),
            Err(KeyLineError::Missing)
        ));
        let limit = usize::try_from(MAX_KEY_LINE).unwrap();
        let longest = format!("{}\n", "k".repeat(limit - 1));
        assert_eq!(key_line(&mut longest.as_bytes()).unwrap().len(), limit - 1);
        for oversized in [
            "k".repeat(limit + 1),
            format!("{}\n", "k".repeat(limit)),
            "\u{e9}".repeat(limit),
        ] {
            let error = key_line(&mut oversized.as_bytes()).unwrap_err();
            assert!(matches!(error, KeyLineError::TooLong));
            assert_eq!(error.report().code, "credential_too_long");
        }
    }
    fn assert_bytes(outcome: &Outcome, historical: &serde_json::Value, expected_text: &str) {
        for format in [render::Format::Json, render::Format::Toon] {
            let mut actual = Vec::new();
            render::labelled(outcome.label(), outcome, &mut actual, format).unwrap();
            let mut expected = if format == render::Format::Json {
                serde_json::to_vec(historical).unwrap()
            } else {
                nouli_core::codec::encode_toon(historical)
                    .unwrap()
                    .into_bytes()
            };
            expected.push(b'\n');
            assert_eq!(actual, expected, "{format:?}");
        }
        let mut actual = Vec::new();
        text(&mut actual, outcome).unwrap();
        assert_eq!(actual, expected_text.as_bytes());
    }

    #[test]
    fn login_serialized_keys_and_text_keep_their_historical_bytes() {
        for disabled in [false, true] {
            let outcome = Outcome::Login(LoginOutcome {
                version: 1,
                provider: "gateway".into(),
                source: "env",
                authentication_disabled_for_default_model: disabled,
                path: "/tmp/auth.toml".into(),
            });
            let old = serde_json::json!({"version":1,"provider":"gateway","source":"env","authentication_disabled_for_default_model":disabled,"path":"/tmp/auth.toml"});
            let mut expected =
                "Stored the env credential for gateway in /tmp/auth.toml\n".to_owned();
            if disabled {
                expected.push_str("Primary authentication is disabled for the default model; this source is available to models that enable it\n");
            }
            assert_bytes(&outcome, &old, &expected);
        }
    }

    #[test]
    fn logout_serialized_keys_and_text_keep_their_historical_bytes() {
        for removed in [false, true] {
            let outcome = Outcome::Logout(LogoutOutcome {
                version: 1,
                provider: "gateway".into(),
                removed,
            });
            let old = serde_json::json!({"version":1,"provider":"gateway","removed":removed});
            assert_bytes(
                &outcome,
                &old,
                if removed {
                    "Removed the stored credential for gateway\n"
                } else {
                    "No stored credential for gateway\n"
                },
            );
        }
    }

    #[test]
    fn credential_rows_keep_nulls_key_order_and_text_bytes() {
        let outcome = Outcome::List(CredentialsOutcome {
            version: 1,
            providers: vec![
                CredentialRow {
                    provider: "configured".into(),
                    configured: true,
                    stored: Some("env"),
                    source: "disabled",
                },
                CredentialRow {
                    provider: "orphan".into(),
                    configured: false,
                    stored: None,
                    source: "none",
                },
            ],
        });
        let old = serde_json::json!({"version":1,"providers":[{"provider":"configured","configured":true,"stored":"env","source":"disabled"},{"provider":"orphan","configured":false,"stored":null,"source":"none"}]});
        let text = format!(
            "{:<16} stored: {:<8} using: disabled\n{:<16} stored: {:<8} using: none  (not in the configuration)\n",
            "configured", "env", "orphan", "-"
        );
        assert_bytes(&outcome, &old, &text);
        assert_bytes(
            &Outcome::List(CredentialsOutcome {
                version: 1,
                providers: vec![],
            }),
            &serde_json::json!({"version":1,"providers":[]}),
            "No providers are configured\n",
        );
    }
}
