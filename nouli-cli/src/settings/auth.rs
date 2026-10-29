//! The central credential store: reading and writing the file safely, running
//! command sources, and answering credential lookups. A credential value is
//! never logged, printed, or put in an error.

use super::environment::{Environment, Origin};
use nouli_core::{
    config::auth::{AuthError, AuthStore, CredentialSource, KeySource},
    identity::Id,
    protocol::{ErrorCategory, ErrorReport},
};
use std::{
    collections::BTreeSet,
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};
use thiserror::Error;

pub(crate) const FILE_NAME: &str = "auth.toml";
const MAX_STORE_BYTES: u64 = 1024 * 1024;
const MAX_COMMAND_OUTPUT: u64 = 64 * 1024;
const COMMAND_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, Error)]
pub(crate) enum AuthFileError {
    #[error("Cannot read the credential store: {path}")]
    Read { path: PathBuf },
    #[error(
        "The credential store is not safe to read: {path}. Run `chmod 600 {path}`, and make sure you own it"
    )]
    Insecure { path: PathBuf },
    #[error("Invalid credential store {path}: {source}")]
    Invalid { path: PathBuf, source: AuthError },
    #[error("Cannot write the credential store: {path}")]
    Write { path: PathBuf },
}

impl AuthFileError {
    pub(crate) fn report(&self) -> ErrorReport {
        let (category, code) = match self {
            Self::Read { .. } => (ErrorCategory::Configuration, "auth_store_unreadable"),
            Self::Insecure { .. } => (ErrorCategory::Configuration, "auth_store_insecure"),
            Self::Invalid { .. } => (ErrorCategory::Configuration, "invalid_auth_store"),
            Self::Write { .. } => (ErrorCategory::Storage, "auth_store_write_failed"),
        };
        ErrorReport {
            category,
            code: code.into(),
            message: self.to_string(),
            retryable: false,
        }
    }
}

/// The store sits next to the configuration file.
pub(crate) fn path_for(config: &Path) -> PathBuf {
    config
        .parent()
        .map_or_else(|| PathBuf::from(FILE_NAME), |parent| parent.join(FILE_NAME))
}

/// Reads the store, which may be absent. A file that others can read, or that
/// someone else owns, is refused before a byte of it is parsed.
pub(crate) fn read(path: &Path) -> Result<AuthStore, AuthFileError> {
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
    let file = match fs::OpenOptions::new()
        .read(true)
        .custom_flags(nix::libc::O_NOFOLLOW)
        .open(path)
    {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(AuthStore::default());
        }
        Err(error) if error.raw_os_error() == Some(nix::libc::ELOOP) => {
            return Err(AuthFileError::Insecure { path: path.into() });
        }
        Err(_) => return Err(AuthFileError::Read { path: path.into() }),
    };
    let metadata = file
        .metadata()
        .map_err(|_| AuthFileError::Read { path: path.into() })?;
    if !metadata.is_file()
        || metadata.mode() & 0o077 != 0
        || metadata.uid() != nix::unistd::Uid::effective().as_raw()
    {
        return Err(AuthFileError::Insecure { path: path.into() });
    }
    let mut text = String::new();
    file.take(MAX_STORE_BYTES + 1)
        .read_to_string(&mut text)
        .map_err(|_| AuthFileError::Read { path: path.into() })?;
    if text.len() as u64 > MAX_STORE_BYTES {
        return Err(AuthFileError::Read { path: path.into() });
    }
    AuthStore::parse(&text).map_err(|source| AuthFileError::Invalid {
        path: path.into(),
        source,
    })
}

/// Replaces the store atomically. The directory is created private, and the
/// temporary file is private from the moment it exists, so no wider mode is
/// ever visible at the final path.
pub(crate) fn write(path: &Path, store: &AuthStore) -> Result<(), AuthFileError> {
    use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
    let failed = || AuthFileError::Write { path: path.into() };
    let parent = path.parent().ok_or_else(failed)?;
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(parent)
        .map_err(|_| failed())?;
    let text = store.to_toml().map_err(|_| failed())?;
    let temporary = parent.join(format!(".auth-{}.tmp", Id::generate()));
    let written = (|| {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&temporary)?;
        file.write_all(text.as_bytes())?;
        file.flush()?;
        file.sync_all()?;
        fs::rename(&temporary, path)
    })();
    if written.is_err() {
        let _ = fs::remove_file(&temporary);
        return Err(failed());
    }
    if let Ok(directory) = fs::File::open(parent) {
        let _ = directory.sync_all();
    }
    Ok(())
}

/// Runs a command source directly, without a shell, and returns its trimmed
/// output. Its error output is discarded and its output is never reported.
pub(crate) fn run_command(argv: &[String], timeout: Duration) -> Option<String> {
    let (program, arguments) = argv.split_first()?;
    let mut command = Command::new(program);
    command
        .args(arguments)
        .env_clear()
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    for name in ["PATH", "HOME"] {
        if let Some(value) = std::env::var_os(name) {
            command.env(name, value);
        }
    }
    let mut child = command.spawn().ok()?;
    let output = child.stdout.take()?;
    let reader = thread::spawn(move || {
        let mut bytes = Vec::new();
        let _ = output.take(MAX_COMMAND_OUTPUT).read_to_end(&mut bytes);
        bytes
    });
    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(10)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                // A grandchild may still hold the pipe, so do not wait for the reader.
                return None;
            }
        }
    };
    let bytes = reader.join().ok()?;
    if !status.success() {
        return None;
    }
    let text = String::from_utf8(bytes).ok()?;
    let key = text.trim();
    (!key.is_empty()).then(|| key.to_owned())
}

/// Every place a credential can come from, in the order they are tried.
#[derive(Default)]
pub(crate) struct Credentials {
    environment: Environment,
    store: AuthStore,
}

