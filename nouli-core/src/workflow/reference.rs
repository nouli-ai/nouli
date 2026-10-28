//! Structured references and conditions, with explicit missing-value policy.

use serde::{Deserialize, Serialize};

use super::WorkflowError;
use crate::data::Data;

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(tag = "policy", rename_all = "snake_case", deny_unknown_fields)]
pub enum MissingPolicy {
    #[default]
    Required,
    Optional,
    Default {
        value: Data,
    },
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResultField {
    /// Full typed per-question result, including a native refusal.
    Outcome,
    #[default]
    Answer,
    Selected,
    Probability,
    Score,
    Confidence,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResultReference {
    pub node: String,
    pub question: String,
    #[serde(default)]
    pub field: ResultField,
    pub pointer: Option<String>,
    #[serde(default)]
    pub on_missing: MissingPolicy,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Absence {
    Skipped,
    Refused,
    Failed,
    Blocked,
    MissingField,
}

/// `Present { value: null }` differs from `Absent` in memory and on the wire.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "presence", rename_all = "snake_case", deny_unknown_fields)]
pub enum ReferenceValue {
    Present { value: Data },
    Absent { reason: Absence },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Operand {
    Literal { value: Data },
    Reference { reference: ResultReference },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Comparison {
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum Condition {
    Compare {
        left: Operand,
        comparison: Comparison,
        right: Operand,
    },
    All {
        conditions: Vec<Condition>,
    },
    Any {
        conditions: Vec<Condition>,
    },
    Not {
        condition: Box<Condition>,
    },
    Exists {
        reference: ResultReference,
    },
    Refused {
        node: String,
        question: String,
    },
}

impl ResultReference {
    pub fn validate(&self) -> Result<(), WorkflowError> {
        if self.node.trim().is_empty() || self.question.trim().is_empty() {
            return Err(WorkflowError::Shape("reference IDs"));
        }
        if let Some(pointer) = &self.pointer {
            if !pointer.is_empty() && !pointer.starts_with('/') {
                return Err(WorkflowError::Pointer);
            }
            let mut chars = pointer.chars();
            while let Some(c) = chars.next() {
                if c == '~' && !matches!(chars.next(), Some('0' | '1')) {
                    return Err(WorkflowError::Pointer);
                }
            }
        }
        Ok(())
    }
}

impl Condition {
    pub fn validate(&self) -> Result<(), WorkflowError> {
        let mut stack = vec![(self, 0)];
        while let Some((condition, depth)) = stack.pop() {
            if depth > 64 {
                return Err(WorkflowError::ConditionDepth);
            }
            match condition {
                Self::Compare { left, right, .. } => {
                    for operand in [left, right] {
                        if let Operand::Reference { reference } = operand {
                            reference.validate()?;
                        }
                    }
                }
                Self::All { conditions } | Self::Any { conditions } => {
                    if conditions.is_empty() {
                        return Err(WorkflowError::Shape("empty condition group"));
                    }
                    stack.extend(conditions.iter().map(|c| (c, depth + 1)));
                }
                Self::Not { condition } => stack.push((condition, depth + 1)),
                Self::Exists { reference } => reference.validate()?,
                Self::Refused { node, question } => {
                    if node.trim().is_empty() || question.trim().is_empty() {
                        return Err(WorkflowError::Shape("refusal IDs"));
                    }
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn null_is_present_and_default_is_explicit() {
        let value = Data::new(serde_json::Value::Null).unwrap();
        assert_ne!(
            ReferenceValue::Present {
                value: value.clone()
            },
            ReferenceValue::Absent {
                reason: Absence::Skipped
            }
        );
        let policy = MissingPolicy::Default { value };
        assert_eq!(
            policy,
            serde_json::from_str(&serde_json::to_string(&policy).unwrap()).unwrap()
        );
    }

    #[test]
    fn validates_pointer_escapes_and_prohibits_executable_expressions() {
        let mut reference = ResultReference {
            node: "a".into(),
            question: "q".into(),
            field: ResultField::Answer,
            pointer: None,
            on_missing: MissingPolicy::Required,
        };
        for pointer in ["", "/a~1b/~0/0"] {
            reference.pointer = Some(pointer.into());
            reference.validate().unwrap();
        }
        for pointer in ["a", "/bad~", "/bad~2"] {
            reference.pointer = Some(pointer.into());
            assert_eq!(reference.validate(), Err(WorkflowError::Pointer));
        }
        assert!(serde_json::from_str::<Condition>(r#"{"op":"eval","script":"true"}"#).is_err());
    }
}
