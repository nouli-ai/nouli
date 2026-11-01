//! Atomically publish a private starter file without replacing existing config.
use nouli_core::config::{Config, ConfigError};
use std::{io, path::Path};
use thiserror::Error;
const STARTER: &str = include_str!("../../config.default.toml");
#[derive(Debug, Error)]
pub(super) enum BootstrapError {
    #[error(transparent)]
    Config(#[from] ConfigError),
    #[error("cannot create starter configuration")]
    Io(#[from] io::Error),
}
pub(super) fn initialize(path: &Path) -> Result<Config, BootstrapError> {
    match Config::load(path, false) {
        Ok(config) => return Ok(config),
        Err(ConfigError::Read { source, .. }) if source.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    Config::parse(STARTER)?;
    super::private_file::publish_new(path, STARTER.as_bytes())?;
    Ok(Config::load(path, false)?)
}
#[cfg(test)]
mod tests {
    use super::*;
    use nouli_core::identity::Id;
    use std::fs;
    #[test]
    fn competing_initializers_publish_one_complete_private_config_and_preserve_edits() {
        let root = std::env::temp_dir().join(format!("nouli-bootstrap-{}", Id::generate()));
        let path = root.join(".nouli/config.toml");
        std::thread::scope(|scope| {
            let handles = (0..8)
                .map(|_| scope.spawn(|| initialize(&path).unwrap()))
                .collect::<Vec<_>>();
            for handle in handles {
                assert!(handle.join().unwrap().providers.is_empty());
            }
        });
        assert_eq!(fs::read_to_string(&path).unwrap(), STARTER);
        assert_eq!(fs::read_dir(path.parent().unwrap()).unwrap().count(), 1);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
            assert_eq!(
                fs::metadata(path.parent().unwrap())
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o700
            );
        }
        let edited = "output='json'\n# User configuration\n";
        fs::write(&path, edited).unwrap();
        assert_eq!(
            initialize(&path).unwrap().output,
            nouli_core::config::OutputFormat::Json
        );
        assert_eq!(fs::read_to_string(&path).unwrap(), edited);
        fs::write(&path, "invalid configuration content").unwrap();
        assert!(initialize(&path).is_err());
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            "invalid configuration content"
        );
        fs::remove_dir_all(root).unwrap();
    }
}
