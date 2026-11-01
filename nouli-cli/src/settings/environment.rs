//! CLI credential sources. Parsing never mutates the process environment.

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

use nouli_core::protocol::{ErrorCategory, ErrorReport};
use thiserror::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Origin {
    Shell,
    File,
}

// Credential values deliberately have no Debug or serialization implementation.
#[derive(Clone, Default)]
pub(crate) struct Environment {
    values: BTreeMap<String, String>,
}

#[derive(Debug, Error)]
pub(crate) enum EnvironmentError {
    #[error("Cannot determine the current directory for the environment file")]
    CurrentDirectory,
    #[error("Environment file not found: {path}")]
    Missing { path: PathBuf },
    #[error("Cannot read environment file: {path}")]
    Read { path: PathBuf },
    // Parser errors can contain the original line, including a credential.
    #[error("Invalid environment file syntax or encoding: {path}")]
    Invalid { path: PathBuf },
}

impl EnvironmentError {
    pub(crate) fn report(&self) -> ErrorReport {
        ErrorReport {
            category: ErrorCategory::Configuration,
            code: match self {
                Self::CurrentDirectory => "env_directory_unavailable",
                Self::Missing { .. } => "env_file_not_found",
                Self::Read { .. } => "env_file_unreadable",
                Self::Invalid { .. } => "invalid_env_file",
            }
            .into(),
            message: self.to_string(),
            retryable: false,
        }
    }
}

impl Environment {
    pub(crate) fn load(explicit: Option<&Path>) -> Result<Self, EnvironmentError> {
        let directory = std::env::current_dir().map_err(|_| EnvironmentError::CurrentDirectory)?;
        let path = directory.join(explicit.unwrap_or(Path::new(".env")));
        let input = match std::fs::read_to_string(&path) {
            Ok(input) => input,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return if explicit.is_none() {
                    Ok(Self::default())
                } else {
                    Err(EnvironmentError::Missing { path })
                };
            }
            Err(error) if error.kind() == std::io::ErrorKind::InvalidData => {
                return Err(EnvironmentError::Invalid { path });
            }
            Err(_) => return Err(EnvironmentError::Read { path }),
        };
        // Match dotenvy's line trimming at EOF. Its quoted-line reader otherwise
        // rejects trailing whitespace without a newline. Do not append a newline:
        // that would hide an unfinished final escape from the reader.
        let input = input.strip_prefix('\u{feff}').unwrap_or(&input).trim_end();
        let values = dotenvy::from_read_iter(input.as_bytes())
            .collect::<Result<_, _>>()
            .map_err(|_| EnvironmentError::Invalid { path })?;
        Ok(Self { values })
    }

    /// Where a variable comes from. A value that cannot be used still has an origin.
    pub(crate) fn origin(&self, name: &str) -> Option<Origin> {
        match std::env::var(name) {
            Ok(_) => Some(Origin::Shell),
            Err(std::env::VarError::NotPresent) => {
                self.values.contains_key(name).then_some(Origin::File)
            }
            Err(std::env::VarError::NotUnicode(_)) => None,
        }
    }

    pub(crate) fn lookup(&self, name: &str) -> Option<String> {
        match std::env::var(name) {
            Ok(value) => Some(value),
            Err(std::env::VarError::NotPresent) => self.values.get(name).cloned(),
            // An invalid exported value must not silently fall back to the file.
            Err(std::env::VarError::NotUnicode(_)) => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn eof_whitespace_preserves_quoted_values_and_rejects_unfinished_input() {
        let directory = std::env::temp_dir().join(format!(
            "nouli-env-eof-{}",
            nouli_core::identity::Id::generate()
        ));
        std::fs::create_dir(&directory).unwrap();
        let path = directory.join(".env");
        for (input, expected) in [
            ("VALUE=plain ", "plain"),
            ("VALUE=plain\t", "plain"),
            ("VALUE=plain\n\n \t", "plain"),
            ("VALUE=\"quoted value\" ", "quoted value"),
            ("VALUE='quoted space '\t", "quoted space "),
            ("VALUE=\"quoted space \"  ", "quoted space "),
        ] {
            std::fs::write(&path, input).unwrap();
            let environment = Environment::load(Some(&path)).unwrap();
            assert_eq!(environment.values["VALUE"], expected, "{input:?}");
        }
        for input in [
            "VALUE=unfinished\\",
            "VALUE='unfinished ",
            "VALUE=\"unfinished ",
            "VALUE=\"bad\\q\"",
        ] {
            std::fs::write(&path, input).unwrap();
            assert!(
                matches!(
                    Environment::load(Some(&path)),
                    Err(EnvironmentError::Invalid { .. })
                ),
                "{input:?}"
            );
        }
        std::fs::remove_dir_all(directory).unwrap();
    }
}
