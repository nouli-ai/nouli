//! Static selector schemas and comparison compatibility, without execution.
use super::CompileError;
use crate::{
    decision::{AnswerSpace, Question},
    workflow::reference::{Comparison, ResultField, ResultReference},
};
use serde::Serialize;
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ValueType {
    Number,
    String,
    Boolean,
    Null,
    Object,
    Array,
    NullableNumber,
    NullableObject,
}
impl ValueType {
    #[must_use]
    pub fn of(value: &serde_json::Value) -> Self {
        match value {
            serde_json::Value::Number(_) => Self::Number,
            serde_json::Value::String(_) => Self::String,
            serde_json::Value::Bool(_) => Self::Boolean,
            serde_json::Value::Null => Self::Null,
            serde_json::Value::Object(_) => Self::Object,
            serde_json::Value::Array(_) => Self::Array,
        }
    }
    #[must_use]
    pub fn accepts(self, value: &serde_json::Value) -> bool {
        let actual = Self::of(value);
        self == actual
            || matches!(
                (self, actual),
                (Self::NullableNumber, Self::Number | Self::Null)
                    | (Self::NullableObject, Self::Object | Self::Null)
            )
    }
}
pub(super) fn comparable(left: ValueType, right: ValueType, comparison: Comparison) -> bool {
    match comparison {
        Comparison::Eq | Comparison::Ne => {
            left == right
                || matches!(
                    (left, right),
                    (
                        ValueType::NullableNumber,
                        ValueType::Number | ValueType::Null
                    ) | (
                        ValueType::Number | ValueType::Null,
                        ValueType::NullableNumber
                    ) | (
                        ValueType::NullableObject,
                        ValueType::Object | ValueType::Null
                    ) | (
                        ValueType::Object | ValueType::Null,
                        ValueType::NullableObject
                    )
                )
        }
        _ => left == right && matches!(left, ValueType::Number | ValueType::String),
    }
}
fn index(value: &str, len: usize) -> bool {
    value
        .parse::<usize>()
        .is_ok_and(|i| i < len && i.to_string() == value)
}
fn answer_path(space: &AnswerSpace, path: &[&str]) -> Result<ValueType, CompileError> {
    Ok(match (space, path) {
        (_, []) => ValueType::Object,
        (_, ["type"]) => ValueType::String,
        (AnswerSpace::Choice { .. }, ["selected"]) => ValueType::String,
        (AnswerSpace::Choice { .. }, ["distribution"]) => ValueType::Array,
        (AnswerSpace::Choice { options }, ["distribution", i, "option_id"])
            if index(i, options.len()) =>
        {
            ValueType::String
        }
        (AnswerSpace::Choice { options }, ["distribution", i, "probability"])
            if index(i, options.len()) =>
        {
            ValueType::Number
        }
        (AnswerSpace::Noul, ["probability"]) => ValueType::Number,
        (AnswerSpace::Score { .. }, ["value"]) => ValueType::Number,
        (AnswerSpace::Score { .. }, ["probabilities"]) => ValueType::Array,
        (AnswerSpace::Score { levels }, ["probabilities", i]) if index(i, levels.len()) => {
            ValueType::Number
        }
        _ => return Err(CompileError::Selector),
    })
}
pub fn reference_type(
    reference: &ResultReference,
    question: &Question,
) -> Result<ValueType, CompileError> {
    let path = reference.pointer.as_deref().unwrap_or("");
    let components: Vec<_> = if path.is_empty() {
        vec![]
    } else {
        path.strip_prefix('/')
            .ok_or(CompileError::Selector)?
            .split('/')
            .collect()
    };
    match reference.field {
        ResultField::Answer => answer_path(&question.answer_space, &components),
        ResultField::Outcome => Ok(match components.as_slice() {
            [] => ValueType::Object,
            ["status" | "question_id" | "reason"] => ValueType::String,
            ["answer", rest @ ..] => return answer_path(&question.answer_space, rest),
            ["confidence"] => ValueType::NullableObject,
            ["confidence", "value" | "answer_confidence"] => ValueType::NullableNumber,
            ["confidence", "meaning"] => ValueType::String,
            _ => return Err(CompileError::Selector),
        }),
        _ if !components.is_empty() => Err(CompileError::Selector),
        ResultField::Selected if matches!(question.answer_space, AnswerSpace::Choice { .. }) => {
            Ok(ValueType::String)
        }
        ResultField::Probability
            if matches!(
                question.answer_space,
                AnswerSpace::Choice { .. } | AnswerSpace::Noul
            ) =>
        {
            Ok(ValueType::Number)
        }
        ResultField::Score if matches!(question.answer_space, AnswerSpace::Score { .. }) => {
            Ok(ValueType::Number)
        }
        ResultField::Confidence => Ok(ValueType::Number),
        _ => Err(CompileError::Selector),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workflow::reference::MissingPolicy;
    #[test]
    fn selectors_follow_answer_kind_and_typed_distribution_indices() {
        let request: crate::decision::TurnRequest = serde_json::from_str(include_str!(
            "../../../../fixtures/contracts/decision-request.json"
        ))
        .unwrap();
        let mut reference = ResultReference {
            node: "node".into(),
            question: "travel".into(),
            field: ResultField::Score,
            pointer: None,
            on_missing: MissingPolicy::Required,
        };
        assert_eq!(
            reference_type(&reference, &request.questions[0]),
            Err(CompileError::Selector)
        );
        reference.field = ResultField::Probability;
        assert_eq!(
            reference_type(&reference, &request.questions[0]),
            Ok(ValueType::Number)
        );
        assert_eq!(
            reference_type(&reference, &request.questions[2]),
            Err(CompileError::Selector)
        );
        reference.field = ResultField::Answer;
        reference.pointer = Some("/distribution/1/probability".into());
        assert_eq!(
            reference_type(&reference, &request.questions[0]),
            Ok(ValueType::Number)
        );
        reference.pointer = Some("/distribution/01/probability".into());
        assert_eq!(
            reference_type(&reference, &request.questions[0]),
            Err(CompileError::Selector)
        );
        reference.pointer = Some("/probabilities/3".into());
        assert_eq!(
            reference_type(&reference, &request.questions[2]),
            Err(CompileError::Selector)
        );
        assert!(!comparable(
            ValueType::Number,
            ValueType::String,
            Comparison::Ge
        ));
        assert!(!comparable(
            ValueType::Object,
            ValueType::Object,
            Comparison::Lt
        ));
    }
}
