//! The central credential store: reading and writing the file safely, running
//! command sources, and answering credential lookups. A credential value is
//! never logged, printed, or put in an error.

use super::environment::{Environment, Origin};
use nix::{
    sys::signal::{Signal, killpg},
    unistd::Pid,
};
use nouli_core::{
    config::auth::{AuthError, AuthStore, CredentialSource, KeySource},
    identity::Id,
    protocol::{ErrorCategory, ErrorReport},
};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{Mutex, mpsc},
    thread,
    time::{Duration, Instant},
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
    run_command_until(argv, timeout, &CancellationToken::new())
}

/// What the reader thread reports about the command's output.
enum Output {
    Complete(Vec<u8>),
    TooLong,
}

/// Runs a command in its own process group, so a deadline, a cancellation, or
/// an oversized output stops the command and everything it started. The whole
/// run, including the wait for the output to close, ends by the deadline.
pub(crate) fn run_command_until(
    argv: &[String],
    timeout: Duration,
    cancel: &CancellationToken,
) -> Option<String> {
    use std::os::unix::process::CommandExt;
    let (program, arguments) = argv.split_first()?;
    let mut command = Command::new(program);
    command
        .args(arguments)
        .env_clear()
        .process_group(0)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    for name in ["PATH", "HOME"] {
        if let Some(value) = std::env::var_os(name) {
            command.env(name, value);
        }
    }
    let mut child = command.spawn().ok()?;
    let group = Pid::from_raw(i32::try_from(child.id()).ok()?);
    let Some(mut output) = child.stdout.take() else {
        stop(&mut child, group);
        return None;
    };
    let (sender, receiver) = mpsc::channel();
    // The reader ends when every holder of the pipe closes it, which a
    // grandchild can delay; it is only read from, never joined.
    thread::spawn(move || {
        let mut bytes = Vec::new();
        let mut chunk = [0_u8; 4096];
        let result = loop {
            match output.read(&mut chunk) {
                Ok(0) => break Output::Complete(bytes),
                Ok(count) => {
                    bytes.extend_from_slice(&chunk[..count]);
                    if bytes.len() > MAX_COMMAND_OUTPUT {
                        break Output::TooLong;
                    }
                }
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
                Err(_) => break Output::TooLong,
            }
        };
        let _ = sender.send(result);
    });
    let deadline = Instant::now() + timeout;
    let mut status = None;
    let mut bytes = None;
    while status.is_none() || bytes.is_none() {
        let now = Instant::now();
        if now >= deadline || cancel.is_cancelled() {
            tracing::warn!("credential command stopped before it finished");
            stop(&mut child, group);
            return None;
        }
        if status.is_none() {
            match child.try_wait() {
                Ok(Some(done)) if !done.success() => return None,
                Ok(done) => status = done,
                Err(_) => {
                    stop(&mut child, group);
                    return None;
                }
            }
        }
        let wait = (deadline - now).min(Duration::from_millis(10));
        if bytes.is_some() {
            thread::sleep(wait);
            continue;
        }
        match receiver.recv_timeout(wait) {
            Ok(Output::Complete(output)) => bytes = Some(output),
            Ok(Output::TooLong) | Err(mpsc::RecvTimeoutError::Disconnected) => {
                tracing::warn!("credential command output was unusable");
                stop(&mut child, group);
                return None;
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }
    }
    if !status?.success() {
        return None;
    }
    let text = String::from_utf8(bytes?).ok()?;
    let key = text.trim();
    (!key.is_empty()).then(|| key.to_owned())
}

/// Kills the command's whole process group and reaps the command.
fn stop(child: &mut Child, group: Pid) {
    let _ = killpg(group, Signal::SIGKILL);
    let _ = child.kill();
    let _ = child.wait();
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
        let Some(KeySource::Command(argv)) = self.store.get(provider).map(|entry| entry.source())
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
    fn a_grandchild_holding_the_output_open_cannot_outlast_the_timeout() {
        let directory = Directory::new();
        fs::create_dir_all(&directory.0).unwrap();
        let pids = directory.0.join("pid");
        let script = format!("sleep 30 & echo $! > {}; echo k", pids.display());
        let started = Instant::now();
        let argv = ["sh".to_owned(), "-c".into(), script];
        assert_eq!(run_command(&argv, Duration::from_millis(500)), None);
        assert!(started.elapsed() < Duration::from_secs(5));
        let pid: i32 = fs::read_to_string(&pids).unwrap().trim().parse().unwrap();
        // The timed-out command's descendants are gone, not orphaned.
        let gone = (0..100).any(|_| {
            thread::sleep(Duration::from_millis(50));
            nix::sys::signal::kill(Pid::from_raw(pid), None).is_err()
        });
        assert!(gone, "the grandchild is still running");
        // A background job that keeps the output open is not waited for either.
        let argv = ["sh".to_owned(), "-c".into(), "echo k; sleep 30 &".into()];
        let started = Instant::now();
        assert_eq!(run_command(&argv, Duration::from_millis(200)), None);
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn oversized_output_fails_closed_without_waiting_for_the_timeout() {
        let argv = [
            "sh".to_owned(),
            "-c".into(),
            "head -c 200000 /dev/zero | tr '\\0' a; sleep 30".into(),
        ];
        let started = Instant::now();
        assert_eq!(run_command(&argv, Duration::from_secs(20)), None);
        assert!(started.elapsed() < Duration::from_secs(5));
        // Output just within the cap is accepted.
        let argv = [
            "sh".to_owned(),
            "-c".into(),
            "head -c 60000 /dev/zero | tr '\\0' a".into(),
        ];
        assert_eq!(
            run_command(&argv, Duration::from_secs(5)).map(|key| key.len()),
            Some(60000)
        );
    }

    #[test]
    fn a_canceled_command_stops_early() {
        let cancel = CancellationToken::new();
        let stopper = cancel.clone();
        let handle = thread::spawn(move || {
            thread::sleep(Duration::from_millis(100));
            stopper.cancel();
        });
        let started = Instant::now();
        let argv = ["sleep".to_owned(), "30".into()];
        assert_eq!(
            run_command_until(&argv, Duration::from_secs(20), &cancel),
            None
        );
        assert!(started.elapsed() < Duration::from_secs(5));
        handle.join().unwrap();
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
