//! The status line's built-in fields, its presets, and the saved selection.
//! Fields are a fixed list; the status line never runs anything.
use super::{App, Running};
use crate::{render, settings::private_file};
use nouli_core::config::Protocol;
use serde::Deserialize;
use std::{
    fs, io,
    path::{Path, PathBuf},
};
use thiserror::Error;
use toml_edit::{Array, DocumentMut, value};

/// Display preferences live beside the configuration: they belong to this
/// terminal view, and `config.toml` is `nouli-core`'s strict schema.
const FILE_NAME: &str = "tui.toml";
const KEY: &str = "status_line";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Field {
    Model,
    Profile,
    Session,
    Decisions,
    Context,
    Latency,
}

impl Field {
    pub(super) const ALL: [Self; 6] = [
        Self::Model,
        Self::Profile,
        Self::Session,
        Self::Decisions,
        Self::Context,
        Self::Latency,
    ];

    pub(super) fn label(self) -> &'static str {
        match self {
            Self::Model => "Model",
            Self::Profile => "Profile",
            Self::Session => "Session",
            Self::Decisions => "Decisions",
            Self::Context => "Context items",
            Self::Latency => "Last latency",
        }
    }

    fn key(self) -> &'static str {
        match self {
            Self::Model => "model",
            Self::Profile => "profile",
            Self::Session => "session",
            Self::Decisions => "decisions",
            Self::Context => "context",
            Self::Latency => "latency",
        }
    }
}

pub(super) const PRESETS: [(&str, &[Field]); 4] = [
    ("Minimal", &[Field::Model]),
    ("Session", &[Field::Model, Field::Session, Field::Decisions]),
    ("Request", &[Field::Model, Field::Profile, Field::Latency]),
    ("Hidden", &[]),
];

/// The preset that shows exactly `fields`, if one does.
pub(super) fn preset_of(fields: &[Field]) -> Option<usize> {
    PRESETS.iter().position(|(_, preset)| {
        preset.len() == fields.len() && preset.iter().all(|field| fields.contains(field))
    })
}

#[derive(Debug, Error)]
pub(super) enum StatusLineError {
    #[error("cannot save the status line: {0}")]
    Save(#[from] io::Error),
    #[error("cannot save the status line: tui.toml is not valid TOML")]
    Invalid,
}

#[derive(Deserialize)]
struct Saved {
    status_line: Option<Vec<String>>,
}

fn path_beside(config: &Path) -> PathBuf {
    config.with_file_name(FILE_NAME)
}

/// The saved fields in display order. A missing or unreadable file gives the
/// first preset, and a field name from a newer version is left out.
pub(super) fn load(config: &Path) -> Vec<Field> {
    let default = || PRESETS[0].1.to_vec();
    let text = match fs::read_to_string(path_beside(config)) {
        Ok(text) => text,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return default(),
        Err(error) => {
            tracing::warn!(%error, "cannot read the status line preference");
            return default();
        }
    };
    // Parse errors can quote the file, so they stay out of the log.
    let Ok(saved) = toml_edit::de::from_str::<Saved>(&text) else {
        tracing::warn!("ignoring a malformed status line preference");
        return default();
    };
    let Some(names) = saved.status_line else {
        return default();
    };
    Field::ALL
        .into_iter()
        .filter(|field| names.iter().any(|name| name == field.key()))
        .collect()
}

/// Saves the selection, keeping anything else in the file. A file that does
/// not parse is left alone rather than replaced.
pub(super) fn save(config: &Path, fields: &[Field]) -> Result<(), StatusLineError> {
    let path = path_beside(config);
    let _lock = private_file::lock_beside(&path)?;
    let mut document = match fs::read_to_string(&path) {
        Ok(text) => text.parse().map_err(|_| StatusLineError::Invalid)?,
        Err(error) if error.kind() == io::ErrorKind::NotFound => DocumentMut::new(),
        Err(error) => return Err(error.into()),
    };
    let names: Array = fields.iter().map(|field| field.key()).collect();
    document[KEY] = value(names);
    private_file::publish(&path, document.to_string().as_bytes())?;
    Ok(())
}

/// The part of the status line dialog that has the keys.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Focus {
    Preset(usize),
    Field(usize),
    Apply,
}

