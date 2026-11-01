//! Terminal prompts and choices for connection authoring.
use super::{Connect, ProtocolArg, Recipe, SetupError};
use crate::settings::config_file::read;
use clap::ValueEnum;
use nouli_core::config::{ConnectionSettings, Protocol, auth::KeySource};
use std::{
    io::{self, IsTerminal, Write},
    path::Path,
};
pub(crate) fn read_key(label: &str) -> Result<nouli_core::config::auth::Secret, SetupError> {
    let value = rpassword::prompt_password(label)?;
    if value == "/cancel" {
        return Err(SetupError::Cancelled);
    }
    nouli_core::config::auth::Secret::new(value).map_err(|_| SetupError::InvalidKey)
}
pub(crate) fn prompt(label: &str, default: &str) -> Result<String, SetupError> {
    if !io::stdin().is_terminal() {
        return Err(SetupError::Terminal);
    }
    write!(
        io::stdout(),
        "{label}{}: ",
        if default.is_empty() {
            String::new()
        } else {
            format!(" [{default}]")
        }
    )?;
    io::stdout().flush()?;
    let mut line = String::new();
    if io::stdin().read_line(&mut line)? == 0 {
        return Err(SetupError::Cancelled);
    }
    let line = line.trim();
    if line == "/cancel" {
        return Err(SetupError::Cancelled);
    }
    Ok(if line.is_empty() {
        default.into()
    } else {
        line.into()
    })
}
pub(super) fn choose(
    value: Option<String>,
    label: &str,
    default: &str,
    interactive: bool,
) -> Result<String, SetupError> {
    match value {
        Some(v) => Ok(v),
        None if interactive => prompt(label, default),
        None if !default.is_empty() => Ok(default.into()),
        None => Err(SetupError::Terminal),
    }
}
pub(super) fn choose_protocol(
    options: &Connect,
    recipe: Option<&Recipe>,
    interactive: bool,
) -> Result<Protocol, SetupError> {
    if let Some(protocol) = options.protocol {
        return Ok(protocol.into());
    }
    if let Some(recipe) = recipe {
        return Ok(recipe.protocol);
    }
    let chosen = choose(
        None,
        "Protocol (system-one or openai-decisions)",
        "",
        interactive,
    )?;
    ProtocolArg::from_str(&chosen, false)
        .map(Into::into)
        .map_err(|_| SetupError::InvalidProtocol)
}
/// The endpoint from the options or recipe. Interactive Cloudflare setup
/// builds it from the account ID, since its recipe has no fixed endpoint.
pub(super) fn choose_endpoint(
    options: &Connect,
    service: &str,
    recipe: Option<&Recipe>,
    interactive: bool,
) -> Result<String, SetupError> {
    let mut url = options
        .base_url
        .clone()
        .unwrap_or_else(|| recipe.map_or("", |r| r.url).into());
    if url.is_empty() && service == "cloudflare" && interactive {
        let account = prompt("Cloudflare account ID", "")?;
        if !account.bytes().all(|b| b.is_ascii_alphanumeric()) || account.is_empty() {
            return Err(SetupError::InvalidAccountId);
        }
        url = format!("https://api.cloudflare.com/client/v4/accounts/{account}/ai");
    }
    choose(
        if url.is_empty() { None } else { Some(url) },
        "Endpoint",
        "",
        interactive,
    )
}
/// The credential source from the options, or from a prompt when the
/// connection needs one. Records the authentication choice on `connection`.
pub(super) fn choose_credential(
    options: &Connect,
    connection: &mut ConnectionSettings,
    interactive: bool,
) -> Result<Option<KeySource>, SetupError> {
    if options.no_auth {
        connection.no_auth = Some(true);
        return Ok(None);
    }
    let key = if let Some(env) = &options.env {
        KeySource::env(env.clone()).map_err(|_| SetupError::InvalidEnvironmentVariable)?
    } else if !options.command.is_empty() {
        KeySource::command(options.command.clone()).map_err(|_| SetupError::InvalidCommand)?
    } else if options.stdin {
        let key = crate::auth::key_line(&mut io::stdin().lock())?;
        KeySource::Key(
            nouli_core::config::auth::Secret::new(key).map_err(|_| SetupError::InvalidKey)?,
        )
    } else if connection.no_auth != Some(true) {
        if !interactive {
            return Err(SetupError::Terminal);
        }
        return prompt_credential(connection);
    } else {
        return Ok(None);
    };
    connection.no_auth = Some(false);
    Ok(Some(key))
}
fn prompt_credential(connection: &mut ConnectionSettings) -> Result<Option<KeySource>, SetupError> {
    let method = prompt("Authentication (key, env, command, none)", "key")?;
    if method == "none" {
        connection.no_auth = Some(true);
        return Ok(None);
    }
    prompted_source(&method).map(Some)
}
/// Prompts for the credential source `method` names.
pub(super) fn prompted_source(method: &str) -> Result<KeySource, SetupError> {
    match method {
        "key" => Ok(KeySource::Key(read_key("API key: ")?)),
        "env" => KeySource::env(prompt("Environment variable", "")?)
            .map_err(|_| SetupError::InvalidEnvironmentVariable),
        "command" => KeySource::command(
            shlex::split(&prompt("Credential program and arguments", "")?)
                .ok_or(SetupError::InvalidCommand)?,
        )
        .map_err(|_| SetupError::InvalidCommand),
        _ => Err(SetupError::InvalidCredentialSource),
    }
}
pub(super) fn choose_model(
    options: &Connect,
    recipe: Option<&Recipe>,
    interactive: bool,
) -> Result<String, SetupError> {
    let suggestions = recipe.map_or(&[][..], |r| r.models);
    if interactive && !suggestions.is_empty() {
        let _ = writeln!(
            io::stdout(),
            "Suggested models: {} (any model ID is accepted)",
            suggestions.join(", ")
        );
    }
    choose(
        options.model.clone(),
        "Model ID",
        suggestions.first().copied().unwrap_or(""),
        interactive,
    )
}
pub(super) fn prompt_default(file: &Path) -> Result<bool, SetupError> {
    let suggested = if read(file)?.get("default_provider").is_none() {
        "yes"
    } else {
        "no"
    };
    Ok(prompt("Make startup default? (yes/no)", suggested)? == "yes")
}
