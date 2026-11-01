//! Explicit configured limits and separately attributed preflight token counts.

pub mod count;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{
    codec::CodecError,
    config::ProviderProfile,
    decision::{AnswerSpace, ContractError, TurnRequest},
    history::TokenCount,
};
use count::{InputCounts, TokenCounter};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Modality {
    Text,
    Image,
    Audio,
    Video,
}

#[derive(Debug, Error, PartialEq)]
pub enum LimitError {
    #[error("context budget is zero or cannot fit the configured output reserve")]
    InvalidBudget,
    #[error("unsupported input modality")]
    Modality,
    #[error("provider question/choice/rubric count limit exceeded")]
    Count,
    #[error("serialized request exceeds configured byte limit")]
    RequestBytes,
    #[error("total input plus output reserve exceeds context budget")]
    TotalCapacity,
    #[error("state plus one complete question exceeds its context limit")]
    QuestionCapacity,
    #[error("token count arithmetic overflow")]
    Overflow,
    #[error("counter returned inconsistent count evidence")]
    Counter,
    #[error("invalid provider profile")]
    Profile,
    #[error(transparent)]
    Contract(#[from] ContractError),
    #[error(transparent)]
    Codec(#[from] CodecError),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResolvedLimits {
    pub profile: ProviderProfile,
    pub protocol: crate::config::Protocol,
    pub model: String,
    pub budget: u64,
    pub per_question: Option<u64>,
    pub reserved_output_tokens: u64,
    pub max_questions: Option<usize>,
    pub max_choices: Option<usize>,
    pub max_score_levels: Option<usize>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BudgetAssessment {
    pub limits: ResolvedLimits,
    pub total: TokenCount,
    pub per_question: Vec<TokenCount>,
}

impl ResolvedLimits {
    pub fn for_profile(profile: &ProviderProfile) -> Result<Self, LimitError> {
        profile.validate().map_err(|_| LimitError::Profile)?;
        let configured = profile.context_window_tokens;
        let budget = profile.context_budget_tokens.unwrap_or(8192);
        let budget = configured.map_or(budget, |window| budget.min(window));
        if budget == 0 {
            return Err(LimitError::InvalidBudget);
        }
        let reserve = profile.reserved_output_tokens.unwrap_or(0);
        if reserve >= budget {
            return Err(LimitError::InvalidBudget);
        }
        Ok(Self {
            profile: profile.clone(),
            protocol: profile.protocol,
            model: profile.model.clone(),
            budget,
            per_question: profile.limits.per_question_context_tokens,
            reserved_output_tokens: reserve,
            max_questions: profile.limits.max_questions,
            max_choices: profile.limits.max_choices,
            max_score_levels: profile.limits.max_score_levels,
        })
    }

    pub fn validate_request(
        &self,
        request: &TurnRequest,
        modality: Modality,
    ) -> Result<(), LimitError> {
        request.validate()?;
        if self
            .profile
            .limits
            .input_modalities
            .as_ref()
            .is_some_and(|m| !m.contains(&modality))
        {
            return Err(LimitError::Modality);
        }
        if !matches!(modality, Modality::Text | Modality::Image) {
            return Err(LimitError::Modality);
        }
        if self
            .max_questions
            .is_some_and(|n| request.questions.len() > n)
        {
            return Err(LimitError::Count);
        }
        for question in &request.questions {
            match &question.answer_space {
                AnswerSpace::Choice { options }
                    if self.max_choices.is_some_and(|n| options.len() > n) =>
                {
                    return Err(LimitError::Count);
                }
                AnswerSpace::Score { levels }
                    if self.max_score_levels.is_some_and(|n| levels.len() > n) =>
                {
                    return Err(LimitError::Count);
                }
                _ => {}
            }
        }
        Ok(())
    }

    pub fn assess(
        &self,
        context_toon: &str,
        request: &TurnRequest,
        counter: &impl TokenCounter,
    ) -> Result<BudgetAssessment, LimitError> {
        let evidence = crate::media::Evidence::read(context_toon)?;
        self.validate_request(
            request,
            if evidence.images.is_empty() {
                Modality::Text
            } else {
                Modality::Image
            },
        )?;
        let InputCounts {
            total,
            per_question,
        } = counter.count(&self.profile, context_toon, &request.questions)?;
        if per_question.len() != request.questions.len()
            || !total.is_consistent()
            || total.method == crate::history::CountMethod::ProviderReported
            || per_question.iter().any(|count| {
                !count.is_consistent()
                    || count.method == crate::history::CountMethod::ProviderReported
                    || count.tokens > total.tokens
            })
        {
            return Err(LimitError::Counter);
        }
        if total
            .tokens
            .checked_add(self.reserved_output_tokens)
            .ok_or(LimitError::Overflow)?
            > self.budget
        {
            return Err(LimitError::TotalCapacity);
        }
        if let Some(limit) = self.per_question
            && per_question.iter().any(|count| {
                count
                    .tokens
                    .checked_add(self.reserved_output_tokens)
                    .is_none_or(|total| total > limit)
            })
        {
            return Err(LimitError::QuestionCapacity);
        }
        Ok(BudgetAssessment {
            limits: self.clone(),
            total,
            per_question,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::history::ModelSelection;
    use crate::{
        config::{Config, Protocol},
        history::CountMethod,
    };
    use count::ByteCounter;

    fn profile(name: &str) -> ProviderProfile {
        let config = Config::parse(include_str!("../../fixtures/config/providers.toml")).unwrap();
        let model = config.providers[name].models.keys().next().unwrap().clone();
        config
            .resolve_snapshot(
                &crate::config::resolve::RuntimeOverrides {
                    selection: crate::history::ModelSelection {
                        provider: Some(name.into()),
                        model: Some(model),
                    },
                    ..Default::default()
                },
                &ModelSelection::default(),
                &ModelSelection::default(),
                std::path::Path::new("/home"),
            )
            .unwrap()
            .profile
    }
    fn request() -> TurnRequest {
        serde_json::from_str(include_str!(
            "../../fixtures/contracts/decision-request.json"
        ))
        .unwrap()
    }

    #[test]
    fn unknown_hosts_and_models_use_operational_budget_and_explicit_limits() {
        let mut custom = profile("jev");
        custom.base_url = "https://arbitrary.example".into();
        custom.model = "unlisted".into();
        assert_eq!(ResolvedLimits::for_profile(&custom).unwrap().budget, 8192);
        custom.context_window_tokens = Some(512);
        assert_eq!(ResolvedLimits::for_profile(&custom).unwrap().budget, 512);
        custom.context_budget_tokens = Some(256);
        assert_eq!(ResolvedLimits::for_profile(&custom).unwrap().budget, 256);
        custom.limits.max_questions = Some(1);
        assert_eq!(
            ResolvedLimits::for_profile(&custom)
                .unwrap()
                .validate_request(&request(), Modality::Text),
            Err(LimitError::Count)
        );
    }

    struct GivenCounts(InputCounts);
    impl TokenCounter for GivenCounts {
        fn count(
            &self,
            _profile: &crate::config::ProviderProfile,
            _context: &str,
            _questions: &[crate::decision::Question],
        ) -> Result<InputCounts, LimitError> {
            Ok(self.0.clone())
        }
    }

    #[test]
    fn contradictory_count_evidence_and_reserve_overflow_fail_closed() {
        let limits = ResolvedLimits::for_profile(&profile("jev")).unwrap();
        let request = request();
        let valid = FixedCounter {
            total: 100,
            one: 50,
            method: CountMethod::ByteEstimate,
        }
        .count(
            &crate::limits::count::tests::profile(Protocol::SystemOne, "jev"),
            "state",
            &request.questions,
        )
        .unwrap();
        let mut invalid = valid.clone();
        invalid.total.exact = true;
        assert_eq!(
            limits.assess("state", &request, &GivenCounts(invalid)),
            Err(LimitError::Counter)
        );
        let mut invalid = valid.clone();
        invalid.total.method = CountMethod::VerifiedProtocol;
        assert_eq!(
            limits.assess("state", &request, &GivenCounts(invalid)),
            Err(LimitError::Counter)
        );
        let mut invalid = valid.clone();
        invalid.per_question.pop();
        assert_eq!(
            limits.assess("state", &request, &GivenCounts(invalid)),
            Err(LimitError::Counter)
        );
        let mut invalid = valid;
        invalid.per_question[0].tokens = 101;
        assert_eq!(
            limits.assess("state", &request, &GivenCounts(invalid)),
            Err(LimitError::Counter)
        );
        let mut profile = profile("local");
        profile.context_budget_tokens = Some(u64::MAX);
        profile.reserved_output_tokens = Some(1);
        let limits = ResolvedLimits::for_profile(&profile).unwrap();
        let overflow = FixedCounter {
            total: u64::MAX,
            one: 50,
            method: CountMethod::ByteEstimate,
        };
        assert_eq!(
            limits.assess("state", &request, &overflow),
            Err(LimitError::Overflow)
        );
    }
    struct FixedCounter {
        total: u64,
        one: u64,
        method: CountMethod,
    }
    impl TokenCounter for FixedCounter {
        fn count(
            &self,
            _profile: &crate::config::ProviderProfile,
            _context: &str,
            questions: &[crate::decision::Question],
        ) -> Result<InputCounts, LimitError> {
            let exact = matches!(
                self.method,
                CountMethod::VerifiedProtocol | CountMethod::ProviderReported
            );
            Ok(InputCounts {
                total: TokenCount {
                    tokens: self.total,
                    exact,
                    method: self.method,
                },
                per_question: questions
                    .iter()
                    .map(|_| TokenCount {
                        tokens: self.one,
                        exact,
                        method: self.method,
                    })
                    .collect(),
            })
        }
    }

    #[test]
    fn total_and_per_question_limits_both_apply_with_count_provenance() {
        let mut configured = profile("jev");
        configured.context_budget_tokens = Some(64000);
        configured.limits.per_question_context_tokens = Some(32000);
        let limits = ResolvedLimits::for_profile(&configured).unwrap();
        let request = request();
        let counter = |total, one| FixedCounter {
            total,
            one,
            method: CountMethod::VerifiedProtocol,
        };
        assert_eq!(
            limits.assess("state", &request, &counter(64001, 1000)),
            Err(LimitError::TotalCapacity)
        );
        assert_eq!(
            limits.assess("state", &request, &counter(50000, 32001)),
            Err(LimitError::QuestionCapacity)
        );
        let assessment = limits
            .assess("state", &request, &counter(64000, 32000))
            .unwrap();
        assert!(assessment.total.exact);
        assert_eq!(assessment.total.method, CountMethod::VerifiedProtocol);
        let tokenizer = FixedCounter {
            total: 100,
            one: 50,
            method: CountMethod::TokenizerEstimate,
        };
        assert!(
            !limits
                .assess("state", &request, &tokenizer)
                .unwrap()
                .total
                .exact
        );
        let provider = FixedCounter {
            total: 100,
            one: 50,
            method: CountMethod::ProviderReported,
        };
        assert_eq!(
            limits.assess("state", &request, &provider),
            Err(LimitError::Counter)
        );
        let estimated = limits
            .assess("🦀 evidence", &request, &ByteCounter)
            .unwrap();
        assert_eq!(estimated.total.method, CountMethod::ByteEstimate);
    }
}
