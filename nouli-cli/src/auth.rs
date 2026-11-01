//! `nouli auth`: store, remove, and list provider credentials. Values are
//! never printed, logged, or put in an error.
use crate::{
    Args, inspect, render, render_error,
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

/// A completed auth command: its structured-output label and its report.
pub(crate) type Outcome = (&'static str, serde_json::Value);

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
    let format = match render::resolve(
        &args.output,
        settings.config.output,
        io::stdout().is_terminal(),
    ) {
        Ok(format) if format != render::Format::Jsonl => format,
        _ => return ExitCode::from(2),
    };
    let path = store::path_for(&settings.config_path);
    let outcome = match command {
        Command::Login(login) => log_in(&settings, &path, login),
        Command::Logout { provider } => log_out(&path, provider),
        Command::List => list(&mut settings),
    };
    match outcome {
        Ok(outcome) => write_outcome(command, &outcome, format),
        Err(error) => render_error(args, settings.config.output, error),
    }
}

/// Stores a key source for a configured provider.
fn log_in(settings: &Settings, path: &Path, login: &Login) -> Result<Outcome, ErrorReport> {
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
) -> Result<Outcome, ErrorReport> {
    store::update(path, |entries| {
        entries.insert(provider.to_owned(), AuthEntry::ApiKey(source.clone()));
        ((), true)
    })
    .map_err(|error| error.report())?;
    Ok((
        "Stored credential",
        serde_json::json!({
            "version": 1,
            "provider": provider,
            "source": source.kind(),
            "authentication_disabled_for_default_model": authentication_disabled,
            "path": path.display().to_string(),
        }),
    ))
}

/// Removes a provider's stored credential, reporting whether one existed.
pub(crate) fn log_out(path: &Path, provider: &str) -> Result<Outcome, ErrorReport> {
    let removed = store::update(path, |entries| {
        let removed = entries.remove(provider);
        (removed, removed)
    })
    .map_err(|error| error.report())?;
    Ok((
        "Removed credential",
        serde_json::json!({"version": 1, "provider": provider, "removed": removed}),
    ))
}

/// Lists every configured or stored provider with the source that supplies its key.
fn list(settings: &mut Settings) -> Result<Outcome, ErrorReport> {
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
    Ok((
        "Credentials",
        serde_json::json!({"version": 1, "providers": rows}),
    ))
}

/// One provider's listing: whether it is configured, its stored source, and
/// the source that supplies its key (or `disabled` when the default model
/// turns authentication off).
fn credential_row(settings: &Settings, name: &str) -> serde_json::Value {
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
    serde_json::json!({
        "provider": name,
        "configured": definition.is_some(),
        "stored": credentials.store().get(name).map(|e| e.source().kind()),
        "source": source,
    })
}

fn write_outcome(command: &Command, (label, value): &Outcome, format: render::Format) -> ExitCode {
    let mut sink = io::stdout().lock();
    let written = if format == render::Format::Text {
        text(&mut sink, command, value)
    } else {
        inspect::write(label, value, &mut sink, format).map_err(|_| io::Error::other("output"))
    };
    match written {
        Ok(()) => ExitCode::SUCCESS,
        Err(_) => ExitCode::from(6),
    }
}

fn text(sink: &mut impl io::Write, command: &Command, value: &serde_json::Value) -> io::Result<()> {
    let word = |key: &str| render::clean(value[key].as_str().unwrap_or(""));
    match command {
        Command::Login(_) => {
            writeln!(
                sink,
                "Stored the {} credential for {} in {}",
                word("source"),
                word("provider"),
                word("path")
            )?;
            if value["authentication_disabled_for_default_model"] == true {
                writeln!(
                    sink,
                    "Primary authentication is disabled for the default model; this source is available to models that enable it"
                )?;
            }
            Ok(())
        }
        Command::Logout { .. } => {
            if value["removed"] == true {
                writeln!(
                    sink,
                    "Removed the stored credential for {}",
                    word("provider")
                )
            } else {
                writeln!(sink, "No stored credential for {}", word("provider"))
            }
        }
        Command::List => {
            let rows = value["providers"].as_array().map_or(&[][..], Vec::as_slice);
            if rows.is_empty() {
                writeln!(sink, "No providers are configured")?;
            }
            for row in rows {
                let name = render::clean(row["provider"].as_str().unwrap_or(""));
                let stored = row["stored"].as_str().unwrap_or("-");
                let note = if row["configured"] == false {
                    "  (not in the configuration)"
                } else {
                    ""
                };
                writeln!(
                    sink,
                    "{name:<16} stored: {stored:<8} using: {}{note}",
                    row["source"].as_str().unwrap_or("none")
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
}