/// The status line dialog: an edit of the fields, applied on confirmation.
#[derive(Debug, PartialEq, Eq)]
pub(super) struct Editor {
    pub(super) fields: Vec<Field>,
    pub(super) focus: Focus,
}

impl Editor {
    pub(super) fn new(fields: &[Field]) -> Self {
        Self {
            fields: fields.to_vec(),
            focus: Focus::Preset(preset_of(fields).unwrap_or(0)),
        }
    }

    pub(super) fn next(&mut self) {
        self.focus = match self.focus {
            Focus::Preset(_) => Focus::Field(0),
            Focus::Field(index) if index + 1 < Field::ALL.len() => Focus::Field(index + 1),
            Focus::Field(_) => Focus::Apply,
            Focus::Apply => Focus::Preset(preset_of(&self.fields).unwrap_or(0)),
        };
    }

    pub(super) fn previous(&mut self) {
        self.focus = match self.focus {
            Focus::Preset(_) => Focus::Apply,
            Focus::Field(0) => Focus::Preset(preset_of(&self.fields).unwrap_or(0)),
            Focus::Field(index) => Focus::Field(index - 1),
            Focus::Apply => Focus::Field(Field::ALL.len() - 1),
        };
    }

    /// Moves across the presets and takes the one reached, wrapping at either end.
    pub(super) fn across(&mut self, forward: bool) {
        if let Focus::Preset(index) = self.focus {
            let step = if forward { 1 } else { PRESETS.len() - 1 };
            let next = (index + step) % PRESETS.len();
            self.focus = Focus::Preset(next);
            self.fields = PRESETS[next].1.to_vec();
        }
    }

    /// Takes the focused preset, or turns the focused field on or off.
    pub(super) fn toggle(&mut self) {
        match self.focus {
            Focus::Preset(index) => self.fields = PRESETS[index].1.to_vec(),
            Focus::Field(index) => {
                let field = Field::ALL[index];
                let on = self.fields.contains(&field);
                self.fields = Field::ALL
                    .into_iter()
                    .filter(|other| {
                        if *other == field {
                            !on
                        } else {
                            self.fields.contains(other)
                        }
                    })
                    .collect();
            }
            Focus::Apply => {}
        }
    }
}

fn protocol_name(protocol: Protocol) -> &'static str {
    match protocol {
        Protocol::SystemOne => "system-one",
        Protocol::OpenaiDecisions => "openai-decisions",
    }
}

