//! Credential sources and the parsed form of the central credential store.
//!
//! This module only parses, validates, and serializes. Reading the file,
//! checking its permissions, and running commands belong to the caller. No
//! error, `Debug`, or `Display` output here contains a credential value.

use std::{collections::BTreeMap, fmt};

use thiserror::Error;

/// Where a provider's credential comes from, as frozen into a saved run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CredentialSource {
    /// An environment variable, read from the shell and then the env file.
    Env(String),
    /// The provider's entry in the central credential store.
    Auth(String),
}

impl fmt::Display for CredentialSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Env(name) => write!(f, "env:{name}"),
            Self::Auth(provider) => write!(f, "auth:{provider}"),
        }
    }
}

/// A credential value. It has no `Display` and never appears in `Debug`.
#[derive(Clone, PartialEq, Eq)]
pub struct Secret(String);

impl Secret {
    /// Wraps a credential value.
    ///
    /// # Errors
    ///
    /// Returns [`AuthError::InvalidKey`] if the value is blank or contains control characters.
    pub fn new(value: String) -> Result<Self, AuthError> {
        if value.trim().is_empty() || value.chars().any(char::is_control) {
            return Err(AuthError::InvalidKey);
        }
        Ok(Self(value))
    }

    #[must_use]
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Secret([REDACTED])")
    }
}

/// How a stored API key is found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeySource {
    /// The key itself.
    Key(Secret),
    /// The name of an environment variable that holds the key.
    Env(String),
    /// A program and arguments, run without a shell; its trimmed output is the key.
    Command(Vec<String>),
}

impl KeySource {
    /// A source that reads the key from a named environment variable.
    ///
    /// # Errors
    ///
    /// Returns [`AuthError::InvalidEnvName`] if `name` is not a valid environment variable name.
    pub fn env(name: String) -> Result<Self, AuthError> {
        if env_name(&name) {
            Ok(Self::Env(name))
        } else {
            Err(AuthError::InvalidEnvName)
        }
    }

    /// A source that runs a program, with no shell, and reads its output.
    ///
    /// # Errors
    ///
    /// Returns [`AuthError::InvalidCommandWords`] if `words` is empty or contains an empty word.
    pub fn command(words: Vec<String>) -> Result<Self, AuthError> {
        if words.is_empty() || words.iter().any(String::is_empty) {
            Err(AuthError::InvalidCommandWords)
        } else {
            Ok(Self::Command(words))
        }
    }

    /// A word for listings; never the value.
    #[must_use]
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Key(_) => "key",
            Self::Env(_) => "env",
            Self::Command(_) => "command",
        }
    }
}

/// One provider's stored credential. Other types, such as OAuth, can be added
/// without changing the file format.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthEntry {
    ApiKey(KeySource),
}

impl AuthEntry {
    #[must_use]
    pub fn source(&self) -> &KeySource {
        match self {
            Self::ApiKey(source) => source,
        }
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum AuthError {
    #[error("the credential store is not valid TOML")]
    Syntax,
    #[error("the credential store may only contain [providers.NAME] tables")]
    Layout,
    #[error("provider \"{0}\" needs `type = \"api_key\"`")]
    MissingType(String),
    #[error("provider \"{0}\" has an unsupported credential type; only api_key is supported")]
    UnknownType(String),
    #[error("provider \"{0}\" must set exactly one of key, env, or command")]
    Source(String),
    #[error("provider \"{provider}\" has an unknown field `{field}`")]
    Field { provider: String, field: String },
    #[error("provider \"{0}\" names an invalid environment variable")]
    InvalidEnv(String),
    #[error("provider \"{0}\" has an invalid command; use a non-empty list of strings")]
    InvalidCommand(String),
    #[error("provider \"{0}\" has an empty or invalid key")]
    EmptyKey(String),
    #[error("a credential cannot be empty or contain control characters")]
    InvalidKey,
    #[error(
        "an environment variable name must use letters, digits, and underscores, and not start with a digit"
    )]
    InvalidEnvName,
    #[error("a credential command must be a non-empty list of non-empty strings")]
    InvalidCommandWords,
}

/// The parsed credential store: provider name to entry.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AuthStore {
    entries: BTreeMap<String, AuthEntry>,
}

fn env_name(name: &str) -> bool {
    let mut bytes = name.bytes();
    bytes
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == b'_')
        && bytes.all(|c| c.is_ascii_alphanumeric() || c == b'_')
}

