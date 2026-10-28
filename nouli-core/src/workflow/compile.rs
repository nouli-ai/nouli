//! Pure acyclic graph compilation, separate from execution progress.
pub mod types;
use crate::{
    data::Input,
    workflow::{
        OutputBinding, WorkflowError, WorkflowSpec,
        reference::{Condition, MissingPolicy, Operand, ResultField, ResultReference},
    },
};
use serde::Serialize;
use std::collections::{BTreeSet, HashMap, HashSet};
use thiserror::Error;
use types::ValueType;

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CompiledReference {
    pub reference: ResultReference,
    pub node: usize,
    pub value_type: ValueType,
}
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CompiledBinding {
    pub name: String,
    pub selector: CompiledReference,
}
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct NodePlan {
    pub dependencies: Vec<usize>,
    pub consumers: Vec<usize>,
    pub ancestors: Vec<usize>,
    pub inputs: Vec<CompiledBinding>,
    pub condition_references: Vec<CompiledReference>,
}
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CompiledWorkflow {
    spec: WorkflowSpec,
    nodes: Vec<NodePlan>,
    order: Vec<usize>,
    outputs: Vec<CompiledBinding>,
    fallback_outputs: Vec<usize>,
}
impl CompiledWorkflow {
    pub fn definition(&self) -> &WorkflowSpec {
        &self.spec
    }
    pub fn nodes(&self) -> &[NodePlan] {
        &self.nodes
    }
    pub fn order(&self) -> &[usize] {
        &self.order
    }
    pub fn outputs(&self) -> &[CompiledBinding] {
        &self.outputs
    }
    pub fn fallback_outputs(&self) -> &[usize] {
        &self.fallback_outputs
    }
    pub fn base_state(&self) -> &[Input] {
        &self.spec.state
    }
}
#[derive(Debug, Error, PartialEq)]
pub enum CompileError {
    #[error(transparent)]
    Contract(#[from] WorkflowError),
    #[error("dependency does not identify a node")]
    UnknownDependency,
    #[error("workflow contains self or cyclic dependencies")]
    Cycle,
    #[error("reference is not a declared ancestor")]
    ForeignReference,
    #[error("reference does not identify a declared node or question")]
    UnknownReference,
    #[error("reference field or pointer is incompatible with its answer type")]
    Selector,
    #[error("conditional reference needs an optional/default policy or an explicit presence test")]
    ConditionalMissing,
    #[error("reference default has an incompatible type")]
    DefaultType,
    #[error("condition compares incompatible types")]
    ComparisonType,
}
struct Scope<'a> {
    spec: &'a WorkflowSpec,
    index: &'a HashMap<&'a str, usize>,
    ancestors: Option<&'a HashSet<usize>>,
}
impl Scope<'_> {
    fn reference(
        &self,
        reference: &ResultReference,
        handles_missing: bool,
    ) -> Result<CompiledReference, CompileError> {
        reference.validate()?;
        let index = *self
            .index
            .get(reference.node.as_str())
            .ok_or(CompileError::UnknownReference)?;
        if self
            .ancestors
            .is_some_and(|ancestors| !ancestors.contains(&index))
        {
            return Err(CompileError::ForeignReference);
        }
        let node = &self.spec.nodes[index];
        let question = node
            .turn
            .questions
            .iter()
            .find(|question| question.id == reference.question)
            .ok_or(CompileError::UnknownReference)?;
        if node.when.is_some()
            && matches!(reference.on_missing, MissingPolicy::Required)
            && !handles_missing
        {
            return Err(CompileError::ConditionalMissing);
        }
        let value_type = types::reference_type(reference, question)?;
        if let MissingPolicy::Default { value } = &reference.on_missing
            && !value_type.accepts(value.value())
        {
            return Err(CompileError::DefaultType);
        }
        Ok(CompiledReference {
            reference: reference.clone(),
            node: index,
            value_type,
        })
    }
    fn bindings(&self, bindings: &[OutputBinding]) -> Result<Vec<CompiledBinding>, CompileError> {
        bindings
            .iter()
            .map(|binding| {
                Ok(CompiledBinding {
                    name: binding.name.clone(),
                    selector: self.reference(&binding.reference, false)?,
                })
            })
            .collect()
    }
    fn operand(
        &self,
        operand: &Operand,
        refs: &mut Vec<CompiledReference>,
    ) -> Result<ValueType, CompileError> {
        match operand {
            Operand::Literal { value } => Ok(ValueType::of(value.value())),
            Operand::Reference { reference } => {
                let reference = self.reference(reference, false)?;
                let ty = reference.value_type;
                refs.push(reference);
                Ok(ty)
            }
        }
    }
    fn condition(
        &self,
        condition: &Condition,
        refs: &mut Vec<CompiledReference>,
    ) -> Result<(), CompileError> {
        match condition {
            Condition::Compare {
                left,
                comparison,
                right,
            } => {
                let left = self.operand(left, refs)?;
                let right = self.operand(right, refs)?;
                if !types::comparable(left, right, *comparison) {
                    return Err(CompileError::ComparisonType);
                }
            }
            Condition::All { conditions } | Condition::Any { conditions } => {
                for condition in conditions {
                    self.condition(condition, refs)?;
                }
            }
            Condition::Not { condition } => self.condition(condition, refs)?,
            Condition::Exists { reference } => refs.push(self.reference(reference, true)?),
            Condition::Refused { node, question } => refs.push(self.reference(
                &ResultReference {
                    node: node.clone(),
                    question: question.clone(),
                    field: ResultField::Outcome,
                    pointer: None,
                    on_missing: MissingPolicy::Optional,
                },
                true,
            )?),
        }
        Ok(())
    }
}

