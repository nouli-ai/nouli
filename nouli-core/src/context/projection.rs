//! Causal value and retention revisions over an immutable decision DAG.

use super::{ContextError, ContextRecord, ContextUpdate, PinTarget};
use crate::{
    history::{
        FinalizedTurn, HistoryError, SourcePoint, base::ContextBase, order::canonical_history,
    },
    identity::Id,
};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RevisionOrigin {
    Turn { session: Id, turn: Id },
    Run { session: Id, run: Id },
    Ephemeral { node: Option<String> },
    Pending { session: Option<Id>, token: Id },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ActiveRecord {
    pub record: ContextRecord,
    pub pinned: bool,
    pub origin: RevisionOrigin,
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
    /// Fails on unresolved independent revisions, except for facts: a fact's
    /// alternatives stay active together, the model sees each one, and the
    /// caller settles them with a replacement that targets all of them.
    fn ensure_resolved(&self, heads: &[usize]) -> Result<(), ProjectionError> {
        let mut conflicts = self.conflicts(heads);
        conflicts.retain(|conflict| !conflict.key.starts_with(super::facts::KEY_PREFIX));
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
        origin: &RevisionOrigin,
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
                    origin: origin.clone(),
                    revision: index,
                },
                parents,
            });
            heads.push(id);
        }
        Ok(())
    }
}

/// Request updates are projected without mutating their source histories.
/// The caller publishes them only as part of a successfully finalized turn.
///
/// # Errors
///
/// Returns any [`project_with_base`] error.
pub fn project(
    turns: &[FinalizedTurn],
    sources: &[SourcePoint],
    updates: &[ContextUpdate],
    session: Id,
    turn: Id,
) -> Result<Vec<ActiveRecord>, ProjectionError> {
    project_with_base(
        turns,
        sources,
        None,
        updates,
        &RevisionOrigin::Pending {
            session: Some(session),
            token: turn,
        },
    )
}

fn apply_base(
    projector: &mut Projector,
    base: &ContextBase,
    parents: &[Id],
    index: &HashMap<Id, &FinalizedTurn>,
    states: &HashMap<Id, Vec<usize>>,
    cache: &mut HashMap<Id, (ContextBase, Vec<usize>)>,
) -> Result<Vec<usize>, ProjectionError> {
    base.validate()?;
    if index.contains_key(&base.id) {
        return Err(HistoryError::InvalidManifest.into());
    }
    let required: HashSet<_> = base
        .sources
        .iter()
        .flat_map(|source| source.frontier.iter().copied())
        .collect();
    let mut reachable = HashSet::new();
    let mut pending = parents.to_vec();
    while let Some(id) = pending.pop() {
        if reachable.insert(id) {
            pending.extend(
                index
                    .get(&id)
                    .ok_or(HistoryError::UnknownTurn)?
                    .parents
                    .iter()
                    .copied(),
            );
        }
    }
    if !required.is_subset(&reachable) {
        return Err(HistoryError::InvalidManifest.into());
    }
    for source in &base.sources {
        for head in &source.frontier {
            if index
                .get(head)
                .is_none_or(|turn| turn.session != source.session)
            {
                return Err(HistoryError::ForeignHead.into());
            }
        }
    }
    if let Some((existing, heads)) = cache.get(&base.id) {
        if existing != base {
            return Err(HistoryError::InvalidManifest.into());
        }
        return Ok(heads.clone());
    }
    if required.iter().any(|head| !states.contains_key(head)) {
        return Err(HistoryError::UnknownTurn.into());
    }
    let mut heads = projector.merge(
        base.sources
            .iter()
            .flat_map(|source| source.frontier.iter())
            .flat_map(|head| states.get(head).into_iter().flatten().copied()),
    );
    let origin = if base.runtime_only {
        RevisionOrigin::Ephemeral { node: None }
    } else {
        RevisionOrigin::Run {
            session: base.session,
            run: base.id,
        }
    };
    projector.apply(&mut heads, &base.updates, &origin)?;
    cache.insert(base.id, (base.clone(), heads.clone()));
    Ok(heads)
}

