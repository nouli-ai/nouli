//! Counting is a seam for complete protocol input, never generic tokenizer claims.

use super::LimitError;
use crate::{
    codec,
    config::{ImageEncoding, Protocol, ProviderProfile},
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
    /// Count the tokens for the shared context and each question under `profile`.
    ///
    /// # Errors
    ///
    /// Returns a [`LimitError`] if the input cannot be counted. [`ByteCounter`] returns
    /// [`LimitError::Codec`] for malformed or undecodable image evidence and
    /// [`LimitError::Overflow`] if a count overflows.
    fn count(
        &self,
        profile: &ProviderProfile,
        context_toon: &str,
        questions: &[Question],
    ) -> Result<InputCounts, LimitError>;
}

#[derive(Debug, Clone, Copy, Default)]
pub struct ByteCounter;

fn estimate(tokens: u64, multimodal: bool) -> TokenCount {
    TokenCount {
        tokens,
        exact: false,
        method: if multimodal {
            CountMethod::MultimodalEstimate
        } else {
            CountMethod::ByteEstimate
        },
    }
}

impl TokenCounter for ByteCounter {
    #[expect(
        clippy::too_many_lines,
        reason = "pending refactor into smaller functions"
    )]
    fn count(
        &self,
        profile: &ProviderProfile,
        context_toon: &str,
        questions: &[Question],
    ) -> Result<InputCounts, LimitError> {
        let protocol = profile.protocol;
        let encoding =
            profile
                .connection
                .image_encoding
                .unwrap_or(if protocol == Protocol::SystemOne {
                    ImageEncoding::Assets
                } else {
                    ImageEncoding::OpenaiContent
                });
        let shared_only = matches!(
            encoding,
            ImageEncoding::ImagesBase64
                | ImageEncoding::ImagesDataUrls
                | ImageEncoding::StateImageUrls
        );
        let evidence = crate::media::Evidence::read(context_toon)?;
        let multimodal = !evidence.images.is_empty();
        let mut shared = evidence.text.len().div_ceil(4) as u64;
        let mut image_costs = Vec::new();
        for image in &evidence.images {
            let cost = if let Some(patch) = profile.connection.pixel_patch_size {
                let prepared = image.prepare(&profile.connection)?;
                let (width, height) = prepared.dimensions()?;
                u64::from(width.div_ceil(patch))
                    .checked_mul(u64::from(height.div_ceil(patch)))
                    .and_then(|n| n.checked_mul(profile.connection.pixel_multiplier.unwrap_or(4)))
                    .and_then(|n| n.checked_add(profile.connection.pixel_overhead.unwrap_or(256)))
                    .ok_or(LimitError::Overflow)?
            } else {
                image
                    .prepare(&profile.connection)?
                    .data_base64
                    .len()
                    .div_ceil(4) as u64
            };
            let shared_uses = if shared_only {
                1
            } else {
                evidence.text.matches(&image.reference).count() as u64
            };
            shared = shared
                .checked_add(cost.checked_mul(shared_uses).ok_or(LimitError::Overflow)?)
                .ok_or(LimitError::Overflow)?;
            image_costs.push((image.reference.as_str(), cost));
        }
        // These are explicit estimates, not asserted provider token constants.
        let envelope = match protocol {
            Protocol::SystemOne | Protocol::OpenaiDecisions => 32,
        };
        let base = shared.checked_add(envelope).ok_or(LimitError::Overflow)?;
        let mut total = base;
        let mut per_question = Vec::with_capacity(questions.len());
        for question in questions {
            fn occurrences(value: &serde_json::Value, reference: &str) -> u64 {
                match value {
                    serde_json::Value::String(text) => text.matches(reference).count() as u64,
                    serde_json::Value::Array(values) => values
                        .iter()
                        .map(|value| occurrences(value, reference))
                        .sum(),
                    serde_json::Value::Object(values) => values
                        .values()
                        .map(|value| occurrences(value, reference))
                        .sum(),
                    _ => 0,
                }
            }
            let serialized = codec::encode_toon(question)?;
            let mut question_tokens = (serialized.len().div_ceil(4) as u64)
                .checked_add(16)
                .ok_or(LimitError::Overflow)?;
            let values = if shared_only {
                serde_json::Value::Null
            } else if protocol == Protocol::OpenaiDecisions {
                serde_json::to_value((&question.prompt, &question.criteria))
                    .map_err(|_| crate::codec::CodecError::Shape)?
            } else {
                serde_json::to_value((&question.prompt, &question.criteria, &question.answer_space))
                    .map_err(|_| crate::codec::CodecError::Shape)?
            };
            for (reference, cost) in &image_costs {
                question_tokens = question_tokens
                    .checked_add(
                        cost.checked_mul(occurrences(&values, reference))
                            .ok_or(LimitError::Overflow)?,
                    )
                    .ok_or(LimitError::Overflow)?;
            }
            total = total
                .checked_add(question_tokens)
                .ok_or(LimitError::Overflow)?;
            per_question.push(estimate(
                base.checked_add(question_tokens)
                    .ok_or(LimitError::Overflow)?,
                multimodal,
            ));
        }
        Ok(InputCounts {
            total: estimate(total, multimodal),
            per_question,
        })
    }
}

