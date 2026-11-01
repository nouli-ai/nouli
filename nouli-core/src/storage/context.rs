//! Read-only branch projections used before preparing or publishing a turn.
use std::collections::HashMap;

use super::{Ledger, turns::TurnError};
use crate::{
    context::{
        ContextKind, ContextUpdate,
        facts::{Fact, KEY_PREFIX},
        projection::{self, ActiveRecord},
    },
    history::SourcePoint,
    identity::{Id, SessionSelector},
};

impl Ledger {
    /// Returns the session's current frontier, or its frozen sources while it
    /// has no turns of its own.
    ///
    /// # Errors
    ///
    /// Returns [`TurnError::Session`] if the session does not exist, the SQLite
    /// query fails, or a stored row is invalid.
    pub fn context_sources(&self, session: Id) -> Result<Vec<SourcePoint>, TurnError> {
        let current = self.session(&SessionSelector::Id(session))?;
        Ok(super::sessions::read_context_sources(
            &self.connection,
            &current,
        )?)
    }

    /// Projects the active context records for a turn from the given sources
    /// and pending updates.
    ///
    /// # Errors
    ///
    /// Returns [`TurnError::UnknownTurn`] or [`TurnError::Storage`] if an
    /// ancestor turn is missing or unreadable, [`TurnError::History`] if the
    /// ancestry is invalid, or [`TurnError::Projection`] if the updates cannot
    /// be applied.
    pub fn context_at(
        &self,
        sources: &[SourcePoint],
        updates: &[ContextUpdate],
        session: Id,
        turn: Id,
    ) -> Result<Vec<ActiveRecord>, TurnError> {
        let history = self.ancestry(sources)?;
        Ok(projection::project(
            &history, sources, updates, session, turn,
        )?)
    }
}

