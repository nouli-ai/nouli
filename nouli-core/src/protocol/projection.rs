//! Compact final workflow reports with self-describing selected values.
use super::{InspectionReference, NodeReport, ProjectedOutput, RunReport, SelectedChoice};
use crate::{
    engine::workflow::WorkflowResult,
    identity::Id,
    outcome::{Answer, Confidence, QuestionOutcome},
    storage::workflows::recovery::RunInspection,
    workflow::{
        NodeStatus, RunStatus, WorkflowSpec, compile,
        schedule::{self, NodeOutput, ResolvedBinding},
    },
};
use thiserror::Error;
#[derive(Debug, Error)]
pub enum ProjectionError {
    #[error("workflow result does not match its definition or inspection")]
    Snapshot,
}
/// The full intermediate outcomes remain in the engine result and ledger.
/// This public report includes only selected outputs and structural node metadata.
///
/// # Errors
///
/// Returns [`ProjectionError::Snapshot`] if the definition does not compile, or the
/// result or `inspection` disagrees with the definition, is unfinished, or has outputs
/// or outcomes that do not match it.
pub fn workflow(
    definition: &WorkflowSpec,
    result: &WorkflowResult,
    inspection: Option<&RunInspection>,
) -> Result<RunReport, ProjectionError> {
    let references = verify_snapshot(definition, result, inspection)?;
    let nodes = node_reports(&references);
    let outputs = result
        .result
        .outputs
        .iter()
        .map(|binding| projected_output(binding, &references))
        .collect::<Result<_, ProjectionError>>()?;
    Ok(RunReport {
        run: result.run,
        session: result.session,
        persisted: result.persisted,
        replayed: result.replayed,
        status: result.result.status,
        sources: result.sources.clone(),
        frontier: result.frontier.clone().unwrap_or_default(),
        nodes,
        outputs,
    })
}

/// Checks that `result` (and `inspection`, when given) is a finished run of
/// `definition` whose outputs and outcomes match it, and returns the verified
/// snapshot's [`References`].
fn verify_snapshot<'a>(
    definition: &'a WorkflowSpec,
    result: &'a WorkflowResult,
    inspection: Option<&'a RunInspection>,
) -> Result<References<'a>, ProjectionError> {
    let graph = compile::compile(definition).map_err(|_| ProjectionError::Snapshot)?;
    if !is_finished_run_of(definition, result)
        || inspection.is_some_and(|inspection| !inspects(inspection, definition, result))
    {
        return Err(ProjectionError::Snapshot);
    }
    if schedule::project_outputs(&graph, &result.result.nodes)
        .map_err(|_| ProjectionError::Snapshot)?
        != result.result.outputs
    {
        return Err(ProjectionError::Snapshot);
    }
    for (node, progress) in definition.nodes.iter().zip(&result.result.nodes) {
        if let Some(output) = &progress.output {
            output
                .outcome
                .validate(&node.turn)
                .map_err(|_| ProjectionError::Snapshot)?;
        }
    }
    Ok(References {
        definition,
        result,
        inspection,
    })
}

/// Whether the result has one settled node per definition node and records
/// run, session, and frontier exactly when persisted.
fn is_finished_run_of(definition: &WorkflowSpec, result: &WorkflowResult) -> bool {
    definition.nodes.len() == result.result.nodes.len()
        && result.persisted == result.run.is_some()
        && result.persisted == result.session.is_some()
        && result.persisted == result.frontier.is_some()
        && !matches!(
            result.result.status,
            RunStatus::Pending | RunStatus::Running
        )
        && !result
            .result
            .nodes
            .iter()
            .any(|node| matches!(node.status, NodeStatus::Pending | NodeStatus::Running))
}

/// Whether the inspection describes this run of this definition, node for node.
fn inspects(
    inspection: &RunInspection,
    definition: &WorkflowSpec,
    result: &WorkflowResult,
) -> bool {
    Some(inspection.run.frozen.definition.run) == result.run
        && inspection.run.frozen.definition.request.workflow == *definition
        && inspection.run.nodes == result.result.nodes
        && inspection.nodes.len() == definition.nodes.len()
        && inspection
            .nodes
            .iter()
            .enumerate()
            .all(|(index, node)| node.node == index)
}

/// Builds the inspection links that point from the report back to the ledger.
///
/// Only [`verify_snapshot`] creates one, so the definition, result, and
/// inspection are known to have one entry per node and can be indexed directly.
struct References<'a> {
    definition: &'a WorkflowSpec,
    result: &'a WorkflowResult,
    inspection: Option<&'a RunInspection>,
}
impl References<'_> {
    fn output(&self, index: usize) -> Option<&NodeOutput> {
        self.result.result.nodes[index].output.as_ref()
    }

    /// The node's saved turn, reported only for persisted runs.
    fn turn(&self, index: usize) -> Option<Id> {
        if !self.result.persisted {
            return None;
        }
        self.output(index)
            .and_then(|output| output.source.as_ref())
            .and_then(|source| source.frontier.first())
            .copied()
    }

    fn node(&self, index: usize) -> InspectionReference {
        InspectionReference {
            run: self.result.run,
            session: self.result.session,
            node: Some(self.definition.nodes[index].id.clone()),
            turn: self.turn(index),
            attempts: self
                .inspection
                .map(|inspection| {
                    inspection.nodes[index]
                        .attempts
                        .iter()
                        .map(|attempt| attempt.intent.id)
                        .collect()
                })
                .unwrap_or_default(),
        }
    }
}

