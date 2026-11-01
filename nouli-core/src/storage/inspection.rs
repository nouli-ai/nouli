//! Coherent read-only history exports; reduced model context never replaces originals.
use super::{
    Ledger, StorageError,
    replay::FrozenRequest,
    reservations,
    sessions::{Session, SessionError},
    sql_error,
    turns::{AttemptSnapshot, TurnError},
    workflows::{self, FrozenWorkflow, StoredRun, WorkflowError, nodes::NodeIntent},
};
use crate::{
    context::projection::{ActiveRecord, ContextConflict, ProjectionError},
    decision::CONTRACT_VERSION,
    history::{FinalizedTurn, ModelSelection, SourcePoint},
    identity::{Id, SessionName, SessionSelector},
};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use thiserror::Error;
#[derive(Debug, Error)]
pub enum InspectionError {
    #[error(transparent)]
    Storage(#[from] StorageError),
    #[error(transparent)]
    Session(#[from] SessionError),
    #[error(transparent)]
    Turn(#[from] TurnError),
    #[error(transparent)]
    Workflow(#[from] WorkflowError),
    #[error("select one finalized --turn for this session")]
    Head,
    #[error("selected turn does not belong to this session")]
    ForeignTurn,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum ContextView {
    Resolved { records: Vec<ActiveRecord> },
    Conflicted { conflicts: Vec<ContextConflict> },
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionProvenance {
    pub id: Id,
    pub name: Option<SessionName>,
    pub defaults: ModelSelection,
    pub sources: Vec<SourcePoint>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AcceptedRequest {
    pub request_id: String,
    pub state: String,
    pub frozen: FrozenRequest,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WorkflowArchive {
    pub run: StoredRun,
    pub rounds: Vec<super::workflows::recovery::RoundResult>,
    pub events: Vec<super::workflows::events::StoredEvent>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionExport {
    pub version: u32,
    pub session: Session,
    pub sources: Vec<SourcePoint>,
    pub provenance: Vec<SessionProvenance>,
    pub turns: Vec<FinalizedTurn>,
    pub attempts: Vec<AttemptSnapshot>,
    pub requests: Vec<AcceptedRequest>,
    pub workflow_definitions: Vec<FrozenWorkflow>,
    pub workflows: Vec<WorkflowArchive>,
    pub node_intents: Vec<NodeIntent>,
    pub context: ContextView,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TurnInspection {
    pub version: u32,
    pub turn: FinalizedTurn,
    pub attempts: Vec<AttemptSnapshot>,
    pub request: Option<AcceptedRequest>,
    pub node_intents: Vec<NodeIntent>,
    pub workflow: Option<FrozenWorkflow>,
    pub context: ContextView,
}
fn context(
    ledger: &Ledger,
    sources: &[SourcePoint],
    session: Id,
) -> Result<ContextView, InspectionError> {
    match ledger.context_at(sources, &[], session, session) {
        Ok(records) => Ok(ContextView::Resolved { records }),
        Err(TurnError::Projection(ProjectionError::Conflicts { conflicts })) => {
            Ok(ContextView::Conflicted { conflicts })
        }
        Err(error) => Err(error.into()),
    }
}
fn requests(
    ledger: &Ledger,
    session: Option<Id>,
    keys: &HashSet<String>,
) -> Result<Vec<AcceptedRequest>, InspectionError> {
    let mut selected = vec![];
    let mut seen = HashSet::new();
    if let Some(session) = session {
        for (key, state, payload) in reservations::session_requests(&ledger.connection, session)? {
            seen.insert(key.clone());
            selected.push(AcceptedRequest {
                request_id: key,
                state: state.as_str().to_owned(),
                frozen: Ledger::decode_payload(&payload)?,
            });
        }
    }
    let mut keys = keys.iter().collect::<Vec<_>>();
    keys.sort();
    for key in keys {
        if seen.contains(key) {
            continue;
        }
        let row = reservations::referenced_request(&ledger.connection, key)?;
        if let Some((state, payload)) = row {
            selected.push(AcceptedRequest {
                request_id: key.clone(),
                state: state.as_str().to_owned(),
                frozen: Ledger::decode_payload(&payload)?,
            });
        }
    }
    Ok(selected)
}

fn node_intents(
    ledger: &Ledger,
    attempts: &[AttemptSnapshot],
) -> Result<Vec<NodeIntent>, InspectionError> {
    let mut seen = HashSet::new();
    let mut intents = vec![];
    for attempt in attempts {
        if let Some(scope) = &attempt.intent.workflow
            && seen.insert((scope.run, scope.node, scope.round))
        {
            let node = i64::try_from(scope.node).map_err(|_| StorageError::Payload)?;
            let payload:String=ledger.connection.query_row("SELECT i.payload_toon FROM workflow_node_intents i JOIN workflow_nodes n ON n.run_id=i.run_id AND n.node_id=i.node_id WHERE i.run_id=?1 AND n.declaration_order=?2 AND i.round=?3",rusqlite::params![scope.run.to_string(),node,scope.round],|row|row.get(0)).map_err(|error|sql_error("read accepted node metadata",&error))?;
            intents.push(Ledger::decode_payload(&payload)?);
        }
    }
    Ok(intents)
}
fn related_attempts(
    attempts: Vec<AttemptSnapshot>,
    turns: &HashSet<Id>,
    all_session: Option<Id>,
) -> Vec<AttemptSnapshot> {
    let keys: HashSet<_> = attempts
        .iter()
        .filter(|attempt| {
            attempt
                .finalized_turn
                .is_some_and(|turn| turns.contains(&turn))
        })
        .filter_map(|attempt| attempt.intent.input.request_id.as_ref())
        .cloned()
        .collect();
    attempts
        .into_iter()
        .filter(|attempt| {
            Some(attempt.intent.session) == all_session
                || attempt
                    .finalized_turn
                    .is_some_and(|turn| turns.contains(&turn))
                || attempt
                    .intent
                    .input
                    .request_id
                    .as_ref()
                    .is_some_and(|key| keys.contains(key))
        })
        .collect()
}
impl Ledger {
    /// Reads a finalized turn with its attempts, request, workflow definition,
    /// and resulting context from one consistent snapshot.
    ///
    /// # Errors
    ///
    /// Returns [`InspectionError::Turn`] if the turn or its attempts or context
    /// cannot be read, [`InspectionError::Workflow`] if its workflow definition
    /// cannot be read, or [`InspectionError::Storage`] if the SQLite read fails
    /// or a stored payload is invalid.
    pub fn inspect_turn(&self, id: Id) -> Result<TurnInspection, InspectionError> {
        let tx = self
            .connection
            .unchecked_transaction()
            .map_err(|error| sql_error("begin turn inspection", &error))?;
        let result = self.inspect_turn_record(self.turn(id)?)?;
        tx.commit()
            .map_err(|error| sql_error("finish turn inspection", &error))?;
        Ok(result)
    }
    /// Inspects the given turn of a session, or the session's single head turn
    /// when none is given.
    ///
    /// # Errors
    ///
    /// - [`InspectionError::Session`] if the session cannot be read.
    /// - [`InspectionError::Head`] if no turn is given and the session does not
    ///   have exactly one head turn.
    /// - [`InspectionError::ForeignTurn`] if the turn belongs to another session.
    /// - Any error from [`Ledger::inspect_turn`] reading the turn's records.
    pub fn inspect_session_turn(
        &self,
        selector: &SessionSelector,
        turn: Option<Id>,
    ) -> Result<TurnInspection, InspectionError> {
        let tx = self
            .connection
            .unchecked_transaction()
            .map_err(|error| sql_error("begin selected turn inspection", &error))?;
        let session = self.session(selector)?;
        let id = match turn {
            Some(id) => id,
            None if session.frontier.len() == 1 => session.frontier[0],
            None => return Err(InspectionError::Head),
        };
        let turn = self.turn(id)?;
        if turn.session != session.id {
            return Err(InspectionError::ForeignTurn);
        }
        let result = self.inspect_turn_record(turn)?;
        tx.commit()
            .map_err(|error| sql_error("finish selected turn inspection", &error))?;
        Ok(result)
    }
    fn inspect_turn_record(&self, turn: FinalizedTurn) -> Result<TurnInspection, InspectionError> {
        let id = turn.id;
        let attempts = related_attempts(
            self.list_attempts(turn.session)?,
            &HashSet::from([id]),
            None,
        );
        let keys = attempts
            .iter()
            .filter_map(|attempt| attempt.intent.input.request_id.clone())
            .collect();
        // No unrelated accepted intents are included when inspecting a source turn.
        let request = requests(self, None, &keys)?
            .into_iter()
            .find(|request| keys.contains(&request.request_id));
        let node_intents = node_intents(self, &attempts)?;
        let workflow = turn
            .workflow_run
            .map(|run| workflows::frozen(&self.connection, run))
            .transpose()?;
        let context = context(
            self,
            &[SourcePoint {
                session: turn.session,
                frontier: vec![id],
            }],
            turn.session,
        )?;
        Ok(TurnInspection {
            version: CONTRACT_VERSION,
            turn,
            attempts,
            request,
            node_intents,
            workflow,
            context,
        })
    }
    /// Exports a session's complete history, provenance, attempts, requests,
    /// workflows, and current context from one consistent snapshot.
    ///
    /// # Errors
    ///
    /// Returns [`InspectionError::Session`] if the session or one of its source
    /// sessions cannot be read, [`InspectionError::Turn`] if its history or
    /// context cannot be read, [`InspectionError::Workflow`] if a workflow
    /// record cannot be read, or [`InspectionError::Storage`] if the SQLite
    /// read fails or a stored payload is invalid.
    pub fn export_session(
        &self,
        selector: &SessionSelector,
    ) -> Result<SessionExport, InspectionError> {
        let tx = self
            .connection
            .unchecked_transaction()
            .map_err(|error| sql_error("begin complete session export", &error))?;
        let session = self.session(selector)?;
        let sources = self.session_sources(session.id)?;
        let heads = if session.frontier.is_empty() {
            sources.clone()
        } else {
            vec![SourcePoint {
                session: session.id,
                frontier: session.frontier.clone(),
            }]
        };
        let turns = self.ancestry(&heads)?;
        let included = turns.iter().map(|turn| turn.id).collect::<HashSet<_>>();
        let mut sessions = vec![session.id];
        let mut seen = HashSet::from([session.id]);
        for turn in &turns {
            if seen.insert(turn.session) {
                sessions.push(turn.session);
            }
        }
        let mut provenance = vec![];
        let mut all_attempts = vec![];
        for id in sessions {
            let origin = self.session(&SessionSelector::Id(id))?;
            provenance.push(SessionProvenance {
                id,
                name: origin.name,
                defaults: origin.defaults,
                sources: self.session_sources(id)?,
            });
            all_attempts.extend(self.list_attempts(id)?);
        }
        let attempts = related_attempts(all_attempts, &included, Some(session.id));
        let keys = attempts
            .iter()
            .filter_map(|attempt| attempt.intent.input.request_id.clone())
            .collect();
        let requests = requests(self, Some(session.id), &keys)?;
        let node_intents = node_intents(self, &attempts)?;
        let mut runs = vec![];
        let mut seen = HashSet::new();
        for turn in &turns {
            if let Some(run) = turn.workflow_run
                && seen.insert(run)
            {
                runs.push(run);
            }
        }
        let mut statement = tx
            .prepare("SELECT id FROM workflow_runs WHERE session_id=?1 ORDER BY rowid")
            .map_err(|error| sql_error("list session workflow records", &error))?;
        let owned = statement
            .query_map([session.id.to_string()], |row| row.get::<_, String>(0))
            .map_err(|error| sql_error("read session workflow IDs", &error))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| sql_error("decode session workflow IDs", &error))?;
        drop(statement);
        let mut workflows = vec![];
        for id in owned {
            let id = Id::try_from(id).map_err(|_| StorageError::Payload)?;
            if seen.insert(id) {
                runs.push(id);
            }
            workflows.push(WorkflowArchive {
                run: workflows::read_run(&tx, id, || {})?,
                rounds: workflows::recovery::read_round_results(&tx, id)?,
                events: workflows::events::read_events(&tx, id)?,
            });
        }
        let workflow_definitions = runs
            .into_iter()
            .map(|run| workflows::frozen(&tx, run))
            .collect::<Result<_, _>>()?;
        let context = context(self, &heads, session.id)?;
        tx.commit()
            .map_err(|error| sql_error("finish complete session export", &error))?;
        Ok(SessionExport {
            version: CONTRACT_VERSION,
            session,
            sources,
            provenance,
            turns,
            attempts,
            requests,
            workflow_definitions,
            workflows,
            node_intents,
            context,
        })
    }
}