/// Projects the active context records at `sources`, applying an optional context base and then
/// `updates` under `origin`.
///
/// # Errors
///
/// - [`ProjectionError::History`] if the history cannot be ordered or a context base is invalid,
///   unreachable, or inconsistent with the history.
/// - [`ProjectionError::Context`] if an update is invalid, repeats a record ID, or targets a record
///   that is unavailable or no longer active.
/// - [`ProjectionError::Conflicts`] if independent revisions of a non-fact key remain unresolved,
///   or a pin or unpin target has several active revisions.
pub fn project_with_base(
    turns: &[FinalizedTurn],
    sources: &[SourcePoint],
    base: Option<&ContextBase>,
    updates: &[ContextUpdate],
    origin: &RevisionOrigin,
) -> Result<Vec<ActiveRecord>, ProjectionError> {
    let ordered = canonical_history(turns, sources)?;
    let index: HashMap<_, _> = ordered.iter().map(|turn| (turn.id, *turn)).collect();
    let mut projector = Projector::default();
    let mut states: HashMap<Id, Vec<usize>> = HashMap::new();
    let mut bases = HashMap::new();
    for turn in ordered {
        let mut heads = projector.merge(
            turn.parents
                .iter()
                .flat_map(|parent| states[parent].iter().copied()),
        );
        if let Some(base) = &turn.context_base {
            let shared = apply_base(
                &mut projector,
                base,
                &turn.parents,
                &index,
                &states,
                &mut bases,
            )?;
            heads = projector.merge(heads.into_iter().chain(shared));
        }
        let origin = if turn.runtime_only {
            RevisionOrigin::Ephemeral {
                node: turn.workflow_node.clone(),
            }
        } else {
            RevisionOrigin::Turn {
                session: turn.session,
                turn: turn.id,
            }
        };
        projector.apply(&mut heads, &turn.updates, &origin)?;
        projector.ensure_resolved(&heads)?;
        states.insert(turn.id, heads);
    }
    let parents: Vec<_> = sources
        .iter()
        .flat_map(|source| source.frontier.iter().copied())
        .collect();
    let mut heads = projector.merge(parents.iter().flat_map(|head| states[head].iter().copied()));
    if let Some(base) = base {
        let shared = apply_base(&mut projector, base, &parents, &index, &states, &mut bases)?;
        heads = projector.merge(heads.into_iter().chain(shared));
    }
    projector.apply(&mut heads, updates, origin)?;
    projector.ensure_resolved(&heads)?;
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
                assert_eq!(
                    conflicts[0].revisions[0].origin,
                    RevisionOrigin::Turn {
                        session: left.session,
                        turn: left.id
                    }
                );
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
    fn shared_run_base_is_one_causal_event_across_independent_roots() {
        let shared = record("shared", 8);
        let mut a = turn(vec![], vec![]);
        let mut b = turn(vec![], vec![]);
        b.session = a.session;
        let base = ContextBase {
            id: Id::generate(),
            session: a.session,
            sources: vec![],
            state: vec![],
            updates: vec![ContextUpdate::Pin {
                target: PinTarget::New {
                    record: shared.clone(),
                },
            }],
            runtime_only: false,
        };
        for (node, name) in [(&mut a, "a"), (&mut b, "b")] {
            node.workflow_run = Some(base.id);
            node.workflow_node = Some(name.into());
            node.context_base = Some(base.clone());
        }
        let sources = [SourcePoint {
            session: a.session,
            frontier: vec![a.id, b.id],
        }];
        let active = selected(&[a.clone(), b.clone()], &sources, &[]).unwrap();
        assert_eq!(active.len(), 1);
        assert_eq!(active[0].record, shared);
        assert!(active[0].pinned);
        assert_eq!(
            active[0].origin,
            RevisionOrigin::Run {
                session: a.session,
                run: base.id
            }
        );
        b.context_base.as_mut().unwrap().state = vec![crate::data::Input::Text {
            text: "changed base".into(),
        }];
        assert!(matches!(
            selected(&[a, b], &sources, &[]),
            Err(ProjectionError::History(HistoryError::InvalidManifest))
        ));
    }
    #[test]
    fn base_sources_cannot_import_an_unrelated_completed_sibling() {
        let root = turn(vec![], vec![]);
        let mut branch = turn(vec![], vec![]);
        let base = ContextBase {
            id: Id::generate(),
            session: branch.session,
            sources: vec![source(&root)],
            state: vec![],
            updates: vec![],
            runtime_only: false,
        };
        branch.workflow_run = Some(base.id);
        branch.workflow_node = Some("branch".into());
        branch.context_base = Some(base);
        assert!(matches!(
            selected(&[root, branch.clone()], &[source(&branch)], &[]),
            Err(ProjectionError::History(HistoryError::InvalidManifest))
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