fn node_reports(references: &References<'_>) -> Vec<NodeReport> {
    references
        .definition
        .nodes
        .iter()
        .zip(&references.result.result.nodes)
        .enumerate()
        .map(|(index, (node, progress))| {
            let inspection = references.node(index);
            NodeReport {
                provider: progress
                    .output
                    .as_ref()
                    .map(|output| output.outcome.provider.clone()),
                node: node.id.clone(),
                status: progress.status,
                turn: inspection.turn,
                outcome: None,
                error: progress.error.clone(),
                inspection: Some(inspection),
            }
        })
        .collect()
}

/// Describes one selected output with its question and the answer behind it.
fn projected_output(
    binding: &ResolvedBinding,
    references: &References<'_>,
) -> Result<ProjectedOutput, ProjectionError> {
    let definition = references.definition;
    let index = definition
        .nodes
        .iter()
        .position(|node| node.id == binding.reference.node)
        .ok_or(ProjectionError::Snapshot)?;
    let question = definition.nodes[index]
        .turn
        .questions
        .iter()
        .find(|question| question.id == binding.reference.question)
        .ok_or(ProjectionError::Snapshot)?;
    let output = references.output(index);
    let outcome = output.and_then(|output| output.outcome.outcome_for(&question.id));
    Ok(ProjectedOutput {
        name: binding.name.clone(),
        value: binding.value.clone(),
        reference: Some(binding.reference.clone()),
        question: Some(question.clone()),
        provider: output.map(|output| output.outcome.provider.clone()),
        confidence: outcome.and_then(confidence),
        selected_choice: outcome.and_then(selected_choice),
        defaulted: binding.defaulted,
        required_missing: binding.required_missing,
        inspection: Some(references.node(index)),
    })
}

fn confidence(outcome: &QuestionOutcome) -> Option<Confidence> {
    match outcome {
        QuestionOutcome::Answered { confidence, .. } => confidence.clone(),
        QuestionOutcome::Refused { .. } => None,
    }
}

