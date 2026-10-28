//! Canonical ancestor-first ordering, deduplicated by immutable turn identity.

use std::collections::{HashMap, HashSet};

use super::{FinalizedTurn, HistoryError, SourcePoint};

/// Sources, frontier entries, and parent lists supply tie order. Storage order
/// and wall-clock completion times never influence this projection.
pub fn canonical_history<'a>(
    turns: &'a [FinalizedTurn],
    sources: &[SourcePoint],
) -> Result<Vec<&'a FinalizedTurn>, HistoryError> {
    let mut index = HashMap::new();
    for turn in turns {
        if index.insert(turn.id, turn).is_some() {
            return Err(HistoryError::DuplicateTurn);
        }
    }
    let mut ordered = Vec::new();
    let mut complete = HashSet::new();
    let mut visiting = HashSet::new();
    for source in sources {
        let mut unique = HashSet::new();
        if source.frontier.is_empty() || source.frontier.iter().any(|id| !unique.insert(id)) {
            return Err(HistoryError::InvalidFrontier);
        }
        for id in &source.frontier {
            let head = index.get(id).ok_or(HistoryError::UnknownTurn)?;
            if head.session != source.session {
                return Err(HistoryError::ForeignHead);
            }
            // Iterative DFS avoids stack overflow on a long valid session.
            let mut stack = vec![(*id, false)];
            while let Some((current, exiting)) = stack.pop() {
                if exiting {
                    visiting.remove(&current);
                    complete.insert(current);
                    ordered.push(*index.get(&current).ok_or(HistoryError::UnknownTurn)?);
                    continue;
                }
                if complete.contains(&current) {
                    continue;
                }
                if !visiting.insert(current) {
                    return Err(HistoryError::Cycle);
                }
                let turn = index.get(&current).ok_or(HistoryError::UnknownTurn)?;
                turn.validate()?;
                stack.push((current, true));
                for parent in turn.parents.iter().rev() {
                    stack.push((*parent, false));
                }
            }
        }
    }
    Ok(ordered)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::Id;
    use crate::{
        context::ContextUpdate,
        history::{AttemptRecord, SessionSelection},
    };

    #[derive(serde::Deserialize)]
    struct Fixture {
        turns: Vec<FinalizedTurn>,
        sources: Vec<SourcePoint>,
        expected: Vec<Id>,
        unrelated_pinned_record: Id,
        failed_attempt: AttemptRecord,
        selections: std::collections::HashMap<String, SessionSelection>,
    }

    #[test]
    fn fixture_deduplicates_diamond_and_preserves_source_order() {
        let mut fixture: Fixture =
            serde_json::from_str(include_str!("../../../fixtures/contracts/history.json")).unwrap();
        fixture.turns.reverse();
        let ordered = canonical_history(&fixture.turns, &fixture.sources).unwrap();
        assert!(matches!(
            fixture.selections["continuation"],
            SessionSelection::Continue { .. }
        ));
        assert!(
            matches!(&fixture.selections["branch"], SessionSelection::From {sources, ..} if sources.len() == 1)
        );
        assert!(
            matches!(&fixture.selections["blend"], SessionSelection::From {sources, ..} if sources.len() == 2)
        );
        assert_eq!(
            ordered.iter().map(|t| t.id).collect::<Vec<_>>(),
            fixture.expected
        );
        assert!(
            ordered
                .iter()
                .flat_map(|t| &t.updates)
                .filter_map(ContextUpdate::new_record)
                .all(|r| r.id != fixture.unrelated_pinned_record)
        );
        let source = SourcePoint {
            session: fixture.failed_attempt.session,
            frontier: vec![fixture.failed_attempt.id],
        };
        assert_eq!(
            canonical_history(&fixture.turns, &[source]),
            Err(HistoryError::UnknownTurn)
        );
        let original = &ordered[0];
        assert!(matches!(original.updates[0], ContextUpdate::Pin { .. }));
        assert!(matches!(
            ordered[1].updates[0],
            ContextUpdate::Supersede { .. }
        ));
        assert!(matches!(
            ordered[2].updates[0],
            ContextUpdate::Supersede { .. }
        ));
    }

    #[test]
    fn rejects_foreign_heads_cycles_and_empty_frontiers() {
        let mut fixture: Fixture =
            serde_json::from_str(include_str!("../../../fixtures/contracts/history.json")).unwrap();
        let mut source = fixture.sources[0].clone();
        source.session = Id::generate();
        assert_eq!(
            canonical_history(&fixture.turns, &[source.clone()]),
            Err(HistoryError::ForeignHead)
        );
        source.frontier.clear();
        assert_eq!(
            canonical_history(&fixture.turns, &[source]),
            Err(HistoryError::InvalidFrontier)
        );
        fixture.turns[0].parents = vec![fixture.turns[0].id];
        assert_eq!(
            canonical_history(&fixture.turns, &fixture.sources),
            Err(HistoryError::Cycle)
        );
    }
}
