//! Choosing a model inside the view. The list opens from the configuration
//! at once, and models the providers list join it as they arrive.
use super::{App, Status, overlay::Overlay};
use crate::connect;
use nouli_core::{
    config::{
        ExecutionSettings,
        auth::{KeySource, Secret},
        resolve::RuntimeOverrides,
    },
    history::ModelSelection,
    provider::models,
    transport::HttpTransport,
};
use std::{
    collections::{BTreeMap, HashSet},
    sync::{
        Arc,
        mpsc::{Receiver, TryRecvError, channel},
    },
};
use tokio_util::sync::CancellationToken;

/// How long a provider's list may take, so one slow provider stays out of the way.
const FETCH_SECONDS: u64 = 10;
/// A model ID resolved only to read a connection's credentials.
const ANY_MODEL: &str = "any";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Choice {
    pub(super) provider: String,
    pub(super) model: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Row {
    Model(Choice),
    /// A typed ID no provider listed; any exact ID is accepted.
    Typed(Choice),
    AddConnection,
}

/// What a key is for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Keyed {
    Model(Choice),
    Service(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Step {
    List,
    Services,
    Key(Keyed),
}

/// The open picker. `Debug` is left out so an API key being typed never prints.
pub(super) struct Picker {
    pub(super) query: String,
    pub(super) choice: usize,
    pub(super) step: Step,
    pub(super) key: String,
    needs_keys: BTreeMap<String, bool>,
    pub(super) pending: Option<Choice>,
}

enum Fetched {
    Credential {
        provider: String,
        needs_key: bool,
    },
    Models {
        provider: String,
        models: Vec<String>,
    },
}

/// The fetches under way, cancelled when the picker closes.
pub(super) struct Fetches {
    received: Receiver<Fetched>,
    _cancel: tokio_util::sync::DropGuard,
}

impl App<'_> {
    /// Opens the picker on the current model, filtered by `query`.
    pub(super) fn open_model_picker(&mut self, query: &str) {
        let mut picker = Picker {
            query: query.to_owned(),
            choice: 0,
            step: Step::List,
            key: String::new(),
            needs_keys: BTreeMap::new(),
            pending: None,
        };
        let current = self.current_choice();
        picker.choice = self
            .picker_rows(&picker.query)
            .iter()
            .position(|row| matches!(row, Row::Model(choice) if Some(choice) == current.as_ref()))
            .unwrap_or(0);
        self.overlay = Some(Overlay::Models(picker));
        self.fetch_models();
    }

    pub(super) fn current_choice(&self) -> Option<Choice> {
        let selection = &self.selected_model;
        Some(Choice {
            provider: selection.provider.clone()?,
            model: selection.model.clone()?,
        })
    }

    /// The rows for `query`: the current connection first, then the rest.
    pub(super) fn picker_rows(&self, query: &str) -> Vec<Row> {
        let config = &self.repl.settings.config;
        let current = self.current_choice();
        let first = current.as_ref().map(|choice| choice.provider.as_str());
        let mut providers: Vec<&String> = config.providers.keys().collect();
        providers.sort_by_key(|name| Some(name.as_str()) != first);
        let wanted = query.trim().to_lowercase();
        let mut rows = Vec::new();
        for provider in providers {
            let mut models: Vec<String> = Vec::new();
            let mut seen = HashSet::new();
            let mut add = |model: &String| {
                if seen.insert(model.clone()) {
                    models.push(model.clone());
                }
            };
            if let Some(choice) = current
                .as_ref()
                .filter(|choice| &choice.provider == provider)
            {
                add(&choice.model);
            }
            if config.default_provider.as_ref() == Some(provider)
                && let Some(model) = &config.default_model
            {
                add(model);
            }
            config.providers[provider].models.keys().for_each(&mut add);
            if let Some(listed) = self.catalog.get(provider) {
                listed.iter().for_each(&mut add);
            }
            rows.extend(
                models
                    .into_iter()
                    .filter(|model| {
                        format!("{model} {provider}")
                            .to_lowercase()
                            .contains(&wanted)
                    })
                    .map(|model| {
                        Row::Model(Choice {
                            provider: provider.clone(),
                            model,
                        })
                    }),
            );
        }
        let exact = rows
            .iter()
            .any(|row| matches!(row, Row::Model(choice) if choice.model == query.trim()));
        if let (false, false, Some(provider)) = (
            wanted.is_empty(),
            exact,
            current
                .map(|choice| choice.provider)
                .or_else(|| config.providers.keys().next().cloned()),
        ) {
            rows.push(Row::Typed(Choice {
                provider,
                model: query.trim().to_owned(),
            }));
        }
        rows.push(Row::AddConnection);
        rows
    }

    /// Asks every connection that has its key for its models.
    fn fetch_models(&mut self) {
        let Ok(handle) = tokio::runtime::Handle::try_current() else {
            return;
        };
        let cancel = CancellationToken::new();
        let (send, received) = channel();
        let settings = &self.repl.settings;
        let credentials = Arc::new(settings.credentials.isolated());
        for provider in settings.config.providers.keys() {
            let snapshot = settings.config.resolve_snapshot(
                &RuntimeOverrides {
                    selection: ModelSelection {
                        provider: Some(provider.clone()),
                        model: Some(ANY_MODEL.to_owned()),
                    },
                    stored_credentials: credentials.stored(),
                    ..RuntimeOverrides::default()
                },
                &ModelSelection::default(),
                &ModelSelection::default(),
                &settings.home,
            );
            let (provider, credentials, send, cancel) = (
                provider.clone(),
                Arc::clone(&credentials),
                send.clone(),
                cancel.clone(),
            );
            handle.spawn(async move {
                let resolved = match snapshot {
                    Ok(snapshot) => {
                        if snapshot.profile.api_key_env.is_some()
                            || snapshot.profile.stored_credential
                        {
                            credentials
                                .fetch(&provider, snapshot.profile.api_key_env.as_deref(), &cancel)
                                .await;
                        }
                        snapshot.with_credentials(|source| credentials.lookup(source))
                    }
                    Err(error) => Err(error),
                };
                if cancel.is_cancelled() {
                    return;
                }
                let needs_key = resolved.as_ref().map_or(true, |resolved| {
                    resolved.credential.is_none()
                        && resolved.snapshot.profile.connection.no_auth != Some(true)
                });
                if send
                    .send(Fetched::Credential {
                        provider: provider.clone(),
                        needs_key,
                    })
                    .is_err()
                    || needs_key
                {
                    return;
                }
                let Ok(resolved) = resolved else {
                    return;
                };
                let execution = ExecutionSettings {
                    attempt_timeout_seconds: FETCH_SECONDS,
                    ..resolved.snapshot.execution.clone()
                };
                let listed = match HttpTransport::new(&execution) {
                    Ok(transport) => models::list(&transport, &resolved, &cancel).await,
                    Err(error) => Err(error.into()),
                };
                match listed {
                    Ok(models) => drop(send.send(Fetched::Models { provider, models })),
                    Err(error) => tracing::debug!(%provider, %error, "model list unavailable"),
                }
            });
        }
        self.fetches = Some(Fetches {
            received,
            _cancel: cancel.drop_guard(),
        });
    }

    /// Takes the lists that have arrived, and keeps the row in view.
    pub(super) fn poll_models(&mut self) {
        let Some(fetches) = &self.fetches else {
            return;
        };
        let mut received = Vec::new();
        loop {
            match fetches.received.try_recv() {
                Ok(message) => received.push(message),
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    self.fetches = None;
                    break;
                }
            }
        }
        if received.is_empty() {
            return;
        }
        let selected = match &self.overlay {
            Some(Overlay::Models(picker)) if picker.step == Step::List => {
                self.picker_rows(&picker.query).get(picker.choice).cloned()
            }
            _ => None,
        };
        for fetched in received {
            match fetched {
                Fetched::Credential {
                    provider,
                    needs_key,
                } => {
                    if let Some(Overlay::Models(picker)) = &mut self.overlay {
                        picker.needs_keys.insert(provider, needs_key);
                    }
                }
                Fetched::Models { provider, models } => {
                    self.catalog.insert(provider, models);
                }
            }
        }
        if let (Some(selected), Some(Overlay::Models(picker))) = (selected, &self.overlay) {
            let index =
                self.picker_rows(&picker.query)
                    .iter()
                    .position(|row| match (row, &selected) {
                        (Row::Model(a) | Row::Typed(a), Row::Model(b) | Row::Typed(b)) => a == b,
                        _ => row == &selected,
                    });
            if let (Some(index), Some(Overlay::Models(picker))) = (index, &mut self.overlay) {
                picker.choice = index;
            }
        }
        if let Some(Overlay::Models(picker)) = &mut self.overlay
            && let Some(choice) = picker.pending.take()
        {
            self.pick(Row::Model(choice));
        }
    }

    pub(super) fn close_model_picker(&mut self) {
        self.overlay = None;
        self.fetches = None;
    }

    /// Enter in the picker's list.
    pub(super) fn pick(&mut self, row: Row) {
        match row {
            Row::AddConnection => {
                if let Some(Overlay::Models(picker)) = &mut self.overlay {
                    picker.step = Step::Services;
                    picker.choice = 0;
                }
            }
            Row::Model(choice) | Row::Typed(choice) => {
                let Some(Overlay::Models(picker)) = &mut self.overlay else {
                    return;
                };
                match picker.needs_keys.get(&choice.provider) {
                    Some(true) => self.ask_key(Keyed::Model(choice)),
                    Some(false) => self.use_model(choice),
                    None => picker.pending = Some(choice),
                }
            }
        }
    }

    /// Enter on a service: the setup that fits, in the view when it can be.
    pub(super) fn pick_service(&mut self, index: usize) {
        let services = connect::quick_services();
        match services.get(index) {
            Some(&(service, true)) => self.ask_key(Keyed::Service(service.to_owned())),
            Some(&(service, false)) => self.add_service(service, None),
            None => {
                self.close_model_picker();
                self.pending_setup = Some("/connect".to_owned());
            }
        }
    }

    pub(super) fn ask_key(&mut self, keyed: Keyed) {
        if let Some(Overlay::Models(picker)) = &mut self.overlay {
            picker.step = Step::Key(keyed);
            picker.key.clear();
        }
    }

    /// Enter on the key prompt. An empty key goes on without one, for a
    /// connection that does not need it.
    pub(super) fn submit_key(&mut self, keyed: Keyed, key: &str) {
        let secret = if key.trim().is_empty() {
            None
        } else {
            match Secret::new(key.trim().to_owned()) {
                Ok(secret) => Some(secret),
                Err(error) => {
                    self.status = Some(Status::error(error.to_string()));
                    return;
                }
            }
        };
        match keyed {
            Keyed::Service(service) => self.add_service(&service, secret),
            Keyed::Model(choice) => {
                if let Some(secret) = secret {
                    let path = crate::settings::auth::path_for(&self.repl.settings.config_path);
                    let stored = crate::auth::store_key(
                        &path,
                        &choice.provider,
                        &KeySource::Key(secret),
                        false,
                    );
                    if let Err(report) = stored {
                        self.status = Some(Status::error(report.message));
                        return;
                    }
                    if let Err(error) = self.reload_settings() {
                        self.status = Some(Status::error(error.to_string()));
                        return;
                    }
                }
                self.use_model(choice);
            }
        }
    }

    fn add_service(&mut self, service: &str, key: Option<Secret>) {
        let file = self.repl.settings.config_path.clone();
        match connect::add_service(&file, service, key) {
            Ok(selection) => {
                self.close_model_picker();
                self.repl.model = selection;
                match self.reload_settings() {
                    Ok(()) => self.refresh_model(),
                    Err(error) => self.status = Some(Status::error(error.to_string())),
                }
            }
            Err(error) => self.status = Some(Status::error(error.to_string())),
        }
    }

    /// Uses `choice` from now on, here and in later views.
    fn use_model(&mut self, choice: Choice) {
        self.close_model_picker();
        let saved = crate::settings::config_file::set_default(
            &self.repl.settings.config_path,
            &choice.provider,
            &choice.model,
        );
        self.repl.model = ModelSelection {
            provider: Some(choice.provider),
            model: Some(choice.model),
        };
        match saved.and_then(|()| self.reload_settings()) {
            Ok(()) => {}
            Err(error) => self.status = Some(Status::error(error.to_string())),
        }
        self.refresh_model();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repl::tui::{draw, tests::*};
    use ratatui::{Terminal, backend::TestBackend};

    const LOCKED_CONFIG: &str = "default_provider = \"locked\"\ndefault_model = \"m\"\n\
        [providers.locked]\nprotocol = \"system-one\"\nbase_url = \"http://127.0.0.1:9\"\n";

    fn locked_settings() -> Settings {
        let mut settings = configured();
        std::fs::create_dir_all(&settings.store).unwrap();
        settings.config_path = settings.store.join("config.toml");
        std::fs::write(&settings.config_path, LOCKED_CONFIG).unwrap();
        settings.config = nouli_core::config::Config::parse(LOCKED_CONFIG).unwrap();
        let env = settings.store.join("empty.env");
        std::fs::write(&env, "").unwrap();
        settings.env_file = Some(env);
        settings
    }

    async fn poll_until(app: &mut App<'_>, ready: impl Fn(&App<'_>) -> bool) {
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            loop {
                app.poll_models();
                if ready(app) {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("picker background work finishes");
    }

    fn send_models(app: &mut App<'_>, provider: &str, models: &[&str]) {
        let (send, received) = channel();
        let cancel = CancellationToken::new();
        app.fetches = Some(Fetches {
            received,
            _cancel: cancel.drop_guard(),
        });
        send.send(Fetched::Models {
            provider: provider.to_owned(),
            models: models.iter().map(|s| (*s).to_owned()).collect(),
        })
        .unwrap();
        app.poll_models();
    }

    fn shown(app: &mut App, width: u16, height: u16) -> Vec<String> {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal
            .draw(|frame| draw::screen(frame, app, None))
            .unwrap();
        let buffer = terminal.backend().buffer();
        (0..height)
            .map(|y| (0..width).map(|x| buffer[(x, y)].symbol()).collect())
            .collect()
    }

    fn configured() -> Settings {
        let mut settings = settings();
        settings.config = nouli_core::config::Config::parse(
            "default_provider = \"typesafe\"\ndefault_model = \"jev-latest\"\n\
             [providers.typesafe]\nprotocol = \"system-one\"\nbase_url = \"http://127.0.0.1:9\"\n\
             [providers.typesafe.connection]\nno_auth = true\n\
             [providers.local]\nprotocol = \"system-one\"\nbase_url = \"http://127.0.0.1:9\"\n\
             [providers.local.connection]\nno_auth = true\n\
             [providers.local.models.\"maker/small\"]\n",
        )
        .unwrap();
        settings
    }

    fn names(rows: &[Row]) -> Vec<String> {
        rows.iter()
            .map(|row| match row {
                Row::Model(choice) => format!("{} {}", choice.model, choice.provider),
                Row::Typed(choice) => format!("typed {} {}", choice.model, choice.provider),
                Row::AddConnection => "add".to_owned(),
            })
            .collect()
    }

    #[test]
    fn the_list_starts_on_the_current_model_with_configured_models_after_it() {
        let settings = configured();
        let mut app = app(&settings);
        assert_eq!(
            names(&app.picker_rows("")),
            ["jev-latest typesafe", "maker/small local", "add"]
        );
        app.catalog.insert(
            "typesafe".into(),
            vec!["jev-latest".into(), "jev-preview".into()],
        );
        assert_eq!(
            names(&app.picker_rows("")),
            [
                "jev-latest typesafe",
                "jev-preview typesafe",
                "maker/small local",
                "add"
            ],
            "listed models join without repeats"
        );
    }

    #[test]
    fn typing_filters_by_model_or_provider_and_offers_an_exact_id() {
        let settings = configured();
        let app = app(&settings);
        assert_eq!(
            names(&app.picker_rows("LOCAL")),
            ["maker/small local", "typed LOCAL typesafe", "add"]
        );
        assert_eq!(
            names(&app.picker_rows("jev-latest")),
            ["jev-latest typesafe", "add"]
        );
        assert_eq!(
            names(&app.picker_rows("my-model")),
            ["typed my-model typesafe", "add"]
        );
    }

    #[test]
    fn opening_lands_on_the_current_model_and_never_leaves_the_view() {
        let settings = configured();
        let mut app = app(&settings);
        press(&mut app, KeyCode::Char('l'), KeyModifiers::CONTROL);
        assert!(matches!(
            app.overlay,
            Some(Overlay::Models(Picker { choice: 0, .. }))
        ));
        assert!(app.pending_setup.is_none());
        press(&mut app, KeyCode::Esc, KeyModifiers::NONE);
        typed(&mut app, "/model loc");
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE);
        assert!(app.pending_setup.is_none(), "/model stays in the view");
        let Some(Overlay::Models(picker)) = &app.overlay else {
            panic!("the picker is open");
        };
        assert_eq!(picker.query, "loc");
    }

    #[tokio::test]
    async fn a_connection_without_its_key_asks_in_the_view_and_hides_what_is_typed() {
        let mut settings = configured();
        let path = settings.store.clone();
        std::fs::create_dir_all(&path).unwrap();
        settings.config_path = path.join("config.toml");
        std::fs::write(
            &settings.config_path,
            "default_provider = \"locked\"\ndefault_model = \"m\"\n\
             [providers.locked]\nprotocol = \"system-one\"\nbase_url = \"http://127.0.0.1:9\"\n",
        )
        .unwrap();
        settings.config = nouli_core::config::Config::parse(
            &std::fs::read_to_string(&settings.config_path).unwrap(),
        )
        .unwrap();
        let mut app = app(&settings);
        app.open_model_picker("");
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE);
        poll_until(&mut app, |app| matches!(&app.overlay, Some(Overlay::Models(picker)) if matches!(picker.step, Step::Key(_)))).await;
        let Some(Overlay::Models(picker)) = &app.overlay else {
            panic!("the picker stays open");
        };
        assert!(matches!(picker.step, Step::Key(_)));
        typed(&mut app, "sk-secret");
        let screen = shown(&mut app, 100, 30).join("\n");
        assert!(screen.contains("API key for locked"), "{screen}");
        assert!(screen.contains("•••••••••"), "{screen}");
        assert!(!screen.contains("sk-secret"), "{screen}");
        // Esc goes back to the list, and the key is gone.
        press(&mut app, KeyCode::Esc, KeyModifiers::NONE);
        let Some(Overlay::Models(picker)) = &app.overlay else {
            panic!("Esc returns to the list");
        };
        assert_eq!((&picker.step, picker.key.as_str()), (&Step::List, ""));
    }

    #[tokio::test]
    async fn choosing_a_model_saves_it_for_later_views() {
        let mut settings = configured();
        let path = settings.store.clone();
        std::fs::create_dir_all(&path).unwrap();
        settings.config_path = path.join("config.toml");
        std::fs::write(
            &settings.config_path,
            "default_provider = \"typesafe\"\ndefault_model = \"jev-latest\"\n\
             [providers.typesafe]\nprotocol = \"system-one\"\nbase_url = \"http://127.0.0.1:9\"\n\
             [providers.typesafe.connection]\nno_auth = true\n",
        )
        .unwrap();
        let mut app = app(&settings);
        app.open_model_picker("jev-preview");
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE);
        poll_until(&mut app, |app| app.overlay.is_none()).await;
        assert!(app.overlay.is_none());
        assert_eq!(app.selected_model.model.as_deref(), Some("jev-preview"));
        let saved = std::fs::read_to_string(&settings.config_path).unwrap();
        assert!(saved.contains("default_model = \"jev-preview\""), "{saved}");
    }

    #[test]
    fn arriving_models_preserve_the_model_or_action_about_to_be_chosen() {
        for index in [1, 2] {
            let settings = configured();
            let mut app = app(&settings);
            app.open_model_picker("");
            let Some(Overlay::Models(picker)) = &mut app.overlay else {
                unreachable!()
            };
            picker.choice = index;
            let before = app.picker_rows("")[index].clone();
            send_models(&mut app, "typesafe", &["new-model"]);
            let Some(Overlay::Models(picker)) = &app.overlay else {
                unreachable!()
            };
            assert_eq!(app.picker_rows("")[picker.choice], before);
            press(&mut app, KeyCode::Enter, KeyModifiers::NONE);
            let Some(Overlay::Models(picker)) = &app.overlay else {
                unreachable!()
            };
            match before {
                Row::Model(choice) => assert_eq!(picker.pending.as_ref(), Some(&choice)),
                Row::AddConnection => assert_eq!(picker.step, Step::Services),
                Row::Typed(_) => unreachable!(),
            }
        }
    }

    #[test]
    fn an_exact_typed_id_stays_selected_when_the_provider_lists_it() {
        let settings = configured();
        let mut app = app(&settings);
        app.open_model_picker("new-model");
        send_models(&mut app, "typesafe", &["new-model", "new-model-extra"]);
        let Some(Overlay::Models(picker)) = &app.overlay else {
            unreachable!()
        };
        assert_eq!(
            app.picker_rows(&picker.query)[picker.choice],
            Row::Model(Choice {
                provider: "typesafe".to_owned(),
                model: "new-model".to_owned(),
            })
        );
    }

    #[tokio::test]
    async fn a_credential_helper_runs_once_in_background_even_when_a_pick_is_pending() {
        use nouli_core::config::auth::{AuthEntry, AuthStore};
        const HELPER: &str = "printf x >> \"$1\"; \
            while [ ! -f \"$2\" ]; do sleep 0.02; done; printf fixture-key";
        let mut settings = locked_settings();
        let root = settings.store.clone();
        let calls = root.join("calls");
        let gate = root.join("gate");
        let mut store = AuthStore::default();
        store.insert(
            "locked".into(),
            AuthEntry::ApiKey(
                KeySource::command(vec![
                    "/bin/sh".into(),
                    "-c".into(),
                    HELPER.into(),
                    "helper".into(),
                    calls.to_string_lossy().into_owned(),
                    gate.to_string_lossy().into_owned(),
                ])
                .unwrap(),
            ),
        );
        crate::settings::auth::write(
            &crate::settings::auth::path_for(&settings.config_path),
            &store,
        )
        .unwrap();
        settings.load_environment().unwrap();
        let mut app = app(&settings);
        app.open_model_picker("");
        assert!(matches!(app.overlay, Some(Overlay::Models(_))));
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE);
        // The command cannot finish until this write; Enter must return first.
        std::fs::write(&gate, "go").unwrap();
        poll_until(&mut app, |app| app.overlay.is_none()).await;
        assert_eq!(std::fs::read_to_string(&calls).unwrap(), "x");
        assert_eq!(app.selected_model.provider.as_deref(), Some("locked"));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn closing_the_picker_cancels_a_waiting_credential_helper() {
        use nouli_core::config::auth::{AuthEntry, AuthStore};
        for key in [
            KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
            KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL),
        ] {
            let mut settings = locked_settings();
            let root = settings.store.clone();
            let pidfile = root.join("pid");
            let mut store = AuthStore::default();
            store.insert(
                "locked".into(),
                AuthEntry::ApiKey(
                    KeySource::command(vec![
                        "/bin/sh".into(),
                        "-c".into(),
                        "printf '%s' \"$$\" > \"$1\"; exec sleep 30".into(),
                        "helper".into(),
                        pidfile.to_string_lossy().into_owned(),
                    ])
                    .unwrap(),
                ),
            );
            let auth = root.join("auth.toml");
            crate::settings::auth::write(&auth, &store).unwrap();
            settings.load_environment().unwrap();
            let mut app = app(&settings);
            app.open_model_picker("");
            poll_until(&mut app, |_| {
                std::fs::read_to_string(&pidfile).is_ok_and(|s| !s.is_empty())
            })
            .await;
            let pid: i32 = std::fs::read_to_string(&pidfile).unwrap().parse().unwrap();
            press(&mut app, key.code, key.modifiers);
            assert!(app.overlay.is_none());
            assert!(app.fetches.is_none());
            poll_until(&mut app, |_| {
                matches!(
                    nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid), None),
                    Err(nix::errno::Errno::ESRCH)
                )
            })
            .await;
            std::fs::remove_dir_all(root).unwrap();
        }
    }

    #[tokio::test]
    async fn closing_the_picker_cancels_an_in_flight_model_list() {
        use tokio::{io::AsyncReadExt, net::TcpListener};
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let mut settings = configured();
        settings
            .config
            .providers
            .get_mut("typesafe")
            .unwrap()
            .base_url = format!("http://{}", listener.local_addr().unwrap());
        let mut app = app(&settings);
        app.open_model_picker("");
        let (mut socket, _) =
            tokio::time::timeout(std::time::Duration::from_secs(5), listener.accept())
                .await
                .unwrap()
                .unwrap();
        let mut request = Vec::new();
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            while !request.windows(4).any(|window| window == b"\r\n\r\n") {
                let mut buffer = [0; 1024];
                let read = socket.read(&mut buffer).await.unwrap();
                assert!(read > 0);
                request.extend_from_slice(&buffer[..read]);
            }
        })
        .await
        .unwrap();
        assert!(request.starts_with(b"GET /v1/models HTTP/1.1"));
        press(&mut app, KeyCode::Esc, KeyModifiers::NONE);
        let mut byte = [0];
        assert_eq!(
            tokio::time::timeout(std::time::Duration::from_secs(5), socket.read(&mut byte))
                .await
                .unwrap()
                .unwrap(),
            0
        );
        assert!(app.fetches.is_none());
    }
}
