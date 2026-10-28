//! Counting is a seam for complete protocol input, never generic tokenizer claims.

use super::LimitError;
use crate::{
    codec,
    config::Protocol,
    decision::Question,
    history::{CountMethod, TokenCount},
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InputCounts {
    pub total: TokenCount,
    pub per_question: Vec<TokenCount>,
}

/// A verified implementation must account for the complete selected protocol,
/// model context, and live question envelope, not just tokenize the state string.
pub trait TokenCounter {
    fn count(
        &self,
        protocol: Protocol,
        model: &str,
        context_toon: &str,
        questions: &[Question],
    ) -> Result<InputCounts, LimitError>;
}

#[derive(Debug, Clone, Copy, Default)]
pub struct ByteCounter;

fn estimate(tokens: u64) -> TokenCount {
    TokenCount {
        tokens,
        exact: false,
        method: CountMethod::ByteEstimate,
    }
}

impl TokenCounter for ByteCounter {
    fn count(
        &self,
        protocol: Protocol,
        _model: &str,
        context_toon: &str,
        questions: &[Question],
    ) -> Result<InputCounts, LimitError> {
        let shared = context_toon.len().div_ceil(4) as u64;
        // These are explicit estimates, not asserted provider token constants.
        let envelope = match protocol {
            Protocol::Typesafe | Protocol::OpenaiDecisions => 32,
            Protocol::OpenrouterDecisions => 48,
        };
        let base = shared.checked_add(envelope).ok_or(LimitError::Overflow)?;
        let mut total = base;
        let mut per_question = Vec::with_capacity(questions.len());
        for question in questions {
            let serialized = codec::encode_toon(question)?;
            let question_tokens = (serialized.len().div_ceil(4) as u64)
                .checked_add(16)
                .ok_or(LimitError::Overflow)?;
            total = total
                .checked_add(question_tokens)
                .ok_or(LimitError::Overflow)?;
            per_question.push(estimate(
                base.checked_add(question_tokens)
                    .ok_or(LimitError::Overflow)?,
            ));
        }
        Ok(InputCounts {
            total: estimate(total),
            per_question,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unicode_estimates_are_stable_and_complete_live_questions_count() {
        let request: crate::decision::TurnRequest = serde_json::from_str(include_str!(
            "../../../fixtures/contracts/decision-request.json"
        ))
        .unwrap();
        let count = ByteCounter
            .count(Protocol::Typesafe, "fixture", "🦀🦀", &request.questions)
            .unwrap();
        assert_eq!(
            count,
            ByteCounter
                .count(Protocol::Typesafe, "fixture", "🦀🦀", &request.questions)
                .unwrap()
        );
        assert_eq!(count.total.method, CountMethod::ByteEstimate);
        assert!(!count.total.exact);
        let mut more = request.questions.clone();
        more[0].criteria.push(crate::decision::Criterion::Text(
            "extra criteria".repeat(100),
        ));
        assert!(
            ByteCounter
                .count(Protocol::Typesafe, "fixture", "🦀🦀", &more)
                .unwrap()
                .total
                .tokens
                > count.total.tokens
        );
    }
}
