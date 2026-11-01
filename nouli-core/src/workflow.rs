//! Caller-defined acyclic turn networks. Compilation and scheduling are separate.

pub mod compile;
pub(crate) mod node;
pub mod reference;
pub mod resolve;
pub mod schedule;

use std::collections::HashSet;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{
    context::{ContextError, ContextUpdate},
    data::Input,
    decision::{CONTRACT_VERSION, ContractError, TurnRequest},
    history::{HistoryError, ModelSelection, SessionSelection},
    identity::Id,
};
use reference::{Condition, ResultReference};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunMode {
    #[default]
    Durable,
    Ephemeral,
}

fn default_parallelism() -> usize {
    4
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkflowSpec {
    pub version: u32,
    #[serde(default)]
    pub state: Vec<Input>,
    pub nodes: Vec<WorkflowNode>,
    #[serde(default)]
    pub outputs: Vec<OutputBinding>,
    #[serde(default = "default_parallelism")]
    pub max_parallel: usize,
    #[serde(default)]
    pub mode: RunMode,
    #[serde(default)]
    pub model: ModelSelection,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkflowNode {
    pub id: String,
    #[serde(default)]
    pub dependencies: Vec<String>,
    pub turn: TurnRequest,
    #[serde(default)]
    pub inputs: Vec<OutputBinding>,
    pub when: Option<Condition>,
    #[serde(default)]
    pub updates: Vec<ContextUpdate>,
    #[serde(default)]
    pub required_records: Vec<Id>,
    #[serde(default)]
    pub model: ModelSelection,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OutputBinding {
    pub name: String,
    pub reference: ResultReference,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkflowRequest {
    pub workflow: WorkflowSpec,
    pub session: SessionSelection,
    #[serde(default)]
    pub updates: Vec<ContextUpdate>,
    #[serde(default)]
    pub required_records: Vec<Id>,
    pub request_id: Option<String>,
}

/// Invalid workflow request intent, independent of ledger storage.
#[derive(Debug, Error)]
pub enum RequestError {
    #[error(transparent)]
    Compile(#[from] compile::CompileError),
    #[error(transparent)]
    History(#[from] HistoryError),
}

impl WorkflowRequest {
    /// Validate the workflow graph and request intent without fingerprinting or
    /// opening a ledger. Execution-mode options are checked by the engine.
    ///
    /// # Errors
    ///
    /// - [`RequestError::Compile`] if the workflow graph or node contracts are invalid.
    /// - [`RequestError::History`] if the session selection, request ID, required
    ///   records, or context updates are invalid.
    pub fn validate(&self) -> Result<(), RequestError> {
        compile::compile(&self.workflow)?;
        self.session.validate()?;
        if self
            .request_id
            .as_ref()
            .is_some_and(|id| id.trim().is_empty())
            || self.required_records.iter().collect::<HashSet<_>>().len()
                != self.required_records.len()
        {
            return Err(HistoryError::InvalidSelection.into());
        }
        for update in &self.updates {
            update.validate().map_err(HistoryError::from)?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NodeStatus {
    Pending,
    Running,
    Completed,
    Skipped,
    Failed,
    Blocked,
    Interrupted,
}

impl NodeStatus {
    #[must_use]
    pub fn can_transition_to(self, next: Self) -> bool {
        matches!(
            (self, next),
            (
                Self::Pending,
                Self::Running | Self::Skipped | Self::Blocked | Self::Interrupted
            ) | (
                Self::Running,
                Self::Completed | Self::Failed | Self::Interrupted
            ) | (
                Self::Interrupted | Self::Failed | Self::Blocked,
                Self::Pending
            )
        )
    }
    #[must_use]
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Completed | Self::Skipped | Self::Failed | Self::Blocked
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunStatus {
    Pending,
    Running,
    Completed,
    Partial,
    Failed,
    Interrupted,
}

impl RunStatus {
    #[must_use]
    pub fn can_transition_to(self, next: Self) -> bool {
        matches!(
            (self, next),
            (Self::Pending, Self::Running | Self::Interrupted)
                | (
                    Self::Running,
                    Self::Completed | Self::Partial | Self::Failed | Self::Interrupted
                )
                | (
                    Self::Interrupted | Self::Partial | Self::Failed,
                    Self::Running
                )
        )
    }
}

#[derive(Debug, Error, PartialEq)]
pub enum WorkflowError {
    #[error(transparent)]
    Decision(#[from] ContractError),
    #[error(transparent)]
    Context(#[from] ContextError),
    #[error("invalid workflow shape: {0}")]
    Shape(&'static str),
    #[error("invalid JSON pointer")]
    Pointer,
    #[error("condition nesting exceeds 64 levels")]
    ConditionDepth,
}

fn validate_bindings(bindings: &[OutputBinding]) -> Result<(), WorkflowError> {
    let mut names = HashSet::new();
    for binding in bindings {
        if binding.name.trim().is_empty() || !names.insert(&binding.name) {
            return Err(WorkflowError::Shape("binding names"));
        }
        binding.reference.validate()?;
    }
    Ok(())
}

impl WorkflowSpec {
    /// Shape validation; DAG/type/ancestry compilation is a later engine phase.
    ///
    /// # Errors
    ///
    /// - Returns [`WorkflowError::Decision`] if the contract version is
    ///   unsupported or a node's turn request is invalid.
    /// - Returns [`WorkflowError::Shape`] if there are no nodes, parallelism is
    ///   zero, or node IDs, dependencies, node models, or binding names are
    ///   empty or duplicated.
    /// - Returns [`WorkflowError::Context`] if a node's context update is
    ///   invalid.
    /// - Returns [`WorkflowError::Pointer`] or [`WorkflowError::ConditionDepth`]
    ///   if a condition or binding reference is malformed.
    pub fn validate(&self) -> Result<(), WorkflowError> {
        if self.version != CONTRACT_VERSION {
            return Err(ContractError::Version(self.version).into());
        }
        if self.nodes.is_empty() || self.max_parallel == 0 {
            return Err(WorkflowError::Shape("nodes/parallelism"));
        }
        let mut ids = HashSet::new();
        for node in &self.nodes {
            if node.id.trim().is_empty() || !ids.insert(&node.id) {
                return Err(WorkflowError::Shape("node IDs"));
            }
            node.turn.validate()?;
            if node.dependencies.iter().any(|id| id.trim().is_empty())
                || node.dependencies.iter().collect::<HashSet<_>>().len() != node.dependencies.len()
            {
                return Err(WorkflowError::Shape("dependencies"));
            }
            for update in &node.updates {
                update.validate()?;
            }
            for value in [&node.model.provider, &node.model.model] {
                if value.as_ref().is_some_and(|s| s.trim().is_empty()) {
                    return Err(WorkflowError::Shape("node model"));
                }
            }
            if let Some(condition) = &node.when {
                condition.validate()?;
            }
            validate_bindings(&node.inputs)?;
        }
        validate_bindings(&self.outputs)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn branch_join_fixture_validates_and_round_trips() {
        let workflow: WorkflowSpec =
            serde_json::from_str(include_str!("../../fixtures/contracts/workflow.json")).unwrap();
        workflow.validate().unwrap();
        assert_eq!(workflow.nodes.len(), 5);
        assert_eq!(
            workflow,
            serde_json::from_str(&serde_json::to_string(&workflow).unwrap()).unwrap()
        );
    }

    #[test]
    fn finalized_nodes_cannot_rerun_but_interrupted_nodes_can_resume() {
        for status in [
            NodeStatus::Completed,
            NodeStatus::Skipped,
            NodeStatus::Failed,
            NodeStatus::Blocked,
        ] {
            assert!(status.is_terminal());
            assert!(!status.can_transition_to(NodeStatus::Running));
        }
        assert!(NodeStatus::Interrupted.can_transition_to(NodeStatus::Pending));
        assert!(NodeStatus::Failed.can_transition_to(NodeStatus::Pending));
        assert!(NodeStatus::Blocked.can_transition_to(NodeStatus::Pending));
        assert!(NodeStatus::Running.can_transition_to(NodeStatus::Interrupted));
        assert!(RunStatus::Interrupted.can_transition_to(RunStatus::Running));
        assert!(RunStatus::Partial.can_transition_to(RunStatus::Running));
        assert!(!RunStatus::Completed.can_transition_to(RunStatus::Running));
    }
    fn request() -> WorkflowRequest {
        WorkflowRequest {
            workflow: serde_json::from_str(include_str!("../../fixtures/contracts/workflow.json"))
                .unwrap(),
            session: SessionSelection::New { name: None },
            updates: vec![],
            required_records: vec![],
            request_id: None,
        }
    }

    #[test]
    fn request_validation_preserves_graph_and_intent_error_precedence() {
        let original = request();
        original.validate().unwrap();
        let mut invalid = original.clone();
        invalid.workflow.nodes[0].dependencies = vec!["missing".into()];
        invalid.request_id = Some(" ".into());
        assert!(matches!(
            invalid.validate(),
            Err(RequestError::Compile(
                compile::CompileError::UnknownDependency
            ))
        ));
        invalid.workflow = original.workflow.clone();
        assert!(matches!(
            invalid.validate(),
            Err(RequestError::History(HistoryError::InvalidSelection))
        ));
        invalid.request_id = None;
        invalid.session = SessionSelection::From {
            sources: vec![],
            name: None,
        };
        assert!(matches!(
            invalid.validate(),
            Err(RequestError::History(HistoryError::InvalidSelection))
        ));
        invalid.session = original.session;
        let id = Id::generate();
        invalid.required_records = vec![id, id];
        assert!(matches!(
            invalid.validate(),
            Err(RequestError::History(HistoryError::InvalidSelection))
        ));
        invalid.required_records.clear();
        invalid.updates = vec![ContextUpdate::Append {
            record: crate::context::ContextRecord {
                id,
                key: " ".into(),
                value: crate::data::Data::new(serde_json::json!(1)).unwrap(),
                kind: crate::context::ContextKind::Evidence,
            },
        }];
        assert!(matches!(
            invalid.validate(),
            Err(RequestError::History(HistoryError::Context(
                ContextError::EmptyKey
            )))
        ));
    }
}
