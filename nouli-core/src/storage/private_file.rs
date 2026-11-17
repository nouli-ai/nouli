//! Private local files shared by the ledger and its embedding applications.
//!
//! Windows files are owned by the effective user and grant access only to that
//! user, SYSTEM, and local Administrators. Existing ACLs are never repaired.
//! Unix creation uses 0700/0600; reading private files also checks their owner.
#[cfg(unix)]
mod unix;
#[cfg(windows)]
mod windows;
#[cfg(unix)]
use unix as platform;
#[cfg(windows)]
use windows as platform;

use crate::identity::Id;
use std::{
    fs,
    io::{self, Write},
    path::{Path, PathBuf},
};
use thiserror::Error;

/// A protected file could not be accessed safely.
#[derive(Debug, Error)]
pub enum Error {
    /// Permissions, ownership, or a filesystem link violate the private policy.
    #[error(
        "unsafe private file or directory at {0}; restrict access to its owner and administrative principals before retrying"
    )]
    Insecure(PathBuf),
    /// A filesystem operation failed.
    #[error(transparent)]
    Io(#[from] io::Error),
}

impl Error {
    /// Whether this is a missing path, rather than unsafe existing data.
    #[must_use]
    pub fn is_not_found(&self) -> bool {
        matches!(self, Self::Io(error) if error.kind() == io::ErrorKind::NotFound)
    }
}

/// Creates private directories without changing existing permissions.
///
/// # Errors
/// Returns an I/O error, or an unsafe-directory error on Windows.
pub fn create_directory(path: &Path) -> Result<(), Error> {
    platform::create_directory(path)
}

/// Creates a new private regular file, refusing an existing name.
///
/// # Errors
/// Returns an I/O error if creation or protection fails.
pub fn create_new(path: &Path) -> Result<fs::File, Error> {
    platform::open(path, true, true)
}

/// Opens a private regular file for reading without following its final link.
///
/// # Errors
/// Returns an error before reading contents if the file is unsafe or unavailable.
pub fn open_read(path: &Path) -> Result<fs::File, Error> {
    platform::open(path, false, false)
}

/// Opens a private lock file, creating it if absent, without acquiring its lock.
///
/// # Errors
/// Returns an error if the file is unsafe or unavailable.
pub fn open_lock_file(path: &Path) -> Result<fs::File, Error> {
    platform::open_lock(path)
}

/// Validates and pins an existing Windows directory against rename/deletion.
/// On Unix this opens the directory without changing existing permissions.
///
/// # Errors
/// Returns an I/O error or an unsafe-directory error on Windows.
pub fn open_directory(path: &Path) -> Result<fs::File, Error> {
    platform::open_directory(path)
}

fn directory_of(path: &Path) -> &Path {
    path.parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or(Path::new("."))
}

/// Acquires an exclusive lock on a private sibling `<path>.lock` file.
///
/// # Errors
/// Returns an error if directory creation, validation, or locking fails.
pub fn lock_beside(path: &Path) -> Result<fs::File, Error> {
    let directory = directory_of(path);
    create_directory(directory)?;
    let mut name = path
        .file_name()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "path names no file"))?
        .to_owned();
    name.push(".lock");
    let file = open_lock_file(&directory.join(name))?;
    file.lock()?;
    Ok(file)
}

struct Temporary(PathBuf);
impl Drop for Temporary {
    fn drop(&mut self) {
        if let Err(error) = fs::remove_file(&self.0)
            && error.kind() != io::ErrorKind::NotFound
        {
            tracing::warn!("private temporary file cleanup failed");
        }
    }
}

fn publish_inner(path: &Path, bytes: &[u8], replace: bool) -> Result<(), Error> {
    let directory = directory_of(path);
    create_directory(directory)?;
    // Keep the parent pinned throughout validation and publication on Windows.
    #[cfg(windows)]
    let _directory = open_directory(directory)?;
    #[cfg(windows)]
    match open_read(path) {
        Ok(_) => {}
        Err(error) if error.is_not_found() => {}
        Err(error) => return Err(error),
    }
    let name = path.file_name().unwrap_or_default().to_string_lossy();
    let temporary = Temporary(directory.join(format!(".{name}-{}.tmp", Id::generate())));
    let mut file = create_new(&temporary.0)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    drop(file);
    platform::publish(&temporary.0, path, replace)?;
    Ok(())
}

/// Atomically replaces a file using a protected, synced sibling temporary file.
/// The caller must hold the sibling lock across any read/modify/write operation.
///
/// # Errors
/// Returns an error if protection, writing, or publication fails.
pub fn publish(path: &Path, bytes: &[u8]) -> Result<(), Error> {
    publish_inner(path, bytes, true)
}

/// Atomically publishes a private file only when the destination is absent.
///
/// # Errors
/// Returns an error if protection, writing, or publication fails.
pub fn publish_new(path: &Path, bytes: &[u8]) -> Result<(), Error> {
    publish_inner(path, bytes, false)
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Directory(PathBuf);
    impl Drop for Directory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    fn directory() -> Directory {
        Directory(std::env::temp_dir().join(format!("nouli-private-{}", Id::generate())))
    }

    #[test]
    fn publication_and_locking_preserve_complete_values() {
        let root = directory();
        let path = root.0.join("nested/config.toml");
        let held = lock_beside(&path).unwrap();
        let other = open_lock_file(&root.0.join("nested/config.toml.lock")).unwrap();
        assert!(other.try_lock().is_err());
        publish_new(&path, b"first").unwrap();
        publish_new(&path, b"ignored").unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"first");
        publish(&path, b"second").unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"second");
        open_read(&path).unwrap();
        drop(held);
        assert!(other.try_lock().is_ok());
        assert_eq!(fs::read_dir(path.parent().unwrap()).unwrap().count(), 2);
    }

    #[cfg(unix)]
    #[test]
    fn unix_permissions_and_link_refusal_are_enforced() {
        use std::os::unix::fs::PermissionsExt;
        let root = directory();
        let path = root.0.join("auth.toml");
        publish(&path, b"private").unwrap();
        assert_eq!(
            fs::metadata(&root.0).unwrap().permissions().mode() & 0o777,
            0o700
        );
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        assert!(matches!(open_read(&path), Err(Error::Insecure(_))));
        std::os::unix::fs::symlink(&path, root.0.join("link")).unwrap();
        assert!(matches!(
            open_read(&root.0.join("link")),
            Err(Error::Insecure(_))
        ));
        std::os::unix::fs::symlink(root.0.join("missing"), root.0.join("auth.toml.lock")).unwrap();
        assert!(matches!(lock_beside(&path), Err(Error::Insecure(_))));
        assert!(!root.0.join("missing").exists());
    }

    #[cfg(unix)]
    #[test]
    fn publication_does_not_require_directory_read_permission() {
        use std::os::unix::fs::PermissionsExt;
        let root = directory();
        create_directory(&root.0).unwrap();
        fs::set_permissions(&root.0, fs::Permissions::from_mode(0o300)).unwrap();
        let path = root.0.join("private");
        let result = publish(&path, b"first").and_then(|()| publish(&path, b"second"));
        fs::set_permissions(&root.0, fs::Permissions::from_mode(0o700)).unwrap();
        result.unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"second");
    }
}
