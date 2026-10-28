//! Version-1 requests shared by every provider.

use std::collections::HashSet;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::data::Input;

pub const CONTRACT_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TurnRequest {
    pub version: u32,
    #[serde(default)]
    pub state: Vec<Input>,
    pub questions: Vec<Question>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Question {
    pub id: String,
    pub prompt: String,
    #[serde(default)]
    pub criteria: Vec<Criterion>,
    pub answer_space: AnswerSpace,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged, deny_unknown_fields)]
pub enum Criterion {
    Text(String),
    Detailed { label: String, description: String },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum AnswerSpace {
    Choice { options: Vec<ChoiceOption> },
    Noul,
    Score { levels: Vec<Criterion> },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChoiceOption {
    pub id: String,
    pub description: String,
}

/// Contract errors contain safe field identifiers rather than supplied content.
#[derive(Debug, Error, PartialEq)]
pub enum ContractError {
    #[error("unsupported contract version {0}")]
    Version(u32),
    #[error("required field is empty: {0}")]
    Empty(&'static str),
    #[error("duplicate identifier in {0}")]
    Duplicate(&'static str),
    #[error("invalid cardinality for {0}")]
    Cardinality(&'static str),
    #[error("unknown identifier in {0}")]
    Unknown(&'static str),
    #[error("missing entries in {0}")]
    Missing(&'static str),
    #[error("answer type does not match question")]
    AnswerType,
    #[error("non-finite or out-of-range number in {0}")]
    Number(&'static str),
    #[error("probability distribution does not sum to one")]
    Distribution,
    #[error("score differs from the probability-weighted level-index expectation")]
    ScoreExpectation,
}

pub(crate) fn nonempty(value: &str, field: &'static str) -> Result<(), ContractError> {
    if value.trim().is_empty() {
        Err(ContractError::Empty(field))
    } else {
        Ok(())
    }
}

impl Criterion {
    pub fn validate(&self) -> Result<(), ContractError> {
        match self {
            Self::Text(text) => nonempty(text, "criterion"),
            Self::Detailed { label, description } => {
                nonempty(label, "criterion.label")?;
                nonempty(description, "criterion.description")
            }
        }
    }
}

impl Question {
    pub fn validate(&self) -> Result<(), ContractError> {
        nonempty(&self.id, "question.id")?;
        nonempty(&self.prompt, "question.prompt")?;
        for criterion in &self.criteria {
            criterion.validate()?;
        }
        match &self.answer_space {
            AnswerSpace::Choice { options } => {
                if !(2..=255).contains(&options.len()) {
                    return Err(ContractError::Cardinality("choice.options"));
                }
                let mut ids = HashSet::new();
                for option in options {
                    nonempty(&option.id, "option.id")?;
                    nonempty(&option.description, "option.description")?;
                    if !ids.insert(&option.id) {
                        return Err(ContractError::Duplicate("choice.options"));
                    }
                }
            }
            AnswerSpace::Score { levels } => {
                if !(2..=10).contains(&levels.len()) {
                    return Err(ContractError::Cardinality("score.levels"));
                }
                for level in levels {
                    level.validate()?;
                }
            }
            AnswerSpace::Noul => {}
        }
        Ok(())
    }
}

impl TurnRequest {
    pub fn validate(&self) -> Result<(), ContractError> {
        if self.version != CONTRACT_VERSION {
            return Err(ContractError::Version(self.version));
        }
        if self.questions.is_empty() {
            return Err(ContractError::Empty("questions"));
        }
        let mut ids = HashSet::new();
        for question in &self.questions {
            question.validate()?;
            if !ids.insert(&question.id) {
                return Err(ContractError::Duplicate("questions"));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(crate) fn fixture() -> TurnRequest {
        serde_json::from_str(include_str!(
            "../../fixtures/contracts/decision-request.json"
        ))
        .unwrap()
    }

    #[test]
    fn fixture_round_trips_and_has_all_primitives() {
        let request = fixture();
        request.validate().unwrap();
        assert_eq!(request.questions.len(), 4);
        assert_eq!(
            request,
            serde_json::from_str(&serde_json::to_string(&request).unwrap()).unwrap()
        );
    }

    #[test]
    fn rejects_invalid_versions_empty_and_duplicate_questions() {
        let mut request = fixture();
        request.version = 2;
        assert_eq!(request.validate(), Err(ContractError::Version(2)));
        request.version = 1;
        request.questions.push(request.questions[0].clone());
        assert_eq!(
            request.validate(),
            Err(ContractError::Duplicate("questions"))
        );
        request.questions.clear();
        assert_eq!(request.validate(), Err(ContractError::Empty("questions")));
    }

    #[test]
    fn enforces_option_and_level_cardinality_and_unique_ids() {
        let mut question = fixture().questions.remove(0);
        if let AnswerSpace::Choice { options } = &mut question.answer_space {
            options[1].id = options[0].id.clone();
        }
        assert_eq!(
            question.validate(),
            Err(ContractError::Duplicate("choice.options"))
        );
        for count in [0, 1, 256] {
            question.answer_space = AnswerSpace::Choice {
                options: (0..count)
                    .map(|i| ChoiceOption {
                        id: i.to_string(),
                        description: "option".into(),
                    })
                    .collect(),
            };
            assert!(matches!(
                question.validate(),
                Err(ContractError::Cardinality(_))
            ));
        }
        for count in [0, 1, 11] {
            question.answer_space = AnswerSpace::Score {
                levels: vec![Criterion::Text("level".into()); count],
            };
            assert!(matches!(
                question.validate(),
                Err(ContractError::Cardinality(_))
            ));
        }
    }
}