#[cfg(test)]
pub(crate) mod tests {
    pub(crate) fn profile(
        protocol: crate::config::Protocol,
        model: &str,
    ) -> crate::config::ProviderProfile {
        crate::config::ProviderProfile {
            protocol,
            model: model.into(),
            base_url: "http://localhost:8000".into(),
            api_key_env: None,
            stored_credential: false,
            connection: crate::config::ConnectionSettings {
                image_encoding: Some(if model == "cloudflare/clef" {
                    crate::config::ImageEncoding::StateImageUrls
                } else if protocol == crate::config::Protocol::OpenaiDecisions {
                    crate::config::ImageEncoding::OpenaiContent
                } else {
                    crate::config::ImageEncoding::Assets
                }),
                pixel_patch_size: if model == "cloudflare/clef" {
                    None
                } else {
                    Some(28)
                },
                min_image_dimension: Some(280),
                max_image_dimension: Some(1568),
                ..Default::default()
            },
            limits: ModelSettings::default(),
            context_budget_tokens: None,
            context_window_tokens: None,
            reserved_output_tokens: None,
            max_attempts: None,
            attempt_timeout_seconds: None,
        }
    }
    use super::*;
    use crate::config::ModelSettings;

    #[test]
    fn images_use_separate_estimates_and_each_reference_is_counted() {
        let picture = crate::media::tests::picture(1912, 1044);
        let context = codec::encode_toon(&serde_json::json!({"records":[{"record":{"key":"image:fixture", "value":{"nouli_image":picture}}}]})).unwrap();
        let mut question = crate::decision::Question {
            id: "image".into(),
            prompt: "Describe the image.".into(),
            criteria: vec![],
            answer_space: crate::decision::AnswerSpace::Noul,
        };
        let count = ByteCounter
            .count(
                &crate::limits::count::tests::profile(Protocol::SystemOne, "jev-1.14.0"),
                &context,
                &[question.clone()],
            )
            .unwrap();
        assert!(count.per_question[0].tokens < 32_000);
        assert_eq!(count.total.method, CountMethod::MultimodalEstimate);
        assert!(!count.total.exact);
        question.prompt = "Compare [photo #123] and [photo #123].".into();
        let repeated = ByteCounter
            .count(
                &crate::limits::count::tests::profile(Protocol::SystemOne, "jev-1.14.0"),
                &context,
                &[question],
            )
            .unwrap();
        assert!(repeated.per_question[0].tokens > count.per_question[0].tokens * 2);
        let clef = ByteCounter
            .count(
                &crate::limits::count::tests::profile(Protocol::SystemOne, "cloudflare/clef"),
                &context,
                &[],
            )
            .unwrap();
        let prepared = picture
            .prepare(
                &crate::limits::count::tests::profile(Protocol::SystemOne, "cloudflare/clef")
                    .connection,
            )
            .unwrap();
        assert!(clef.total.tokens >= prepared.data_base64.len().div_ceil(4) as u64);
        let mut question = crate::decision::Question {
            id: "test".into(),
            prompt: "Look at this.".into(),
            criteria: vec![],
            answer_space: crate::decision::AnswerSpace::Noul,
        };
        let plain = ByteCounter
            .count(
                &crate::limits::count::tests::profile(Protocol::SystemOne, "cloudflare/clef"),
                &context,
                &[question.clone()],
            )
            .unwrap();
        question.prompt = "Look at [photo #123] and [photo #123].".into();
        let refs = ByteCounter
            .count(
                &crate::limits::count::tests::profile(Protocol::SystemOne, "cloudflare/clef"),
                &context,
                &[question.clone()],
            )
            .unwrap();
        assert!(
            refs.total.tokens.abs_diff(plain.total.tokens) < 100,
            "router references stay text; one native image is sent"
        );
        question.answer_space = crate::decision::AnswerSpace::Choice {
            options: vec![
                crate::decision::ChoiceOption {
                    id: "a".into(),
                    description: "First".into(),
                },
                crate::decision::ChoiceOption {
                    id: "b".into(),
                    description: "Second".into(),
                },
            ],
        };
        let plain = ByteCounter
            .count(
                &crate::limits::count::tests::profile(Protocol::OpenaiDecisions, "gpt-6-luna"),
                &context,
                &[question.clone()],
            )
            .unwrap();
        if let crate::decision::AnswerSpace::Choice { options } = &mut question.answer_space {
            options[0].description = "[photo #123]".into();
        }
        let refs = ByteCounter
            .count(
                &crate::limits::count::tests::profile(Protocol::OpenaiDecisions, "gpt-6-luna"),
                &context,
                &[question],
            )
            .unwrap();
        assert!(
            refs.total.tokens.abs_diff(plain.total.tokens) < 100,
            "OpenAI option descriptions stay text"
        );
    }
    #[test]
    fn unicode_estimates_are_stable_and_complete_live_questions_count() {
        let request: crate::decision::TurnRequest = serde_json::from_str(include_str!(
            "../../../fixtures/contracts/decision-request.json"
        ))
        .unwrap();
        let count = ByteCounter
            .count(
                &crate::limits::count::tests::profile(Protocol::SystemOne, "fixture"),
                "🦀🦀",
                &request.questions,
            )
            .unwrap();
        assert_eq!(
            count,
            ByteCounter
                .count(
                    &crate::limits::count::tests::profile(Protocol::SystemOne, "fixture"),
                    "🦀🦀",
                    &request.questions
                )
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
                .count(
                    &crate::limits::count::tests::profile(Protocol::SystemOne, "fixture"),
                    "🦀🦀",
                    &more
                )
                .unwrap()
                .total
                .tokens
                > count.total.tokens
        );
    }
}
