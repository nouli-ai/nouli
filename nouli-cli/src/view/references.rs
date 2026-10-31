//! Reference dependencies are a read-only graph projection, never ledger parents.
use crate::{inspect::InspectError, render::RenderError};
use nouli_core::{
    history::{FinalizedTurn, Representation, SourcePoint},
    identity::{Id, SessionSelector},
    storage::{
        Ledger,
        decisions::DecisionSource,
        inspection::{InspectionError, SessionExport, SessionProvenance},
    },
};
use std::collections::{HashMap, HashSet};

#[derive(Clone)]
pub(super) struct Link {
    pub source: DecisionSource,
    pub target: Id,
    pub question: String,
}

pub(super) fn links(turns: &[&FinalizedTurn]) -> Result<Vec<Link>, RenderError> {
    let records = turns
        .iter()
        .flat_map(|turn| {
            turn.context_base
                .iter()
                .flat_map(|base| base.updates.iter())
                .chain(turn.updates.iter())
        })
        .filter_map(|update| update.new_record())
        .map(|record| (record.id, record))
        .collect::<HashMap<_, _>>();
    let mut bindings = HashMap::new();
    for record in records
        .values()
        .filter(|record| record.key.starts_with("decision:"))
    {
        let value = &record.value.value()["nouli_decision_reference"];
        let Some(reference) = value["reference"].as_str() else {
            continue;
        };
        if crate::render::decision_tags(reference).first() != Some(&(0..reference.len())) {
            continue;
        }
        let Ok(source) = serde_json::from_value::<DecisionSource>(value["source"].clone()) else {
            continue;
        };
        bindings.insert(record.id, (reference, source));
    }
    let mut links = Vec::new();
    let mut seen = HashSet::new();
    for turn in turns {
        let present = turn
            .manifest
            .records
            .iter()
            .filter(|record| record.representation != Representation::Omitted)
            .map(|record| record.record)
            .collect::<HashSet<_>>();
        for (record, (reference, source)) in &bindings {
            if !present.contains(record) {
                continue;
            }
            let shared = serde_json::to_string(&turn.request.state)
                .map_err(|_| RenderError::Protocol)?
                .contains(reference)
                || present
                    .iter()
                    .filter_map(|id| records.get(id))
                    .any(|record| {
                        !record.key.starts_with("decision:")
                            && !record.key.starts_with("image:")
                            && record.value.value().to_string().contains(reference)
                    });
            let explicit = turn
                .request
                .questions
                .iter()
                .filter(|question| {
                    serde_json::to_string(question).is_ok_and(|text| text.contains(reference))
                })
                .map(|q| q.id.as_str())
                .collect::<HashSet<_>>();
            for question in &turn.request.questions {
                if (shared || explicit.is_empty() || explicit.contains(question.id.as_str()))
                    && seen.insert((
                        source.turn,
                        source.question.clone(),
                        turn.id,
                        question.id.clone(),
                    ))
                {
                    links.push(Link {
                        source: source.clone(),
                        target: turn.id,
                        question: question.id.clone(),
                    });
                }
            }
        }
    }
    links.sort_by(|a, b| {
        (&a.source.turn, &a.source.question, &a.target, &a.question).cmp(&(
            &b.source.turn,
            &b.source.question,
            &b.target,
            &b.question,
        ))
    });
    Ok(links)
}

/// Include fixed referenced source points even after reopening a destination
/// without ever visiting its source session. Structured exports remain unchanged.
pub(crate) fn include(
    ledger: &Ledger,
    mut exports: Vec<SessionExport>,
) -> Result<Vec<SessionExport>, InspectError> {
    if exports.is_empty() {
        return Ok(exports);
    }
    let mut seen = exports
        .iter()
        .flat_map(|export| &export.turns)
        .map(|turn| turn.id)
        .collect::<HashSet<_>>();
    let mut provenance = exports
        .iter()
        .flat_map(|export| &export.provenance)
        .map(|session| session.id)
        .collect::<HashSet<_>>();
    let mut checked = HashSet::new();
    loop {
        let dependencies = links(
            &exports
                .iter()
                .flat_map(|export| &export.turns)
                .collect::<Vec<_>>(),
        )?;
        let mut added = false;
        for link in dependencies {
            if !checked.insert((link.source.turn, link.source.question.clone())) {
                continue;
            }
            let source = ledger
                .turn(link.source.turn)
                .map_err(InspectionError::from)?;
            if source.session != link.source.session
                || !source
                    .request
                    .questions
                    .iter()
                    .any(|q| q.id == link.source.question)
            {
                return Err(RenderError::Protocol.into());
            }
            if seen.contains(&source.id) {
                continue;
            }
            let history = ledger
                .ancestry(&[SourcePoint {
                    session: source.session,
                    frontier: vec![source.id],
                }])
                .map_err(InspectionError::from)?;
            for turn in history {
                if !seen.insert(turn.id) {
                    continue;
                }
                if provenance.insert(turn.session) {
                    let session = ledger.session(&SessionSelector::Id(turn.session))?;
                    exports[0].provenance.push(SessionProvenance {
                        id: session.id,
                        name: session.name,
                        defaults: session.defaults,
                        sources: ledger.session_sources(session.id)?,
                    });
                }
                exports[0].turns.push(turn);
                added = true;
            }
        }
        if !added {
            break;
        }
    }
    Ok(exports)
}