/// The chosen option of a choice answer, with the probability it was given.
fn selected_choice(outcome: &QuestionOutcome) -> Option<SelectedChoice> {
    let QuestionOutcome::Answered {
        answer: answer @ Answer::Choice { selected, .. },
        ..
    } = outcome
    else {
        return None;
    };
    answer
        .selected_probability()
        .map(|probability| SelectedChoice {
            id: selected.clone(),
            probability,
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        data::Data,
        decision::AnswerSpace,
        identity::Id,
        outcome::TurnOutcome,
        workflow::{
            NodeStatus, OutputBinding, RunStatus, compile,
            reference::{Absence, MissingPolicy, ReferenceValue, ResultField, ResultReference},
            schedule::{NodeOutput, NodeProgress, ScheduleResult, project_outputs},
        },
    };
    fn definition() -> WorkflowSpec {
        serde_json::from_str(include_str!("../../../fixtures/contracts/workflow.json")).unwrap()
    }
    fn result(definition: &WorkflowSpec, statuses: &[NodeStatus]) -> WorkflowResult {
        let session = Id::generate();
        let nodes = statuses
            .iter()
            .map(|status| NodeProgress {
                status: *status,
                output: (*status == NodeStatus::Completed).then(|| NodeOutput {
                    outcome: serde_json::from_str::<TurnOutcome>(include_str!(
                        "../../../fixtures/contracts/decision-outcome.json"
                    ))
                    .unwrap(),
                    source: Some(crate::history::SourcePoint {
                        session,
                        frontier: vec![Id::generate()],
                    }),
                    evaluation: None,
                }),
                error: None,
                inputs: vec![],
                ancestors: vec![],
            })
            .collect::<Vec<_>>();
        let graph = compile::compile(definition).unwrap();
        let outputs = project_outputs(&graph, &nodes).unwrap();
        let status = if statuses
            .iter()
            .any(|status| matches!(status, NodeStatus::Failed | NodeStatus::Blocked))
            || outputs.iter().any(|output| output.required_missing)
        {
            RunStatus::Partial
        } else {
            RunStatus::Completed
        };
        WorkflowResult {
            request_id: None,
            run: Some(Id::generate()),
            session: Some(session),
            persisted: true,
            replayed: false,
            sources: vec![],
            frontier: Some(vec![]),
            result: ScheduleResult {
                status,
                nodes,
                start_order: vec![],
                completion_order: vec![],
                outputs,
            },
        }
    }
    fn output(
        name: &str,
        node: &str,
        question: &str,
        field: ResultField,
        pointer: Option<&str>,
    ) -> OutputBinding {
        OutputBinding {
            name: name.into(),
            reference: ResultReference {
                node: node.into(),
                question: question.into(),
                field,
                pointer: pointer.map(str::to_owned),
                on_missing: MissingPolicy::Optional,
            },
        }
    }
    #[test]
    fn declared_fractional_score_retains_scale_and_record_links_without_intermediate_bodies() {
        let definition = definition();
        let result = result(
            &definition,
            &[
                NodeStatus::Completed,
                NodeStatus::Completed,
                NodeStatus::Skipped,
                NodeStatus::Completed,
                NodeStatus::Completed,
            ],
        );
        let report = workflow(&definition, &result, None).unwrap();
        assert_eq!(report.outputs.len(), 2);
        let rating = &report.outputs[0];
        assert_eq!(
            rating.value,
            ReferenceValue::Present {
                value: Data::new(serde_json::json!(1.43)).unwrap()
            }
        );
        assert!(
            matches!(&rating.question.as_ref().unwrap().answer_space,AnswerSpace::Score {levels} if levels.len()==3)
        );
        assert_eq!(
            rating.question.as_ref().unwrap(),
            &definition.nodes[4].turn.questions[2]
        );
        assert_eq!(
            rating.inspection.as_ref().unwrap().turn,
            report.nodes[4].turn
        );
        assert!(rating.inspection.as_ref().unwrap().turn.is_some());
        assert_eq!(
            report.outputs[1].value,
            ReferenceValue::Absent {
                reason: Absence::Skipped
            }
        );
        assert!(!report.outputs[1].required_missing);
        assert!(report.nodes.iter().all(|node| node.outcome.is_none()));
        let mut confidence_definition = definition.clone();
        confidence_definition.outputs = vec![output(
            "certainty",
            "final",
            "travel",
            ResultField::Confidence,
            None,
        )];
        let confidence_result = self::result(&confidence_definition, &[NodeStatus::Completed; 5]);
        let confidence_report = workflow(&confidence_definition, &confidence_result, None).unwrap();
        assert_eq!(
            confidence_report.outputs[0]
                .confidence
                .as_ref()
                .unwrap()
                .meaning,
            "fixture native certainty"
        );
        assert_eq!(
            confidence_report.outputs[0]
                .provider
                .as_ref()
                .unwrap()
                .model,
            "offline"
        );

        assert!(
            !serde_json::to_string(&report)
                .unwrap()
                .contains("probabilities")
        );
    }
    #[test]
    fn fallback_keeps_terminal_answers_refusal_and_all_skipped_absence() {
        let mut definition = definition();
        definition.outputs.clear();
        let result = result(
            &definition,
            &[
                NodeStatus::Completed,
                NodeStatus::Completed,
                NodeStatus::Skipped,
                NodeStatus::Completed,
                NodeStatus::Completed,
            ],
        );
        let report = workflow(&definition, &result, None).unwrap();
        assert_eq!(report.outputs.len(), 4);
        assert!(
            report
                .outputs
                .iter()
                .all(|output| output.reference.as_ref().unwrap().node == "final")
        );
        let ReferenceValue::Present { value } = &report.outputs[3].value else {
            panic!("whole refusal")
        };
        assert_eq!(value.value()["status"], "refused");
        let skipped = self::result(&definition, &[NodeStatus::Skipped; 5]);
        let report = workflow(&definition, &skipped, None).unwrap();
        assert_eq!(report.status, RunStatus::Completed);
        assert!(report.outputs.iter().all(|output| output.value
            == ReferenceValue::Absent {
                reason: Absence::Skipped
            }));
    }
    #[test]
    fn present_null_required_failure_and_typed_whole_refusal_stay_distinct() {
        let mut definition = definition();
        definition.outputs = vec![
            output(
                "confidence",
                "final",
                "quality",
                ResultField::Outcome,
                Some("/confidence"),
            ),
            output("refusal", "final", "private", ResultField::Outcome, None),
            output(
                "refused_answer",
                "final",
                "private",
                ResultField::Answer,
                None,
            ),
        ];
        definition.outputs[2].reference.on_missing = MissingPolicy::Required;
        let result = result(&definition, &[NodeStatus::Completed; 5]);
        let report = workflow(&definition, &result, None).unwrap();
        assert_eq!(report.status, RunStatus::Partial);
        assert_eq!(
            report.outputs[0].value,
            ReferenceValue::Present {
                value: Data::new(serde_json::Value::Null).unwrap()
            }
        );
        assert!(matches!(
            report.outputs[1].value,
            ReferenceValue::Present { .. }
        ));
        assert_eq!(
            report.outputs[2].value,
            ReferenceValue::Absent {
                reason: Absence::Refused
            }
        );
        assert!(report.outputs[2].required_missing);
        let mut ephemeral = result;
        ephemeral.run = None;
        ephemeral.session = None;
        ephemeral.frontier = None;
        ephemeral.persisted = false;
        let report = workflow(&definition, &ephemeral, None).unwrap();
        assert!(report.nodes.iter().all(|node| node.turn.is_none()));
        assert!(
            report
                .outputs
                .iter()
                .all(|output| output.inspection.as_ref().unwrap().turn.is_none())
        );
    }
}
