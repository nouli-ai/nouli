//! Configuration authoring under a private lock and atomic publication.
use crate::{Args, connect::SetupError, settings};
use nouli_core::{
    config::{
        Config, ProviderDefinition,
        auth::{AuthEntry, KeySource},
    },
    history::ModelSelection,
};
use std::{
    fs, io,
    path::{Path, PathBuf},
};
use toml_edit::{DocumentMut, Item, value};
/// A validated connection, ready to be written to the configuration.
pub(crate) struct Draft {
    pub(crate) name: String,
    pub(crate) model: String,
    pub(crate) definition: ProviderDefinition,
    pub(crate) key: Option<KeySource>,
}

pub(crate) fn path(args: &Args) -> Result<PathBuf, SetupError> {
    args.config
        .clone()
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".nouli/config.toml")))
        .ok_or(SetupError::HomeUnavailable)
}
pub(crate) fn read(path: &Path) -> Result<DocumentMut, SetupError> {
    match fs::read_to_string(path) {
        Ok(s) => {
            Config::parse(&s)?;
            s.parse()
                .map_err(|_| SetupError::Configuration(nouli_core::config::ConfigError::Syntax))
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(DocumentMut::new()),
        Err(source) => Err(nouli_core::config::ConfigError::Read {
            path: path.to_owned(),
            source,
        }
        .into()),
    }
}
/// Replaces the configuration atomically.
fn publish(path: &Path, doc: &DocumentMut) -> Result<(), SetupError> {
    settings::private_file::publish(path, doc.to_string().as_bytes()).map_err(|_| SetupError::Write)
}
pub(crate) fn has_provider(doc: &DocumentMut, name: &str) -> bool {
    doc.get("providers")
        .and_then(|providers| providers.get(name))
        .is_some()
}
/// Opens and locks the configuration's lock file, creating its directory.
fn lock_config(file: &Path) -> Result<fs::File, SetupError> {
    settings::private_file::lock_beside(file).map_err(|_| SetupError::Write)
}
/// Writes the draft under the configuration lock, storing its credential
/// before publishing the configuration.
pub(crate) fn save(
    file: &Path,
    draft: Draft,
    default: bool,
    replace: bool,
) -> Result<ModelSelection, SetupError> {
    let _lock = lock_config(file)?;
    let mut doc = read(file)?;
    if has_provider(&doc, &draft.name) && !replace {
        return Err(SetupError::Exists);
    }
    let provider =
        toml_edit::ser::to_document(&draft.definition).map_err(|_| SetupError::Encode)?;
    doc["providers"][&draft.name] = Item::Table(provider.as_table().clone());
    if default {
        doc["default_provider"] = value(&draft.name);
        doc["default_model"] = value(&draft.model);
    }
    Config::parse(&doc.to_string())?;
    if let Some(key) = draft.key {
        settings::auth::update(&settings::auth::path_for(file), |store| {
            store.insert(draft.name.clone(), AuthEntry::ApiKey(key));
            ((), true)
        })?;
    }
    publish(file, &doc)?;
    Ok(ModelSelection {
        provider: Some(draft.name),
        model: Some(draft.model),
    })
}
/// Updates the configured startup selection while holding the authoring lock.
pub(crate) fn set_default(file: &Path, provider: &str, model: &str) -> Result<(), SetupError> {
    let _lock = lock_config(file)?;
    let mut doc = read(file)?;
    doc["default_provider"] = value(provider);
    doc["default_model"] = value(model);
    Config::parse(&doc.to_string())?;
    publish(file, &doc)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connect::{
        Connect, configure,
        tests::{Directory, custom},
    };

    #[test]
    fn generated_config_is_ordinary_config_and_preserves_unrelated_edits() {
        let dir = Directory::new();
        fs::create_dir_all(&dir.0).unwrap();
        fs::write(
            dir.0.join("config.toml"),
            "# Keep my comment\noutput = 'json'\n[execution]\nmax_parallel = 7 # keep this too\n",
        )
        .unwrap();
        configure(&dir.0.join("config.toml"), &custom("my gateway")).unwrap();
        let text = fs::read_to_string(dir.0.join("config.toml")).unwrap();
        assert!(text.contains("# Keep my comment"));
        assert!(text.contains("# keep this too"));
        let generated = Config::parse(&text).unwrap();
        let hand = Config::parse(
            r#"
default_provider = "my gateway"
default_model = "user/model.v2"
output = "json"
[execution]
max_parallel = 7
[providers."my gateway"]
protocol = "system-one"
base_url = "https://arbitrary.example/prefix"
[providers."my gateway".connection]
no_auth = true
[providers."my gateway".models."user/model.v2"]
"#,
        )
        .unwrap();
        assert_eq!(generated, hand);
        let before = text;
        assert!(matches!(
            configure(&dir.0.join("config.toml"), &custom("my gateway")),
            Err(SetupError::Exists)
        ));
        assert_eq!(
            fs::read_to_string(dir.0.join("config.toml")).unwrap(),
            before
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(dir.0.join("config.toml"))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
    }
    #[test]
    fn concurrent_setup_retains_both_connections() {
        let dir = Directory::new();
        std::thread::scope(|scope| {
            for name in ["one", "two"] {
                let args = dir.args();
                scope.spawn(move || configure(&path(&args).unwrap(), &custom(name)).unwrap());
            }
        });
        let config =
            Config::parse(&fs::read_to_string(dir.0.join("config.toml")).unwrap()).unwrap();
        assert_eq!(config.providers.len(), 2);
    }
    #[test]
    fn cloudflare_wire_model_is_an_editable_exact_model_setting() {
        let dir = Directory::new();
        let draft = Connect {
            service: Some("cloudflare".into()),
            name: Some("edge".into()),
            base_url: Some("https://edge.example/accounts/demo/ai".into()),
            model: Some("@cf/cloudflare/clef".into()),
            env: Some("CF_TOKEN".into()),
            non_interactive: true,
            ..Default::default()
        };
        configure(&dir.0.join("config.toml"), &draft).unwrap();
        let config =
            Config::parse(&fs::read_to_string(dir.0.join("config.toml")).unwrap()).unwrap();
        let definition = &config.providers["edge"];
        assert_eq!(definition.connection.wire_model, None);
        assert_eq!(
            definition.models["@cf/cloudflare/clef"]
                .connection
                .wire_model
                .as_deref(),
            Some("clef")
        );
        assert!(
            !fs::read_to_string(dir.0.join("config.toml"))
                .unwrap()
                .contains("CF_TOKEN")
        );
    }
}
