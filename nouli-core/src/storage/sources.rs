//! Consistent fixed source snapshots shared by durable and read-only callers.
use super::{
    Ledger, StorageError,
    sessions::{Session, SessionError, insert_session, read_session},
    sql_error,
};
use crate::{
    history::{HistoryError, ModelSelection, SourcePoint, SourceSelection},
    identity::{Id, SessionName},
};
use rusqlite::{Connection, OptionalExtension};
use std::collections::HashSet;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum SourceError {
    #[error(transparent)]
    Storage(#[from] StorageError),
    #[error(transparent)]
    Session(#[from] SessionError),
    #[error(transparent)]
    History(#[from] HistoryError),
}

pub(super) fn resolve_on(
    connection: &Connection,
    selections: &[SourceSelection],
) -> Result<Vec<SourcePoint>, SourceError> {
    if selections.is_empty() {
        return Err(HistoryError::InvalidSelection.into());
    }
    selections
        .iter()
        .map(|selection| {
            let session = read_session(connection, &selection.session)?;
            let frontier = selection.frontier.clone().unwrap_or(session.frontier);
            let mut seen = HashSet::new();
            if frontier.is_empty() || frontier.iter().any(|id| !seen.insert(*id)) {
                return Err(HistoryError::InvalidFrontier.into());
            }
            for id in &frontier {
                let owner: Option<String> = connection
                    .query_row(
                        "SELECT session_id FROM turns WHERE id=?1",
                        [id.to_string()],
                        |row| row.get(0),
                    )
                    .optional()
                    .map_err(|error| sql_error("validate source membership", &error))?;
                if owner.as_deref() != Some(session.id.to_string().as_str()) {
                    return Err(HistoryError::ForeignHead.into());
                }
            }
            Ok(SourcePoint {
                session: session.id,
                frontier,
            })
        })
        .collect()
}

pub(super) fn resolve_selection(
    connection: &Connection,
    selection: &crate::history::SessionSelection,
    current: &Session,
) -> Result<Vec<SourcePoint>, SourceError> {
    use crate::history::SessionSelection;
    match selection {
        SessionSelection::New { .. } => Ok(vec![]),
        SessionSelection::Continue { .. } => {
            if current.frontier.is_empty() {
                Ok(super::sessions::read_sources(connection, current.id)?)
            } else {
                Ok(vec![SourcePoint {
                    session: current.id,
                    frontier: current.frontier.clone(),
                }])
            }
        }
        SessionSelection::From { sources, .. } => resolve_on(connection, sources),
    }
}

impl Ledger {
    /// One read transaction freezes all unspecified heads together. This also
    /// works on a read-only ledger and creates no session or guard files.
    ///
    /// # Errors
    ///
    /// Returns [`SourceError::History`] if no sources are selected, a frontier
    /// is empty or repeats a turn, or a frontier turn belongs to another
    /// session; [`SourceError::Session`] if a selected session cannot be read;
    /// or [`SourceError::Storage`] if the SQLite read fails.
    pub fn resolve_sources(
        &self,
        selections: &[SourceSelection],
    ) -> Result<Vec<SourcePoint>, SourceError> {
        let transaction = self
            .connection
            .unchecked_transaction()
            .map_err(|error| sql_error("begin source snapshot", &error))?;
        let sources = resolve_on(&transaction, selections)?;
        transaction
            .commit()
            .map_err(|error| sql_error("finish source snapshot", &error))?;
        Ok(sources)
    }

    /// Record references without copying or changing the selected sources.
    /// Evaluation updates can subsequently resolve conflicts during preparation.
    ///
    /// # Errors
    ///
    /// - Any error from [`Ledger::resolve_sources`] for the selections.
    /// - [`SourceError::Session`] if the defaults are invalid, the name is
    ///   taken, or no automatic name is available.
    /// - [`SourceError::Storage`] if the ledger is read-only or the SQLite
    ///   write fails.
    pub fn create_session_from(
        &mut self,
        name: Option<SessionName>,
        defaults: ModelSelection,
        selections: &[SourceSelection],
    ) -> Result<Session, SourceError> {
        super::sessions::validate_defaults(&defaults)?;
        let mut session = Session {
            id: Id::generate(),
            name,
            defaults,
            frontier: vec![],
            revision: 0,
        };
        self.transaction::<_, SourceError>("create derived session", |transaction| {
            let sources = resolve_on(transaction, selections)?;
            insert_session(transaction, &mut session, &sources)?;
            Ok(())
        })?;
        Ok(session)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::history::StandingChanges;
    use crate::{
        context::{ContextKind, ContextRecord, ContextUpdate, PinTarget},
        data::Data,
        history::FinalizedTurn,
        identity::SessionSelector,
        storage::{OpenMode, turns::AttemptInput},
    };
    struct Store(std::path::PathBuf);
    impl Store {
        fn new() -> Self {
            Self(std::env::temp_dir().join(format!("nouli-sources-{}", Id::generate())))
        }
        fn open(&self) -> Ledger {
            Ledger::open(&self.0, OpenMode::ReadWriteCreate).unwrap()
        }
    }
    impl Drop for Store {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    fn fixture() -> FinalizedTurn {
        let data: serde_json::Value =
            serde_json::from_str(include_str!("../../../fixtures/contracts/history.json")).unwrap();
        serde_json::from_value(data["turns"][0].clone()).unwrap()
    }
    fn commit(
        ledger: &mut Ledger,
        session: Id,
        updates: Vec<ContextUpdate>,
        model: &str,
    ) -> FinalizedTurn {
        let guard = ledger.session_guard(session).unwrap();
        let mut fixture = fixture();
        fixture.manifest.sources = ledger.context_sources(session).unwrap();
        fixture.manifest.provider.model = model.into();
        fixture.outcome.provider.model = model.into();
        let attempt = ledger
            .prepare_attempt(
                &guard,
                AttemptInput {
                    standing: StandingChanges::default(),
                    request: fixture.request,
                    updates,
                    manifest: fixture.manifest,
                    request_id: None,
                },
            )
            .unwrap();
        ledger
            .finalize_attempt(&guard, attempt.id, fixture.outcome)
            .unwrap()
    }
    fn selection(session: Id, frontier: Option<Vec<Id>>) -> SourceSelection {
        SourceSelection {
            session: SessionSelector::Id(session),
            frontier,
        }
    }
    fn budget_record(budget: i64) -> ContextRecord {
        ContextRecord {
            id: Id::generate(),
            key: "budget".into(),
            value: Data::new(serde_json::json!(budget)).unwrap(),
            kind: ContextKind::Evidence,
        }
    }
    fn rebudgeted(record: &ContextRecord, budget: i64) -> ContextRecord {
        ContextRecord {
            id: Id::generate(),
            value: Data::new(serde_json::json!(budget)).unwrap(),
            ..record.clone()
        }
    }
    fn supersede(targets: Vec<Id>, replacement: ContextRecord) -> ContextUpdate {
        ContextUpdate::Supersede {
            targets,
            replacement,
            pinned: None,
        }
    }
    /// A named root with one pinned budget record, two branches that each add a
    /// turn on a different model, and a named blend of both branches.
    struct Diamond {
        root: Session,
        left: Session,
        right: Session,
        blend: Session,
        record: ContextRecord,
        first: FinalizedTurn,
        left_turn: FinalizedTurn,
        right_turn: FinalizedTurn,
    }
    fn blend_defaults() -> ModelSelection {
        ModelSelection {
            provider: Some("explicit-new-profile".into()),
            model: Some("new-model".into()),
        }
    }
    fn diamond(ledger: &mut Ledger) -> Diamond {
        let root = ledger
            .create_session(
                Some(SessionName::try_from("root".to_owned()).unwrap()),
                ModelSelection::default(),
            )
            .unwrap();
        let record = budget_record(8);
        let first = commit(
            ledger,
            root.id,
            vec![ContextUpdate::Pin {
                target: PinTarget::New {
                    record: record.clone(),
                },
            }],
            "source-model",
        );
        let left = ledger
            .create_session_from(
                None,
                ModelSelection::default(),
                &[SourceSelection {
                    session: SessionSelector::Name(root.name.clone().unwrap()),
                    frontier: None,
                }],
            )
            .unwrap();
        let left_turn = commit(ledger, left.id, vec![], "left-model");
        let right = ledger
            .create_session_from(None, ModelSelection::default(), &[selection(root.id, None)])
            .unwrap();
        let right_turn = commit(ledger, right.id, vec![], "right-model");
        let blend = ledger
            .create_session_from(
                Some(SessionName::try_from("blend".to_owned()).unwrap()),
                blend_defaults(),
                &[selection(left.id, None), selection(right.id, None)],
            )
            .unwrap();
        Diamond {
            root,
            left,
            right,
            blend,
            record,
            first,
            left_turn,
            right_turn,
        }
    }
    #[test]
    fn branches_and_blends_freeze_ordered_mixed_provider_ancestry() {
        let store = Store::new();
        let mut ledger = store.open();
        let diamond = diamond(&mut ledger);
        let captured = ledger.session_sources(diamond.blend.id).unwrap();
        assert_eq!(diamond.blend.defaults, blend_defaults());
        let history = ledger.ancestry(&captured).unwrap();
        assert_eq!(
            history.iter().map(|t| t.id).collect::<Vec<_>>(),
            vec![
                diamond.first.id,
                diamond.left_turn.id,
                diamond.right_turn.id
            ]
        );
        assert_eq!(
            history
                .iter()
                .map(|t| t.outcome.provider.model.as_str())
                .collect::<Vec<_>>(),
            vec!["source-model", "left-model", "right-model"]
        );
        let active = ledger
            .context_at(&captured, &[], diamond.blend.id, Id::generate())
            .unwrap();
        assert_eq!(active.len(), 1);
        assert_eq!(active[0].record, diamond.record);
        assert!(
            ledger
                .create_session_from(
                    diamond.blend.name.clone(),
                    ModelSelection::default(),
                    &[selection(diamond.root.id, None)]
                )
                .is_err()
        );
        assert_eq!(ledger.list_sessions().unwrap().len(), 4);
    }
    #[test]
    fn blend_sources_stay_frozen_after_their_sources_advance() {
        let store = Store::new();
        let mut ledger = store.open();
        let diamond = diamond(&mut ledger);
        let captured = ledger.session_sources(diamond.blend.id).unwrap();
        let active = ledger
            .context_at(&captured, &[], diamond.blend.id, Id::generate())
            .unwrap();
        for (session, budget, model) in [
            (diamond.root.id, 12, "later-model"),
            (diamond.left.id, 6, "later-left-model"),
        ] {
            let update = supersede(vec![diamond.record.id], rebudgeted(&diamond.record, budget));
            commit(&mut ledger, session, vec![update], model);
        }
        assert_eq!(ledger.session_sources(diamond.blend.id).unwrap(), captured);
        assert_eq!(
            ledger
                .context_at(&captured, &[], diamond.blend.id, Id::generate())
                .unwrap(),
            active
        );
        assert_eq!(
            ledger
                .resolve_sources(&[selection(diamond.root.id, Some(vec![diamond.first.id]))])
                .unwrap()[0]
                .frontier,
            vec![diamond.first.id]
        );
        drop(ledger);
        let read = Ledger::open(&store.0, OpenMode::ReadOnlyExisting).unwrap();
        assert_eq!(
            read.resolve_sources(&[
                selection(diamond.left.id, Some(vec![diamond.left_turn.id])),
                selection(diamond.right.id, None)
            ])
            .unwrap(),
            captured
        );
    }
    #[test]
    fn blend_conflicts_require_resolution_attached_to_the_new_evaluation() {
        let store = Store::new();
        let mut ledger = store.open();
        let root = ledger
            .create_session(None, ModelSelection::default())
            .unwrap();
        let original = budget_record(8);
        commit(
            &mut ledger,
            root.id,
            vec![ContextUpdate::Pin {
                target: PinTarget::New {
                    record: original.clone(),
                },
            }],
            "root",
        );
        let mut branches = Vec::new();
        let mut records = Vec::new();
        for budget in [6, 20] {
            let branch = ledger
                .create_session_from(None, ModelSelection::default(), &[selection(root.id, None)])
                .unwrap();
            let replacement = rebudgeted(&original, budget);
            commit(
                &mut ledger,
                branch.id,
                vec![supersede(vec![original.id], replacement.clone())],
                "branch",
            );
            branches.push(selection(branch.id, None));
            records.push(replacement.id);
        }
        let blend = ledger
            .create_session_from(None, ModelSelection::default(), &branches)
            .unwrap();
        let guard = ledger.session_guard(blend.id).unwrap();
        let fixture = fixture();
        let mut input = AttemptInput {
            standing: StandingChanges::default(),
            request: fixture.request,
            manifest: fixture.manifest,
            updates: vec![],
            request_id: None,
        };
        input.manifest.sources = ledger.session_sources(blend.id).unwrap();
        assert!(matches!(
            ledger.prepare_attempt(&guard, input.clone()),
            Err(super::super::turns::TurnError::Projection(
                crate::context::projection::ProjectionError::Conflicts { .. }
            ))
        ));
        let replacement = rebudgeted(&original, 12);
        input.updates.push(supersede(records, replacement.clone()));
        let attempt = ledger.prepare_attempt(&guard, input).unwrap();
        ledger
            .finalize_attempt(&guard, attempt.id, fixture.outcome)
            .unwrap();
        let active = ledger
            .context_at(
                &ledger.context_sources(blend.id).unwrap(),
                &[],
                blend.id,
                Id::generate(),
            )
            .unwrap();
        assert_eq!(active.len(), 1);
        assert_eq!(active[0].record, replacement);
        assert!(active[0].pinned);
    }

    #[test]
    fn continuation_of_unfinalized_derived_session_retains_captured_sources() {
        use crate::{
            config::{Config, OutputFormat, resolve::SettingsSnapshot},
            history::{EvaluationRequest, SessionSelection},
            storage::replay::RequestDecision,
        };
        let store = Store::new();
        let mut ledger = store.open();
        let root = ledger
            .create_session(None, ModelSelection::default())
            .unwrap();
        commit(&mut ledger, root.id, vec![], "offline");
        let derived = ledger
            .create_session_from(None, ModelSelection::default(), &[selection(root.id, None)])
            .unwrap();
        let captured = ledger.session_sources(derived.id).unwrap();
        let fixture = fixture();
        let request = EvaluationRequest {
            standing: StandingChanges::default(),
            turn: fixture.request,
            session: SessionSelection::Continue {
                session: SessionSelector::Id(derived.id),
            },
            updates: vec![],
            required_records: vec![],
            model: ModelSelection::default(),
            request_id: Some("continue-derived".into()),
        };
        let config =
            Config::parse(include_str!("../../../fixtures/config/providers.toml")).unwrap();
        let profile = config
            .resolve_snapshot(
                &crate::config::resolve::RuntimeOverrides {
                    selection: ModelSelection {
                        provider: Some("local".into()),
                        model: Some("offline".into()),
                    },
                    context_budget_tokens: Some(4096),
                    ..Default::default()
                },
                &ModelSelection::default(),
                &ModelSelection::default(),
                std::path::Path::new("/unused"),
            )
            .unwrap()
            .profile;
        let settings = SettingsSnapshot {
            format_version: 2,
            profile_name: "fixture".into(),
            profile,
            execution: config.execution,
            output: OutputFormat::Json,
            store: store.0.clone(),
        };
        let RequestDecision::Accepted(owner) = ledger.accept_request(&request, settings).unwrap()
        else {
            panic!("expected owner")
        };
        assert_eq!(owner.frozen().sources, captured);
        let mut manifest = fixture.manifest;
        manifest.sources = captured.clone();
        let attempt = owner.prepare(&mut ledger, manifest).unwrap();
        let turn = ledger
            .finalize_attempt(owner.guard(), attempt.id, fixture.outcome)
            .unwrap();
        assert_eq!(turn.manifest.sources, captured);
        assert_eq!(turn.parents, captured[0].frontier);
    }

    #[test]
    fn missing_unfinalized_and_foreign_heads_fail_without_creating_sessions() {
        let store = Store::new();
        let mut ledger = store.open();
        let a = ledger
            .create_session(None, ModelSelection::default())
            .unwrap();
        let b = ledger
            .create_session(None, ModelSelection::default())
            .unwrap();
        let finalized = commit(&mut ledger, a.id, vec![], "a");
        for selection in [
            selection(b.id, None),
            selection(b.id, Some(vec![finalized.id])),
            selection(a.id, Some(vec![Id::generate()])),
            selection(a.id, Some(vec![finalized.id, finalized.id])),
        ] {
            assert!(
                ledger
                    .create_session_from(None, ModelSelection::default(), &[selection])
                    .is_err()
            );
        }
        assert_eq!(ledger.list_sessions().unwrap().len(), 2);
    }
}