impl Credentials {
    pub(crate) fn new(environment: Environment, store: AuthStore) -> Self {
        Self { environment, store }
    }

    pub(crate) fn store(&self) -> &AuthStore {
        &self.store
    }

    pub(crate) fn lookup(&self, source: &CredentialSource) -> Option<String> {
        match source {
            CredentialSource::Env(name) => self.environment.lookup(name),
            CredentialSource::Auth(provider) => match self.store.get(provider)?.source() {
                KeySource::Key(secret) => Some(secret.expose().to_owned()),
                KeySource::Env(name) => self.environment.lookup(name),
                KeySource::Command(argv) => run_command(argv, COMMAND_TIMEOUT),
            },
        }
    }

    /// Providers with an entry, so a provider with no `api_key_env` can use it.
    pub(crate) fn stored(&self) -> BTreeSet<String> {
        self.store.providers().map(str::to_owned).collect()
    }

    /// Which source currently supplies a provider's key, by name only.
    pub(crate) fn winner(&self, provider: &str, variable: Option<&str>) -> &'static str {
        match variable.and_then(|name| self.environment.origin(name)) {
            Some(Origin::Shell) => "shell",
            Some(Origin::File) => "env file",
            None if self.store.get(provider).is_some() => "store",
            None => "none",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nouli_core::config::auth::AuthEntry;
    use std::os::unix::fs::PermissionsExt;

    struct Directory(PathBuf);
    impl Directory {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!("nouli-auth-{}", Id::generate()));
            Self(path)
        }
    }
    impl Drop for Directory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn mode(path: &Path) -> u32 {
        use std::os::unix::fs::MetadataExt;
        fs::metadata(path).unwrap().mode() & 0o777
    }

    #[test]
    fn writing_creates_a_private_directory_and_file_and_reading_round_trips() {
        let directory = Directory::new();
        let path = directory.0.join("nested").join(FILE_NAME);
        let mut store = AuthStore::default();
        store.insert(
            "p".into(),
            AuthEntry::ApiKey(KeySource::Key(
                nouli_core::config::auth::Secret::new("SENTINEL".into()).unwrap(),
            )),
        );
        write(&path, &store).unwrap();
        assert_eq!(mode(&path), 0o600);
        assert_eq!(mode(path.parent().unwrap()), 0o700);
        assert_eq!(read(&path).unwrap(), store);
        // No temporary file is left behind.
        assert_eq!(fs::read_dir(path.parent().unwrap()).unwrap().count(), 1);
        assert_eq!(
            read(&directory.0.join("absent.toml")).unwrap(),
            AuthStore::default()
        );
    }

    #[test]
    fn a_file_others_can_read_or_a_link_is_refused_without_parsing_it() {
        let directory = Directory::new();
        fs::create_dir_all(&directory.0).unwrap();
        let path = directory.0.join(FILE_NAME);
        fs::write(
            &path,
            "[providers.p]\ntype = \"api_key\"\nkey = \"SENTINEL\"\n",
        )
        .unwrap();
        for wide in [0o640, 0o604, 0o660, 0o644, 0o666] {
            fs::set_permissions(&path, fs::Permissions::from_mode(wide)).unwrap();
            let error = read(&path).unwrap_err();
            assert!(matches!(error, AuthFileError::Insecure { .. }), "{wide:o}");
            let text = format!("{error} {error:?} {}", error.report().message);
            assert!(
                text.contains("chmod 600") && !text.contains("SENTINEL"),
                "{text}"
            );
        }
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        assert!(read(&path).is_ok());
        fs::set_permissions(&path, fs::Permissions::from_mode(0o400)).unwrap();
        assert!(read(&path).is_ok());
        let link = directory.0.join("link.toml");
        std::os::unix::fs::symlink(&path, &link).unwrap();
        assert!(matches!(read(&link), Err(AuthFileError::Insecure { .. })));
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        fs::write(&path, "providers = ").unwrap();
        let error = read(&path).unwrap_err();
        assert!(matches!(error, AuthFileError::Invalid { .. }));
        assert_eq!(error.report().code, "invalid_auth_store");
    }

    #[test]
    fn command_sources_run_without_a_shell_and_failures_yield_nothing() {
        let run = |words: &[&str], timeout| {
            let argv: Vec<String> = words.iter().map(|word| (*word).to_owned()).collect();
            run_command(&argv, timeout)
        };
        let second = Duration::from_secs(5);
        assert_eq!(
            run(&["printf", "  key-value\n"], second).as_deref(),
            Some("key-value")
        );
        // Arguments are not interpreted by a shell.
        assert_eq!(
            run(&["printf", "%s", "$HOME;x"], second).as_deref(),
            Some("$HOME;x")
        );
        assert_eq!(run(&["false"], second), None);
        assert_eq!(run(&["true"], second), None);
        assert_eq!(run(&["no-such-program-anywhere"], second), None);
        assert_eq!(run(&[], second), None);
        // Output from a failing command is never used.
        assert_eq!(run(&["sh", "-c", "echo SENTINEL; exit 1"], second), None);
        let started = Instant::now();
        assert_eq!(run(&["sleep", "30"], Duration::from_millis(200)), None);
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    #[test]
    fn a_command_sees_only_path_and_home() {
        let seen = run_command(
            &[
                "sh".into(),
                "-c".into(),
                "env | cut -d= -f1 | sort | tr '\\n' ' '".into(),
            ],
            Duration::from_secs(5),
        )
        .unwrap();
        for name in seen.split_whitespace() {
            assert!(
                matches!(name, "PATH" | "HOME" | "PWD" | "SHLVL" | "_" | "OLDPWD"),
                "{name} leaked into the command"
            );
        }
    }
}
