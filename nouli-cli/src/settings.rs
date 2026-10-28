//! CLI-owned filesystem/environment discovery. Loading has no ledger side effects.

use std::path::PathBuf;

use nouli_core::config::{Config, ConfigError};

pub(crate) fn load(args: &super::Args) -> Result<(Config, PathBuf), ConfigError> {
    let home = std::env::var_os("HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .ok_or(ConfigError::Invalid("HOME"))?;
    let path = args
        .config
        .clone()
        .unwrap_or_else(|| home.join(".nouli/config.toml"));
    let config = Config::load(&path, args.config.is_none())?;
    let store = config.store_path(&home, args.store.as_deref())?;
    Ok((config, store))
}
