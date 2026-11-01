//! Session identities, history selection, creation, and REPL session operations.
use super::{Current, Repl, start};
use crate::{
    evaluate::input_error,
    render, selection,
    settings::Settings,
    turns::{Creation, Draft},
};
use nouli_core::{
    engine::EngineError,
    history::{FinalizedTurn, ModelSelection, SourcePoint, SourceSelection},
    identity::{Id, SessionName, SessionSelector},
    protocol::{ErrorReport, SessionReport},
    storage::{Ledger, OpenMode, StorageError, replay::ReplayError, sessions::Session},
};
use std::{
    io::{self, Write},
    path::Path,
};

pub(super) fn short(id: Id) -> String {
    let id = id.to_string();
    id[id.len() - 8..].to_owned()
}

pub(super) fn label(session: &Session) -> String {
    session
        .name
        .as_ref()
        .map_or_else(|| short(session.id), |name| name.as_str().to_owned())
}

/// The ledger, or `None` when the store has no ledger yet.
pub(crate) fn open_existing(store: &Path) -> Result<Option<Ledger>, ErrorReport> {
    match Ledger::open(store, OpenMode::ReadOnlyExisting) {
        Ok(ledger) => Ok(Some(ledger)),
        Err(StorageError::Io { source, .. }) if source.kind() == io::ErrorKind::NotFound => {
            Ok(None)
        }
        Err(error) => Err(error.report()),
    }
}

/// History runs oldest first, including the session's inherited turns.
pub(crate) fn session_history(
    ledger: &Ledger,
    session: &Session,
) -> Result<Vec<FinalizedTurn>, ErrorReport> {
    if session.frontier.is_empty() {
        return Ok(vec![]);
    }
    ledger
        .ancestry(&[SourcePoint {
            session: session.id,
            frontier: session.frontier.clone(),
        }])
        .map_err(|error| nouli_core::engine::EngineError::Turn(error).report())
}

/// A session's own turns, excluding inherited history.
pub(crate) fn own_history(
    history: &[FinalizedTurn],
    session: Id,
) -> impl DoubleEndedIterator<Item = &FinalizedTurn> {
    history.iter().filter(move |turn| turn.session == session)
}

/// How many turns a session wrote itself, not counting inherited history.
pub(super) fn own_turns(ledger: &Ledger, session: &Session) -> Result<usize, ErrorReport> {
    Ok(own_history(&session_history(ledger, session)?, session.id).count())
}

pub(crate) fn create(settings: &Settings, creation: &Creation) -> Result<Session, ErrorReport> {
    let mut ledger =
        Ledger::open(&settings.store, OpenMode::ReadWriteCreate).map_err(|error| error.report())?;
    let session = if creation.sources.is_empty() {
        ledger
            .create_session(Some(creation.name.clone()), creation.defaults.clone())
            .map_err(|error| error.report())?
    } else {
        ledger
            .create_session_from(
                Some(creation.name.clone()),
                creation.defaults.clone(),
                &creation.sources,
            )
            .map_err(|error| EngineError::Source(error).report())?
    };
    Ok(session)
}

pub(crate) fn session_report(
    settings: &Settings,
    session: Session,
) -> Result<SessionReport, ErrorReport> {
    let ledger = Ledger::open(&settings.store, OpenMode::ReadOnlyExisting)
        .map_err(|error| error.report())?;
    let sources = ledger
        .session_sources(session.id)
        .map_err(|error| error.report())?;
    Ok(SessionReport {
        session: session.id,
        name: session.name,
        sources,
    })
}

