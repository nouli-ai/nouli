//! `nouli auth`: store, remove, and list provider credentials. Values are
//! never printed, logged, or put in an error.
use crate::{
    Args, inspect, render, render_error,
    settings::{self, auth as store},
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
    io::{self, BufRead, IsTerminal},
    process::ExitCode,
};

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

/// One line from the reader, without its line ending.
fn line(reader: &mut impl BufRead) -> Option<String> {
    let mut text = String::new();
    let mut limited = io::Read::take(reader, 16 * 1024);
    if limited.read_line(&mut text).ok()? == 0 {
        return None;
    }
    Some(text.trim_end_matches(['\n', '\r']).to_owned())
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
        line(&mut io::stdin().lock())
            .ok_or_else(|| input("credential_required", "No key was read from standard input"))?
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
    let outcome = (|| -> Result<(&'static str, serde_json::Value), ErrorReport> {
        match command {
            Command::Login(login) => {
                if settings.config.providers.is_empty() {
                    return Err(settings.config_error(ConfigError::NoProviders));
                }
                if !settings.config.providers.contains_key(&login.provider) {
                    return Err(settings.config_error(ConfigError::UnknownProvider));
                }
                let authentication_disabled = settings
                    .config
                    .connection_settings(&login.provider, settings.config.default_model.as_deref())
                    .map_err(|error| settings.config_error(error))?
                    .no_auth
                    == Some(true);
                let source = key_source(login)?;
                store::update(&path, |entries| {
                    entries.insert(login.provider.clone(), AuthEntry::ApiKey(source.clone()));
                    ((), true)
                })
                .map_err(|error| error.report())?;
                Ok((
                    "Stored credential",
                    serde_json::json!({
                        "version": 1,
                        "provider": login.provider,
                        "source": source.kind(),
                        "authentication_disabled_for_default_model": authentication_disabled,
                        "path": path.display().to_string(),
                    }),
                ))
            }
            Command::Logout { provider } => {
                let removed = store::update(&path, |entries| {
                    let removed = entries.remove(provider);
                    (removed, removed)
                })
                .map_err(|error| error.report())?;
                Ok((
                    "Removed credential",
                    serde_json::json!({"version": 1, "provider": provider, "removed": removed}),
                ))
            }
            Command::List => {
                settings.load_environment()?;
                let credentials = settings.credentials.clone();
                let mut names: std::collections::BTreeSet<&str> = settings
                    .config
                    .providers
                    .keys()
                    .map(String::as_str)
                    .collect();
                names.extend(credentials.store().providers());
                let rows: Vec<_> = names
                    .into_iter()
                    .map(|name| {
                        let definition = settings.config.providers.get(name);
                        let variable = definition.and_then(|d| d.api_key_env.as_deref());
                        serde_json::json!({
                            "provider": name,
                            "configured": definition.is_some(),
                            "stored": credentials.store().get(name).map(|e| e.source().kind()),
                            "source": if settings.config.connection_settings(name, settings.config.default_model.as_deref()).is_ok_and(|connection| connection.no_auth == Some(true)) { "disabled" } else { credentials.winner(name, variable) },
                        })
                    })
                    .collect();
                Ok((
                    "Credentials",
                    serde_json::json!({"version": 1, "providers": rows}),
                ))
            }
        }
    })();
    let (label, value) = match outcome {
        Ok(outcome) => outcome,
        Err(error) => return render_error(args, settings.config.output, error),
    };
    let mut sink = io::stdout().lock();
    let written = if format == render::Format::Text {
        text(&mut sink, command, &value)
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
