//! Standing questions folded from the changes saved on each session's turns.

use super::{Ledger, replay::FrozenRequest, turns::TurnError};
use crate::{
    decision::{Question, TurnRequest},
    history::{EvaluationRequest, HistoryError, SessionSelection, SourcePoint},
    identity::{Id, SessionSelector},
};

impl Ledger {
    /// The session's current standing questions, in the order first set.
    pub fn standing_questions(
        &self,
        selector: &SessionSelector,
    ) -> Result<Vec<Question>, TurnError> {
        let session = self.session(selector)?;
        self.standing_at(session.id, &session.frontier)
    }

    /// The standing set of the single source in `sources`, as it was at that
    /// source's frontier. A blend (several sources) or no source has none.
    pub fn source_standing(&self, sources: &[SourcePoint]) -> Result<Vec<Question>, TurnError> {
        match sources {
            [source] => self.standing_at(source.session, &source.frontier),
            _ => Ok(vec![]),
        }
    }

    /// The questions a frozen request asks. Call it only after the request was
    /// accepted: the frozen request keeps the caller's own questions so replay
    /// fingerprints stay stable, and the session head cannot move while the
    /// request owns the session, so the result is deterministic.
    pub fn effective_turn(&self, frozen: &FrozenRequest) -> Result<TurnRequest, TurnError> {
        let base = self.standing_at(frozen.session, &frozen.frontier)?;
        Ok(frozen
            .request
            .standing
            .effective_turn(base, &frozen.request.turn)?)
    }

    /// Checks a request's standing changes before acceptance so a mistake
    /// fails without reserving the request or creating a session.
    pub fn preview_turn(&self, request: &EvaluationRequest) -> Result<TurnRequest, TurnError> {
        let base = match &request.session {
            SessionSelection::New { .. } => vec![],
            SessionSelection::Continue { session } => self.standing_questions(session)?,
            SessionSelection::From { sources, .. } => {
                let sources = self.resolve_sources(sources).map_err(|error| match error {
                    super::sources::SourceError::Storage(error) => TurnError::Storage(error),
                    super::sources::SourceError::Session(error) => TurnError::Session(error),
                    super::sources::SourceError::History(error) => TurnError::History(error),
                })?;
                self.source_standing(&sources)?
            }
        };
        Ok(request.standing.effective_turn(base, &request.turn)?)
    }

    /// The standing set of `session` once its own turns up to `frontier` are
    /// applied, on top of its base. A branch's base is its source's set at the
    /// captured frontier; any other session starts empty.
    pub(super) fn standing_at(
        &self,
        session: Id,
        frontier: &[Id],
    ) -> Result<Vec<Question>, TurnError> {
        let mut set = match self.session_sources(session)?.as_slice() {
            [source] => self.standing_at(source.session, &source.frontier)?,
            _ => vec![],
        };
        if frontier.is_empty() {
            return Ok(set);
        }
        let history = self.ancestry(&[SourcePoint {
            session,
            frontier: frontier.to_vec(),
        }])?;
        for turn in history.iter().filter(|turn| turn.session == session) {
            set = turn
                .standing
                .apply(set)
                .map_err(|error: HistoryError| TurnError::History(error))?;
        }
        Ok(set)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        history::{FinalizedTurn, ModelSelection, SourceSelection, StandingChanges},
        storage::{OpenMode, turns::AttemptInput},
    };

