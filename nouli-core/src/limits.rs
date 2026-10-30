//! Verified capability limits and separately attributed preflight token counts.

pub mod count;
pub mod registry;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{
    codec::CodecError,
    config::ProviderProfile,
    decision::{AnswerSpace, ContractError, TurnRequest},
    history::TokenCount,
};
use count::{InputCounts, TokenCounter};
use registry::Capabilities;

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
    #[error("unknown/custom model requires an explicit context budget")]
    MissingBudget,
    #[error("context budget is zero or exceeds a known/configured limit")]
    InvalidBudget,
    #[error("unsupported input modality")]
    Modality,
    #[error("provider question/choice/rubric count limit exceeded")]
    Count,
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
    pub protocol: crate::config::Protocol,
    pub model: String,
    pub budget: u64,
    pub per_question: Option<u64>,
    pub reserved_output_tokens: u64,
    pub capabilities: Option<Capabilities>,
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
        let capabilities = registry::lookup(profile);
        let known = capabilities.as_ref().map(|c| c.total_context);
        let configured = profile.context_window_tokens.or(known);
        if profile
            .context_window_tokens
            .zip(known)
            .is_some_and(|(configured, known)| configured > known)
        {
            return Err(LimitError::InvalidBudget);
        }
        let budget = profile
            .context_budget_tokens
            .or(configured)
            .ok_or(LimitError::MissingBudget)?;
        if budget == 0 || configured.is_some_and(|limit| budget > limit) {
            return Err(LimitError::InvalidBudget);
        }
        let reserve = profile.reserved_output_tokens.unwrap_or(0);
        if reserve >= budget {
            return Err(LimitError::InvalidBudget);
        }
        Ok(Self {
            protocol: profile.protocol,
            model: profile.model.clone(),
            budget,
            per_question: capabilities.as_ref().and_then(|c| c.per_question_context),
            reserved_output_tokens: reserve,
            capabilities,
        })
    }

    pub fn validate_request(
        &self,
        request: &TurnRequest,
        modality: Modality,
    ) -> Result<(), LimitError> {
        request.validate()?;
        if !matches!(modality, Modality::Text | Modality::Image) {
            return Err(LimitError::Modality);
        }
        if let Some(capabilities) = &self.capabilities {
            if !capabilities.modalities.contains(&modality) {
                return Err(LimitError::Modality);
            }
            if capabilities
                .max_questions
                .is_some_and(|limit| request.questions.len() > limit)
            {
                return Err(LimitError::Count);
            }
            for question in &request.questions {
                match &question.answer_space {
                    AnswerSpace::Choice { options } if options.len() > capabilities.max_choices => {
                        return Err(LimitError::Count);
                    }
                    AnswerSpace::Score { levels }
                        if levels.len() > capabilities.max_score_levels =>
                    {
                        return Err(LimitError::Count);
                    }
                    _ => {}
                }
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
        } = counter.count(self.protocol, &self.model, context_toon, &request.questions)?;
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
                &Default::default(),
                &Default::default(),
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
    fn known_unknown_custom_and_smaller_budgets_are_explicit() {
        let mut jev = profile("jev");
        let limits = ResolvedLimits::for_profile(&jev).unwrap();
        assert_eq!(limits.budget, 64000);
        assert_eq!(limits.per_question, Some(32000));
        jev.context_budget_tokens = Some(1000);
        assert_eq!(ResolvedLimits::for_profile(&jev).unwrap().budget, 1000);
        jev.context_budget_tokens = Some(64001);
        assert_eq!(
            ResolvedLimits::for_profile(&jev),
            Err(LimitError::InvalidBudget)
        );
        jev.context_budget_tokens = None;
        jev.model = "unknown".into();
        assert_eq!(
            ResolvedLimits::for_profile(&jev),
            Err(LimitError::MissingBudget)
        );
        jev.context_budget_tokens = Some(512);
        assert_eq!(ResolvedLimits::for_profile(&jev).unwrap().budget, 512);
        let mut custom = profile("jev");
        custom.base_url = "http://localhost:8000".into();
        assert_eq!(
            ResolvedLimits::for_profile(&custom),
            Err(LimitError::MissingBudget)
        );
        custom.context_window_tokens = Some(512);
        assert!(
            ResolvedLimits::for_profile(&custom)
                .unwrap()
                .capabilities
                .is_none()
        );
    }

    #[test]
    fn direct_and_router_question_limits_and_modalities_stay_distinct() {
        let direct = ResolvedLimits::for_profile(&profile("openai")).unwrap();
        let router = ResolvedLimits::for_profile(&profile("router")).unwrap();
        assert_eq!(
            direct.capabilities.as_ref().unwrap().max_questions,
            Some(400)
        );
        assert_eq!(
            router.capabilities.as_ref().unwrap().max_questions,
            Some(200)
        );
        let mut request = request();
        let question = request.questions[1].clone();
        request.questions = (0..201)
            .map(|index| {
                let mut question = question.clone();
                question.id = index.to_string();
                question
            })
            .collect();
        direct.validate_request(&request, Modality::Text).unwrap();
        assert_eq!(
            router.validate_request(&request, Modality::Text),
            Err(LimitError::Count)
        );
        assert_eq!(direct.validate_request(&request, Modality::Image), Ok(()));
        request.questions = (0..401)
            .map(|index| {
                let mut question = question.clone();
                question.id = index.to_string();
                question
            })
            .collect();
        assert_eq!(
            direct.validate_request(&request, Modality::Text),
            Err(LimitError::Count)
        );
    }

    struct FixedCounter {
        total: u64,
        one: u64,
        method: CountMethod,
    }

    struct GivenCounts(InputCounts);
    impl TokenCounter for GivenCounts {
        fn count(
            &self,
            _protocol: Protocol,
            _model: &str,
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
        .count(Protocol::Typesafe, "jev", "state", &request.questions)
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
    impl TokenCounter for FixedCounter {
        fn count(
            &self,
            _protocol: Protocol,
            _model: &str,
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
        let limits = ResolvedLimits::for_profile(&profile("jev")).unwrap();
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
