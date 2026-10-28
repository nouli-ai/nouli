//! Read-only branch projections used before preparing or publishing a turn.
use super::{Ledger, turns::TurnError};
use crate::{
    context::{
        ContextUpdate,
        projection::{self, ActiveRecord},
    },
    history::SourcePoint,
    identity::{Id, SessionSelector},
};

impl Ledger {
    pub fn context_sources(&self, session: Id) -> Result<Vec<SourcePoint>, TurnError> {
        let current = self.session(&SessionSelector::Id(session))?;
        if current.frontier.is_empty() {
            Ok(self.session_sources(session)?)
        } else {
            Ok(vec![SourcePoint {
                session,
                frontier: current.frontier,
            }])
        }
    }

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

#[cfg(test)]
mod tests {
    use super::*;
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
        assert!(
            ledger
                .context_at(
                    &ledger.context_sources(session.id).unwrap(),
                    &[],
                    session.id,
                    Id::generate()
                )
                .unwrap()
                .is_empty()
        );
        ledger.connection.execute_batch("CREATE TRIGGER reject_context_publish BEFORE UPDATE OF state ON attempt_states WHEN NEW.state='completed' BEGIN SELECT RAISE(ABORT,'fault'); END;").unwrap();
        assert!(
            ledger
                .finalize_attempt(&guard, prepared.id, fixture.outcome.clone())
                .is_err()
        );
        assert!(ledger.context_sources(session.id).unwrap().is_empty());
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
        assert_eq!(active[0].source_turn, turn.id);
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
