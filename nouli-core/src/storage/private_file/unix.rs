use super::Error;
use std::{
    fs, io,
    os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt},
    path::Path,
};

pub(super) fn create_directory(path: &Path) -> Result<(), Error> {
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(path)?;
    Ok(())
}
pub(super) fn open_directory(path: &Path) -> Result<fs::File, Error> {
    Ok(fs::File::open(path)?)
}

fn validate(file: &fs::File, path: &Path) -> Result<(), Error> {
    let metadata = file.metadata()?;
    if !metadata.is_file()
        || metadata.mode() & 0o077 != 0
        || metadata.uid() != nix::unistd::Uid::effective().as_raw()
    {
        return Err(Error::Insecure(path.into()));
    }
    Ok(())
}
pub(super) fn open(path: &Path, write: bool, exclusive: bool) -> Result<fs::File, Error> {
    let file = fs::OpenOptions::new()
        .read(true)
        .write(write)
        .create_new(exclusive)
        .mode(0o600)
        .custom_flags(nix::libc::O_NOFOLLOW)
        .open(path)
        .map_err(|error| {
            if error.raw_os_error() == Some(nix::libc::ELOOP) {
                Error::Insecure(path.into())
            } else {
                Error::Io(error)
            }
        })?;
    validate(&file, path)?;
    Ok(file)
}
pub(super) fn open_lock(path: &Path) -> Result<fs::File, Error> {
    // Existing Unix lock-file modes are preserved, as before this shared API.
    // Lock files contain no credentials; their private parent controls access.
    fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(nix::libc::O_NOFOLLOW)
        .open(path)
        .map_err(|error| {
            if error.raw_os_error() == Some(nix::libc::ELOOP) {
                Error::Insecure(path.into())
            } else {
                Error::Io(error)
            }
        })
}
pub(super) fn publish(source: &Path, destination: &Path, replace: bool) -> Result<(), Error> {
    if replace {
        fs::rename(source, destination)?;
    } else {
        match fs::hard_link(source, destination) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error.into()),
        }
    }
    if fs::File::open(destination.parent().unwrap_or(Path::new(".")))
        .and_then(|file| file.sync_all())
        .is_err()
    {
        tracing::warn!("private directory sync failed after publishing");
    }
    Ok(())
}
