//! The sessions `/sessions` offers to resume.
use crate::repl::label;
use nouli_core::{
    context::{ContextUpdate, facts},
    history::FinalizedTurn,
    identity::Id,
    protocol::ErrorReport,
};
use std::path::Path;

/// A session to resume, with the first thing said in it.
pub(super) struct Entry {
    pub(super) id: Id,
    pub(super) label: String,
    pub(super) first: String,
    pub(super) turns: usize,
}

/// Every session with turns of its own except `current`, the one with the
/// latest turn first.
pub(super) fn entries(store: &Path, current: Id) -> Result<Vec<Entry>, ErrorReport> {
    let Some(ledger) = crate::repl::open_existing(store)? else {
        return Ok(vec![]);
    };
    let mut found = Vec::new();
    for session in ledger.list_sessions().map_err(|error| error.report())? {
        if session.id == current || session.frontier.is_empty() {
            continue;
        }
        let history = crate::repl::session_history(&ledger, &session)?;
        let own: Vec<_> = crate::repl::own_history(&history, session.id).collect();
        let (Some(first), Some(last)) = (own.first(), own.last()) else {
            continue;
        };
        found.push((
            last.id,
            Entry {
                id: session.id,
                label: label(&session),
                first: first_message(first),
                turns: own.len(),
            },
        ));
    }
    // IDs are time-ordered, so the latest turn has the greatest.
    found.sort_by_key(|(id, _)| std::cmp::Reverse(*id));
    Ok(found.into_iter().map(|(_, entry)| entry).collect())
}

/// The turn's first fact, or its first question when it added none.
fn first_message(turn: &FinalizedTurn) -> String {
    turn.updates
        .iter()
        .find_map(|update| match update {
            ContextUpdate::Append { record } if record.key.starts_with(facts::KEY_PREFIX) => {
                record.value.value().as_str().map(str::to_owned)
            }
            _ => None,
        })
        .or_else(|| {
            turn.request
                .questions
                .first()
                .map(|question| question.prompt.clone())
        })
        .unwrap_or_default()
}

use super::{App, Status};
use crate::render;
use std::io;

impl App<'_> {
    pub(super) fn open_sessions(&mut self) {
        match entries(&self.repl.settings.store, self.repl.session.id) {
            Ok(entries) if entries.is_empty() => {
                self.status = Some(Status::info("no other session has turns yet"));
            }
            Ok(entries) => {
                self.sessions = Some(entries);
                self.choice = 0;
            }
            Err(error) => self.status = Some(Status::error(render::clean(&error.message))),
        }
    }

    /// Switches to the chosen session, as `/use` would.
    pub(super) fn resume(&mut self) -> io::Result<()> {
        let choice = self.choice;
        let Some(entry) = self
            .sessions
            .take()
            .and_then(|entries| entries.into_iter().nth(choice))
        else {
            return Ok(());
        };
        self.repl.handle(&format!("/use {}", entry.id))?;
        self.drain();
        self.refresh_model();
        self.refresh_previous_question();
        self.refresh_graph();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repl::tui::{tests::*, *};

    #[test]
    fn the_sessions_list_keeps_the_keys_until_it_closes() {
        let settings = settings();
        let mut app = app(&settings);
        let entry = |label: &str| sessions::Entry {
            id: Id::generate(),
            label: label.into(),
            first: "a fact".into(),
            turns: 1,
        };
        let open = |app: &mut App| app.sessions = Some(vec![entry("a"), entry("b")]);
        open(&mut app);
        // Tab and typing do nothing while it is open; Down moves within it.
        press(&mut app, KeyCode::Tab, KeyModifiers::NONE);
        typed(&mut app, "x");
        press(&mut app, KeyCode::Down, KeyModifiers::NONE);
        press(&mut app, KeyCode::Down, KeyModifiers::NONE);
        assert_eq!((app.choice, app.form.input.as_str()), (1, ""));
        press(&mut app, KeyCode::Esc, KeyModifiers::NONE);
        assert!(app.sessions.is_none());
        open(&mut app);
        press(&mut app, KeyCode::Char('g'), KeyModifiers::CONTROL);
        assert!(app.sessions.is_none());
        assert!(app.graph_focused);
        press(&mut app, KeyCode::Esc, KeyModifiers::NONE);
        assert!(!app.graph_focused);
        open(&mut app);
        press(&mut app, KeyCode::Char('c'), KeyModifiers::CONTROL);
        assert!(
            app.sessions.is_none(),
            "Ctrl-C closes it before anything else"
        );
        // Enter resumes through /use, which reports a session it cannot find.
        open(&mut app);
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE);
        assert!(app.sessions.is_none());
        assert!(
            transcript(&app).contains("no session"),
            "{}",
            transcript(&app)
        );
    }
}
