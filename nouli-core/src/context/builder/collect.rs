//! Collects context entries from ordered history, context bases, and pending updates.
use super::{BuildError, ContextEntry, EntryKind, data};
use crate::{
    context::{
        ContextRecord, ContextUpdate,
        projection::{ActiveRecord, RevisionOrigin},
    },
    decision::TurnRequest,
    history::{FinalizedTurn, Representation, base::ContextBase},
    identity::Id,
    outcome::TurnOutcome,
};
use serde::Serialize;
use std::collections::{HashMap, HashSet};

/// Returns `value` only for evidence that is persisted beyond the current run.
pub(super) fn unless_runtime_only<T>(runtime_only: bool, value: T) -> Option<T> {
    if runtime_only { None } else { Some(value) }
}

#[derive(Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum RevisionSource {
    Finalized { session: Id, turn: Id, index: usize },
    Run { session: Id, run: Id, index: usize },
    Ephemeral { node: Option<String>, index: usize },
    Pending { index: usize },
}
impl RevisionSource {
    fn of(active: &ActiveRecord) -> Self {
        let index = active.revision;
        match &active.origin {
            RevisionOrigin::Turn { session, turn } => Self::Finalized {
                session: *session,
                turn: *turn,
                index,
            },
            RevisionOrigin::Run { session, run } => Self::Run {
                session: *session,
                run: *run,
                index,
            },
            RevisionOrigin::Ephemeral { node } => Self::Ephemeral {
                node: node.clone(),
                index,
            },
            RevisionOrigin::Pending { .. } => Self::Pending { index },
        }
    }
}

#[derive(Serialize)]
struct ContextMaterial<'a> {
    record: &'a ContextRecord,
    active: bool,
    pinned: bool,
    created_session: Option<Id>,
    created_turn: Option<Id>,
    created_run: Option<Id>,
    created_node: Option<String>,
    revision: Option<RevisionSource>,
}

fn context_entry(
    record: &ContextRecord,
    active: Option<&ActiveRecord>,
    created: &RevisionOrigin,
) -> Result<ContextEntry, BuildError> {
    let pinned = active.is_some_and(|record| record.pinned);
    let (created_session, created_turn, created_run, created_node) = match created {
        RevisionOrigin::Turn { session, turn } => (Some(*session), Some(*turn), None, None),
        RevisionOrigin::Run { session, run } => (Some(*session), None, Some(*run), None),
        RevisionOrigin::Ephemeral { node } => (None, None, None, node.clone()),
        RevisionOrigin::Pending { .. } => (None, None, None, None),
    };
    Ok(ContextEntry {
        id: record.id,
        kind: EntryKind::Context,
        protected: pinned,
        representation: Representation::Full,
        value: data(&ContextMaterial {
            record,
            active: active.is_some(),
            pinned,
            created_session,
            created_turn,
            created_run,
            created_node,
            revision: active.map(RevisionSource::of),
        })?,
    })
}

#[derive(Serialize)]
struct Decision<'a> {
    session: Option<Id>,
    turn: Option<Id>,
    #[serde(skip_serializing_if = "Option::is_none")]
    workflow_node: Option<&'a str>,
    request: &'a TurnRequest,
    outcome: &'a TurnOutcome,
}

fn decision_entry(turn: &FinalizedTurn) -> Result<ContextEntry, BuildError> {
    Ok(ContextEntry {
        id: turn.id,
        kind: EntryKind::Decision,
        value: data(&Decision {
            session: unless_runtime_only(turn.runtime_only, turn.session),
            turn: unless_runtime_only(turn.runtime_only, turn.id),
            workflow_node: turn.workflow_node.as_deref(),
            request: &turn.request,
            outcome: &turn.outcome,
        })?,
        protected: false,
        representation: Representation::Full,
    })
}

