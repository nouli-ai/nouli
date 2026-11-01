//! The central credential store: reading and writing the file safely, running
//! command sources, and answering credential lookups. A credential value is
//! never logged, printed, or put in an error.

use super::{
    environment::{Environment, Origin},
    private_file,
};
use nouli_core::{
    config::auth::{AuthError, AuthStore, CredentialSource, KeySource},
    protocol::{ErrorCategory, ErrorReport},
};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::Read,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::Mutex,
    time::Duration,
};
use thiserror::Error;
use tokio_util::sync::CancellationToken;

pub(crate) const FILE_NAME: &str = "auth.toml";
const MAX_STORE_BYTES: u64 = 1024 * 1024;
const MAX_COMMAND_OUTPUT: usize = 64 * 1024;
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
    let failed = || AuthFileError::Write { path: path.into() };
    let text = store.to_toml().map_err(|_| failed())?;
    private_file::publish(path, text.as_bytes()).map_err(|_| failed())
}

/// Reads the store, applies `change`, and writes the result back when `change`
/// says it modified the store, all under an exclusive lock on a sibling
/// `auth.toml.lock`, so two commands cannot overwrite each other's entries. The
/// lock is released when this returns.
pub(crate) fn update<T>(
    path: &Path,
    change: impl FnOnce(&mut AuthStore) -> (T, bool),
) -> Result<T, AuthFileError> {
    let _lock =
        private_file::lock_beside(path).map_err(|_| AuthFileError::Write { path: path.into() })?;
    let mut store = read(path)?;
    let (result, modified) = change(&mut store);
    if modified {
        write(path, &store)?;
    }
    Ok(result)
}

/// Runs a command source directly, without a shell, and returns its trimmed
/// output. Its error output is discarded and its output is never reported.
pub(crate) fn run_command(argv: &[String], timeout: Duration) -> Option<String> {
    run_command_until(argv, timeout, &CancellationToken::new())
}

/// Runs a command with the credential boundary's restricted environment.
pub(crate) fn run_command_until(
    argv: &[String],
    timeout: Duration,
    cancel: &CancellationToken,
) -> Option<String> {
    use crate::process::{RunError, run_bounded};
    let (program, arguments) = argv.split_first()?;
    let mut command = Command::new(program);
    command.args(arguments).env_clear().stdout(Stdio::piped());
    for name in ["PATH", "HOME"] {
        if let Some(value) = std::env::var_os(name) {
            command.env(name, value);
        }
    }
    let bytes = match run_bounded(
        &mut command,
        None,
        timeout,
        MAX_COMMAND_OUTPUT as u64,
        Some(cancel),
        None,
    ) {
        Ok(bytes) => bytes,
        Err(RunError::Timeout | RunError::Cancelled) => {
            tracing::warn!("credential command stopped before it finished");
            return None;
        }
        Err(RunError::OutputLimit | RunError::Io(_)) => {
            tracing::warn!("credential command output was unusable");
            return None;
        }
        Err(_) => return None,
    };
    let text = String::from_utf8(bytes).ok()?;
    let key = text.trim();
    (!key.is_empty()).then(|| key.to_owned())
}

/// Every place a credential can come from, in the order they are tried.
#[derive(Default)]
pub(crate) struct Credentials {
    environment: Environment,
    store: AuthStore,
    /// Command results fetched ahead of a turn, by provider, so the engine's
    /// synchronous lookup does not have to wait on a command.
    fetched: Mutex<BTreeMap<String, Option<String>>>,
}

impl Credentials {
    pub(crate) fn new(environment: Environment, store: AuthStore) -> Self {
        Self {
            environment,
            store,
            fetched: Mutex::default(),
        }
    }

    /// Runs the provider's command source, when it has one that a lookup would
    /// reach, off the async runtime and until `cancel`. The result answers the
    /// next lookups until [`Credentials::forget`].
    pub(crate) async fn fetch(
        &self,
        provider: &str,
        variable: Option<&str>,
        cancel: &CancellationToken,
    ) {
        if variable.is_some_and(|name| self.environment.lookup(name).is_some())
            || self
                .fetched
                .lock()
                .is_ok_and(|fetched| fetched.contains_key(provider))
        {
            return;
        }
        let Some(KeySource::Command(argv)) = self
            .store
            .get(provider)
            .map(nouli_core::config::auth::AuthEntry::source)
        else {
            return;
        };
        let (argv, cancel) = (argv.clone(), cancel.clone());
        let Ok(key) =
            tokio::task::spawn_blocking(move || run_command_until(&argv, COMMAND_TIMEOUT, &cancel))
                .await
        else {
            return;
        };
        if let Ok(mut fetched) = self.fetched.lock() {
            fetched.insert(provider.to_owned(), key);
        }
    }

    /// Drops fetched command results, so the next turn runs its command again.
    pub(crate) fn forget(&self) {
        if let Ok(mut fetched) = self.fetched.lock() {
            fetched.clear();
        }
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
                KeySource::Command(argv) => {
                    let fetched = self
                        .fetched
                        .lock()
                        .ok()
                        .and_then(|fetched| fetched.get(provider).cloned());
                    fetched.unwrap_or_else(|| run_command(argv, COMMAND_TIMEOUT))
                }
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
    use nouli_core::{config::auth::AuthEntry, identity::Id};
    use std::{os::unix::fs::PermissionsExt, thread};

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
    fn concurrent_updates_all_survive_under_the_lock() {
        let directory = Directory::new();
        let path = directory.0.join(FILE_NAME);
        let handles: Vec<_> = (0..8)
            .map(|index| {
                let path = path.clone();
                thread::spawn(move || {
                    for round in 0..5 {
                        update(&path, |store| {
                            store.insert(
                                format!("provider-{index}-{round}"),
                                AuthEntry::ApiKey(KeySource::Env("KEY".into())),
                            );
                            ((), true)
                        })
                        .unwrap();
                    }
                })
            })
            .collect();
        for handle in handles {
            handle.join().unwrap();
        }
        assert_eq!(read(&path).unwrap().providers().count(), 40);
        // The lock file is private, and nothing is left beside the store.
        let lock = path.with_file_name(format!("{FILE_NAME}.lock"));
        assert_eq!(mode(&lock), 0o600);
        assert_eq!(fs::read_dir(&directory.0).unwrap().count(), 2);
        // An unmodified store is not rewritten, and a missing entry is reported.
        let removed = update(&path, |store| (store.remove("absent"), false)).unwrap();
        assert!(!removed);
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

    #[tokio::test]
    async fn a_fetched_command_result_answers_lookups_until_forgotten() {
        let directory = Directory::new();
        fs::create_dir_all(&directory.0).unwrap();
        let runs = directory.0.join("runs");
        let script = format!("echo x >> {}; echo key", runs.display());
        let mut store = AuthStore::default();
        store.insert(
            "p".into(),
            AuthEntry::ApiKey(KeySource::Command(vec!["sh".into(), "-c".into(), script])),
        );
        let credentials = Credentials::new(Environment::default(), store);
        let source = CredentialSource::Auth("p".into());
        let count = || fs::read_to_string(&runs).map_or(0, |text| text.lines().count());
        credentials
            .fetch("p", None, &CancellationToken::new())
            .await;
        assert_eq!(credentials.lookup(&source).as_deref(), Some("key"));
        assert_eq!(credentials.lookup(&source).as_deref(), Some("key"));
        assert_eq!(count(), 1);
        credentials.forget();
        assert_eq!(credentials.lookup(&source).as_deref(), Some("key"));
        assert_eq!(count(), 2);
    }
}