impl App<'_> {
    /// The provider and model the next or running request uses, or the one
    /// that served the last request.
    pub(super) fn model_text(&self, running: Option<&Running>) -> String {
        if running.is_none()
            && let Some(served) = &self.model
        {
            return served.clone();
        }
        let selection = running.map_or(&self.selected_model, |turn| &turn.model);
        match (&selection.provider, &selection.model) {
            (Some(provider), Some(model)) => {
                format!("{} · {}", render::clean(model), render::clean(provider))
            }
            _ => "Model not configured".into(),
        }
    }

    /// A field's text, or `None` while it has nothing to show.
    pub(super) fn field_text(&self, field: Field, running: Option<&Running>) -> Option<String> {
        match field {
            Field::Model => Some(self.model_text(running)),
            Field::Profile => {
                let selection = running.map_or(&self.selected_model, |turn| &turn.model);
                let provider = selection.provider.as_ref()?;
                let definition = self.repl.settings.config.providers.get(provider)?;
                Some(protocol_name(definition.protocol).to_owned())
            }
            Field::Session => Some(render::clean(&self.repl.session.label)),
            Field::Decisions => Some(render::plural(self.graph.decisions, "decision")),
            Field::Context => Some(render::plural(self.repl.queue.len(), "item")),
            Field::Latency => self
                .latency
                .map(|latency| format!("{} ms", latency.as_millis())),
        }
    }

    /// The fields the status line shows, including an unapplied edit.
    pub(super) fn status_fields(&self) -> &[Field] {
        self.overlay
            .as_ref()
            .and_then(super::overlay::Overlay::editor)
            .map_or(&self.status_line, |editor| &editor.fields)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nouli_core::identity::Id;

    fn directory() -> PathBuf {
        let root = std::env::temp_dir().join(format!("nouli-status-{}", Id::generate()));
        fs::create_dir_all(&root).unwrap();
        root
    }

    #[test]
    fn the_selection_survives_a_restart_in_display_order() {
        let config = directory().join("config.toml");
        assert_eq!(load(&config), [Field::Model], "the first preset by default");
        save(&config, &[Field::Session, Field::Latency]).unwrap();
        assert_eq!(load(&config), [Field::Session, Field::Latency]);
        save(&config, &[]).unwrap();
        assert_eq!(load(&config), [] as [Field; 0], "Hidden stays hidden");
    }

    #[test]
    fn saving_keeps_other_preferences_and_unknown_fields_are_left_out() {
        let config = directory().join("config.toml");
        let file = path_beside(&config);
        fs::write(
            &file,
            "status_line = [\"future\", \"latency\"]\nother = 1\n",
        )
        .unwrap();
        assert_eq!(load(&config), [Field::Latency]);
        save(&config, &[Field::Model]).unwrap();
        let text = fs::read_to_string(&file).unwrap();
        assert!(text.contains("other = 1"), "{text}");
        assert_eq!(load(&config), [Field::Model]);
    }

    #[test]
    fn an_unreadable_preference_falls_back_to_the_default() {
        let config = directory().join("config.toml");
        fs::write(path_beside(&config), "status_line = 3").unwrap();
        assert_eq!(load(&config), [Field::Model]);
        fs::write(path_beside(&config), "not toml").unwrap();
        assert_eq!(load(&config), [Field::Model]);
    }

    #[test]
    fn saving_leaves_a_malformed_file_as_it_was() {
        let config = directory().join("config.toml");
        let text = "theme = \"mine\"\nkeep = [1,\n";
        fs::write(path_beside(&config), text).unwrap();
        assert!(matches!(
            save(&config, &[Field::Model]),
            Err(StatusLineError::Invalid)
        ));
        assert_eq!(fs::read_to_string(path_beside(&config)).unwrap(), text);
    }

    #[test]
    fn a_file_without_the_key_gives_the_default() {
        let config = directory().join("config.toml");
        fs::write(path_beside(&config), "other = 1\n").unwrap();
        assert_eq!(load(&config), [Field::Model]);
    }

    #[test]
    fn presets_match_their_fields_in_any_order() {
        assert_eq!(preset_of(&[Field::Model]), Some(0));
        assert_eq!(
            preset_of(&[Field::Decisions, Field::Model, Field::Session]),
            Some(1)
        );
        assert_eq!(preset_of(&[]), Some(3));
        assert_eq!(preset_of(&[Field::Latency]), None);
    }

    #[test]
    fn the_editor_walks_every_control_and_keeps_display_order() {
        let mut editor = Editor::new(&[Field::Model]);
        assert_eq!(editor.focus, Focus::Preset(0));
        editor.previous();
        assert_eq!(editor.focus, Focus::Apply);
        editor.next();
        assert_eq!(editor.focus, Focus::Preset(0));
        editor.across(false);
        assert_eq!(editor.focus, Focus::Preset(3));
        assert_eq!(editor.fields, [] as [Field; 0], "moving takes the preset");
        editor.next();
        for _ in 0..5 {
            editor.next();
        }
        assert_eq!(editor.focus, Focus::Field(5));
        editor.toggle();
        editor.previous();
        editor.previous();
        editor.toggle();
        assert_eq!(editor.fields, [Field::Decisions, Field::Latency]);
        editor.next();
        editor.next();
        editor.next();
        assert_eq!(editor.focus, Focus::Apply);
    }
}
