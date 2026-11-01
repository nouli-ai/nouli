//! Pure node scope and evaluation construction shared by scheduling and storage.
use super::{
    NodeStatus,
    compile::CompiledWorkflow,
    resolve::ResolveError,
    schedule::{NodeEvaluation, NodeOutput, NodeProgress, ResolvedBinding, compiled_binding},
};
use crate::{
    codec,
    data::{Data, Input},
    history::{EvaluationRequest, FinalizedTurn, ModelSelection, SourcePoint, StandingChanges},
    identity::{Id, SessionSelector},
    storage::workflows::{FrozenWorkflow, WorkflowError},
};
use std::collections::HashSet;

pub(crate) fn child_key(run: Id, node: usize) -> String {
    format!("{run}/node/{node}")
}

pub(crate) struct NodeScope {
    pub parents: Vec<usize>,
    pub ancestors: Vec<usize>,
    /// Ephemeral executors may complete nodes without publishing saved sources.
    pub sources: Option<Vec<SourcePoint>>,
    pub inputs: Vec<ResolvedBinding>,
}

/// Keep source order identical to the completed parent order. A missing source
/// stays distinct from an empty parent set, so durable storage can reject it.
pub(crate) fn parent_sources(
    parents: &[usize],
    source: impl FnMut(usize) -> Option<SourcePoint>,
) -> Option<Vec<SourcePoint>> {
    parents.iter().copied().map(source).collect()
}

pub(crate) fn node_scope(
    graph: &CompiledWorkflow,
    states: &[NodeProgress],
    node: usize,
) -> Result<NodeScope, ResolveError> {
    let declared = graph.nodes().get(node).ok_or(ResolveError::Incomplete)?;
    let inputs = declared
        .inputs
        .iter()
        .map(|binding| compiled_binding(binding, states))
        .collect::<Result<Vec<_>, _>>()?;
    let parents: Vec<_> = declared
        .dependencies
        .iter()
        .filter(|parent| states[**parent].status == NodeStatus::Completed)
        .copied()
        .collect();
    let sources = parent_sources(&parents, |parent| {
        states[parent]
            .output
            .as_ref()
            .and_then(|output| output.source.clone())
    });
    let mut ancestors = HashSet::new();
    for parent in &parents {
        ancestors.insert(*parent);
        ancestors.extend(states[*parent].ancestors.iter().copied());
    }
    let ancestors = graph
        .order()
        .iter()
        .filter(|ancestor| ancestors.contains(ancestor))
        .copied()
        .collect();
    Ok(NodeScope {
        parents,
        ancestors,
        sources,
        inputs,
    })
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

pub(crate) fn request_for(
    frozen: &FrozenWorkflow,
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        identity::Id,
        workflow::{WorkflowSpec, compile, reference::ReferenceValue},
    };

    fn progress(source: Option<SourcePoint>, ancestors: Vec<usize>) -> NodeProgress {
        NodeProgress {
            status: NodeStatus::Completed,
            output: Some(NodeOutput {
                outcome: serde_json::from_str(include_str!(
                    "../../../fixtures/contracts/decision-outcome.json"
                ))
                .unwrap(),
                source,
                evaluation: None,
            }),
            error: None,
            ancestors,
            inputs: vec![],
        }
    }

    #[test]
    fn scope_orders_unique_ancestors_and_keeps_skipped_and_unsaved_sources_distinct() {
        let mut spec: WorkflowSpec =
            serde_json::from_str(include_str!("../../../fixtures/contracts/workflow.json"))
                .unwrap();
        spec.nodes[3].inputs = vec![spec.outputs[1].clone()];
        let graph = compile::compile(&spec).unwrap();
        let sources: Vec<_> = (0..spec.nodes.len())
            .map(|_| SourcePoint {
                session: Id::generate(),
                frontier: vec![Id::generate()],
            })
            .collect();
        let mut states: Vec<_> = sources
            .iter()
            .cloned()
            .map(|source| progress(Some(source), vec![]))
            .collect();
        states[1].ancestors = vec![0];
        states[2].ancestors = vec![0];
        let scope = node_scope(&graph, &states, 3).unwrap();
        assert_eq!(scope.parents, vec![1, 2]);
        assert_eq!(scope.ancestors, vec![0, 1, 2]);
        assert_eq!(
            scope.sources,
            Some(vec![sources[1].clone(), sources[2].clone()])
        );
        assert!(matches!(
            scope.inputs[0].value,
            ReferenceValue::Present { .. }
        ));
        states[2].status = NodeStatus::Skipped;
        states[2].output = None;
        let scope = node_scope(&graph, &states, 3).unwrap();
        assert_eq!(scope.parents, vec![1]);
        assert_eq!(scope.ancestors, vec![0, 1]);
        assert_eq!(scope.sources, Some(vec![sources[1].clone()]));
        assert!(matches!(
            scope.inputs[0].value,
            ReferenceValue::Absent { .. }
        ));
        assert!(!scope.inputs[0].required_missing);
        states[1].output.as_mut().unwrap().source = None;
        assert_eq!(node_scope(&graph, &states, 3).unwrap().sources, None);
        let root = node_scope(&graph, &states, 0).unwrap();
        assert_eq!(root.parents, [] as [usize; 0]);
        assert_eq!(root.ancestors, [] as [usize; 0]);
        assert_eq!(root.sources, Some(vec![]));
    }
}
