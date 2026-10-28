//! Causal value and retention revisions over an immutable decision DAG.

use super::{ContextError, ContextRecord, ContextUpdate, PinTarget};
use crate::{
    history::{FinalizedTurn, HistoryError, SourcePoint, order::canonical_history},
    identity::Id,
};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ActiveRecord {
    pub record: ContextRecord,
    pub pinned: bool,
    pub source_session: Id,
    pub source_turn: Id,
    pub revision: usize,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ContextConflict {
    pub key: String,
    pub revisions: Vec<ActiveRecord>,
}
#[derive(Debug, Error)]
pub enum ProjectionError {
    #[error(transparent)]
    Context(#[from] ContextError),
    #[error(transparent)]
    History(#[from] HistoryError),
    #[error("selected context contains unresolved revisions")]
    Conflicts { conflicts: Vec<ContextConflict> },
}

struct Revision {
    active: ActiveRecord,
    parents: Vec<usize>,
}
#[derive(Default)]
struct Projector {
    revisions: Vec<Revision>,
    records: HashSet<Id>,
}
impl Projector {
    fn merge(&self, heads: impl IntoIterator<Item = usize>) -> Vec<usize> {
        let heads: Vec<_> = heads.into_iter().collect();
        let mut covered = HashSet::new();
        let mut pending: Vec<_> = heads
            .iter()
            .flat_map(|id| self.revisions[*id].parents.iter().copied())
            .collect();
        while let Some(id) = pending.pop() {
            if covered.insert(id) {
                pending.extend(self.revisions[id].parents.iter().copied());
            }
        }
        let mut seen = HashSet::new();
        heads
            .into_iter()
            .filter(|id| !covered.contains(id) && seen.insert(*id))
            .collect()
    }
    fn conflicts(&self, heads: &[usize]) -> Vec<ContextConflict> {
        let mut groups: Vec<ContextConflict> = Vec::new();
        for id in heads {
            let active = &self.revisions[*id].active;
            if let Some(group) = groups.iter_mut().find(|g| g.key == active.record.key) {
                group.revisions.push(active.clone());
            } else {
                groups.push(ContextConflict {
                    key: active.record.key.clone(),
                    revisions: vec![active.clone()],
                });
            }
        }
        groups
            .into_iter()
            .filter(|g| g.revisions.len() > 1)
            .collect()
    }
    fn ensure_resolved(&self, heads: &[usize]) -> Result<(), ProjectionError> {
        let conflicts = self.conflicts(heads);
        if conflicts.is_empty() {
            Ok(())
        } else {
            Err(ProjectionError::Conflicts { conflicts })
        }
    }
    fn apply(
        &mut self,
        heads: &mut Vec<usize>,
        updates: &[ContextUpdate],
        session: Id,
        turn: Id,
    ) -> Result<(), ProjectionError> {
        for (index, update) in updates.iter().enumerate() {
            update.validate()?;
            if let Some(record) = update.new_record()
                && !self.records.insert(record.id)
            {
                return Err(ContextError::DuplicateRecord.into());
            }
            let active_targets = |targets: &[Id]| -> Result<Vec<usize>, ProjectionError> {
                let mut result = Vec::new();
                for target in targets {
                    let matching: Vec<_> = heads
                        .iter()
                        .filter(|id| self.revisions[**id].active.record.id == *target)
                        .copied()
                        .collect();
                    if matching.is_empty() {
                        return Err(if self.records.contains(target) {
                            ContextError::InactiveRecord
                        } else {
                            ContextError::UnavailableRecord
                        }
                        .into());
                    }
                    result.extend(matching);
                }
                Ok(result)
            };
            let (record, pinned, parents) = match update {
                ContextUpdate::Append { record } => (record.clone(), false, vec![]),
                ContextUpdate::Pin {
                    target: PinTarget::New { record },
                } => (record.clone(), true, vec![]),
                ContextUpdate::Pin {
                    target: PinTarget::Existing { target },
                }
                | ContextUpdate::Unpin { target } => {
                    let parents = active_targets(&[*target])?;
                    if parents.len() != 1 {
                        return Err(ProjectionError::Conflicts {
                            conflicts: self.conflicts(heads),
                        });
                    }
                    let record = self.revisions[parents[0]].active.record.clone();
                    (record, matches!(update, ContextUpdate::Pin { .. }), parents)
                }
                ContextUpdate::Supersede {
                    targets,
                    replacement,
                    pinned,
                } => {
                    let parents = active_targets(targets)?;
                    if parents
                        .iter()
                        .any(|id| self.revisions[*id].active.record.key != replacement.key)
                    {
                        return Err(ContextError::InvalidTargets.into());
                    }
                    let inherited = parents.iter().any(|id| self.revisions[*id].active.pinned);
                    (replacement.clone(), pinned.unwrap_or(inherited), parents)
                }
            };
            heads.retain(|id| !parents.contains(id));
            let id = self.revisions.len();
            self.revisions.push(Revision {
                active: ActiveRecord {
                    record,
                    pinned,
                    source_session: session,
                    source_turn: turn,
                    revision: index,
                },
                parents,
            });
            heads.push(id);
        }
        self.ensure_resolved(heads)
    }
}

/// Request updates are projected without mutating their source histories.
/// The caller publishes them only as part of a successfully finalized turn.
pub fn project(
    turns: &[FinalizedTurn],
    sources: &[SourcePoint],
    updates: &[ContextUpdate],
    session: Id,
    turn: Id,
) -> Result<Vec<ActiveRecord>, ProjectionError> {
    let ordered = canonical_history(turns, sources)?;
    let mut projector = Projector::default();
    let mut states: HashMap<Id, Vec<usize>> = HashMap::new();
    for turn in ordered {
        let mut heads = projector.merge(
            turn.parents
                .iter()
                .flat_map(|parent| states[parent].iter().copied()),
        );
        projector.apply(&mut heads, &turn.updates, turn.session, turn.id)?;
        states.insert(turn.id, heads);
    }
    let mut heads = projector.merge(
        sources
            .iter()
            .flat_map(|source| source.frontier.iter())
            .flat_map(|head| states[head].iter().copied()),
    );
    projector.apply(&mut heads, updates, session, turn)?;
    Ok(heads
        .into_iter()
        .map(|id| projector.revisions[id].active.clone())
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{context::ContextKind, data::Data};
    fn record(key: &str, value: i64) -> ContextRecord {
        ContextRecord {
            id: Id::generate(),
            key: key.into(),
            value: Data::new(serde_json::json!(value)).unwrap(),
            kind: ContextKind::Evidence,
        }
    }
    fn turn(parents: Vec<Id>, updates: Vec<ContextUpdate>) -> FinalizedTurn {
        let data: serde_json::Value =
            serde_json::from_str(include_str!("../../../fixtures/contracts/history.json")).unwrap();
        let mut result: FinalizedTurn = serde_json::from_value(data["turns"][0].clone()).unwrap();
        result.id = Id::generate();
        result.session = Id::generate();
        result.parents = parents;
        result.updates = updates;
        result
    }
    fn source(turn: &FinalizedTurn) -> SourcePoint {
        SourcePoint {
            session: turn.session,
            frontier: vec![turn.id],
        }
    }
    fn selected(
        turns: &[FinalizedTurn],
        sources: &[SourcePoint],
        updates: &[ContextUpdate],
    ) -> Result<Vec<ActiveRecord>, ProjectionError> {
        project(turns, sources, updates, Id::generate(), Id::generate())
    }
    #[test]
    fn retention_and_replacements_are_causal_and_old_sources_are_isolated() {
        let first = record("budget", 8);
        let second = record("budget", 12);
        let root = turn(
            vec![],
            vec![ContextUpdate::Pin {
                target: PinTarget::New {
                    record: first.clone(),
                },
            }],
        );
        let branch = turn(
            vec![root.id],
            vec![ContextUpdate::Supersede {
                targets: vec![first.id],
                replacement: second.clone(),
                pinned: None,
            }],
        );
        let release = turn(
            vec![branch.id],
            vec![ContextUpdate::Unpin { target: second.id }],
        );
        let history = vec![root.clone(), branch.clone(), release.clone()];
        let original = selected(&history, &[source(&root)], &[]).unwrap();
        assert_eq!(original[0].record, first);
        assert!(original[0].pinned);
        let inherited = selected(&history, &[source(&branch)], &[]).unwrap();
        assert_eq!(inherited[0].record, second);
        assert!(inherited[0].pinned);
        let diamond = selected(&history, &[source(&branch), source(&release)], &[]).unwrap();
        assert_eq!(diamond.len(), 1);
        assert!(!diamond[0].pinned);
        assert!(matches!(
            selected(
                &history,
                &[source(&branch)],
                &[ContextUpdate::Unpin { target: first.id }]
            ),
            Err(ProjectionError::Context(ContextError::InactiveRecord))
        ));
    }
    #[test]
    fn repeated_replacements_and_diamond_ancestry_never_resurrect_predecessors() {
        let a = record("budget", 8);
        let b = record("budget", 9);
        let c = record("budget", 10);
        let root = turn(vec![], vec![ContextUpdate::Append { record: a.clone() }]);
        let branch = turn(
            vec![root.id],
            vec![ContextUpdate::Supersede {
                targets: vec![a.id],
                replacement: b.clone(),
                pinned: Some(true),
            }],
        );
        let next = turn(
            vec![branch.id],
            vec![ContextUpdate::Supersede {
                targets: vec![b.id],
                replacement: c.clone(),
                pinned: None,
            }],
        );
        let unchanged = turn(vec![root.id], vec![]);
        let joined = turn(vec![next.id, unchanged.id], vec![]);
        let history = vec![unchanged, joined.clone(), next, branch, root];
        let active = selected(&history, &[source(&joined)], &[]).unwrap();
        assert_eq!(active.len(), 1);
        assert_eq!(active[0].record, c);
        assert!(active[0].pinned);
    }

    #[test]
    fn divergent_values_return_identities_and_require_explicit_resolution() {
        let original = record("budget", 8);
        let a = record("budget", 6);
        let b = record("budget", 20);
        let resolved = record("budget", 12);
        let root = turn(
            vec![],
            vec![ContextUpdate::Pin {
                target: PinTarget::New {
                    record: original.clone(),
                },
            }],
        );
        let left = turn(
            vec![root.id],
            vec![ContextUpdate::Supersede {
                targets: vec![original.id],
                replacement: a.clone(),
                pinned: None,
            }],
        );
        let right = turn(
            vec![root.id],
            vec![ContextUpdate::Supersede {
                targets: vec![original.id],
                replacement: b.clone(),
                pinned: Some(false),
            }],
        );
        let history = vec![right.clone(), root, left.clone()];
        let sources = vec![source(&left), source(&right)];
        match selected(&history, &sources, &[]).unwrap_err() {
            ProjectionError::Conflicts { conflicts } => {
                assert_eq!(conflicts[0].key, "budget");
                assert_eq!(
                    conflicts[0]
                        .revisions
                        .iter()
                        .map(|r| r.record.id)
                        .collect::<Vec<_>>(),
                    vec![a.id, b.id]
                );
                assert_eq!(conflicts[0].revisions[0].source_session, left.session);
            }
            _ => panic!("expected structured conflict"),
        }
        let updates = vec![
            ContextUpdate::Supersede {
                targets: vec![a.id, b.id],
                replacement: resolved.clone(),
                pinned: None,
            },
            ContextUpdate::Unpin {
                target: resolved.id,
            },
            ContextUpdate::Append {
                record: record("note", 1),
            },
        ];
        let result = selected(&history, &sources, &updates).unwrap();
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].record, resolved);
        assert!(!result[0].pinned);
        assert!(selected(&history, &sources, &[]).is_err());
    }
    #[test]
    fn incomparable_retention_and_value_changes_do_not_resurrect_or_silently_win() {
        let original = record("budget", 8);
        let replacement = record("budget", 10);
        let root = turn(
            vec![],
            vec![ContextUpdate::Append {
                record: original.clone(),
            }],
        );
        let left = turn(
            vec![root.id],
            vec![ContextUpdate::Pin {
                target: PinTarget::Existing {
                    target: original.id,
                },
            }],
        );
        let right = turn(
            vec![root.id],
            vec![ContextUpdate::Supersede {
                targets: vec![original.id],
                replacement: replacement.clone(),
                pinned: None,
            }],
        );
        let history = vec![root.clone(), left.clone(), right.clone()];
        let sources = vec![source(&left), source(&right)];
        assert!(matches!(
            selected(&history, &sources, &[]),
            Err(ProjectionError::Conflicts { .. })
        ));
        let resolution = ContextUpdate::Supersede {
            targets: vec![original.id, replacement.id],
            replacement: record("budget", 9),
            pinned: Some(false),
        };
        assert_eq!(
            selected(&history, &sources, &[resolution]).unwrap().len(),
            1
        );
        let unpin = turn(
            vec![left.id],
            vec![ContextUpdate::Unpin {
                target: original.id,
            }],
        );
        let pin_again = turn(
            vec![left.id],
            vec![ContextUpdate::Pin {
                target: PinTarget::Existing {
                    target: original.id,
                },
            }],
        );
        let mut history = history;
        history.extend([unpin.clone(), pin_again.clone()]);
        assert!(matches!(
            selected(&history, &[source(&unpin), source(&pin_again)], &[]),
            Err(ProjectionError::Conflicts { .. })
        ));
    }
    #[test]
    fn invalid_targets_and_duplicate_keys_fail_without_content_deduplication() {
        let original = record("budget", 8);
        let root = turn(
            vec![],
            vec![ContextUpdate::Append {
                record: original.clone(),
            }],
        );
        assert!(
            selected(
                std::slice::from_ref(&root),
                &[source(&root)],
                &[ContextUpdate::Append {
                    record: record("budget", 8)
                }]
            )
            .is_err()
        );
        assert!(
            selected(
                std::slice::from_ref(&root),
                &[source(&root)],
                &[ContextUpdate::Supersede {
                    targets: vec![original.id],
                    replacement: original.clone(),
                    pinned: None
                }]
            )
            .is_err()
        );
        assert!(
            selected(
                std::slice::from_ref(&root),
                &[source(&root)],
                &[ContextUpdate::Unpin {
                    target: Id::generate()
                }]
            )
            .is_err()
        );
        let other = ContextRecord {
            id: Id::generate(),
            key: "separate".into(),
            ..original
        };
        assert_eq!(
            selected(
                std::slice::from_ref(&root),
                &[source(&root)],
                &[ContextUpdate::Append { record: other }]
            )
            .unwrap()
            .len(),
            2
        );
    }
}