    struct Store(std::path::PathBuf);
    impl Store {
        fn new() -> Self {
            Self(std::env::temp_dir().join(format!("nouli-standing-{}", Id::generate())))
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
    fn questions() -> Vec<Question> {
        fixture().request.questions
    }
    fn changes(set: &[Question], drop: &[&str]) -> StandingChanges {
        StandingChanges {
            set: set.to_vec(),
            drop: drop.iter().map(|id| (*id).to_owned()).collect(),
        }
    }
    fn commit(ledger: &mut Ledger, session: Id, standing: StandingChanges) -> FinalizedTurn {
        let guard = ledger.session_guard(session).unwrap();
        let mut fixture = fixture();
        fixture.manifest.sources = ledger.context_sources(session).unwrap();
        let attempt = ledger
            .prepare_attempt(
                &guard,
                AttemptInput {
                    request: fixture.request,
                    updates: vec![],
                    standing,
                    manifest: fixture.manifest,
                    request_id: None,
                },
            )
            .unwrap();
        ledger
            .finalize_attempt(&guard, attempt.id, fixture.outcome)
            .unwrap()
    }
    fn ids(questions: &[Question]) -> Vec<&str> {
        questions.iter().map(|q| q.id.as_str()).collect()
    }
    fn standing(ledger: &Ledger, session: Id) -> Vec<Question> {
        ledger
            .standing_questions(&SessionSelector::Id(session))
            .unwrap()
    }

    #[test]
    fn changes_fold_in_turn_order_and_keep_the_order_first_set() {
        let store = Store::new();
        let mut ledger = store.open();
        let session = ledger
            .create_session(None, ModelSelection::default())
            .unwrap();
        assert!(standing(&ledger, session.id).is_empty());
        let all = questions();
        commit(&mut ledger, session.id, changes(&all[..3], &[]));
        assert_eq!(
            ids(&standing(&ledger, session.id)),
            ["travel", "on_time", "quality"]
        );
        let mut replaced = all[1].clone();
        replaced.prompt = "Will it be late?".into();
        commit(&mut ledger, session.id, changes(&[replaced.clone()], &[]));
        let current = standing(&ledger, session.id);
        assert_eq!(ids(&current), ["travel", "on_time", "quality"]);
        assert_eq!(current[1], replaced);
        commit(&mut ledger, session.id, changes(&[], &["travel"]));
        assert_eq!(ids(&standing(&ledger, session.id)), ["on_time", "quality"]);
        // Setting a dropped ID again appends it.
        commit(&mut ledger, session.id, changes(&all[..1], &[]));
        assert_eq!(
            ids(&standing(&ledger, session.id)),
            ["on_time", "quality", "travel"]
        );
    }

    #[test]
    fn branch_inherits_the_source_set_at_its_frontier_and_a_blend_starts_empty() {
        let store = Store::new();
        let mut ledger = store.open();
        let root = ledger
            .create_session(None, ModelSelection::default())
            .unwrap();
        let all = questions();
        let first = commit(&mut ledger, root.id, changes(&all[..2], &[]));
        let second = commit(&mut ledger, root.id, changes(&all[2..3], &["travel"]));
        let select = |session: Id, frontier: Option<Vec<Id>>| SourceSelection {
            session: SessionSelector::Id(session),
            frontier,
        };
        let older = ledger
            .create_session_from(
                None,
                ModelSelection::default(),
                &[select(root.id, Some(vec![first.id]))],
            )
            .unwrap();
        assert_eq!(ids(&standing(&ledger, older.id)), ["travel", "on_time"]);
        let latest = ledger
            .create_session_from(None, ModelSelection::default(), &[select(root.id, None)])
            .unwrap();
        assert_eq!(ids(&standing(&ledger, latest.id)), ["on_time", "quality"]);
        // The branch's own changes build on the inherited set, and the source
        // stays untouched; a branch of the branch inherits both.
        commit(&mut ledger, older.id, changes(&all[3..4], &["on_time"]));
        assert_eq!(ids(&standing(&ledger, older.id)), ["travel", "private"]);
        assert_eq!(ids(&standing(&ledger, root.id)), ["on_time", "quality"]);
        let nested = ledger
            .create_session_from(None, ModelSelection::default(), &[select(older.id, None)])
            .unwrap();
        assert_eq!(ids(&standing(&ledger, nested.id)), ["travel", "private"]);
        let blend = ledger
            .create_session_from(
                None,
                ModelSelection::default(),
                &[
                    select(root.id, Some(vec![second.id])),
                    select(older.id, None),
                ],
            )
            .unwrap();
        assert!(standing(&ledger, blend.id).is_empty());
        // A blend's own turns still fold.
        commit(&mut ledger, blend.id, changes(&all[..1], &[]));
        assert_eq!(ids(&standing(&ledger, blend.id)), ["travel"]);
    }

    #[test]
    fn turns_written_without_standing_changes_open_with_no_standing_questions() {
        let store = Store::new();
        let mut ledger = store.open();
        let session = ledger
            .create_session(None, ModelSelection::default())
            .unwrap();
        // The 0.1.0 fixture has no `standing` key at all.
        let raw = include_str!("../../../fixtures/contracts/history.json");
        assert!(!raw.contains("standing"));
        commit(&mut ledger, session.id, StandingChanges::default());
        assert!(standing(&ledger, session.id).is_empty());
        let turn = ledger.turn(
            ledger
                .session(&SessionSelector::Id(session.id))
                .unwrap()
                .frontier[0],
        );
        assert!(turn.unwrap().standing.is_empty());
        let mut value = serde_json::to_value(fixture()).unwrap();
        assert!(value.as_object_mut().unwrap().remove("standing").is_none());
        let old: FinalizedTurn = serde_json::from_value(value).unwrap();
        assert!(old.standing.is_empty());
    }

    #[test]
    fn effective_questions_put_standing_first_and_reject_collisions_and_unknown_drops() {
        let all = questions();
        let one_off = TurnRequest {
            version: 1,
            state: vec![],
            questions: all[3..].to_vec(),
        };
        let effective = changes(&[], &[])
            .effective_turn(all[..3].to_vec(), &one_off)
            .unwrap();
        assert_eq!(effective.questions, all);
        assert_eq!(
            changes(&[], &[]).effective_turn(all.clone(), &one_off),
            Err(HistoryError::StandingCollision("private".into()))
        );
        assert_eq!(
            changes(&[], &["missing"]).effective_turn(all.clone(), &one_off),
            Err(HistoryError::UnknownStanding("missing".into()))
        );
        let none = TurnRequest {
            questions: vec![],
            ..one_off
        };
        assert_eq!(
            changes(&[], &[]).effective_turn(vec![], &none),
            Err(HistoryError::Decision(
                crate::decision::ContractError::Empty("questions")
            ))
        );
    }
}
