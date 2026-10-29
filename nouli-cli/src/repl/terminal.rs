//! The terminal line editor: history, editing, and a styled prompt.
use super::{Event, Lines};
use rustyline::{
    Cmd, ColorMode, ConditionalEventHandler, Config, Editor, EventContext, EventHandler, Helper,
    KeyEvent, Movement, RepeatCount, completion::Completer, error::ReadlineError,
    highlight::Highlighter, hint::Hinter, history::DefaultHistory, validate::Validator,
};
use std::{io, path::PathBuf};

/// Having a helper is what makes the editor draw the prompt's styled form.
struct Style;
impl Completer for Style {
    type Candidate = String;
}
impl Hinter for Style {
    type Hint = String;
}
impl Highlighter for Style {}
impl Validator for Style {}
impl Helper for Style {}

/// Ctrl-C clears what was typed, pasted lines included; only on an empty
/// prompt does it interrupt.
struct ClearLine;
impl ConditionalEventHandler for ClearLine {
    fn handle(
        &self,
        _: &rustyline::Event,
        _: RepeatCount,
        _: bool,
        context: &EventContext,
    ) -> Option<Cmd> {
        (!context.line().is_empty()).then_some(Cmd::Kill(Movement::WholeBuffer))
    }
}

fn clear_line() -> EventHandler {
    EventHandler::Conditional(Box::new(ClearLine))
}

pub(super) struct Terminal {
    editor: Editor<Style, DefaultHistory>,
    history: PathBuf,
}

impl Terminal {
    pub(super) fn new(history: PathBuf, color: bool) -> io::Result<Self> {
        let config = Config::builder()
            .color_mode(if color {
                ColorMode::Enabled
            } else {
                ColorMode::Disabled
            })
            .build();
        let mut editor = Editor::with_config(config).map_err(io::Error::other)?;
        editor.set_helper(Some(Style));
        editor.bind_sequence(KeyEvent::ctrl('C'), clear_line());
        // A missing history file is the normal first run.
        let _ = editor.load_history(&history);
        Ok(Self { editor, history })
    }
}

/// Ctrl-C and Ctrl-D become events; other editor failures are I/O errors.
fn event(result: Result<String, ReadlineError>) -> io::Result<Event> {
    match result {
        Ok(line) => Ok(Event::Line(line)),
        Err(ReadlineError::Interrupted) => Ok(Event::Interrupt),
        Err(ReadlineError::Eof) => Ok(Event::Eof),
        Err(ReadlineError::Io(error)) => Err(error),
        Err(error) => Err(io::Error::other(error)),
    }
}

impl Lines for Terminal {
    fn read(&mut self, raw: &str, styled: &str, _out: &mut dyn io::Write) -> io::Result<Event> {
        let event = event(self.editor.readline(&(raw, styled)))?;
        if let Event::Line(line) = &event
            && !line.trim().is_empty()
        {
            let _ = self.editor.add_history_entry(line.as_str());
        }
        Ok(event)
    }

    /// Without the clearing binding, Ctrl-C interrupts even after typing, and
    /// the answer stays out of the history.
    fn confirm(&mut self, prompt: &str, _out: &mut dyn io::Write) -> io::Result<Event> {
        self.editor.unbind_sequence(KeyEvent::ctrl('C'));
        let answer = self.editor.readline(prompt);
        self.editor.bind_sequence(KeyEvent::ctrl('C'), clear_line());
        event(answer)
    }

    fn finish(&mut self) {
        if self.editor.save_history(&self.history).is_ok() {
            // Facts can be private, so the history is for its owner alone.
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ =
                    std::fs::set_permissions(&self.history, std::fs::Permissions::from_mode(0o600));
            }
        }
    }
}
