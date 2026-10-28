//! CLI-owned filesystem/environment discovery. Loading has no ledger side effects.

use std::path::PathBuf;

use nouli_core::config::{Config, ConfigError};

pub(crate) struct Settings {
    pub config: Config,
    pub home: PathBuf,
    pub store: PathBuf,
}
pub(crate) fn load(args: &super::Args) -> Result<Settings, ConfigError> {
    discover(args, Config::load)
}
pub(crate) fn load_inspection(args: &super::Args) -> Result<Settings, ConfigError> {
    discover(args, |path, optional| {
        nouli_core::config::StoreSettings::load(path, optional).map(|settings| Config {
            output: settings.output,
            store: settings.store,
            ..Default::default()
        })
    })
}
fn discover(
    args: &super::Args,
    loader: impl FnOnce(&std::path::Path, bool) -> Result<Config, ConfigError>,
) -> Result<Settings, ConfigError> {
    let home = std::env::var_os("HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from);
    let path = args
        .config
        .clone()
        .or_else(|| home.as_ref().map(|home| home.join(".nouli/config.toml")))
        .ok_or(ConfigError::Invalid("HOME or explicit --config"))?;
    let config = loader(&path, args.config.is_none())?;
    if home.is_none() && args.store.is_none() && config.store.is_none() {
        return Err(ConfigError::Invalid("HOME or explicit store"));
    }
    let home = home.unwrap_or_default();
    let store = config.store_path(&home, args.store.as_deref())?;
    Ok(Settings {
        config,
        home,
        store,
    })
}