impl Ledger {
    /// The active facts at the session's head. They follow the order in which
    /// each fact was first added, so a replacement keeps its predecessor's
    /// number. A blend lists the facts of every source.
    ///
    /// # Errors
    ///
    /// Returns [`TurnError::Session`] if the session cannot be selected, [`TurnError::Projection`]
    /// if its context cannot be projected, or another [`TurnError`] if reading its context sources
    /// or ancestry fails.
    pub fn facts(&self, selector: &SessionSelector) -> Result<Vec<Fact>, TurnError> {
        let session = self.session(selector)?;
        let sources = self.context_sources(session.id)?;
        let history = self.ancestry(&sources)?;
        let mut rank: HashMap<&str, usize> = HashMap::new();
        let mut versions: HashMap<&str, Vec<&str>> = HashMap::new();
        for turn in &history {
            for update in &turn.updates {
                if let Some(record) = update.new_record()
                    && record.key.starts_with(KEY_PREFIX)
                {
                    let next = rank.len();
                    rank.entry(record.key.as_str()).or_insert(next);
                    if let Some(text) = record.value.value().as_str() {
                        versions.entry(record.key.as_str()).or_default().push(text);
                    }
                }
            }
        }
        let active = projection::project(&history, &sources, &[], session.id, session.id)?;
        // Group the active versions by key; blended branches that replaced the
        // same fact leave several, which stay together as one numbered fact.
        let mut groups: Vec<(&str, Vec<&projection::ActiveRecord>)> = Vec::new();
        for record in active.iter().filter(|active| {
            active.record.kind == ContextKind::Evidence
                && active.record.key.starts_with(KEY_PREFIX)
                && active.record.value.value().is_string()
        }) {
            match groups
                .iter_mut()
                .find(|(key, _)| *key == record.record.key.as_str())
            {
                Some((_, members)) => members.push(record),
                None => groups.push((record.record.key.as_str(), vec![record])),
            }
        }
        let mut facts: Vec<_> = groups
            .into_iter()
            .filter_map(|(key, members)| {
                let texts: Vec<String> = members
                    .iter()
                    .filter_map(|active| active.record.value.value().as_str().map(str::to_owned))
                    .collect();
                let projection::RevisionOrigin::Turn {
                    session: origin_session,
                    turn,
                } = &members[0].origin
                else {
                    return None;
                };
                let (origin_session, turn) = (*origin_session, *turn);
                let previous = if members.len() == 1 {
                    versions
                        .get(key)
                        .and_then(|texts| texts.iter().rev().nth(1))
                        .map(|text| (*text).to_owned())
                } else {
                    None
                };
                Some((
                    rank.get(key).copied()?,
                    Fact {
                        number: 0,
                        records: members.iter().map(|active| active.record.id).collect(),
                        key: key.to_owned(),
                        text: texts.join(" | "),
                        alternatives: if texts.len() > 1 { texts } else { Vec::new() },
                        turn,
                        source_session: (origin_session != session.id).then_some(origin_session),
                        previous,
                    },
                ))
            })
            .collect();
        facts.sort_by_key(|(rank, _)| *rank);
        Ok(facts
            .into_iter()
            .enumerate()
            .map(|(index, (_, mut fact))| {
                fact.number = index + 1;
                fact
            })
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::history::StandingChanges;
    use crate::{
        context::{ContextKind, ContextRecord, PinTarget},
        data::Data,
        history::{FinalizedTurn, ModelSelection},
        storage::{OpenMode, turns::AttemptInput},
    };
    #[test]
    fn pending_updates_publish_only_with_successful_finalization() {
        let root = std::env::temp_dir().join(format!("nouli-context-{}", Id::generate()));
        let mut ledger = Ledger::open(&root, OpenMode::ReadWriteCreate).unwrap();
        let session = ledger
            .create_session(None, ModelSelection::default())
            .unwrap();
        let guard = ledger.session_guard(session.id).unwrap();
        let value: serde_json::Value =
            serde_json::from_str(include_str!("../../../fixtures/contracts/history.json")).unwrap();
        let fixture: FinalizedTurn = serde_json::from_value(value["turns"][0].clone()).unwrap();
        let record = ContextRecord {
            id: Id::generate(),
            key: "budget".into(),
            value: Data::new(serde_json::json!(8)).unwrap(),
            kind: ContextKind::Commitment,
        };
        let input = AttemptInput {
            standing: StandingChanges::default(),
            request: fixture.request,
            manifest: fixture.manifest,
            updates: vec![ContextUpdate::Pin {
                target: PinTarget::New {
                    record: record.clone(),
                },
            }],
            request_id: None,
        };
        let prepared = ledger.prepare_attempt(&guard, input).unwrap();
        assert_eq!(
            ledger
                .context_at(
                    &ledger.context_sources(session.id).unwrap(),
                    &[],
                    session.id,
                    Id::generate()
                )
                .unwrap(),
            [] as [crate::context::projection::ActiveRecord; 0]
        );
        ledger.connection.execute_batch("CREATE TRIGGER reject_context_publish BEFORE UPDATE OF state ON attempt_states WHEN NEW.state='completed' BEGIN SELECT RAISE(ABORT,'fault'); END;").unwrap();
        assert!(
            ledger
                .finalize_attempt(&guard, prepared.id, fixture.outcome.clone())
                .is_err()
        );
        assert_eq!(
            ledger.context_sources(session.id).unwrap(),
            [] as [crate::history::SourcePoint; 0]
        );
        ledger
            .connection
            .execute_batch("DROP TRIGGER reject_context_publish")
            .unwrap();
        let turn = ledger
            .finalize_attempt(&guard, prepared.id, fixture.outcome)
            .unwrap();
        let active = ledger
            .context_at(
                &ledger.context_sources(session.id).unwrap(),
                &[],
                session.id,
                Id::generate(),
            )
            .unwrap();
        assert_eq!(active.len(), 1);
        assert_eq!(active[0].record, record);
        assert!(active[0].pinned);
        assert_eq!(
            active[0].origin,
            crate::context::projection::RevisionOrigin::Turn {
                session: turn.session,
                turn: turn.id
            }
        );
        drop(guard);
        drop(ledger);
        let ledger = Ledger::open(&root, OpenMode::ReadOnlyExisting).unwrap();
        assert_eq!(
            ledger
                .context_at(
                    &ledger.context_sources(session.id).unwrap(),
                    &[],
                    session.id,
                    Id::generate()
                )
                .unwrap(),
            active
        );
        drop(ledger);
        std::fs::remove_dir_all(root).unwrap();
    }
}