fn entry(provider: &str, table: &toml::Table) -> Result<AuthEntry, AuthError> {
    let name = || provider.to_owned();
    for field in table.keys() {
        if !matches!(field.as_str(), "type" | "key" | "env" | "command") {
            return Err(AuthError::Field {
                provider: name(),
                field: field.clone(),
            });
        }
    }
    match table.get("type") {
        None => return Err(AuthError::MissingType(name())),
        Some(toml::Value::String(kind)) if kind == "api_key" => {}
        Some(_) => return Err(AuthError::UnknownType(name())),
    }
    let sources = ["key", "env", "command"]
        .iter()
        .filter(|field| table.contains_key(**field))
        .count();
    if sources != 1 {
        return Err(AuthError::Source(name()));
    }
    let source = if let Some(value) = table.get("key") {
        let toml::Value::String(key) = value else {
            return Err(AuthError::EmptyKey(name()));
        };
        KeySource::Key(Secret::new(key.clone()).map_err(|_| AuthError::EmptyKey(name()))?)
    } else if let Some(value) = table.get("env") {
        match value {
            toml::Value::String(variable) if env_name(variable) => KeySource::Env(variable.clone()),
            _ => return Err(AuthError::InvalidEnv(name())),
        }
    } else {
        let Some(toml::Value::Array(items)) = table.get("command") else {
            return Err(AuthError::InvalidCommand(name()));
        };
        let words: Option<Vec<String>> = items
            .iter()
            .map(|item| match item {
                toml::Value::String(word) if !word.is_empty() => Some(word.clone()),
                _ => None,
            })
            .collect();
        match words {
            Some(words) if !words.is_empty() => KeySource::Command(words),
            _ => return Err(AuthError::InvalidCommand(name())),
        }
    };
    Ok(AuthEntry::ApiKey(source))
}

impl AuthStore {
    /// Parses the file's text. The parser's own errors are never passed on,
    /// because they quote the line they failed on.
    ///
    /// # Errors
    ///
    /// - [`AuthError::Syntax`] if the text is not valid TOML.
    /// - [`AuthError::Layout`] if it contains anything other than `[providers.NAME]` tables.
    /// - [`AuthError::Field`], [`AuthError::MissingType`], [`AuthError::UnknownType`],
    ///   [`AuthError::Source`], [`AuthError::EmptyKey`], [`AuthError::InvalidEnv`], or
    ///   [`AuthError::InvalidCommand`] if a provider entry is invalid.
    pub fn parse(input: &str) -> Result<Self, AuthError> {
        let table: toml::Table = input.parse().map_err(|_| AuthError::Syntax)?;
        let mut entries = BTreeMap::new();
        for (key, value) in &table {
            let (true, toml::Value::Table(providers)) = (key == "providers", value) else {
                return Err(AuthError::Layout);
            };
            for (provider, value) in providers {
                let toml::Value::Table(fields) = value else {
                    return Err(AuthError::Layout);
                };
                entries.insert(provider.clone(), entry(provider, fields)?);
            }
        }
        Ok(Self { entries })
    }

    /// The file's text, including any stored key. Only the writer should call it.
    ///
    /// # Errors
    ///
    /// Returns [`AuthError::Syntax`] if the store cannot be serialized as TOML.
    pub fn to_toml(&self) -> Result<String, AuthError> {
        let mut providers = toml::Table::new();
        for (provider, entry) in &self.entries {
            let mut fields = toml::Table::new();
            fields.insert("type".into(), "api_key".into());
            match entry.source() {
                KeySource::Key(secret) => {
                    fields.insert("key".into(), secret.expose().into());
                }
                KeySource::Env(name) => {
                    fields.insert("env".into(), name.as_str().into());
                }
                KeySource::Command(words) => {
                    fields.insert(
                        "command".into(),
                        toml::Value::Array(words.iter().map(|word| word.as_str().into()).collect()),
                    );
                }
            }
            providers.insert(provider.clone(), toml::Value::Table(fields));
        }
        let mut root = toml::Table::new();
        if !providers.is_empty() {
            root.insert("providers".into(), toml::Value::Table(providers));
        }
        toml::to_string(&root).map_err(|_| AuthError::Syntax)
    }

    #[must_use]
    pub fn get(&self, provider: &str) -> Option<&AuthEntry> {
        self.entries.get(provider)
    }

    pub fn insert(&mut self, provider: String, entry: AuthEntry) {
        self.entries.insert(provider, entry);
    }

    /// Removes a provider's entry, and says whether there was one.
    pub fn remove(&mut self, provider: &str) -> bool {
        self.entries.remove(provider).is_some()
    }

