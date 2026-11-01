//! What the REPL prints about the ledger: facts, the sessions, and
//! the last turn's context. Each function reads a ledger and writes to a
//! writer, and holds no loop state of its own.
use super::{Queue, label, own_turns, plural, short};
use crate::{
    present,
    render::{
        self,
        turn::{DIM, Paint, TEAL},
    },
};
use nouli_core::{
    history::SourcePoint,
    identity::{Id, SessionSelector},
    protocol::{ErrorCategory, ErrorReport},
    storage::Ledger,
};
use std::fmt::Write as _;
use std::io::{self, Write};

pub(super) fn say(out: &mut impl Write, text: &str) -> io::Result<()> {
    writeln!(out, "{text}")
}

pub(super) fn dim(out: &mut impl Write, paint: Paint, text: &str) -> io::Result<()> {
    let text = paint.apply(DIM, text);
    writeln!(out, "{text}")
}

pub(super) fn fail(out: &mut impl Write, message: &str) -> io::Result<()> {
    writeln!(out, "  error: {}", render::clean(message))
}

/// The session's numbered facts, then what the queue would add or replace.
pub(super) fn facts(
    ledger: &Ledger,
    session: &SessionSelector,
    queue: &Queue,
    paint: Paint,
    out: &mut impl Write,
) -> io::Result<()> {
    let listed = (|| {
        let facts = ledger
            .facts(session)
            .map_err(|error| nouli_core::engine::EngineError::Turn(error).report())?;
        let mut text = Vec::new();
        present::facts_text(ledger, &facts, &mut text).map_err(|_| ErrorReport {
            category: ErrorCategory::Internal,
            code: "facts_unavailable".into(),
            message: "Cannot list the facts".into(),
            retryable: false,
        })?;
        Ok::<_, ErrorReport>(String::from_utf8_lossy(&text).into_owned())
    })();
    let text = match listed {
        Ok(text) => text,
        Err(error) => return fail(out, &error.message),
    };
    for line in text.lines() {
        if line.contains("(from ") || line.contains("(replaced;") || line.contains("no facts yet") {
            dim(out, paint, line)?;
        } else {
            say(out, line)?;
        }
    }
    let pending: Vec<String> =
        queue
            .facts
            .iter()
            .map(|fact| format!("   +  {}   (queued)", render::clean(fact)))
            .chain(queue.replaces.iter().map(|(number, text)| {
                format!("   ~  {number}: {}   (queued)", render::clean(text))
            }))
            .collect();
    for line in pending {
        dim(out, paint, &line)?;
    }
    Ok(())
}

/// Every session with its own turn count and the sessions it came from;
/// `current` is marked.
pub(super) fn view(
    ledger: &Ledger,
    current: Id,
    paint: Paint,
    out: &mut impl Write,
) -> io::Result<()> {
    let rows = (|| {
        let mut rows = Vec::new();
        for session in ledger.list_sessions().map_err(|e| e.report())? {
            let sources = ledger.session_sources(session.id).map_err(|e| e.report())?;
            // A bare `nouli` that was left before its first turn leaves an
            // empty session with no name; there is nothing in it to show.
            let empty = session.name.is_none() && session.frontier.is_empty() && sources.is_empty();
            if empty && session.id != current {
                continue;
            }
            let parents: Vec<String> = sources
                .iter()
                .filter_map(|source| {
                    ledger
                        .session(&SessionSelector::Id(source.session))
                        .ok()
                        .map(|origin| label(&origin))
                })
                .collect();
            rows.push((
                session.id,
                label(&session),
                own_turns(ledger, &session)?,
                parents,
            ));
        }
        Ok::<_, ErrorReport>(rows)
    })();
    let rows = match rows {
        Ok(rows) => rows,
        Err(error) => return fail(out, &error.message),
    };
    for (id, name, turns, parents) in rows {
        let is_current = id == current;
        let mut text = format!(
            "  {} {}  {}",
            if is_current { "●" } else { "○" },
            render::clean(&name),
            plural(turns, "turn")
        );
        if !parents.is_empty() {
            let _ = write!(text, "  ← {}", render::clean(&parents.join(" + ")));
        }
        let text = paint.apply(if is_current { TEAL } else { DIM }, &text);
        say(out, &text)?;
    }
    Ok(())
}

