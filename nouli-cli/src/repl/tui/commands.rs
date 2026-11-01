//! Command discovery and the terminal help text.
use super::{App, Row};
use crate::repl::HELP;
/// Commands handled by [`App::setup_command`], outside the turn loop.
pub(super) const SETUP_COMMANDS: [&str; 3] = ["/connect", "/login", "/logout"];

/// The commands only this view has. Setup commands prompt interactively, so
/// the line loop does not offer them.
pub(super) const COMMANDS: [(&str, &str); 6] = [
    ("/sessions", "resume an earlier session"),
    ("/statusline", "choose the status line's fields"),
    ("/connect", "configure a connection"),
    ("/login [NAME]", "store an API key"),
    ("/logout [NAME]", "remove a stored credential"),
    ("/model", "choose a connection and model"),
];

/// The keys, for the `/help` dialog. `/` lists the commands themselves.
pub(super) const SHORTCUTS: [(&str, &str); 21] = [
    ("Tab", "Move to the next box"),
    ("↑↓", "Move between rows and boxes"),
    ("Shift-Tab", "Cycle answer type"),
    ("Enter", "Add a row in Context; send in Question"),
    ("Enter ×2", "Send from options / levels"),
    ("Backspace", "Reopen the row above an empty row"),
    ("Ctrl-S", "Send from any field"),
    ("Ctrl-P", "Hold / resume view for copying"),
    ("Esc", "Resume view / close overlay"),
    ("Ctrl-L", "Choose a model"),
    ("Ctrl-G", "Focus decisions; resize or scroll them"),
    ("Ctrl-C", "Cancel request / clear / quit"),
    ("Ctrl-D", "Delete forward / quit"),
    ("@", "Insert a reference"),
    ("Ctrl-O", "Inspect the nearby reference"),
    ("PgUp / PgDn", "Scroll the transcript"),
    ("Drop files", "Insert into the focused field"),
    ("Mouse drag", "Select text"),
    ("/", "List commands"),
    ("/help", "Show shortcuts"),
    ("/statusline", "Choose status fields"),
];

/// The slash commands, for the list that opens as the input row starts with
/// `/`: each one's usage and meaning, as `/help` lists them.
pub(super) fn commands() -> impl Iterator<Item = (&'static str, &'static str)> {
    [("/help", "the keys and shortcuts")].into_iter().chain(
        COMMANDS
            .into_iter()
            .chain(HELP)
            .filter(|(usage, _)| usage.starts_with('/')),
    )
}

/// The commands that start with what the input row holds, while it is still
/// a command name.
pub(super) fn matching(input: &str) -> Vec<(&'static str, &'static str)> {
    if !input.starts_with('/') || input.contains(char::is_whitespace) {
        return vec![];
    }
    commands()
        .filter(|(usage, _)| usage.starts_with(input))
        .collect()
}

/// Whether a usage such as `/use NAME` needs more after the command name.
pub(super) fn takes_argument(usage: &str) -> bool {
    usage
        .split_once(' ')
        .is_some_and(|(name, argument)| !name.ends_with(',') && !argument.starts_with('['))
}

impl App<'_> {
    /// The commands the list shows under the input row, if any.
    pub(super) fn commands_shown(&self) -> Vec<(&'static str, &'static str)> {
        if self.hidden || self.graph_focused || self.literal_input || self.form.row != Row::Input {
            return vec![];
        }
        matching(&self.form.input)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repl::tui::{tests::*, *};

    #[test]
    fn help_lists_exactly_the_setup_commands_the_tui_dispatches() {
        let listed: Vec<&str> = COMMANDS
            .iter()
            .filter_map(|(usage, _)| usage.split_whitespace().next())
            .filter(|name| !matches!(*name, "/sessions" | "/statusline" | "/model"))
            .collect();
        assert_eq!(listed, SETUP_COMMANDS);
    }

    #[test]
    fn typing_a_slash_lists_the_commands_and_enter_or_tab_takes_one() {
        let settings = settings();
        let mut app = app(&settings);
        app.repl.queue.facts.push("queued".into());
        typed(&mut app, "/");
        assert!(app.commands_shown().len() > 10, "every command at first");
        typed(&mut app, "cl");
        assert_eq!(app.commands_shown(), [("/clear", "empty the queue")]);
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE);
        assert!(app.repl.queue.is_empty(), "Enter ran /clear");
        // Tab fills in a command that needs a name, and the list closes.
        typed(&mut app, "/us");
        press(&mut app, KeyCode::Tab, KeyModifiers::NONE);
        assert_eq!(app.form.input, "/use ");
        assert_eq!(app.commands_shown(), [] as [(&str, &str); 0]);
        // Esc hides the list until the row changes.
        app.form.clear_row();
        typed(&mut app, "/v");
        press(&mut app, KeyCode::Esc, KeyModifiers::NONE);
        assert_eq!(app.commands_shown(), [] as [(&str, &str); 0]);
        typed(&mut app, "i");
        assert_eq!(app.commands_shown().len(), 1);
        // With no other session, /sessions says so.
        app.form.clear_row();
        typed(&mut app, "/sessions");
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE);
        assert!(app.sessions.is_none());
        assert!(
            app.status
                .as_ref()
                .unwrap()
                .text
                .contains("no other session")
        );
    }

    #[test]
    fn imported_commands_stay_literal_and_clearing_restores_the_command_palette() {
        let settings = settings();
        let mut app = app(&settings);
        app.insert_files(vec![attachments::Loaded {
            text: "/cl".into(),
            image: None,
        }]);
        assert_eq!(app.commands_shown(), [] as [(&str, &str); 0]);
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE);
        assert_eq!(app.repl.queue.facts, ["/cl"]);
        app.insert_files(vec![attachments::Loaded {
            text: "/quit".into(),
            image: None,
        }]);
        press(&mut app, KeyCode::Char('c'), KeyModifiers::CONTROL);
        typed(&mut app, "/cl");
        assert_ne!(app.commands_shown(), [] as [(&str, &str); 0]);
    }
}
