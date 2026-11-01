//! Human text for a finished turn, from the report plus the session's history.
use crate::{
    render::{
        RenderError, clean,
        turn::{self, DIM, Header, Paint},
    },
    turns::fact_counts,
};
use nouli_core::{
    context::{ContextUpdate, facts::Fact},
    history::{FinalizedTurn, SourcePoint},
    identity::SessionSelector,
    protocol::{ErrorReport, TurnReport},
    storage::{Ledger, OpenMode},
};
use std::fmt::Write as _;
use std::{io::Write, path::Path};
use thiserror::Error;

#[derive(Debug, Error)]
pub(crate) enum PresentError {
    #[error(transparent)]
    Render(#[from] RenderError),
    #[error("cannot read the session history")]
    History(ErrorReport),
}

/// The turns that came before this one, oldest first, and how many of them
/// belong to the session itself.
fn earlier(store: &Path, report: &TurnReport) -> Result<(Vec<FinalizedTurn>, usize), PresentError> {
    let sources = match (report.session, report.turn) {
        (Some(session), Some(turn)) => vec![SourcePoint {
            session,
            frontier: vec![turn],
        }],
        _ => report.manifest.sources.clone(),
    };
    if sources.is_empty() {
        return Ok((vec![], 0));
    }
    let history = Ledger::open(store, OpenMode::ReadOnlyExisting)
        .map_err(|error| PresentError::History(error.report()))
        .and_then(|ledger| {
            ledger.ancestry(&sources).map_err(|error| {
                PresentError::History(nouli_core::engine::EngineError::Turn(error).report())
            })
        })?;
    let own = report.session.map_or(0, |session| {
        crate::repl::own_history(&history, session).count()
    });
    Ok((
        history
            .into_iter()
            .filter(|turn| Some(turn.id) != report.turn)
            .collect(),
        own,
    ))
}

#[derive(Clone, Copy)]
pub(crate) enum TurnDisplay {
    Full,
    Transcript,
    Compact(usize),
}

/// Writes a turn as bars and changes. `updates` are the turn's caller updates,
/// which tell how many facts it recorded or replaced.
pub(crate) fn turn_text(
    sink: &mut impl Write,
    report: &TurnReport,
    updates: &[ContextUpdate],
    store: &Path,
    color: bool,
    display: TurnDisplay,
) -> Result<(), PresentError> {
    let (history, own) = earlier(store, report)?;
    let previous = |id: &str| {
        history
            .iter()
            .rev()
            .find_map(|turn| turn.outcome.outcome_for(id))
    };
    let (facts, replaced) = fact_counts(updates);
    let header = Header {
        turn: report.turn.map(|_| own),
        facts,
        replaced,
        questions: report.questions.len(),
    };
    match display {
        TurnDisplay::Compact(columns) => turn::write_compact(
            sink,
            &header,
            &report.questions,
            &report.outcome.outcomes,
            previous,
            color,
            columns,
        )?,
        _ => turn::write(
            sink,
            &header,
            &report.questions,
            &report.outcome.outcomes,
            previous,
            color,
        )?,
    }
    if !matches!(display, TurnDisplay::Full) {
        sink.flush().map_err(RenderError::from)?;
        return Ok(());
    }
    let mut line = format!(
        "  {}/{}",
        clean(&report.outcome.provider.provider),
        clean(&report.outcome.provider.model)
    );
    if let Some(session) = report.session {
        let _ = write!(line, " · session {session}");
    }
    if let Some(turn) = report.turn {
        let _ = write!(
            line,
            " · turn {turn}{}",
            if report.replayed { " (replayed)" } else { "" }
        );
    }
    writeln!(sink, "{}", Paint(color).apply(DIM, &line)).map_err(RenderError::from)?;
    sink.flush().map_err(RenderError::from)?;
    Ok(())
}

/// Writes numbered facts, noting inherited and replaced ones.
pub(crate) fn facts_text(
    ledger: &Ledger,
    facts: &[Fact],
    sink: &mut impl Write,
) -> Result<(), crate::inspect::InspectError> {
    if facts.is_empty() {
        writeln!(sink, "  no facts yet")?;
    }
    for fact in facts {
        let mut line = if fact.alternatives.is_empty() {
            format!("  {:>2}  {}", fact.number, clean(&fact.text))
        } else {
            let alternatives: Vec<_> = fact
                .alternatives
                .iter()
                .map(|text| format!("\"{}\"", clean(text)))
                .collect();
            format!(
                "  {:>2}  {}   (conflict: replaced differently in blended sessions; --replace {} settles it)",
                fact.number,
                alternatives.join(" | "),
                fact.number
            )
        };
        if let Some(source) = fact.source_session {
            let name = ledger
                .session(&SessionSelector::Id(source))
                .ok()
                .and_then(|session| session.name)
                .map_or_else(|| source.to_string(), |name| name.as_str().to_owned());
            let _ = write!(line, "   (from {})", clean(&name));
        }
        if let Some(previous) = &fact.previous {
            let _ = write!(line, "   (replaced; was: {})", clean(previous));
        }
        writeln!(sink, "{line}")?;
    }
    sink.flush()?;
    Ok(())
}
