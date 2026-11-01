//! Terminal lifetime and mouse reporting.
use crossterm::{
    event::{DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture},
    execute,
};
use ratatui::DefaultTerminal;
use std::io;
/// The terminal in raw mode on the alternate screen, with bracketed paste,
/// restored when left or dropped.
pub(super) struct Screen {
    pub(super) terminal: DefaultTerminal,
    entered: bool,
}

impl Screen {
    pub(super) fn enter() -> io::Result<Self> {
        let terminal = match ratatui::try_init() {
            Ok(terminal) => terminal,
            Err(error) => {
                // Raw mode may already be on.
                if let Err(restore) = ratatui::try_restore() {
                    tracing::warn!(error = %restore, "cannot restore the terminal");
                }
                return Err(error);
            }
        };
        let screen = Self {
            terminal,
            entered: true,
        };
        execute!(io::stdout(), EnableBracketedPaste, DisableMouseCapture)?;
        Ok(screen)
    }

    pub(super) fn leave(&mut self) -> io::Result<()> {
        if !std::mem::replace(&mut self.entered, false) {
            return Ok(());
        }
        let paste = execute!(io::stdout(), DisableBracketedPaste, DisableMouseCapture);
        ratatui::try_restore().and(paste)
    }
}

impl Drop for Screen {
    fn drop(&mut self) {
        if let Err(error) = self.leave() {
            tracing::warn!(%error, "cannot restore the terminal");
        }
    }
}

pub(super) fn capture_mouse(capture: bool) -> io::Result<()> {
    if capture {
        execute!(io::stdout(), EnableMouseCapture)?;
    } else {
        execute!(io::stdout(), DisableMouseCapture)?;
    }
    Ok(())
}