/// What the model saw on the session's last turn.
pub(super) fn context(
    ledger: &Ledger,
    session: &SessionSelector,
    paint: Paint,
    out: &mut impl Write,
) -> io::Result<()> {
    let lines = (|| {
        let session = ledger.session(session).map_err(|e| e.report())?;
        let Some(head) = session.frontier.first().copied() else {
            return Ok(vec![
                "  nothing sent yet: the model has seen no turn".to_owned(),
            ]);
        };
        let turn = ledger
            .turn(head)
            .map_err(|e| nouli_core::engine::EngineError::Turn(e).report())?;
        let history = ledger
            .ancestry(&[SourcePoint {
                session: session.id,
                frontier: session.frontier.clone(),
            }])
            .map_err(|e| nouli_core::engine::EngineError::Turn(e).report())?;
        let mut records = std::collections::HashMap::new();
        for earlier in &history {
            for update in &earlier.updates {
                if let Some(record) = update.new_record() {
                    records.insert(record.id, record.clone());
                }
            }
        }
        let mut lines = vec![format!(
            "  {} · last turn {} saw {} records",
            render::clean(&label(&session)),
            short(head),
            turn.manifest.records.len()
        )];
        for projection in &turn.manifest.records {
            let what = if let Some(record) = records.get(&projection.record) {
                let text = record
                    .value
                    .value()
                    .as_str()
                    .map_or_else(|| record.key.clone(), str::to_owned);
                let kind = if record
                    .key
                    .starts_with(nouli_core::context::facts::KEY_PREFIX)
                {
                    "fact"
                } else {
                    "record"
                };
                format!("{kind:<10}{}", render::clean(&text))
            } else {
                format!("{:<10}turn {}", "decision", short(projection.record))
            };
            let how = format!("{:?}", projection.representation).to_lowercase();
            lines.push(format!("  {what}   ({how})"));
        }
        for outcome in &turn.outcome.outcomes {
            lines.push(format!(
                "  {:<10}{}",
                "answer",
                render::clean(outcome.question_id())
            ));
        }
        Ok::<_, ErrorReport>(lines)
    })();
    match lines {
        Ok(lines) => {
            for line in lines {
                dim(out, paint, &line)?;
            }
            Ok(())
        }
        Err(error) => fail(out, &error.message),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nouli_core::{
        history::ModelSelection,
        storage::{Ledger, OpenMode},
    };

    #[test]
    fn view_leaves_out_empty_sessions_without_a_name_except_the_current_one() {
        let root = std::env::temp_dir().join(format!("nouli-show-{}", Id::generate()));
        let mut ledger = Ledger::open(&root, OpenMode::ReadWriteCreate).unwrap();
        let mut create = |name: Option<&str>| {
            let name = name.map(|name| name.to_owned().try_into().unwrap());
            ledger
                .create_session(name, ModelSelection::default())
                .unwrap()
                .id
        };
        let left = create(None);
        let current = create(None);
        create(Some("named"));
        // Old ledgers may still contain genuinely unnamed sessions.
        rusqlite::Connection::open(ledger.path())
            .unwrap()
            .execute(
                "UPDATE sessions SET name=NULL WHERE id=?1",
                [left.to_string()],
            )
            .unwrap();
        let mut out = Vec::new();
        view(&ledger, current, Paint(false), &mut out).unwrap();
        let shown = String::from_utf8(out).unwrap();
        // Keep the generated current name and explicit name, omitting the empty
        // legacy unnamed session.
        assert_eq!(shown.lines().count(), 2, "{shown}");
        assert!(shown.contains("● ") && shown.contains("named"), "{shown}");
        assert_ne!(left, current);
        let _ = std::fs::remove_dir_all(&root);
    }
}