/// Replaces `--replace N` numbers with the session's current facts.
pub(crate) fn resolve(settings: &Settings, draft: Draft) -> Result<Draft, ErrorReport> {
    let Some(session) = draft.replaced_session().cloned() else {
        return Ok(draft);
    };
    let ledger = Ledger::open(&settings.store, OpenMode::ReadOnlyExisting)
        .map_err(|error| error.report())?;
    // A repeated command repeats its saved replacements; after the first run
    // the numbers no longer name the facts they named then.
    if let Some(key) = draft.request_id()
        && let Some(saved) = ledger
            .saved_request_by_id(key)
            .map_err(|error| EngineError::from(error).report())?
    {
        return draft
            .replay(&saved.request.updates)
            .ok_or_else(|| EngineError::from(ReplayError::IntentConflict).report());
    }
    let current = ledger
        .facts(&session)
        .map_err(|error| EngineError::Turn(error).report())?;
    draft.resolve(&current).map_err(|error| input_error(&error))
}
impl<W: Write> Repl<'_, W> {
    pub(super) fn switch(&mut self, session: &Session) {
        self.session = Current {
            id: session.id,
            label: label(session),
        };
    }

    pub(super) fn use_session(&mut self, name: &str) -> io::Result<()> {
        let Ok(selector) = selection::selector(name) else {
            return self.fail("that is not a valid session name");
        };
        let found = self
            .ledger()
            .and_then(|ledger| ledger.session(&selector).map_err(|e| e.report()));
        match found {
            Ok(session) => {
                self.switch(&session);
                self.dim(&format!("  {}", render::clean(&label(&session))))
            }
            Err(_) => self.fail(&format!("no session \"{}\"", render::clean(name))),
        }
    }

    pub(super) fn create(&mut self, name: &str, sources: Vec<SourceSelection>) -> Option<Session> {
        let name = match SessionName::try_from(name.to_owned()) {
            Ok(name) => name,
            Err(error) => {
                let _ = self.fail(&error.to_string());
                return None;
            }
        };
        let creation = Creation {
            name,
            sources,
            defaults: ModelSelection::default(),
        };
        match create(&self.settings, &creation) {
            Ok(session) => Some(session),
            Err(error) => {
                let _ = self.fail(&error.message);
                None
            }
        }
    }

    pub(super) fn rename_session(&mut self, name: &str) -> io::Result<()> {
        let name = match SessionName::try_from(name.to_owned()) {
            Ok(name) => name,
            Err(error) => return self.fail(&error.to_string()),
        };
        let renamed = Ledger::open(&self.settings.store, OpenMode::ReadWriteCreate)
            .map_err(|error| error.report())
            .and_then(|mut ledger| {
                ledger
                    .rename_session(self.session.id, name)
                    .map_err(|error| error.report())
            });
        match renamed {
            Ok(session) => {
                self.switch(&session);
                self.dim(&format!(
                    "  renamed to {}",
                    render::clean(&self.session.label)
                ))
            }
            Err(error) => self.fail(&error.message),
        }
    }

    pub(super) fn new_session(&mut self, name: Option<&str>) -> io::Result<()> {
        let session = if let Some(name) = name {
            self.create(name, vec![])
        } else {
            match start::new_session(&self.settings) {
                Ok(session) => Some(session),
                Err(error) => {
                    self.fail(&error.message)?;
                    None
                }
            }
        };
        if let Some(session) = session {
            self.switch(&session);
            self.dim(&format!(
                "  new session \"{}\"",
                render::clean(&self.session.label)
            ))?;
        }
        Ok(())
    }

    pub(super) fn branch(&mut self, name: &str) -> io::Result<()> {
        let from = self.ledger().and_then(|ledger| {
            let session = ledger.session(&self.selector()).map_err(|e| e.report())?;
            let turns = own_turns(&ledger, &session)?;
            Ok((session, turns))
        });
        let (source, turns) = match from {
            Ok(found) => found,
            Err(error) => return self.fail(&error.message),
        };
        if source.frontier.is_empty() {
            return self.say("  send a turn first: a branch starts from the last one");
        }
        let sources = vec![SourceSelection {
            session: self.selector(),
            frontier: None,
        }];
        if let Some(session) = self.create(name, sources) {
            self.switch(&session);
            self.dim(&format!(
                "  new session \"{}\" from {} @ turn {turns}",
                render::clean(name),
                render::clean(&label(&source))
            ))?;
        }
        Ok(())
    }

    pub(super) fn blend(&mut self, names: &[String], name: &str) -> io::Result<()> {
        let mut sources = Vec::new();
        for source in names {
            let Ok(selector) = selection::selector(source) else {
                return self.fail("that is not a valid session name");
            };
            let found = self
                .ledger()
                .and_then(|ledger| ledger.session(&selector).map_err(|e| e.report()));
            match found {
                Ok(session) if session.frontier.is_empty() => {
                    return self.say(&format!(
                        "  send a turn on \"{}\" first: a blend takes each source's last turn",
                        render::clean(source)
                    ));
                }
                Ok(session) => sources.push(SourceSelection {
                    session: SessionSelector::Id(session.id),
                    frontier: None,
                }),
                Err(_) => return self.fail(&format!("no session \"{}\"", render::clean(source))),
            }
        }
        if let Some(session) = self.create(name, sources) {
            self.switch(&session);
            self.dim(&format!(
                "  new session \"{}\" from {}",
                render::clean(name),
                render::clean(&names.join(" + "))
            ))?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nearby_unnamed_sessions_use_randomness_in_their_short_labels() {
        let first = Id::generate();
        let second = Id::generate();
        assert_ne!(short(first), short(second));
        assert!(first.to_string().ends_with(&short(first)));
        assert!(second.to_string().ends_with(&short(second)));
    }
}
