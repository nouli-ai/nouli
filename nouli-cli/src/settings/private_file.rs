//! Private settings files: a private directory, a lock file beside each file,
//! and atomic replacement through a private temporary file, so no partial file
//! or wider mode is ever visible at the final path.
use nouli_core::identity::Id;
use std::{
    fs,
    io::{self, Write},
    os::unix::fs::{DirBuilderExt, OpenOptionsExt},
    path::{Path, PathBuf},
};

/// The directory holding `path`; a bare file name is in the current directory.
fn directory_of(path: &Path) -> &Path {
    match path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent,
        _ => Path::new("."),
    }
}

/// Creates the directory holding `path`, private to the owner, and returns it.
fn create_directory(path: &Path) -> io::Result<&Path> {
    let directory = directory_of(path);
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(directory)?;
    Ok(directory)
}

/// Opens and exclusively locks `<path>.lock`, creating the private directory
/// and the lock file. The lock is released when the file is dropped.
pub(crate) fn lock_beside(path: &Path) -> io::Result<fs::File> {
    let directory = create_directory(path)?;
    let mut name = path
        .file_name()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "path names no file"))?
        .to_owned();
    name.push(".lock");
    let lock = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(nix::libc::O_NOFOLLOW)
        .open(directory.join(name))?;
    lock.lock()?;
    Ok(lock)
}

/// A private temporary file, removed when dropped unless it has been renamed.
struct Temporary(PathBuf);

impl Temporary {
    /// Writes `bytes` durably to a new private temporary file beside `path`.
    fn write(directory: &Path, path: &Path, bytes: &[u8]) -> io::Result<Self> {
        let name = path
            .file_name()
            .map_or_else(|| "file".into(), |name| name.to_string_lossy().into_owned());
        let temporary = Self(directory.join(format!(".{name}-{}.tmp", Id::generate())));
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&temporary.0)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        Ok(temporary)
    }
}

impl Drop for Temporary {
    fn drop(&mut self) {
        if let Err(error) = fs::remove_file(&self.0)
            && error.kind() != io::ErrorKind::NotFound
        {
            tracing::warn!("private temporary file cleanup failed");
        }
    }
}

/// Syncs the directory so a published name survives a crash. The file is
/// already in place, so a failure is only reported.
fn sync_directory(directory: &Path) {
    if fs::File::open(directory)
        .and_then(|directory| directory.sync_all())
        .is_err()
    {
        tracing::warn!("settings directory sync failed after publishing");
    }
}

/// Replaces `path` with `bytes` atomically.
pub(crate) fn publish(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let directory = create_directory(path)?;
    let temporary = Temporary::write(directory, path, bytes)?;
    fs::rename(&temporary.0, path)?;
    sync_directory(directory);
    Ok(())
}

/// Publishes `bytes` at `path` only if nothing is there yet; an existing file
/// is kept as it is.
pub(crate) fn publish_new(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let directory = create_directory(path)?;
    let temporary = Temporary::write(directory, path, bytes)?;
    match fs::hard_link(&temporary.0, path) {
        Ok(()) => sync_directory(directory),
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(error),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    struct Directory(PathBuf);
    impl Drop for Directory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    fn directory() -> Directory {
        Directory(std::env::temp_dir().join(format!("nouli-private-{}", Id::generate())))
    }
    fn mode(path: &Path) -> u32 {
        fs::metadata(path).unwrap().permissions().mode() & 0o777
    }

    #[test]
    fn bare_file_names_live_in_the_current_directory() {
        assert_eq!(directory_of(Path::new("config.toml")), Path::new("."));
        assert_eq!(directory_of(Path::new("")), Path::new("."));
        assert_eq!(directory_of(Path::new("a/config.toml")), Path::new("a"));
        assert_eq!(directory_of(Path::new("/config.toml")), Path::new("/"));
    }

    #[test]
    fn publish_replaces_privately_and_leaves_only_the_file_and_its_lock() {
        let root = directory();
        let path = root.0.join("nested/config.toml");
        let lock = lock_beside(&path).unwrap();
        publish(&path, b"first").unwrap();
        publish(&path, b"second").unwrap();
        drop(lock);
        assert_eq!(fs::read_to_string(&path).unwrap(), "second");
        assert_eq!(mode(&path), 0o600);
        assert_eq!(mode(path.parent().unwrap()), 0o700);
        let mut names = fs::read_dir(path.parent().unwrap())
            .unwrap()
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .collect::<Vec<_>>();
        names.sort();
        assert_eq!(names, ["config.toml", "config.toml.lock"]);
    }

    #[test]
    fn lock_beside_refuses_a_symlinked_lock_and_excludes_other_holders() {
        let root = directory();
        let path = root.0.join("config.toml");
        fs::create_dir_all(&root.0).unwrap();
        std::os::unix::fs::symlink(root.0.join("elsewhere"), root.0.join("config.toml.lock"))
            .unwrap();
        assert!(lock_beside(&path).is_err());
        assert!(!root.0.join("elsewhere").exists());
        fs::remove_file(root.0.join("config.toml.lock")).unwrap();
        let held = lock_beside(&path).unwrap();
        let other = fs::File::open(root.0.join("config.toml.lock")).unwrap();
        assert!(other.try_lock().is_err());
        drop(held);
        assert!(other.try_lock().is_ok());
    }

    #[test]
    fn publish_new_keeps_an_existing_file() {
        let root = directory();
        let path = root.0.join("config.toml");
        publish_new(&path, b"first").unwrap();
        publish_new(&path, b"second").unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "first");
        assert_eq!(fs::read_dir(&root.0).unwrap().count(), 1);
    }

    #[test]
    fn failed_publish_removes_its_temporary_file() {
        let root = directory();
        let path = root.0.join("config.toml");
        fs::create_dir_all(&path).unwrap();
        fs::write(path.join("occupied"), "").unwrap();
        assert!(publish(&path, b"text").is_err());
        assert_eq!(fs::read_dir(&root.0).unwrap().count(), 1);
    }
}
