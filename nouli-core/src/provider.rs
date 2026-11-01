//! One-attempt protocol adapters. Evaluation owns retry orchestration.

mod confidence;
pub mod openai;
pub mod system_one;
pub mod wire;

use crate::{
    config::{HttpBinding, Protocol, resolve::ResolvedSettings},
    decision::{CONTRACT_VERSION, ContractError, Question, TurnRequest},
    limits::{LimitError, ResolvedLimits, count::TokenCounter},
    outcome::{ProviderIdentity, QuestionOutcome, TurnOutcome, Usage},
    transport::{HttpTransport, TransportError},
};
use serde::{Deserialize, de::DeserializeOwned};
use serde_json::Value;
use thiserror::Error;
use tokio_util::sync::CancellationToken;

#[derive(Debug, Error, PartialEq)]
pub enum ProviderError {
    #[error("provider protocol does not match the selected adapter")]
    Protocol,
    #[error("provider profile is invalid")]
    Profile,
    #[error("provider response does not match its documented schema")]
    Wire,
    #[error(transparent)]
    Codec(#[from] crate::codec::CodecError),
    #[error(transparent)]
    Contract(#[from] ContractError),
    #[error(transparent)]
    Limit(#[from] LimitError),
    #[error(transparent)]
    Transport(#[from] TransportError),
}

#[derive(Deserialize)]
struct NativeUsage {
    input_tokens: Option<u64>,
    output_tokens: Option<u64>,
}
#[derive(Deserialize)]
struct NativeResponse<A> {
    model: String,
    answers: A,
    usage: Option<NativeUsage>,
}
fn usage(native: Option<NativeUsage>) -> Usage {
    match native {
        Some(usage) => Usage {
            input_tokens: usage.input_tokens,
            output_tokens: usage.output_tokens,
            exact: usage.input_tokens.is_some() || usage.output_tokens.is_some(),
            cost: None,
        },
        None => Usage::default(),
    }
}

/// Shared response envelope handling; protocol adapters order and interpret only
/// their native answers. Public normalizers return a validated outcome.
fn normalize_response<A: DeserializeOwned, B>(
    request: &TurnRequest,
    response: Value,
    profile_name: &str,
    ordered_answers: impl FnOnce(&TurnRequest, A) -> Result<Vec<B>, ProviderError>,
    question_outcome: impl Fn(&Question, B, &ProviderIdentity) -> Result<QuestionOutcome, ProviderError>,
) -> Result<TurnOutcome, ProviderError> {
    request.validate()?;
    let response: NativeResponse<A> =
        serde_json::from_value(response).map_err(|_| ProviderError::Wire)?;
    let answers = ordered_answers(request, response.answers)?;
    let identity = ProviderIdentity {
        provider: profile_name.to_owned(),
        model: response.model,
        upstream_provider: None,
        request_id: None,
    };
    let outcomes = request
        .questions
        .iter()
        .zip(answers)
        .map(|(question, answer)| question_outcome(question, answer, &identity))
        .collect::<Result<Vec<_>, _>>()?;
    let outcome = TurnOutcome {
        version: CONTRACT_VERSION,
        provider: identity,
        outcomes,
        usage: usage(response.usage),
    };
    outcome.validate(request)?;
    Ok(outcome)
}

/// Send one decision request using the selected protocol's routing and normalization.
/// Retry orchestration belongs to the engine.
///
/// # Errors
///
/// - [`ProviderError::Limit`] if the profile or input exceeds its limits.
/// - Any error from [`wire::body`] or the selected protocol's normalizer.
/// - [`ProviderError::Transport`] if the request fails or its response is not JSON.
pub async fn evaluate(
    transport: &HttpTransport,
    settings: &ResolvedSettings,
    request: &TurnRequest,
    context_toon: &str,
    counter: &impl TokenCounter,
    cancel: &CancellationToken,
) -> Result<TurnOutcome, ProviderError> {
    type Normalize =
        fn(&TurnRequest, Value, &str, HttpBinding) -> Result<TurnOutcome, ProviderError>;
    let profile = &settings.snapshot.profile;
    let (default_path, normalize): (&str, Normalize) = match profile.protocol {
        Protocol::SystemOne => ("v1/systemone", wire::normalize),
        Protocol::OpenaiDecisions => ("v1/decisions", wire::normalize_openai),
    };
    ResolvedLimits::for_profile(profile)?.assess(context_toon, request, counter)?;
    let body = wire::body(profile, request, context_toon)?;
    let response = transport
        .post_with_headers(
            profile,
            profile
                .connection
                .request_path
                .as_deref()
                .unwrap_or(default_path),
            &body,
            settings.credential.as_ref(),
            &settings.headers,
            cancel,
        )
        .await?;
    normalize(
        request,
        response.json()?,
        &settings.snapshot.profile_name,
        profile.connection.binding.unwrap_or_default(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn fixture() -> (TurnRequest, Value) {
        let mut request: TurnRequest = serde_json::from_str(include_str!(
            "../../fixtures/contracts/decision-request.json"
        ))
        .unwrap();
        request.questions.truncate(3);
        (
            request,
            serde_json::from_str(include_str!(
                "../../fixtures/providers/typesafe/response.json"
            ))
            .unwrap(),
        )
    }

    #[test]
    fn binding_metadata_still_rejects_invalid_fields_after_native_validation() {
        let (request, response) = fixture();
        for (field, value, expected) in [
            (
                "provider",
                json!(" "),
                ContractError::Empty("upstream_provider"),
            ),
            (
                "id",
                json!(" "),
                ContractError::Empty("provider.request_id"),
            ),
            (
                "usage",
                json!({"cost":-0.5}),
                ContractError::Number("usage.cost"),
            ),
        ] {
            let mut invalid = response.clone();
            invalid[field] = value;
            assert_eq!(
                wire::normalize(&request, invalid, "fixture", HttpBinding::Standard),
                Err(ProviderError::Contract(expected))
            );
        }
        let mut invalid = response;
        invalid["model"] = json!(" ");
        invalid["provider"] = json!(" ");
        assert_eq!(
            wire::normalize(&request, invalid, "fixture", HttpBinding::Standard),
            Err(ProviderError::Contract(ContractError::Empty("model")))
        );
    }
}
