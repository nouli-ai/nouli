//! Typed runtime selection and conditions, preserving absence and null.
use crate::{
    codec::{self, CodecError},
    data::{Data, DataError},
    outcome::{Answer, QuestionOutcome, TurnOutcome},
    workflow::{
        NodeStatus,
        reference::{
            Absence, Comparison, Condition, MissingPolicy, Operand, ReferenceValue, ResultField,
            ResultReference,
        },
    },
};
use std::cmp::Ordering;
use thiserror::Error;
#[derive(Debug, Error)]
pub enum ResolveError {
    #[error(transparent)]
    Codec(#[from] CodecError),
    #[error(transparent)]
    Data(#[from] DataError),
    #[error("reference source has not settled")]
    Unsettled,
    #[error(transparent)]
    Contract(#[from] crate::workflow::WorkflowError),
    #[error("completed source has an incomplete outcome")]
    Incomplete,
    #[error("required reference is unavailable ({reason:?})")]
    Required { reason: Absence },
    #[error("condition operand is absent or incompatible")]
    Operand,
}
fn absent(reason: Absence) -> ReferenceValue {
    ReferenceValue::Absent { reason }
}
fn present(value: serde_json::Value) -> Result<ReferenceValue, ResolveError> {
    Ok(ReferenceValue::Present {
        value: Data::new(value)?,
    })
}
/// Selects a reference's raw value from a settled node, reporting skipped,
/// failed, blocked, interrupted, refused, or missing sources as absent.
///
/// # Errors
///
/// - Returns [`ResolveError::Contract`] if the reference is malformed.
/// - Returns [`ResolveError::Unsettled`] if the node is pending or running.
/// - Returns [`ResolveError::Incomplete`] if a completed node lacks an outcome
///   for the question or the selected choice's probability.
/// - Returns [`ResolveError::Operand`] if the field does not apply to the
///   answer kind.
/// - Returns [`ResolveError::Codec`] or [`ResolveError::Data`] if the selected
///   value cannot be encoded as data.
pub fn select(
    reference: &ResultReference,
    status: NodeStatus,
    outcome: Option<&TurnOutcome>,
) -> Result<ReferenceValue, ResolveError> {
    reference.validate()?;
    match status {
        NodeStatus::Pending | NodeStatus::Running => return Err(ResolveError::Unsettled),
        NodeStatus::Skipped => return Ok(absent(Absence::Skipped)),
        NodeStatus::Failed => return Ok(absent(Absence::Failed)),
        NodeStatus::Blocked => return Ok(absent(Absence::Blocked)),
        NodeStatus::Interrupted => return Ok(absent(Absence::Interrupted)),
        NodeStatus::Completed => {}
    }
    let outcome = outcome
        .ok_or(ResolveError::Incomplete)?
        .outcomes
        .iter()
        .find(|outcome| outcome.question_id() == reference.question)
        .ok_or(ResolveError::Incomplete)?;
    let value = if reference.field == ResultField::Outcome {
        codec::to_value(outcome)?
    } else {
        let QuestionOutcome::Answered {
            answer, confidence, ..
        } = outcome
        else {
            return Ok(absent(Absence::Refused));
        };
        match (reference.field, answer) {
            (ResultField::Answer, _) => codec::to_value(answer)?,
            (ResultField::Selected, Answer::Choice { selected, .. }) => {
                serde_json::Value::String(selected.clone())
            }
            (
                ResultField::Probability,
                Answer::Choice {
                    selected,
                    distribution,
                },
            ) => codec::to_value(
                &distribution
                    .iter()
                    .find(|entry| entry.option_id == *selected)
                    .ok_or(ResolveError::Incomplete)?
                    .probability,
            )?,
            (ResultField::Probability, Answer::Noul { probability }) => {
                codec::to_value(probability)?
            }
            (ResultField::Score, Answer::Score { value, .. }) => codec::to_value(value)?,
            (ResultField::Confidence, _) => {
                let Some(value) = confidence.as_ref().and_then(|confidence| confidence.value)
                else {
                    return Ok(absent(Absence::MissingField));
                };
                codec::to_value(&value)?
            }
            _ => return Err(ResolveError::Operand),
        }
    };
    if let Some(pointer) = &reference.pointer {
        match value.pointer(pointer) {
            Some(value) => present(value.clone()),
            None => Ok(absent(
                if matches!(outcome, QuestionOutcome::Refused { .. }) {
                    Absence::Refused
                } else {
                    Absence::MissingField
                },
            )),
        }
    } else {
        present(value)
    }
}
/// Applies a reference's missing-value policy to a selected value.
///
/// # Errors
///
/// Returns [`ResolveError::Required`] if the value is absent and the policy
/// requires it.
pub fn apply(
    reference: &ResultReference,
    value: ReferenceValue,
) -> Result<ReferenceValue, ResolveError> {
    match (&reference.on_missing, &value) {
        (MissingPolicy::Required, ReferenceValue::Absent { reason }) => {
            Err(ResolveError::Required { reason: *reason })
        }
        (MissingPolicy::Default { value }, ReferenceValue::Absent { .. }) => {
            Ok(ReferenceValue::Present {
                value: value.clone(),
            })
        }
        _ => Ok(value),
    }
}
fn unsigned_float(integer: u64, float: f64) -> Ordering {
    if float < 0.0 {
        return Ordering::Greater;
    }
    if float >= 18_446_744_073_709_551_616.0 {
        return Ordering::Less;
    }
    let floor = float.floor();
    match integer.cmp(&(floor as u64)) {
        Ordering::Equal if float > floor => Ordering::Less,
        other => other,
    }
}
fn integer_float(integer: &serde_json::Number, float: f64) -> Result<Ordering, ResolveError> {
    if let Some(integer) = integer.as_u64() {
        Ok(unsigned_float(integer, float))
    } else {
        let integer = integer.as_i64().ok_or(ResolveError::Operand)?;
        if float >= 0.0 {
            Ok(Ordering::Less)
        } else {
            Ok(unsigned_float(integer.unsigned_abs(), -float).reverse())
        }
    }
}
fn numbers(
    left: &serde_json::Number,
    right: &serde_json::Number,
) -> Result<Ordering, ResolveError> {
    if left.is_f64() && right.is_f64() {
        return left
            .as_f64()
            .and_then(|left| left.partial_cmp(&right.as_f64()?))
            .ok_or(ResolveError::Operand);
    }
    if let Some(float) = right.as_f64().filter(|_| right.is_f64()) {
        return integer_float(left, float);
    }
    if let Some(float) = left.as_f64().filter(|_| left.is_f64()) {
        return Ok(integer_float(right, float)?.reverse());
    }
    match (left.as_i64(), right.as_i64()) {
        (Some(left), Some(right)) => Ok(left.cmp(&right)),
        (Some(left), None) if left < 0 => Ok(Ordering::Less),
        (None, Some(right)) if right < 0 => Ok(Ordering::Greater),
        _ => Ok(left
            .as_u64()
            .ok_or(ResolveError::Operand)?
            .cmp(&right.as_u64().ok_or(ResolveError::Operand)?)),
    }
}
fn compare(left: &Data, right: &Data, comparison: Comparison) -> Result<bool, ResolveError> {
    let ordering = match (left.value(), right.value()) {
        (serde_json::Value::Number(left), serde_json::Value::Number(right)) => {
            Some(numbers(left, right)?)
        }
        (serde_json::Value::String(left), serde_json::Value::String(right)) => {
            Some(left.cmp(right))
        }
        _ => None,
    };
    if matches!(comparison, Comparison::Eq | Comparison::Ne) {
        let equal = if let Some(ordering) = ordering {
            ordering == Ordering::Equal
        } else {
            codec::fingerprint(left)? == codec::fingerprint(right)?
        };
        return Ok(if comparison == Comparison::Eq {
            equal
        } else {
            !equal
        });
    }
    let ordering = ordering.ok_or(ResolveError::Operand)?;
    Ok(match comparison {
        Comparison::Lt => ordering.is_lt(),
        Comparison::Le => ordering.is_le(),
        Comparison::Gt => ordering.is_gt(),
        Comparison::Ge => ordering.is_ge(),
        _ => return Err(ResolveError::Operand),
    })
}
pub(super) fn evaluate(
    condition: &Condition,
    select: &impl Fn(&ResultReference) -> Result<ReferenceValue, ResolveError>,
) -> Result<bool, ResolveError> {
    fn operand(
        value: &Operand,
        select: &impl Fn(&ResultReference) -> Result<ReferenceValue, ResolveError>,
    ) -> Result<Data, ResolveError> {
        match value {
            Operand::Literal { value } => Ok(value.clone()),
            Operand::Reference { reference } => match apply(reference, select(reference)?)? {
                ReferenceValue::Present { value } => Ok(value),
                ReferenceValue::Absent { .. } => Err(ResolveError::Operand),
            },
        }
    }
    match condition {
        Condition::Compare {
            left,
            comparison,
            right,
        } => compare(
            &operand(left, select)?,
            &operand(right, select)?,
            *comparison,
        ),
        Condition::Not { condition } => Ok(!evaluate(condition, select)?),
        Condition::All { conditions } | Condition::Any { conditions } => {
            let all = matches!(condition, Condition::All { .. });
            let mut unknown = None;
            for condition in conditions {
                match evaluate(condition, select) {
                    Ok(value) if value != all => return Ok(!all),
                    Ok(_) => {}
                    Err(error) => unknown = Some(error),
                }
            }
            if let Some(error) = unknown {
                Err(error)
            } else {
                Ok(all)
            }
        }
        Condition::Exists { reference } => {
            Ok(matches!(select(reference)?, ReferenceValue::Present { .. }))
        }
        Condition::Refused { node, question } => {
            let reference = ResultReference {
                node: node.clone(),
                question: question.clone(),
                field: ResultField::Outcome,
                pointer: None,
                on_missing: MissingPolicy::Optional,
            };
            Ok(
                matches!(select(&reference)?,ReferenceValue::Present {value} if value.value().get("status").and_then(|value|value.as_str())==Some("refused")),
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn outcome() -> TurnOutcome {
        serde_json::from_str(include_str!(
            "../../../fixtures/contracts/decision-outcome.json"
        ))
        .unwrap()
    }
    fn reference(question: &str, field: ResultField) -> ResultReference {
        ResultReference {
            node: "node".into(),
            question: question.into(),
            field,
            pointer: None,
            on_missing: MissingPolicy::Required,
        }
    }
    #[test]
    fn usable_siblings_refusals_absence_and_present_null_remain_distinct() {
        let outcome = outcome();
        let answered = reference("travel", ResultField::Selected);
        assert!(
            matches!(select(&answered,NodeStatus::Completed,Some(&outcome)).unwrap(),ReferenceValue::Present {value} if value.value()=="train")
        );
        assert!(matches!(
            select(
                &reference("private", ResultField::Probability),
                NodeStatus::Completed,
                Some(&outcome)
            )
            .unwrap(),
            ReferenceValue::Absent {
                reason: Absence::Refused
            }
        ));
        let mut native = reference("on_time", ResultField::Outcome);
        native.pointer = Some("/confidence".into());
        let value = select(&native, NodeStatus::Completed, Some(&outcome)).unwrap();
        assert!(matches!(&value,ReferenceValue::Present {value} if value.value().is_null()));
        assert!(matches!(
            apply(&native, value).unwrap(),
            ReferenceValue::Present { .. }
        ));
        let mut missing = reference("quality", ResultField::Score);
        assert!(matches!(
            apply(
                &missing,
                select(&missing, NodeStatus::Skipped, None).unwrap()
            ),
            Err(ResolveError::Required {
                reason: Absence::Skipped
            })
        ));
        missing.on_missing = MissingPolicy::Default {
            value: Data::new(serde_json::json!(1.43)).unwrap(),
        };
        assert!(
            matches!(apply(&missing,select(&missing,NodeStatus::Failed,None).unwrap()).unwrap(),ReferenceValue::Present {value} if value.value()==1.43)
        );
    }
    #[test]
    fn numeric_comparisons_preserve_full_width_integers_and_fractional_scores() {
        let data = |value| Data::new(value).unwrap();
        assert!(
            compare(
                &data(serde_json::json!(u64::MAX)),
                &data(serde_json::json!(18_446_744_073_709_551_616.0)),
                Comparison::Lt
            )
            .unwrap()
        );
        assert!(
            compare(
                &data(serde_json::json!(9_007_199_254_740_993_u64)),
                &data(serde_json::json!(9_007_199_254_740_992.0)),
                Comparison::Gt
            )
            .unwrap()
        );
        assert!(
            compare(
                &data(serde_json::json!(i64::MIN)),
                &data(serde_json::json!(u64::MAX)),
                Comparison::Lt
            )
            .unwrap()
        );
        assert!(
            compare(
                &data(serde_json::json!(-1)),
                &data(serde_json::json!(-1.5)),
                Comparison::Gt
            )
            .unwrap()
        );
        assert!(
            compare(
                &data(serde_json::json!(1.43)),
                &data(serde_json::json!(1)),
                Comparison::Gt
            )
            .unwrap()
        );
        assert!(
            compare(
                &data(serde_json::json!(1)),
                &data(serde_json::json!(1.0)),
                Comparison::Eq
            )
            .unwrap()
        );
    }
    #[test]
    fn conditions_need_values_unless_presence_or_explicit_logic_resolves_absence() {
        let reference = reference("quality", ResultField::Score);
        let condition = Condition::Compare {
            left: Operand::Reference {
                reference: reference.clone(),
            },
            comparison: Comparison::Ge,
            right: Operand::Literal {
                value: Data::new(serde_json::json!(1.4)).unwrap(),
            },
        };
        let missing = |_: &ResultReference| {
            Ok(ReferenceValue::Absent {
                reason: Absence::Refused,
            })
        };
        assert!(evaluate(&condition, &missing).is_err());
        assert!(
            !evaluate(
                &Condition::Exists {
                    reference: reference.clone()
                },
                &missing
            )
            .unwrap()
        );
        let exists = Condition::Exists { reference };
        assert!(
            !evaluate(
                &Condition::All {
                    conditions: vec![condition.clone(), exists.clone()]
                },
                &missing
            )
            .unwrap()
        );
        assert!(
            evaluate(
                &Condition::Any {
                    conditions: vec![
                        condition,
                        Condition::Not {
                            condition: Box::new(exists)
                        }
                    ]
                },
                &missing
            )
            .unwrap()
        );
    }
}