fn turn_origin(turn: &FinalizedTurn) -> RevisionOrigin {
    if turn.runtime_only {
        RevisionOrigin::Ephemeral {
            node: turn.workflow_node.clone(),
        }
    } else {
        RevisionOrigin::Turn {
            session: turn.session,
            turn: turn.id,
        }
    }
}

fn base_origin(base: &ContextBase) -> RevisionOrigin {
    if base.runtime_only {
        RevisionOrigin::Ephemeral { node: None }
    } else {
        RevisionOrigin::Run {
            session: base.session,
            run: base.id,
        }
    }
}

/// Accumulates entries in evidence order while tracking which context records
/// and bases have already been included.
pub(super) struct EntryCollector<'a> {
    active: HashMap<Id, &'a ActiveRecord>,
    entries: Vec<ContextEntry>,
    records: HashSet<Id>,
    bases: HashSet<Id>,
}
impl<'a> EntryCollector<'a> {
    pub(super) fn new(active: &'a [ActiveRecord]) -> Self {
        let mut index = HashMap::with_capacity(active.len());
        for record in active {
            // Should a record be projected twice, its first entry describes it.
            index.entry(record.record.id).or_insert(record);
        }
        Self {
            active: index,
            entries: Vec::new(),
            records: HashSet::new(),
            bases: HashSet::new(),
        }
    }

    pub(super) fn into_entries(self) -> Vec<ContextEntry> {
        self.entries
    }

    fn push_record(
        &mut self,
        record: &ContextRecord,
        origin: &RevisionOrigin,
    ) -> Result<(), BuildError> {
        let active = self.active.get(&record.id).copied();
        self.entries.push(context_entry(record, active, origin)?);
        Ok(())
    }

    /// Appends a record created by `update` unless an earlier entry already
    /// carries it.
    fn push_new_record(
        &mut self,
        update: &ContextUpdate,
        origin: &RevisionOrigin,
    ) -> Result<(), BuildError> {
        if let Some(record) = update.new_record()
            && self.records.insert(record.id)
        {
            self.push_record(record, origin)?;
        }
        Ok(())
    }

    /// Appends a context base's frozen state and the records it created. A base
    /// seen before is not repeated, but becomes protected when `protected`.
    pub(super) fn push_base(
        &mut self,
        base: &ContextBase,
        protected: bool,
    ) -> Result<(), BuildError> {
        if !self.bases.insert(base.id) {
            if protected
                && let Some(entry) = self.entries.iter_mut().find(|entry| entry.id == base.id)
            {
                entry.protected = true;
            }
            return Ok(());
        }
        if !base.state.is_empty() {
            self.entries.push(ContextEntry {
                id: base.id,
                kind: EntryKind::Base,
                protected,
                representation: Representation::Full,
                value: data(&serde_json::json!({
                    "workflow_base": base.state,
                    "run": unless_runtime_only(base.runtime_only, base.id),
                    "runtime_only": base.runtime_only,
                }))?,
            });
        }
        let origin = base_origin(base);
        for update in &base.updates {
            self.push_new_record(update, &origin)?;
        }
        Ok(())
    }

    /// Appends a finalized decision followed by every record it created.
    ///
    /// Unlike bases and pending updates, a turn's records are never skipped as
    /// already seen: a repeated record is left for the duplicate check.
    pub(super) fn push_turn(&mut self, turn: &FinalizedTurn) -> Result<(), BuildError> {
        self.entries.push(decision_entry(turn)?);
        let origin = turn_origin(turn);
        for update in &turn.updates {
            if let Some(record) = update.new_record() {
                self.records.insert(record.id);
                self.push_record(record, &origin)?;
            }
        }
        Ok(())
    }

    /// Appends the records created by the live turn's pending updates.
    pub(super) fn push_pending(
        &mut self,
        updates: &[ContextUpdate],
        origin: &RevisionOrigin,
    ) -> Result<(), BuildError> {
        for update in updates {
            self.push_new_record(update, origin)?;
        }
        Ok(())
    }
}
