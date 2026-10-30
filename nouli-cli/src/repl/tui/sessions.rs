//! The sessions `/sessions` offers to resume.
use crate::repl::label;
use nouli_core::{
    context::{ContextUpdate, facts},
    history::{FinalizedTurn, SourcePoint},
    identity::Id,
    protocol::ErrorReport,
    storage::{Ledger, OpenMode, StorageError},
};
use std::{io, path::Path};

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
    let ledger = match Ledger::open(store, OpenMode::ReadOnlyExisting) {
        Ok(ledger) => ledger,
        Err(StorageError::Io { source, .. }) if source.kind() == io::ErrorKind::NotFound => {
            return Ok(vec![]);
        }
        Err(error) => return Err(error.report()),
    };
    let mut found = Vec::new();
    for session in ledger.list_sessions().map_err(|error| error.report())? {
        if session.id == current || session.frontier.is_empty() {
            continue;
        }
        let history = ledger
            .ancestry(&[SourcePoint {
                session: session.id,
                frontier: session.frontier.clone(),
            }])
            .map_err(|error| nouli_core::engine::EngineError::Turn(error).report())?;
        // History runs oldest first.
        let own: Vec<_> = history
            .iter()
            .filter(|turn| turn.session == session.id)
            .collect();
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