    pub fn providers(&self) -> impl Iterator<Item = &str> {
        self.entries.keys().map(String::as_str)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_constructors_report_errors_without_a_provider() {
        assert_eq!(
            KeySource::env("1BAD".into()).unwrap_err(),
            AuthError::InvalidEnvName
        );
        assert_eq!(
            KeySource::command(vec!["tool".into(), String::new()]).unwrap_err(),
            AuthError::InvalidCommandWords
        );
        assert!(!AuthError::InvalidEnvName.to_string().contains("provider"));
        assert!(matches!(
            KeySource::env("API_KEY".into()),
            Ok(KeySource::Env(_))
        ));
    }

    const FILE: &str = r#"
[providers.typesafe]
type = "api_key"
key = "SENTINEL_KEY_VALUE"

[providers.openai]
type = "api_key"
env = "OPENAI_API_KEY"

[providers.router]
type = "api_key"
command = ["op", "read", "op://Private/Router/credential"]
"#;

    #[test]
    fn the_three_key_sources_parse_and_round_trip() {
        let store = AuthStore::parse(FILE).unwrap();
        assert_eq!(
            store.providers().collect::<Vec<_>>(),
            ["openai", "router", "typesafe"]
        );
        let kinds: Vec<_> = store
            .providers()
            .map(|p| store.get(p).unwrap().source().kind())
            .collect();
        assert_eq!(kinds, ["env", "command", "key"]);
        assert!(matches!(
            store.get("typesafe").unwrap().source(),
            KeySource::Key(secret) if secret.expose() == "SENTINEL_KEY_VALUE"
        ));
        assert_eq!(AuthStore::parse(&store.to_toml().unwrap()).unwrap(), store);
        assert_eq!(AuthStore::default().to_toml().unwrap(), "");
        assert_eq!(AuthStore::parse("").unwrap(), AuthStore::default());
    }

    #[test]
    fn mistakes_name_the_provider_and_never_repeat_a_value() {
        let case = |body: &str| AuthStore::parse(&format!("[providers.p]\n{body}")).unwrap_err();
        assert_eq!(case("key = \"X\""), AuthError::MissingType("p".into()));
        assert_eq!(
            case("type = \"oauth\"\nkey = \"X\""),
            AuthError::UnknownType("p".into())
        );
        assert_eq!(
            case("type = 3\nkey = \"X\""),
            AuthError::UnknownType("p".into())
        );
        assert_eq!(case("type = \"api_key\""), AuthError::Source("p".into()));
        assert_eq!(
            case("type = \"api_key\"\nkey = \"X\"\nenv = \"Y\""),
            AuthError::Source("p".into())
        );
        assert_eq!(
            case("type = \"api_key\"\nkey = \"X\"\nextra = 1"),
            AuthError::Field {
                provider: "p".into(),
                field: "extra".into()
            }
        );
        assert_eq!(
            case("type = \"api_key\"\nenv = \"1BAD\""),
            AuthError::InvalidEnv("p".into())
        );
        assert_eq!(
            case("type = \"api_key\"\ncommand = []"),
            AuthError::InvalidCommand("p".into())
        );
        assert_eq!(
            case("type = \"api_key\"\ncommand = \"op read\""),
            AuthError::InvalidCommand("p".into())
        );
        assert_eq!(
            case("type = \"api_key\"\nkey = \"  \""),
            AuthError::EmptyKey("p".into())
        );
        assert_eq!(
            AuthStore::parse("not toml SECRET ="),
            Err(AuthError::Syntax)
        );
        assert_eq!(AuthStore::parse("other = 1"), Err(AuthError::Layout));
        assert_eq!(
            AuthStore::parse("[providers]\np = 1"),
            Err(AuthError::Layout)
        );
        for input in [
            "[providers.p]\ntype = \"api_key\"\nkey = \"SENTINEL\"\nextra = 1",
            "x = \"SENTINEL\" =",
        ] {
            let error = AuthStore::parse(input).unwrap_err();
            let text = format!("{error:?} {error}");
            assert!(!text.contains("SENTINEL"), "{text}");
        }
    }

    #[test]
    fn secrets_are_redacted_and_sources_display_their_kind() {
        let store = AuthStore::parse(FILE).unwrap();
        assert!(!format!("{store:?}").contains("SENTINEL_KEY_VALUE"));
        assert!(Secret::new("a\nb".into()).is_err());
        assert!(Secret::new(String::new()).is_err());
        assert_eq!(CredentialSource::Env("NAME".into()).to_string(), "env:NAME");
        assert_eq!(CredentialSource::Auth("p".into()).to_string(), "auth:p");
    }

    #[test]
    fn entries_can_be_replaced_and_removed() {
        let mut store = AuthStore::default();
        store.insert(
            "p".into(),
            AuthEntry::ApiKey(KeySource::Env("TOKEN".into())),
        );
        store.insert(
            "p".into(),
            AuthEntry::ApiKey(KeySource::Command(vec!["echo".into()])),
        );
        assert_eq!(store.get("p").unwrap().source().kind(), "command");
        assert!(store.remove("p") && !store.remove("p"));
    }
}
