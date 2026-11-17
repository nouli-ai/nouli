//! CLI adapters for the shared protected-filesystem implementation.
use nouli_core::storage::private_file::{self, Error};
use std::{fs::File, io, path::Path};

fn error(error: Error) -> io::Error {
    match error {
        Error::Io(error) => error,
        error @ Error::Insecure(_) => io::Error::new(io::ErrorKind::PermissionDenied, error),
    }
}
pub(crate) fn lock_beside(path: &Path) -> io::Result<File> {
    private_file::lock_beside(path).map_err(error)
}
pub(crate) fn publish(path: &Path, bytes: &[u8]) -> io::Result<()> {
    private_file::publish(path, bytes).map_err(error)
}
pub(crate) fn publish_new(path: &Path, bytes: &[u8]) -> io::Result<()> {
    private_file::publish_new(path, bytes).map_err(error)
}
