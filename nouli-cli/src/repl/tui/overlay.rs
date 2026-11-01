//! The dialogs over the view: the shortcuts and the status line's fields.
use super::{
    App, Status,
    model_picker::{Picker, Step},
    status_line::{self, Editor},
};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// The open dialog, which has the keys until it closes.
pub(super) enum Overlay {
    /// The shortcuts, from this first row when they do not all fit.
    Help {
        top: usize,
    },
    StatusLine(Editor),
    Models(Picker),
}

impl Overlay {
    pub(super) fn editor(&self) -> Option<&Editor> {
        match self {
            Self::StatusLine(editor) => Some(editor),
            Self::Help { .. } | Self::Models(_) => None,
        }
    }
}

impl App<'_> {
    pub(super) fn open_help(&mut self) {
        self.overlay = Some(Overlay::Help { top: 0 });
    }

    pub(super) fn open_status_line(&mut self) {
        self.overlay = Some(Overlay::StatusLine(Editor::new(&self.status_line)));
    }

    /// Keys for the open dialog. Esc and Ctrl-C close it without applying
    /// anything. Dialogs only open between turns, so there is none to cancel.
    pub(super) fn overlay_key(&mut self, key: &KeyEvent) {
        let control_c =
            key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c');
        // Esc in a picker's later step goes back to its list.
        if !control_c
            && key.code == KeyCode::Esc
            && let Some(Overlay::Models(picker)) = &mut self.overlay
            && picker.step != Step::List
        {
            picker.step = Step::List;
            picker.choice = 0;
            picker.key.clear();
            picker.pending = None;
            return;
        }
        if key.code == KeyCode::Esc || control_c {
            self.overlay = None;
            self.fetches = None;
            return;
        }
        if matches!(self.overlay, Some(Overlay::Models(_))) {
            self.picker_key(key);
            return;
        }
        // Some terminals report Shift-Tab as Tab with Shift held.
        let shift_tab = key.code == KeyCode::Tab && key.modifiers.contains(KeyModifiers::SHIFT);
        let code = if shift_tab {
            KeyCode::BackTab
        } else {
            key.code
        };
        match self.overlay.as_mut() {
            Some(Overlay::Help { top }) => match key.code {
                KeyCode::Up => *top = top.saturating_sub(1),
                // Drawing stops it at the last row.
                KeyCode::Down => *top = top.saturating_add(1),
                KeyCode::Enter => self.open_status_line(),
                _ => {}
            },
            Some(Overlay::StatusLine(editor)) => match code {
                KeyCode::Up | KeyCode::BackTab => editor.previous(),
                KeyCode::Down | KeyCode::Tab => editor.next(),
                KeyCode::Left => editor.across(false),
                KeyCode::Right => editor.across(true),
                KeyCode::Char(' ') => editor.toggle(),
                KeyCode::Enter => self.apply_status_line(),
                _ => {}
            },
            _ => {}
        }
    }

    fn picker_key(&mut self, key: &KeyEvent) {
        let Some(Overlay::Models(picker)) = &mut self.overlay else {
            return;
        };
        picker.pending = None;
        let text = key
            .modifiers
            .difference(KeyModifiers::SHIFT)
            .is_empty()
            .then_some(key.code);
        match (picker.step.clone(), text) {
            (Step::Key(keyed), Some(KeyCode::Enter)) => {
                let typed = std::mem::take(&mut picker.key);
                self.submit_key(keyed, &typed);
            }
            (Step::Key(_), Some(KeyCode::Backspace)) => drop(picker.key.pop()),
            (Step::Key(_), Some(KeyCode::Char(c))) => picker.key.push(c),
            (Step::Services | Step::List, Some(KeyCode::Up)) => {
                picker.choice = picker.choice.saturating_sub(1);
            }
            (Step::Services, Some(KeyCode::Down)) => {
                let last = crate::connect::quick_services().len();
                picker.choice = (picker.choice + 1).min(last);
            }
            (Step::Services, Some(KeyCode::Enter)) => {
                let index = picker.choice;
                self.pick_service(index);
            }
            (Step::List, Some(KeyCode::Down)) => picker.choice += 1,
            (Step::List, Some(KeyCode::Backspace)) => {
                picker.query.pop();
                picker.choice = 0;
            }
            (Step::List, Some(KeyCode::Char(c))) => {
                picker.query.push(c);
                picker.choice = 0;
            }
            (Step::List, Some(KeyCode::Enter)) => {
                let query = picker.query.clone();
                let index = picker.choice;
                let rows = self.picker_rows(&query);
                if let Some(row) = rows.get(index.min(rows.len().saturating_sub(1))) {
                    self.pick(row.clone());
                }
            }
            _ => {}
        }
        // The row stays on the list as it grows and shrinks.
        if let Some(Overlay::Models(picker)) = &self.overlay
            && picker.step == Step::List
        {
            let last = self.picker_rows(&picker.query).len().saturating_sub(1);
            if let Some(Overlay::Models(picker)) = &mut self.overlay {
                picker.choice = picker.choice.min(last);
            }
        }
    }

    /// Pasted text for the picker's search or key prompt.
    pub(super) fn picker_paste(&mut self, text: &str) {
        let Some(Overlay::Models(picker)) = &mut self.overlay else {
            return;
        };
        let clean: String = text.chars().filter(|c| !c.is_control()).collect();
        picker.pending = None;
        match picker.step {
            Step::List => {
                picker.query.push_str(&clean);
                picker.choice = 0;
            }
            Step::Key(_) => picker.key.push_str(&clean),
            Step::Services => {}
        }
    }

    /// Closes the dialog with its fields in use. They last for this view even
    /// when they cannot be saved for the next one.
    fn apply_status_line(&mut self) {
        let Some(Overlay::StatusLine(editor)) = self.overlay.take() else {
            return;
        };
        self.status_line = editor.fields;
        if let Err(error) = status_line::save(&self.repl.settings.config_path, &self.status_line) {
            tracing::warn!(%error, "status line preference not saved");
            self.status = Some(Status::error(error.to_string()));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repl::tui::{
        status_line::{Field, Focus},
        tests::*,
    };

    #[test]
    fn help_opens_over_the_form_and_leads_to_the_status_line() {
        let settings = settings();
        let mut app = app(&settings);
        typed(&mut app, "/help");
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE);
        assert!(matches!(app.overlay, Some(Overlay::Help { top: 0 })));
        assert_eq!(app.form.input, "");
        assert!(app.transcript.is_empty(), "shortcuts stay out of history");
        typed(&mut app, "x");
        assert_eq!(app.form.input, "", "the dialog has the keys");
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE);
        assert!(matches!(app.overlay, Some(Overlay::StatusLine(_))));
        press(&mut app, KeyCode::Esc, KeyModifiers::NONE);
        assert!(app.overlay.is_none());
    }

    #[test]
    fn the_status_line_command_applies_and_saves_the_choice() {
        let mut settings = settings();
        let directory = settings.store.clone();
        std::fs::create_dir_all(&directory).unwrap();
        settings.config_path = directory.join("config.toml");
        let mut app = app(&settings);
        typed(&mut app, "/statusline");
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE);
        // Session is the second preset.
        press(&mut app, KeyCode::Right, KeyModifiers::NONE);
        assert_eq!(
            app.status_fields(),
            [Field::Model, Field::Session, Field::Decisions],
            "an edit shows before it is applied"
        );
        assert_eq!(app.status_line, [Field::Model]);
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE);
        assert!(app.overlay.is_none());
        assert_eq!(
            app.status_line,
            [Field::Model, Field::Session, Field::Decisions]
        );
        assert_eq!(
            status_line::load(&settings.config_path),
            [Field::Model, Field::Session, Field::Decisions]
        );
    }

    #[test]
    fn escape_discards_an_edit() {
        let settings = settings();
        let mut app = app(&settings);
        app.open_status_line();
        press(&mut app, KeyCode::Left, KeyModifiers::NONE);
        assert_eq!(app.status_fields(), [] as [Field; 0]);
        press(&mut app, KeyCode::Esc, KeyModifiers::NONE);
        assert_eq!(app.status_fields(), [Field::Model]);
    }

    #[test]
    fn enter_keeps_a_selection_no_preset_matches() {
        let mut settings = settings();
        let directory = settings.store.clone();
        std::fs::create_dir_all(&directory).unwrap();
        settings.config_path = directory.join("config.toml");
        let mut app = app(&settings);
        app.status_line = vec![Field::Session, Field::Latency];
        app.open_status_line();
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE);
        assert_eq!(app.status_line, [Field::Session, Field::Latency]);
    }

    #[test]
    fn a_status_save_that_fails_says_so_and_keeps_the_choice_for_this_view() {
        let mut settings = settings();
        let directory = settings.store.clone();
        std::fs::create_dir_all(&directory).unwrap();
        settings.config_path = directory.join("config.toml");
        std::fs::write(directory.join("tui.toml"), "broken = [").unwrap();
        let mut app = app(&settings);
        app.open_status_line();
        press(&mut app, KeyCode::Right, KeyModifiers::NONE);
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE);
        assert_eq!(app.status_line.len(), 3);
        assert!(app.status.as_ref().unwrap().text.contains("not valid TOML"));
    }

    #[test]
    fn dialogs_keep_the_keys_from_the_held_view() {
        let settings = settings();
        let mut app = app(&settings);
        app.open_help();
        press(&mut app, KeyCode::Char('p'), KeyModifiers::CONTROL);
        assert!(!app.selection.active);
        assert!(app.overlay.is_some());
    }

    #[test]
    fn both_shift_tab_encodings_move_back() {
        for (code, modifiers) in [
            (KeyCode::BackTab, KeyModifiers::NONE),
            (KeyCode::Tab, KeyModifiers::SHIFT),
        ] {
            let settings = settings();
            let mut app = app(&settings);
            app.open_status_line();
            press(&mut app, code, modifiers);
            assert!(matches!(
                app.overlay,
                Some(Overlay::StatusLine(Editor {
                    focus: Focus::Apply,
                    ..
                }))
            ));
        }
    }

    #[test]
    fn a_dialog_ignores_paste_and_control_c_closes_it_without_clearing_the_draft() {
        let settings = settings();
        let mut app = app(&settings);
        app.form.input = "keep this".into();
        app.open_help();
        app.event(crossterm::event::Event::Paste("no".into()), &mut None)
            .unwrap();
        assert_eq!(app.form.input, "keep this");
        press(&mut app, KeyCode::Char('c'), KeyModifiers::CONTROL);
        assert!(app.overlay.is_none());
        assert_eq!(app.form.input, "keep this");
    }
}
