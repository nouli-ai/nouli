//! Node intents, attempts and branch-frontier publication for one run owner.
use super::{
    WorkflowError, WorkflowEvent, WorkflowEventKind, WorkflowOwner, child_key, event, node_name,
    validate_owner,
};
use crate::history::StandingChanges;
use crate::storage::{
    Ledger, StorageError, sql_error,
    turns::{
        AttemptInput, AttemptProgress, AttemptStatus, PreparedAttempt, TurnError,
        interrupt_attempt, progress_event,
    },
};
use crate::{
    codec,
    data::{Data, Input},
    history::{EvaluationManifest, FinalizedTurn, HistoryEvent, ModelSelection},
    identity::SessionSelector,
    outcome::TurnOutcome,
    workflow::{
        NodeStatus, compile,
        schedule::{NodeEvaluation, NodeOutput, NodeProgress, compiled_binding},
    },
};
use crate::{
    history::{EvaluationRequest, SourcePoint},
    identity::Id,
};
use rusqlite::{Connection, OptionalExtension, Transaction, params};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NodeAttemptScope {
    pub run: Id,
    pub node: usize,
    pub round: u32,
    pub sources: Vec<SourcePoint>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NodeIntent {
    pub run: Id,
    pub node: usize,
    pub round: u32,
    pub request: EvaluationRequest,
    pub sources: Vec<SourcePoint>,
}

fn read_progress(
    connection: &Connection,
    run: Id,
    node: &str,
) -> Result<NodeProgress, WorkflowError> {
    let row: Option<(String, String)> = connection
        .query_row(
            "SELECT state,payload_toon FROM workflow_nodes WHERE run_id=?1 AND node_id=?2",
            params![run.to_string(), node],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(|error| sql_error("read node progress", &error))?;
    let (state, payload) = row.ok_or(WorkflowError::Snapshot)?;
    let progress: NodeProgress = Ledger::decode_payload(&payload)?;
    if state != node_name(progress.status) {
        return Err(WorkflowError::Snapshot);
    }
    Ok(progress)
}
fn put_progress(
    tx: &Transaction<'_>,
    owner: &WorkflowOwner,
    node: usize,
    progress: &NodeProgress,
    turn: Option<Id>,
) -> Result<(), StorageError> {
    const RECORD_NODE_PROGRESS: &str = "UPDATE workflow_nodes SET state=?3,turn_id=?4,payload_toon=?5 WHERE run_id=?1 AND node_id=?2";

    let definition = &owner.frozen.definition;
    let changed = tx
        .execute(
            RECORD_NODE_PROGRESS,
            params![
                definition.run.to_string(),
                definition.request.workflow.nodes[node].id,
                node_name(progress.status),
                turn.map(|id| id.to_string()),
                Ledger::encode_payload(progress)?
            ],
        )
        .map_err(|error| sql_error("record node progress", &error))?;
    if changed != 1 {
        return Err(StorageError::Conflict);
    }
    Ok(())
}
pub(crate) fn node_output(turn: &FinalizedTurn) -> NodeOutput {
    NodeOutput {
        outcome: turn.outcome.clone(),
        source: Some(SourcePoint {
            session: turn.session,
            frontier: vec![turn.id],
        }),
        evaluation: Some(NodeEvaluation {
            request: turn.request.clone(),
            updates: turn.updates.clone(),
            manifest: turn.manifest.clone(),
        }),
    }
}
fn node_event(
    round: u32,
    node: usize,
    kind: WorkflowEventKind,
    progress: Option<NodeProgress>,
    attempt: Option<Id>,
    turn: Option<Id>,
) -> WorkflowEvent {
    WorkflowEvent {
        version: crate::decision::CONTRACT_VERSION,
        round,
        kind,
        node: Some(node),
        node_status: progress
            .as_ref()
            .map(|progress| progress.status)
            .or_else(|| turn.map(|_| NodeStatus::Completed)),
        run_status: None,
        progress,
        attempt,
        turn,
    }
}
struct ExpectedScope {
    sources: Vec<SourcePoint>,
    ancestors: Vec<usize>,
    inputs: Vec<crate::workflow::schedule::ResolvedBinding>,
}
fn expected_scope(
    ledger: &Ledger,
    owner: &WorkflowOwner,
    node: usize,
) -> Result<ExpectedScope, WorkflowError> {
    let definition = &owner.frozen.definition;
    let plan = compile::compile(&definition.request.workflow)?;
    let declared = plan.nodes().get(node).ok_or(WorkflowError::Snapshot)?;
    let states = definition
        .request
        .workflow
        .nodes
        .iter()
        .map(|node| read_progress(&ledger.connection, definition.run, &node.id))
        .collect::<Result<Vec<_>, _>>()?;
    if declared.dependencies.iter().any(|parent| {
        matches!(
            states[*parent].status,
            NodeStatus::Pending | NodeStatus::Running
        )
    }) {
        return Err(WorkflowError::Conflict);
    }
    let parents: Vec<_> = declared
        .dependencies
        .iter()
        .filter(|parent| states[**parent].status == NodeStatus::Completed)
        .copied()
        .collect();
    let sources = if parents.is_empty() {
        definition.base.sources.clone()
    } else {
        parents
            .iter()
            .map(|parent| {
                states[*parent]
                    .output
                    .as_ref()
                    .and_then(|output| output.source.clone())
                    .ok_or(WorkflowError::Snapshot)
            })
            .collect::<Result<Vec<_>, _>>()?
    };
    let mut ancestors = HashSet::new();
    for parent in parents {
        ancestors.insert(parent);
        ancestors.extend(states[parent].ancestors.iter().copied());
    }
    let ordered = plan
        .order()
        .iter()
        .filter(|ancestor| ancestors.contains(ancestor))
        .copied()
        .collect();
    let inputs = declared
        .inputs
        .iter()
        .map(|binding| compiled_binding(binding, &states).map_err(|_| WorkflowError::Snapshot))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(ExpectedScope {
        sources,
        ancestors: ordered,
        inputs,
    })
}
pub(crate) fn request_for(
    frozen: &super::FrozenWorkflow,
    node: usize,
    inputs: &[crate::workflow::schedule::ResolvedBinding],
) -> Result<EvaluationRequest, WorkflowError> {
    let definition = &frozen.definition;
    let node_definition = definition
        .request
        .workflow
        .nodes
        .get(node)
        .ok_or(WorkflowError::Snapshot)?;
    let mut turn = node_definition.turn.clone();
    if !inputs.is_empty() {
        turn.state.push(Input::Structured {
            data: Data::new(
                codec::to_value(&serde_json::json!({"workflow_inputs":inputs}))
                    .map_err(|_| WorkflowError::Snapshot)?,
            )
            .map_err(|_| WorkflowError::Snapshot)?,
        });
    }
    let mut required = definition.request.required_records.clone();
    for id in &node_definition.required_records {
        if !required.contains(id) {
            required.push(*id);
        }
    }
    let settings = frozen.settings.get(node).ok_or(WorkflowError::Snapshot)?;
    Ok(EvaluationRequest {
        turn,
        standing: StandingChanges::default(),
        session: if definition.base.runtime_only {
            crate::history::SessionSelection::New { name: None }
        } else {
            crate::history::SessionSelection::Continue {
                session: SessionSelector::Id(definition.session),
            }
        },
        updates: node_definition.updates.clone(),
        required_records: required,
        model: ModelSelection {
            provider: Some(settings.profile_name.clone()),
            model: Some(settings.profile.model.clone()),
        },
        request_id: if definition.base.runtime_only {
            None
        } else {
            Some(child_key(definition.run, node))
        },
    })
}

impl Ledger {
    /// Reads the stored progress of one workflow node.
    ///
    /// # Errors
    ///
    /// - Returns [`WorkflowError::Snapshot`] if the node index is out of range
    ///   or its stored row is missing or inconsistent.
    /// - Returns [`WorkflowError::Storage`] if the ledger cannot be read or the
    ///   progress payload cannot be decoded.
    pub fn workflow_node_progress(
        &self,
        owner: &WorkflowOwner,
        node: usize,
    ) -> Result<NodeProgress, WorkflowError> {
        let definition = &owner.frozen.definition;
        let name = &definition
            .request
            .workflow
            .nodes
            .get(node)
            .ok_or(WorkflowError::Snapshot)?
            .id;
        read_progress(&self.connection, definition.run, name)
    }
    /// Records a non-completing state transition for one workflow node.
    ///
    /// Entering an interrupted or failed state also interrupts the node's live
    /// attempts. Repeating the current progress, or any update to a completed
    /// node that does not contradict it, succeeds without writing.
    ///
    /// # Errors
    ///
    /// - Returns [`WorkflowError::Conflict`] or [`WorkflowError::Storage`] if the
    ///   owner no longer holds the running workflow or the ledger is read-only.
    /// - Returns [`WorkflowError::Conflict`] if the transition is not allowed,
    ///   targets the completed state, or changes a completed node.
    /// - Returns [`WorkflowError::Snapshot`] if the node index is invalid, stored
    ///   progress is inconsistent, or running progress does not match the
    ///   expected ancestors and inputs.
    /// - Returns [`WorkflowError::Storage`] if the ledger cannot be read or
    ///   written.
    /// - Returns [`WorkflowError::Conflict`] if the node row changed.
    pub fn record_workflow_progress(
        &mut self,
        owner: &WorkflowOwner,
        node: usize,
        progress: &NodeProgress,
    ) -> Result<(), WorkflowError> {
        const READ_INTERRUPTED_NODE_ATTEMPTS: &str = "SELECT a.id,s.state FROM attempts a JOIN attempt_states s ON s.attempt_id=a.id WHERE a.run_id=?1 AND a.node_id=?2 AND s.state IN ('pending','running')";

        validate_owner(self, owner)?;
        let definition = &owner.frozen.definition;
        let name = &definition
            .request
            .workflow
            .nodes
            .get(node)
            .ok_or(WorkflowError::Snapshot)?
            .id;
        let current = read_progress(&self.connection, definition.run, name)?;
        if current.status == NodeStatus::Completed {
            if progress.status == NodeStatus::Completed && current != *progress {
                return Err(WorkflowError::Conflict);
            }
            return Ok(());
        }
        if current == *progress {
            return Ok(());
        }
        if !current.status.can_transition_to(progress.status) {
            return Err(WorkflowError::Conflict);
        }
        if progress.status == NodeStatus::Running {
            let ExpectedScope {
                ancestors, inputs, ..
            } = expected_scope(self, owner, node)?;
            if progress.ancestors != ancestors
                || progress.inputs != inputs
                || inputs.iter().any(|input| input.required_missing)
            {
                return Err(WorkflowError::Snapshot);
            }
        }
        if progress.status == NodeStatus::Completed {
            return Err(WorkflowError::Conflict);
        }
        self.transaction::<_, StorageError>("record workflow node transition", |tx| {
            put_progress(tx, owner, node, progress, None)?;
            if matches!(
                progress.status,
                NodeStatus::Interrupted | NodeStatus::Failed
            ) {
                let mut statement = tx
                    .prepare(READ_INTERRUPTED_NODE_ATTEMPTS)
                    .map_err(|error| sql_error("read interrupted node attempts", &error))?;
                let rows = statement
                    .query_map(params![definition.run.to_string(), name], |row| {
                        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                    })
                    .map_err(|error| sql_error("read node attempt states", &error))?
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(|error| sql_error("read node attempts", &error))?;
                drop(statement);
                for (id, state) in rows {
                    let id = Id::try_from(id).map_err(|_| StorageError::Payload)?;
                    interrupt_attempt(
                        tx,
                        id,
                        AttemptStatus::parse(&state)?,
                        "Workflow node interrupted",
                    )?;
                }
            }
            event(
                tx,
                definition.run,
                &node_event(
                    owner.round,
                    node,
                    WorkflowEventKind::NodeState,
                    Some(progress.clone()),
                    None,
                    None,
                ),
            )?;
            Ok(())
        })?;
        Ok(())
    }
    /// Records the evaluation intent for a running workflow node in the
    /// owner's current round, returning the saved intent.
    ///
    /// # Errors
    ///
    /// - Returns [`WorkflowError::Conflict`] or [`WorkflowError::Storage`] if the
    ///   owner no longer holds the running workflow or the ledger is read-only.
    /// - Returns [`WorkflowError::Snapshot`] if the node is not running or the
    ///   request, sources, ancestors, or inputs differ from the expected scope.
    /// - Returns [`WorkflowError::Conflict`] if a parent node has not settled.
    /// - Returns [`WorkflowError::Storage`] if the ledger cannot be read or
    ///   written.
    /// - Returns [`WorkflowError::Conflict`] if a different intent was already
    ///   saved for this round.
    pub fn accept_workflow_node(
        &mut self,
        owner: &WorkflowOwner,
        node: usize,
        request: &EvaluationRequest,
        sources: &[SourcePoint],
    ) -> Result<NodeIntent, WorkflowError> {
        const INSPECT_NODE_INTENT: &str = "SELECT payload_toon FROM workflow_node_intents WHERE run_id=?1 AND node_id=?2 AND round=?3";
        const SAVE_NODE_INTENT: &str = "INSERT INTO workflow_node_intents(run_id,node_id,round,payload_toon) VALUES(?1,?2,?3,?4)";

        validate_owner(self, owner)?;
        let definition = &owner.frozen.definition;
        let name = &definition
            .request
            .workflow
            .nodes
            .get(node)
            .ok_or(WorkflowError::Snapshot)?
            .id;
        let progress = read_progress(&self.connection, definition.run, name)?;
        let ExpectedScope {
            sources: expected,
            ancestors,
            inputs,
        } = expected_scope(self, owner, node)?;
        if progress.status != NodeStatus::Running
            || progress.ancestors != ancestors
            || progress.inputs != inputs
            || sources != expected
            || *request != request_for(&owner.frozen, node, &inputs)?
        {
            return Err(WorkflowError::Snapshot);
        }
        let intent = NodeIntent {
            run: definition.run,
            node,
            round: owner.round,
            request: request.clone(),
            sources: sources.to_vec(),
        };
        let payload = Self::encode_payload(&intent)?;
        self.transaction::<_, StorageError>("accept workflow node intent", |tx| {
            let existing: Option<String> = tx
                .query_row(
                    INSPECT_NODE_INTENT,
                    params![definition.run.to_string(), name, owner.round],
                    |row| row.get(0),
                )
                .optional()
                .map_err(|error| sql_error("inspect node intent", &error))?;
            if let Some(existing) = existing {
                if existing != payload {
                    return Err(StorageError::Conflict);
                }
            } else {
                tx.execute(
                    SAVE_NODE_INTENT,
                    params![definition.run.to_string(), name, owner.round, payload],
                )
                .map_err(|error| sql_error("save node intent", &error))?;
                event(
                    tx,
                    definition.run,
                    &node_event(
                        owner.round,
                        node,
                        WorkflowEventKind::NodeIntent,
                        None,
                        None,
                        None,
                    ),
                )?;
            }
            Ok(())
        })?;
        Ok(intent)
    }
    /// Starts a durable attempt for an accepted workflow node intent.
    ///
    /// # Errors
    ///
    /// - Returns [`WorkflowError::Conflict`] or [`WorkflowError::Storage`] if the
    ///   owner no longer holds the running workflow or the ledger is read-only.
    /// - Returns [`WorkflowError::Conflict`] if the intent belongs to another
    ///   run or round, or the node is not running.
    /// - Returns [`WorkflowError::History`] if the manifest is invalid, and
    ///   [`WorkflowError::Snapshot`] if it does not match the intent's sources,
    ///   the node's frozen provider settings, or the required records.
    /// - Returns [`WorkflowError::Turn`] or [`WorkflowError::Session`] if the
    ///   source history cannot be read or projected.
    /// - Returns [`WorkflowError::Storage`] if the ledger cannot be written.
    /// - Returns [`WorkflowError::Conflict`] if the saved intent differs or the
    ///   node already has a live attempt.
    pub fn prepare_workflow_attempt(
        &mut self,
        owner: &WorkflowOwner,
        intent: &NodeIntent,
        manifest: EvaluationManifest,
    ) -> Result<PreparedAttempt, WorkflowError> {
        validate_owner(self, owner)?;
        let definition = &owner.frozen.definition;
        let node = intent.node;
        let name = &definition
            .request
            .workflow
            .nodes
            .get(node)
            .ok_or(WorkflowError::Snapshot)?
            .id;
        if intent.run != definition.run
            || intent.round != owner.round
            || read_progress(&self.connection, definition.run, name)?.status != NodeStatus::Running
        {
            return Err(WorkflowError::Conflict);
        }
        manifest.validate()?;
        let settings = &owner.frozen.settings[node];
        if manifest.sources != intent.sources
            || manifest.provider.provider != settings.profile_name
            || manifest.provider.model != settings.profile.model
            || intent.request.required_records.iter().any(|id| {
                !manifest.records.iter().any(|record| {
                    record.record == *id
                        && record.protected
                        && record.representation != crate::history::Representation::Omitted
                })
            })
        {
            return Err(WorkflowError::Snapshot);
        }
        let history = self.ancestry(&intent.sources)?;
        crate::context::projection::project_with_base(
            &history,
            &intent.sources,
            Some(&definition.base),
            &intent.request.updates,
            &crate::context::projection::RevisionOrigin::Pending {
                session: Some(definition.session),
                token: Id::generate(),
            },
        )
        .map_err(TurnError::from)?;
        let current = self.session(&SessionSelector::Id(definition.session))?;
        let attempt = PreparedAttempt {
            workflow: Some(NodeAttemptScope {
                run: definition.run,
                node,
                round: owner.round,
                sources: intent.sources.clone(),
            }),
            id: Id::generate(),
            owner: Some(owner.session_guard.owner),
            session: definition.session,
            expected_frontier: intent
                .sources
                .iter()
                .flat_map(|source| source.frontier.iter().copied())
                .collect(),
            expected_revision: current.revision,
            input: AttemptInput {
                request: intent.request.turn.clone(),
                updates: intent.request.updates.clone(),
                standing: StandingChanges::default(),
                manifest,
                request_id: intent.request.request_id.clone(),
            },
        };
        self.transaction::<_, StorageError>("prepare workflow attempt", |tx| {
            insert_attempt(tx, owner, intent, name, &attempt)
        })?;
        Ok(attempt)
    }
    /// Records a failed or interrupted outcome for a live workflow attempt.
    ///
    /// # Errors
    ///
    /// - Returns [`WorkflowError::Conflict`] or [`WorkflowError::Storage`] if the
    ///   owner no longer holds the running workflow or the ledger is read-only.
    /// - Returns [`WorkflowError::Turn`] if the attempt cannot be read.
    /// - Returns [`WorkflowError::Snapshot`] if the attempt is not a workflow
    ///   attempt.
    /// - Returns [`WorkflowError::Conflict`] if the attempt belongs to another
    ///   owner, run, or round, is no longer live, or `progress` is not a failed
    ///   or interrupted state with an error.
    /// - Returns [`WorkflowError::Storage`] if the ledger cannot be written.
    /// - Returns [`WorkflowError::Conflict`] if the attempt settled
    ///   concurrently.
    pub fn fail_workflow_attempt(
        &mut self,
        owner: &WorkflowOwner,
        attempt: Id,
        progress: &AttemptProgress,
    ) -> Result<(), WorkflowError> {
        const RECORD_FAILED_WORKFLOW_ATTEMPT: &str = "UPDATE attempt_states SET state=?2 WHERE attempt_id=?1 AND state IN ('pending','running')";

        validate_owner(self, owner)?;
        let snapshot = self.attempt(attempt)?;
        let scope = snapshot.intent.workflow.ok_or(WorkflowError::Snapshot)?;
        if snapshot.intent.owner != Some(owner.session_guard.owner)
            || scope.run != owner.frozen.definition.run
            || scope.round != owner.round
            || !matches!(
                snapshot.progress.state,
                AttemptStatus::Pending | AttemptStatus::Running
            )
            || !matches!(
                progress.state,
                AttemptStatus::Failed | AttemptStatus::Interrupted
            )
            || progress.error.is_none()
        {
            return Err(WorkflowError::Conflict);
        }
        self.transaction::<_, StorageError>("finish workflow attempt", |tx| {
            let state = if progress.state == AttemptStatus::Failed {
                "failed"
            } else {
                "interrupted"
            };
            let changed = tx
                .execute(
                    RECORD_FAILED_WORKFLOW_ATTEMPT,
                    params![attempt.to_string(), state],
                )
                .map_err(|error| sql_error("record failed workflow attempt", &error))?;
            if changed != 1 {
                return Err(StorageError::Conflict);
            }
            progress_event(tx, attempt, progress)?;
            event(
                tx,
                scope.run,
                &node_event(
                    owner.round,
                    scope.node,
                    WorkflowEventKind::AttemptFinished,
                    None,
                    Some(attempt),
                    None,
                ),
            )?;
            Ok(())
        })?;
        Ok(())
    }
    /// Publishes a workflow attempt's outcome as a finalized turn, completes
    /// its node, and advances the session's branch frontier.
    ///
    /// # Errors
    ///
    /// - Returns [`WorkflowError::Conflict`] or [`WorkflowError::Storage`] if the
    ///   owner no longer holds the running workflow or the ledger is read-only.
    /// - Returns [`WorkflowError::Snapshot`] if the attempt is not a workflow
    ///   attempt or its node index is invalid.
    /// - Returns [`WorkflowError::Conflict`] if the attempt belongs to another
    ///   owner, run, or round, is no longer live, or its node is not running.
    /// - Returns [`WorkflowError::Turn`] or [`WorkflowError::History`] if the
    ///   attempt or source history cannot be read or projected, or the turn is
    ///   invalid.
    /// - Returns [`WorkflowError::Storage`] if the ledger cannot be written.
    /// - Returns [`WorkflowError::Conflict`] if the session revision or node
    ///   identity changed concurrently.
    pub fn finalize_workflow_attempt(
        &mut self,
        owner: &WorkflowOwner,
        attempt: Id,
        outcome: TurnOutcome,
    ) -> Result<FinalizedTurn, WorkflowError> {
        validate_owner(self, owner)?;
        let snapshot = self.attempt(attempt)?;
        let prepared = snapshot.intent;
        let scope = prepared.workflow.as_ref().ok_or(WorkflowError::Snapshot)?;
        let definition = &owner.frozen.definition;
        if prepared.owner != Some(owner.session_guard.owner)
            || scope.run != definition.run
            || scope.round != owner.round
            || !matches!(
                snapshot.progress.state,
                AttemptStatus::Pending | AttemptStatus::Running
            )
        {
            return Err(WorkflowError::Conflict);
        }
        let node = scope.node;
        let name = &definition
            .request
            .workflow
            .nodes
            .get(node)
            .ok_or(WorkflowError::Snapshot)?
            .id;
        let mut progress = read_progress(&self.connection, definition.run, name)?;
        if progress.status != NodeStatus::Running {
            return Err(WorkflowError::Conflict);
        }
        let history = self.ancestry(&scope.sources)?;
        crate::context::projection::project_with_base(
            &history,
            &scope.sources,
            Some(&definition.base),
            &prepared.input.updates,
            &crate::context::projection::RevisionOrigin::Pending {
                session: Some(definition.session),
                token: prepared.id,
            },
        )
        .map_err(TurnError::from)?;
        let mut manifest = prepared.input.manifest;
        manifest.provider = outcome.provider.clone();
        let mut seen = HashSet::new();
        let turn = FinalizedTurn {
            context_base: Some(definition.base.clone()),
            workflow_run: Some(definition.run),
            runtime_only: false,
            workflow_node: Some(name.clone()),
            id: Id::generate(),
            session: definition.session,
            parents: scope
                .sources
                .iter()
                .flat_map(|source| source.frontier.iter().copied())
                .filter(|id| seen.insert(*id))
                .collect(),
            request: prepared.input.request,
            outcome,
            updates: prepared.input.updates,
            standing: StandingChanges::default(),
            manifest,
        };
        turn.validate()?;
        progress.status = NodeStatus::Completed;
        progress.output = Some(node_output(&turn));
        progress.error = None;
        self.transaction::<_, StorageError>("publish workflow node", |tx| {
            publish_attempt(tx, owner, attempt, &turn, node, &progress)
        })?;
        Ok(turn)
    }
}

fn insert_attempt(
    tx: &Transaction<'_>,
    owner: &WorkflowOwner,
    intent: &NodeIntent,
    name: &str,
    attempt: &PreparedAttempt,
) -> Result<(), StorageError> {
    const VERIFY_ACCEPTED_NODE_INTENT: &str = "SELECT payload_toon FROM workflow_node_intents WHERE run_id=?1 AND node_id=?2 AND round=?3";
    const CHECK_LIVE_NODE_ATTEMPT: &str = "SELECT EXISTS(SELECT 1 FROM attempts a JOIN attempt_states s ON s.attempt_id=a.id WHERE a.run_id=?1 AND a.node_id=?2 AND s.state IN ('pending','running'))";
    const INSERT_WORKFLOW_ATTEMPT: &str =
        "INSERT INTO attempts(id,session_id,run_id,node_id,payload_toon) VALUES(?1,?2,?3,?4,?5)";
    const START_WORKFLOW_ATTEMPT: &str =
        "INSERT INTO attempt_states(attempt_id,state) VALUES(?1,'running')";
    let definition = &owner.frozen.definition;
    let node = intent.node;

    let saved: String = tx
        .query_row(
            VERIFY_ACCEPTED_NODE_INTENT,
            params![definition.run.to_string(), name, owner.round],
            |row| row.get(0),
        )
        .map_err(|error| sql_error("verify accepted node intent", &error))?;
    if saved != Ledger::encode_payload(intent)? {
        return Err(StorageError::Conflict);
    }
    let live: bool = tx
        .query_row(
            CHECK_LIVE_NODE_ATTEMPT,
            params![definition.run.to_string(), name],
            |row| row.get(0),
        )
        .map_err(|error| sql_error("check live node attempt", &error))?;
    if live {
        return Err(StorageError::Conflict);
    }
    tx.execute(
        INSERT_WORKFLOW_ATTEMPT,
        params![
            attempt.id.to_string(),
            definition.session.to_string(),
            definition.run.to_string(),
            name,
            Ledger::encode_payload(attempt)?
        ],
    )
    .map_err(|error| sql_error("insert workflow attempt", &error))?;
    tx.execute(START_WORKFLOW_ATTEMPT, [attempt.id.to_string()])
        .map_err(|error| sql_error("start workflow attempt", &error))?;
    progress_event(
        tx,
        attempt.id,
        &AttemptProgress {
            state: AttemptStatus::Running,
            error: None,
        },
    )?;
    event(
        tx,
        definition.run,
        &node_event(
            owner.round,
            node,
            WorkflowEventKind::AttemptStarted,
            None,
            Some(attempt.id),
            None,
        ),
    )?;
    Ok(())
}

fn publish_attempt(
    tx: &Transaction<'_>,
    owner: &WorkflowOwner,
    attempt: Id,
    turn: &FinalizedTurn,
    node: usize,
    progress: &NodeProgress,
) -> Result<(), StorageError> {
    const PROJECT_WORKFLOW_BRANCH_FRONTIER: &str = "WITH RECURSIVE ancestors(id) AS (SELECT p.parent_id FROM turn_parents p JOIN workflow_nodes n ON n.turn_id=p.turn_id WHERE n.run_id=?1 AND n.state='completed' UNION SELECT p.parent_id FROM turn_parents p JOIN ancestors a ON p.turn_id=a.id) SELECT n.turn_id FROM workflow_nodes n WHERE n.run_id=?1 AND n.state='completed' AND n.turn_id NOT IN (SELECT id FROM ancestors) ORDER BY n.declaration_order";
    const VERIFY_WORKFLOW_PUBLICATION_OWNER: &str = "SELECT s.revision,r.published_revision FROM sessions s JOIN workflow_runs r ON r.session_id=s.id WHERE r.id=?1 AND r.state='running' AND r.owner_id=?2";
    const PUBLISH_WORKFLOW_BRANCH_FRONTIER: &str =
        "UPDATE sessions SET frontier_toon=?2,revision=revision+1 WHERE id=?1 AND revision=?3";
    const RECORD_PUBLISHED_WORKFLOW_FRONTIER: &str =
        "UPDATE workflow_runs SET frontier_toon=?2,published_revision=?3 WHERE id=?1";
    let definition = &owner.frozen.definition;

    let (revision, published): (i64, i64) = tx
        .query_row(
            VERIFY_WORKFLOW_PUBLICATION_OWNER,
            params![
                definition.run.to_string(),
                owner.session_guard.owner.to_string()
            ],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(|error| sql_error("verify workflow publication owner", &error))?;
    if revision != published || revision == i64::MAX {
        return Err(StorageError::Conflict);
    }
    super::super::publication::insert_turn(tx, turn, attempt, revision)?;
    put_progress(tx, owner, node, progress, Some(turn.id))?;
    let mut statement = tx
        .prepare(PROJECT_WORKFLOW_BRANCH_FRONTIER)
        .map_err(|error| sql_error("project workflow branch frontier", &error))?;
    let heads = statement
        .query_map([definition.run.to_string()], |row| row.get::<_, String>(0))
        .map_err(|error| sql_error("read completed branches", &error))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| sql_error("read branch heads", &error))?;
    drop(statement);
    let heads = heads
        .into_iter()
        .map(|id| Id::try_from(id).map_err(|_| StorageError::Payload))
        .collect::<Result<Vec<_>, _>>()?;
    let frontier = Ledger::encode_payload(&heads)?;
    let changed = tx
        .execute(
            PUBLISH_WORKFLOW_BRANCH_FRONTIER,
            params![definition.session.to_string(), frontier, revision],
        )
        .map_err(|error| sql_error("publish workflow branch frontier", &error))?;
    if changed != 1 {
        return Err(StorageError::Conflict);
    }
    tx.execute(
        RECORD_PUBLISHED_WORKFLOW_FRONTIER,
        params![definition.run.to_string(), frontier, revision + 1],
    )
    .map_err(|error| sql_error("record published workflow frontier", &error))?;
    super::super::reservations::complete_node(
        tx,
        &child_key(definition.run, node),
        &Ledger::encode_payload(turn)?,
        owner.session_guard.owner,
    )?;
    super::super::sessions::append_event(
        tx,
        definition.session,
        &HistoryEvent::TurnFinalized {
            session: definition.session,
            turn: turn.id,
            frontier: heads,
        },
        "record workflow publication",
    )?;
    event(
        tx,
        definition.run,
        &node_event(
            owner.round,
            node,
            WorkflowEventKind::NodeState,
            None,
            Some(attempt),
            Some(turn.id),
        ),
    )?;
    Ok(())
}
