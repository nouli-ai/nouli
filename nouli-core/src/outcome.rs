//! Typed answers and per-question refusals. Provider rounding is never normalized.

use std::collections::HashSet;

use serde::{Deserialize, Serialize};

use crate::decision::{
    AnswerSpace, CONTRACT_VERSION, ContractError, Question, TurnRequest, nonempty,
};

/// Providers may round each reported probability and score to 0.01, so each one can be
/// off from its true value by at most half of that.
const HALF_STEP: f64 = 0.005;
/// Slack for floating-point arithmetic in sums.
const EPSILON: f64 = f64::EPSILON * 16.0;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderIdentity {
    pub provider: String,
    pub model: String,
    #[serde(default)]
    pub upstream_provider: Option<String>,
    #[serde(default)]
    pub request_id: Option<String>,
}

/// Native provider uncertainty, whose meaning is explicitly named, not assumed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Confidence {
    pub value: Option<f64>,
    pub meaning: String,
    /// Optional native maximum-answer probability, separate from confidence.
    #[serde(default)]
    pub answer_confidence: Option<f64>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Usage {
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub exact: bool,
    /// Optional native provider-reported cost, never estimated by Nouli.
    #[serde(default)]
    pub cost: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OptionProbability {
    pub option_id: String,
    pub probability: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum Answer {
    Choice {
        selected: String,
        distribution: Vec<OptionProbability>,
    },
    Noul {
        probability: f64,
    },
    Score {
        value: f64,
        probabilities: Vec<f64>,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
pub enum QuestionOutcome {
    Answered {
        question_id: String,
        answer: Answer,
        confidence: Option<Confidence>,
    },
    Refused {
        question_id: String,
        reason: String,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TurnOutcome {
    pub version: u32,
    pub provider: ProviderIdentity,
    pub outcomes: Vec<QuestionOutcome>,
    pub usage: Usage,
}

fn probability(n: f64) -> Result<(), ContractError> {
    if !n.is_finite() || !(0.0..=1.0).contains(&n) {
        Err(ContractError::Number("probability"))
    } else {
        Ok(())
    }
}

/// The range a true probability can lie in, given its rounded report.
fn bounds(reported: f64) -> (f64, f64) {
    (
        (reported - HALF_STEP).max(0.0),
        (reported + HALF_STEP).min(1.0),
    )
}

/// Accept reported probabilities when some true distribution, each value within
/// rounding of its report, sums to 1. Never renormalizes.
fn distribution(values: impl Iterator<Item = f64>) -> Result<(), ContractError> {
    let (mut low, mut high) = (0.0, 0.0);
    for value in values {
        probability(value)?;
        let (lo, hi) = bounds(value);
        low += lo;
        high += hi;
    }
    if low > 1.0 + EPSILON || high < 1.0 - EPSILON {
        return Err(ContractError::Distribution);
    }
    Ok(())
}

/// A score level index as a number on the score scale.
#[expect(
    clippy::cast_precision_loss,
    reason = "score scales have a handful of levels, far below 2^52"
)]
fn level(index: usize) -> f64 {
    index as f64
}

/// The lowest and highest expected level index over true distributions consistent with
/// the reported probabilities: start every level at its lower bound, then hand the
/// remaining mass to the lowest (or highest) levels first.
fn expectation_range(probabilities: &[f64]) -> (f64, f64) {
    let lows: Vec<f64> = probabilities.iter().map(|p| bounds(*p).0).collect();
    let floor: f64 = lows.iter().enumerate().map(|(i, p)| level(i) * p).sum();
    let slack = (1.0 - lows.iter().sum::<f64>()).max(0.0);
    let fill = |order: &mut dyn Iterator<Item = usize>| {
        let (mut left, mut expected) = (slack, floor);
        for i in order {
            let add = (bounds(probabilities[i]).1 - lows[i]).min(left);
            expected += level(i) * add;
            left -= add;
        }
        expected
    };
    (
        fill(&mut (0..probabilities.len())),
        fill(&mut (0..probabilities.len()).rev()),
    )
}

/// The probability `distribution` gives to `option_id`.
#[must_use]
pub fn probability_of(distribution: &[OptionProbability], option_id: &str) -> Option<f64> {
    distribution
        .iter()
        .find(|entry| entry.option_id == option_id)
        .map(|entry| entry.probability)
}

/// The outcome for the question with `question_id`.
#[must_use]
pub fn outcome_for<'a>(
    outcomes: &'a [QuestionOutcome],
    question_id: &str,
) -> Option<&'a QuestionOutcome> {
    outcomes
        .iter()
        .find(|outcome| outcome.question_id() == question_id)
}

impl Answer {
    /// The probability a choice answer gives to `option_id`; `None` for
    /// other answer types or an option outside its distribution.
    #[must_use]
    pub fn probability_of(&self, option_id: &str) -> Option<f64> {
        match self {
            Self::Choice { distribution, .. } => probability_of(distribution, option_id),
            Self::Noul { .. } | Self::Score { .. } => None,
        }
    }

    /// The probability a choice answer gives to its selected option; `None`
    /// for other answer types or a selection outside its distribution.
    #[must_use]
    pub fn selected_probability(&self) -> Option<f64> {
        match self {
            Self::Choice { selected, .. } => self.probability_of(selected),
            Self::Noul { .. } | Self::Score { .. } => None,
        }
    }

    /// Check that an answer matches the question's answer space.
    ///
    /// # Errors
    ///
    /// - Any error from validating `question`.
    /// - [`ContractError::AnswerType`] if the answer kind differs from the answer space.
    /// - [`ContractError::Unknown`], [`ContractError::Duplicate`], or [`ContractError::Missing`]
    ///   if a choice selection or distribution does not cover the options exactly once.
    /// - [`ContractError::Number`] or [`ContractError::Distribution`] if a probability or
    ///   score is out of range, or a distribution does not sum to one.
    /// - [`ContractError::ScoreExpectation`] if a score is too far from its expectation.
    pub fn validate(&self, question: &Question) -> Result<(), ContractError> {
        question.validate()?;
        match (self, &question.answer_space) {
            (Self::Noul { probability: value }, AnswerSpace::Noul) => probability(*value),
            (
                Self::Choice {
                    selected,
                    distribution: entries,
                },
                AnswerSpace::Choice { options },
            ) => {
                let ids: HashSet<_> = options.iter().map(|o| o.id.as_str()).collect();
                if !ids.contains(selected.as_str()) {
                    return Err(ContractError::Unknown("choice.selected"));
                }
                let mut seen = HashSet::new();
                for entry in entries {
                    if !ids.contains(entry.option_id.as_str()) {
                        return Err(ContractError::Unknown("choice.distribution"));
                    }
                    if !seen.insert(entry.option_id.as_str()) {
                        return Err(ContractError::Duplicate("choice.distribution"));
                    }
                }
                if seen.len() != ids.len() {
                    return Err(ContractError::Missing("choice.distribution"));
                }
                distribution(entries.iter().map(|e| e.probability))
            }
            (
                Self::Score {
                    value,
                    probabilities,
                },
                AnswerSpace::Score { levels },
            ) => {
                if probabilities.len() != levels.len() {
                    return Err(ContractError::Missing("score.probabilities"));
                }
                if !value.is_finite() || !(0.0..=level(levels.len() - 1)).contains(value) {
                    return Err(ContractError::Number("score.value"));
                }
                distribution(probabilities.iter().copied())?;
                // The reported score is itself rounded, so it may sit half a step outside.
                let (low, high) = expectation_range(probabilities);
                if *value < low - HALF_STEP - EPSILON || *value > high + HALF_STEP + EPSILON {
                    return Err(ContractError::ScoreExpectation);
                }
                Ok(())
            }
            _ => Err(ContractError::AnswerType),
        }
    }
}

impl QuestionOutcome {
    #[must_use]
    pub fn question_id(&self) -> &str {
        match self {
            Self::Answered { question_id, .. } | Self::Refused { question_id, .. } => question_id,
        }
    }

    /// Check that a question outcome belongs to `question` and is well formed.
    ///
    /// # Errors
    ///
    /// - [`ContractError::Unknown`] if the question ID differs.
    /// - Any error from [`Answer::validate`].
    /// - [`ContractError::Empty`] or [`ContractError::Number`] if confidence is empty,
    ///   non-finite, or out of range, or a refusal reason is blank.
    pub fn validate(&self, question: &Question) -> Result<(), ContractError> {
        if self.question_id() != question.id {
            return Err(ContractError::Unknown("outcome.question_id"));
        }
        match self {
            Self::Answered {
                answer, confidence, ..
            } => {
                answer.validate(question)?;
                if let Some(confidence) = confidence {
                    if confidence.value.is_none() && confidence.answer_confidence.is_none() {
                        return Err(ContractError::Empty("confidence"));
                    }
                    if confidence.value.is_some_and(|value| !value.is_finite()) {
                        return Err(ContractError::Number("confidence"));
                    }
                    nonempty(&confidence.meaning, "confidence.meaning")?;
                    if let Some(answer_confidence) = confidence.answer_confidence {
                        probability(answer_confidence)?;
                    }
                }
                Ok(())
            }
            Self::Refused { reason, .. } => nonempty(reason, "refusal.reason"),
        }
    }
}

impl TurnOutcome {
    /// The outcome for the question with `question_id`.
    #[must_use]
    pub fn outcome_for(&self, question_id: &str) -> Option<&QuestionOutcome> {
        outcome_for(&self.outcomes, question_id)
    }

    /// Check that a turn outcome answers each requested question exactly once.
    ///
    /// # Errors
    ///
    /// - Any error from validating `request`.
    /// - [`ContractError::Version`] if the contract version differs.
    /// - [`ContractError::Empty`] if a provider identity field is blank, or
    ///   [`ContractError::Number`] if the usage cost is negative or non-finite.
    /// - [`ContractError::Unknown`], [`ContractError::Duplicate`], or [`ContractError::Missing`]
    ///   if outcomes do not cover the questions exactly once.
    /// - Any error from [`QuestionOutcome::validate`].
    pub fn validate(&self, request: &TurnRequest) -> Result<(), ContractError> {
        request.validate()?;
        if self.version != CONTRACT_VERSION {
            return Err(ContractError::Version(self.version));
        }
        nonempty(&self.provider.provider, "provider")?;
        nonempty(&self.provider.model, "model")?;
        if let Some(provider) = &self.provider.upstream_provider {
            nonempty(provider, "upstream_provider")?;
        }
        if let Some(id) = &self.provider.request_id {
            nonempty(id, "provider.request_id")?;
        }
        if self
            .usage
            .cost
            .is_some_and(|cost| !cost.is_finite() || cost < 0.0)
        {
            return Err(ContractError::Number("usage.cost"));
        }
        let mut seen = HashSet::new();
        for outcome in &self.outcomes {
            let question = request
                .questions
                .iter()
                .find(|q| q.id == outcome.question_id())
                .ok_or(ContractError::Unknown("outcomes"))?;
            if !seen.insert(outcome.question_id()) {
                return Err(ContractError::Duplicate("outcomes"));
            }
            outcome.validate(question)?;
        }
        if seen.len() != request.questions.len() {
            return Err(ContractError::Missing("outcomes"));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> (TurnRequest, TurnOutcome) {
        (
            serde_json::from_str(include_str!(
                "../../fixtures/contracts/decision-request.json"
            ))
            .unwrap(),
            serde_json::from_str(include_str!(
                "../../fixtures/contracts/decision-outcome.json"
            ))
            .unwrap(),
        )
    }

    #[test]
    fn accessors_find_outcomes_and_choice_probabilities() {
        let (_, outcome) = fixture();
        let travel = outcome.outcome_for("travel").unwrap();
        assert_eq!(travel.question_id(), "travel");
        assert!(outcome.outcome_for("missing").is_none());
        assert_eq!(
            outcome_for(&outcome.outcomes, "travel"),
            Some(&outcome.outcomes[0])
        );
        let QuestionOutcome::Answered { answer, .. } = travel else {
            panic!("the fixture answers travel");
        };
        assert_eq!(answer.selected_probability(), Some(0.7));
        assert_eq!(answer.probability_of("flight"), Some(0.3));
        assert_eq!(answer.probability_of("boat"), None);
        let noul = Answer::Noul { probability: 0.4 };
        assert_eq!(noul.selected_probability(), None);
        assert_eq!(noul.probability_of("train"), None);
    }

    #[test]
    fn all_primitives_refusal_and_confidence_round_trip() {
        let (request, outcome) = fixture();
        outcome.validate(&request).unwrap();
        assert_eq!(
            outcome,
            serde_json::from_str(&serde_json::to_string(&outcome).unwrap()).unwrap()
        );
        assert!(matches!(
            outcome.outcomes[2],
            QuestionOutcome::Answered {
                answer: Answer::Score { value: 1.43, .. },
                ..
            }
        ));
    }

    #[test]
    fn noul_extremes_and_nonfinite_numbers() {
        let question = &fixture().0.questions[1];
        for p in [0.0, 1.0] {
            Answer::Noul { probability: p }.validate(question).unwrap();
        }
        for p in [-0.01, 1.01, f64::NAN, f64::INFINITY] {
            assert!(Answer::Noul { probability: p }.validate(question).is_err());
        }
    }

    #[test]
    fn score_is_expectation_not_argmax_and_rounding_is_not_rewritten() {
        let question = &fixture().0.questions[2];
        assert_eq!(
            Answer::Score {
                value: 1.0,
                probabilities: vec![0.0, 0.57, 0.43]
            }
            .validate(question),
            Err(ContractError::ScoreExpectation)
        );
        let answer = Answer::Score {
            value: 1.43,
            probabilities: vec![0.0, 0.5705, 0.43],
        };
        answer.validate(question).unwrap();
        let Answer::Score { probabilities, .. } = &answer else {
            panic!("expected a score answer");
        };
        assert_eq!(probabilities, &[0.0, 0.5705, 0.43]);
        assert!(
            Answer::Score {
                value: 1.43,
                probabilities: vec![0.0, 0.7, 0.43]
            }
            .validate(question)
            .is_err()
        );
    }

    fn score(levels: usize) -> Question {
        serde_json::from_value(serde_json::json!({
            "id": "q", "prompt": "Rate it",
            "answer_space": {"type": "score", "levels": (0..levels).map(|i| format!("Level {i}")).collect::<Vec<_>>()}
        }))
        .unwrap()
    }

    fn choice(values: &[f64]) -> (Question, Answer) {
        let ids: Vec<String> = (0..values.len()).map(|i| format!("o{i}")).collect();
        let question = serde_json::from_value(serde_json::json!({
            "id": "q", "prompt": "Pick one",
            "answer_space": {"type": "choice", "options": ids.iter().map(|id| serde_json::json!({"id": id, "description": id})).collect::<Vec<_>>()}
        }))
        .unwrap();
        let answer = Answer::Choice {
            selected: "o0".into(),
            distribution: ids
                .into_iter()
                .zip(values)
                .map(|(option_id, p)| OptionProbability {
                    option_id,
                    probability: *p,
                })
                .collect(),
        };
        (question, answer)
    }

    #[test]
    fn two_decimal_rounding_is_accepted_and_impossible_reports_are_not() {
        let score_ok = |levels, value, probabilities: &[f64]| {
            Answer::Score {
                value,
                probabilities: probabilities.to_vec(),
            }
            .validate(&score(levels))
        };
        // A provider computed 1.98 before rounding each probability to 0.01.
        score_ok(4, 1.98, &[0.01, 0.01, 0.98, 0.0]).unwrap();
        assert_eq!(
            score_ok(4, 1.9, &[0.01, 0.01, 0.98, 0.0]),
            Err(ContractError::ScoreExpectation)
        );
        // Above the feasible range: the highest expectation is 1.99.
        score_ok(4, 1.99, &[0.01, 0.01, 0.98, 0.0]).unwrap();
        assert_eq!(
            score_ok(4, 2.0, &[0.01, 0.01, 0.98, 0.0]),
            Err(ContractError::ScoreExpectation)
        );
        // A reported 1.0 can't hide mass above 1, so the range stays near the top level.
        score_ok(3, 1.99, &[0.0, 0.0, 1.0]).unwrap();
        assert_eq!(
            score_ok(3, 1.98, &[0.0, 0.0, 1.0]),
            Err(ContractError::ScoreExpectation)
        );
        // The argmax level instead of the expectation, at 5 and 10 levels.
        assert_eq!(
            score_ok(5, 2.0, &[0.0, 0.0, 0.95, 0.05, 0.0]),
            Err(ContractError::ScoreExpectation)
        );
        let mut ten = [0.0; 10];
        ten[7] = 0.8;
        ten[8] = 0.2;
        assert_eq!(
            score_ok(10, 7.0, &ten),
            Err(ContractError::ScoreExpectation)
        );
        score_ok(10, 7.2, &ten).unwrap();

        let check = |values: &[f64]| {
            let (question, answer) = choice(values);
            answer.validate(&question)
        };
        // 0.495 + 0.495 reported as 0.5 and 0.49.
        check(&[0.5, 0.49]).unwrap();
        assert_eq!(check(&[0.6, 0.38]), Err(ContractError::Distribution));
        // Small probabilities rounded to zero can hide the rest of the mass.
        let mut many = [0.0; 10];
        many[0] = 0.95;
        check(&many).unwrap();
        // Two options can't both be above one half, however many zeros follow.
        many[0] = 0.53;
        many[1] = 0.52;
        assert_eq!(check(&many), Err(ContractError::Distribution));
    }

    #[test]
    fn rejects_missing_duplicate_foreign_and_mismatched_answers() {
        let (request, mut outcome) = fixture();
        outcome.outcomes.pop();
        assert_eq!(
            outcome.validate(&request),
            Err(ContractError::Missing("outcomes"))
        );
        outcome.outcomes.push(outcome.outcomes[0].clone());
        assert_eq!(
            outcome.validate(&request),
            Err(ContractError::Duplicate("outcomes"))
        );
        outcome.outcomes.pop();
        outcome.outcomes.push(QuestionOutcome::Refused {
            question_id: "foreign".into(),
            reason: "declined".into(),
        });
        assert_eq!(
            outcome.validate(&request),
            Err(ContractError::Unknown("outcomes"))
        );
        assert_eq!(
            Answer::Noul { probability: 0.5 }.validate(&request.questions[0]),
            Err(ContractError::AnswerType)
        );
    }

    #[test]
    fn validates_choice_coverage_ids_sum_and_range() {
        let question = &fixture().0.questions[0];
        let make = |ids: &[&str], values: &[f64]| Answer::Choice {
            selected: "train".into(),
            distribution: ids
                .iter()
                .zip(values)
                .map(|(id, p)| OptionProbability {
                    option_id: (*id).into(),
                    probability: *p,
                })
                .collect(),
        };
        for answer in [
            make(&["train"], &[1.0]),
            make(&["train", "train"], &[0.5, 0.5]),
            make(&["train", "foreign"], &[0.5, 0.5]),
            make(&["train", "flight"], &[0.8, 0.8]),
            make(&["train", "flight"], &[-0.1, 1.1]),
        ] {
            assert!(answer.validate(question).is_err());
        }
    }

    #[test]
    fn native_cost_requires_a_nonnegative_finite_value() {
        let (request, mut outcome) = fixture();
        for cost in [-1.0, f64::NAN, f64::INFINITY] {
            outcome.usage.cost = Some(cost);
            assert_eq!(
                outcome.validate(&request),
                Err(ContractError::Number("usage.cost"))
            );
        }
        outcome.usage.cost = Some(0.0);
        outcome.validate(&request).unwrap();
    }
}