pub fn compile(spec: &WorkflowSpec) -> Result<CompiledWorkflow, CompileError> {
    spec.validate()?;
    if [&spec.model.provider, &spec.model.model]
        .iter()
        .any(|field| field.as_ref().is_some_and(|value| value.trim().is_empty()))
    {
        return Err(WorkflowError::Shape("workflow model").into());
    }
    let index: HashMap<_, _> = spec
        .nodes
        .iter()
        .enumerate()
        .map(|(index, node)| (node.id.as_str(), index))
        .collect();
    let mut nodes: Vec<_> = (0..spec.nodes.len())
        .map(|_| NodePlan {
            dependencies: vec![],
            consumers: vec![],
            ancestors: vec![],
            inputs: vec![],
            condition_references: vec![],
        })
        .collect();
    for (id, node) in spec.nodes.iter().enumerate() {
        if node.required_records.iter().collect::<HashSet<_>>().len() != node.required_records.len()
        {
            return Err(WorkflowError::Shape("required records").into());
        }
        for dependency in &node.dependencies {
            let dependency = *index
                .get(dependency.as_str())
                .ok_or(CompileError::UnknownDependency)?;
            if dependency == id {
                return Err(CompileError::Cycle);
            }
            nodes[id].dependencies.push(dependency);
            nodes[dependency].consumers.push(id);
        }
    }
    let mut indegree: Vec<_> = nodes.iter().map(|node| node.dependencies.len()).collect();
    let mut ready: BTreeSet<_> = indegree
        .iter()
        .enumerate()
        .filter(|(_, n)| **n == 0)
        .map(|(i, _)| i)
        .collect();
    let mut order = Vec::new();
    while let Some(id) = ready.pop_first() {
        order.push(id);
        for child in &nodes[id].consumers {
            indegree[*child] -= 1;
            if indegree[*child] == 0 {
                ready.insert(*child);
            }
        }
    }
    if order.len() != nodes.len() {
        return Err(CompileError::Cycle);
    }
    for id in &order {
        let mut ancestors = HashSet::new();
        for parent in &nodes[*id].dependencies {
            ancestors.insert(*parent);
            ancestors.extend(nodes[*parent].ancestors.iter().copied());
        }
        let scope = Scope {
            spec,
            index: &index,
            ancestors: Some(&ancestors),
        };
        nodes[*id].inputs = scope.bindings(&spec.nodes[*id].inputs)?;
        if let Some(condition) = &spec.nodes[*id].when {
            scope.condition(condition, &mut nodes[*id].condition_references)?;
        }
        nodes[*id].ancestors = order
            .iter()
            .filter(|parent| ancestors.contains(parent))
            .copied()
            .collect();
    }
    let outputs = Scope {
        spec,
        index: &index,
        ancestors: None,
    }
    .bindings(&spec.outputs)?;
    let fallback_outputs = if outputs.is_empty() {
        nodes
            .iter()
            .enumerate()
            .filter(|(_, node)| node.consumers.is_empty())
            .map(|(i, _)| i)
            .collect()
    } else {
        vec![]
    };
    Ok(CompiledWorkflow {
        spec: spec.clone(),
        nodes,
        order,
        outputs,
        fallback_outputs,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{data::Data, workflow::reference::Comparison};
    fn fixture() -> WorkflowSpec {
        serde_json::from_str(include_str!("../../../fixtures/contracts/workflow.json")).unwrap()
    }
    fn binding(node: &str, field: ResultField, policy: MissingPolicy) -> OutputBinding {
        OutputBinding {
            name: "input".into(),
            reference: ResultReference {
                node: node.into(),
                question: "quality".into(),
                field,
                pointer: None,
                on_missing: policy,
            },
        }
    }
    #[test]
    fn dinner_graph_compiles_stably_with_separate_base_and_node_state() {
        let mut spec = fixture();
        spec.state = vec![Input::Text {
            text: "shared frozen base".into(),
        }];
        let compiled = compile(&spec).unwrap();
        assert_eq!(compiled.order(), &[0, 1, 2, 3, 4]);
        assert_eq!(compiled.nodes()[0].consumers, vec![1, 2]);
        assert_eq!(compiled.nodes()[3].dependencies, vec![1, 2]);
        assert_eq!(compiled.nodes()[4].ancestors, vec![0, 1, 2, 3]);
        assert_eq!(
            compiled.nodes()[1].condition_references[0].value_type,
            ValueType::Number
        );
        assert_eq!(compiled.base_state(), spec.state);
        assert_eq!(
            compiled.definition().nodes[0].turn.state,
            spec.nodes[0].turn.state
        );
        assert_eq!(compiled.definition().nodes[0].turn.questions.len(), 4);
        assert_eq!(compiled.outputs()[0].name, "rating");
        assert!(compiled.fallback_outputs().is_empty());
        assert_eq!(
            serde_json::to_value(&compiled).unwrap(),
            serde_json::to_value(compile(&spec).unwrap()).unwrap()
        );
        spec.outputs.clear();
        assert_eq!(compile(&spec).unwrap().fallback_outputs(), &[4]);
    }
    #[test]
    fn missing_cyclic_self_and_foreign_dependencies_fail_before_execution() {
        let mut spec = fixture();
        spec.nodes[0].dependencies = vec!["final".into()];
        assert_eq!(compile(&spec), Err(CompileError::Cycle));
        let mut spec = fixture();
        spec.nodes[0].dependencies = vec!["initial".into()];
        assert_eq!(compile(&spec), Err(CompileError::Cycle));
        let mut spec = fixture();
        spec.nodes[0].dependencies = vec!["unknown".into()];
        assert_eq!(compile(&spec), Err(CompileError::UnknownDependency));
        let mut spec = fixture();
        spec.nodes[1].inputs = vec![binding("low", ResultField::Score, MissingPolicy::Optional)];
        assert_eq!(compile(&spec), Err(CompileError::ForeignReference));
        let mut spec = fixture();
        let duplicate = spec.nodes[0].turn.questions[0].clone();
        spec.nodes[0].turn.questions.push(duplicate);
        assert!(matches!(compile(&spec), Err(CompileError::Contract(_))));
    }
    #[test]
    fn typed_thresholds_and_conditional_missing_policies_are_explicit() {
        let mut spec = fixture();
        spec.nodes[3].inputs = vec![binding("high", ResultField::Score, MissingPolicy::Required)];
        assert_eq!(compile(&spec), Err(CompileError::ConditionalMissing));
        spec.nodes[3].inputs[0].reference.on_missing = MissingPolicy::Optional;
        compile(&spec).unwrap();
        spec.nodes[3].inputs[0].reference.on_missing = MissingPolicy::Default {
            value: Data::new(serde_json::json!("not a score")).unwrap(),
        };
        assert_eq!(compile(&spec), Err(CompileError::DefaultType));
        spec.nodes[3].inputs[0].reference.on_missing = MissingPolicy::Default {
            value: Data::new(serde_json::json!(1.43)).unwrap(),
        };
        compile(&spec).unwrap();
        let mut spec = fixture();
        let Condition::Compare { right, .. } = spec.nodes[1].when.as_mut().unwrap() else {
            panic!("expected threshold")
        };
        *right = Operand::Literal {
            value: Data::new(serde_json::json!("1.4")).unwrap(),
        };
        assert_eq!(compile(&spec), Err(CompileError::ComparisonType));
        let mut spec = fixture();
        spec.nodes[3].when = Some(Condition::Exists {
            reference: binding("high", ResultField::Score, MissingPolicy::Required).reference,
        });
        compile(&spec).unwrap();
        spec.outputs = vec![binding("high", ResultField::Score, MissingPolicy::Required)];
        assert_eq!(compile(&spec), Err(CompileError::ConditionalMissing));
    }
    #[test]
    fn nullable_native_metadata_keeps_known_types_and_present_null_support() {
        let mut spec = fixture();
        let mut input = binding(
            "initial",
            ResultField::Outcome,
            MissingPolicy::Default {
                value: Data::new(serde_json::json!("wrong")).unwrap(),
            },
        );
        input.reference.pointer = Some("/confidence/value".into());
        spec.nodes[1].inputs = vec![input];
        assert_eq!(compile(&spec), Err(CompileError::DefaultType));
        for value in [serde_json::json!({"wrong":1}), serde_json::json!([])] {
            spec.nodes[1].inputs[0].reference.on_missing = MissingPolicy::Default {
                value: Data::new(value).unwrap(),
            };
            assert_eq!(compile(&spec), Err(CompileError::DefaultType));
        }
        for value in [serde_json::json!(null), serde_json::json!(0.8)] {
            spec.nodes[1].inputs[0].reference.on_missing = MissingPolicy::Default {
                value: Data::new(value).unwrap(),
            };
            compile(&spec).unwrap();
        }
        let mut reference = spec.nodes[1].inputs[0].reference.clone();
        reference.on_missing = MissingPolicy::Required;
        spec.nodes[1].when = Some(Condition::Compare {
            left: Operand::Reference {
                reference: reference.clone(),
            },
            comparison: Comparison::Eq,
            right: Operand::Literal {
                value: Data::new(serde_json::json!("wrong")).unwrap(),
            },
        });
        assert_eq!(compile(&spec), Err(CompileError::ComparisonType));
        spec.nodes[1].when = Some(Condition::Compare {
            left: Operand::Reference { reference },
            comparison: Comparison::Eq,
            right: Operand::Literal {
                value: Data::new(serde_json::json!(null)).unwrap(),
            },
        });
        compile(&spec).unwrap();
        spec.nodes[1].inputs[0].reference.pointer = Some("/confidence".into());
        spec.nodes[1].inputs[0].reference.on_missing = MissingPolicy::Default {
            value: Data::new(serde_json::json!(42)).unwrap(),
        };
        assert_eq!(compile(&spec), Err(CompileError::DefaultType));
    }

    #[test]
    fn whole_outcome_refusals_and_metadata_are_addressable_without_expressions() {
        let mut spec = fixture();
        let mut reference =
            binding("initial", ResultField::Outcome, MissingPolicy::Required).reference;
        reference.pointer = Some("/status".into());
        spec.nodes[1].when = Some(Condition::Compare {
            left: Operand::Reference { reference },
            comparison: Comparison::Eq,
            right: Operand::Literal {
                value: Data::new(serde_json::json!("refused")).unwrap(),
            },
        });
        compile(&spec).unwrap();
        spec.nodes[1].inputs = vec![binding(
            "initial",
            ResultField::Outcome,
            MissingPolicy::Required,
        )];
        let compiled = compile(&spec).unwrap();
        assert_eq!(
            compiled.nodes()[1].inputs[0].selector.value_type,
            ValueType::Object
        );
        spec.nodes[1].inputs[0].reference.pointer = Some("/answer/not_a_field".into());
        assert_eq!(compile(&spec), Err(CompileError::Selector));
        let mut value = serde_json::to_value(fixture()).unwrap();
        value["nodes"][1]["when"] = serde_json::json!({"op":"expression","code":"run_shell()"});
        assert!(serde_json::from_value::<WorkflowSpec>(value).is_err());
    }
}
