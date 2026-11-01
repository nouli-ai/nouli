//! Shared System One decision codec.

use std::collections::BTreeMap;

use serde::Deserialize;
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

use super::ProviderError;
use crate::{
    config::{Protocol, resolve::ResolvedSettings},
    decision::{AnswerSpace, CONTRACT_VERSION, TurnRequest},
    limits::{ResolvedLimits, count::TokenCounter},
    outcome::{Answer, OptionProbability, ProviderIdentity, QuestionOutcome, TurnOutcome, Usage},
    transport::HttpTransport,
};

#[derive(Deserialize)]
struct NativeResponse {
    model: String,
    answers: BTreeMap<String, NativeAnswer>,
    usage: Option<NativeUsage>,
}

#[derive(Deserialize)]
struct NativeUsage {
    input_tokens: Option<u64>,
    output_tokens: Option<u64>,
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum NativeAnswer {
    Choice {
        choice: String,
        probabilities: BTreeMap<String, f64>,
        confidence: Option<f64>,
        answer_confidence: Option<f64>,
    },
    Noul {
        noul: f64,
        confidence: Option<f64>,
        answer_confidence: Option<f64>,
    },
    Score {
        score: f64,
        probabilities: BTreeMap<String, f64>,
        legend: Option<BTreeMap<String, Value>>,
        confidence: Option<f64>,
        answer_confidence: Option<f64>,
    },
}

/// Complete live criteria remain in the native question. Shared context is one
/// escaped string field, never concatenated into provider JSON syntax.
///
/// # Errors
///
/// - [`ProviderError::Contract`] if the request is invalid or `model` is blank.
/// - [`ProviderError::Codec`] if the context has malformed image evidence.
pub fn request_body(
    request: &TurnRequest,
    context_toon: &str,
    model: &str,
) -> Result<Value, ProviderError> {
    request.validate()?;
    crate::decision::nonempty(model, "model")?;
    let mut questions = BTreeMap::new();
    for question in &request.questions {
        let instructions = if question.criteria.is_empty() {
            json!(question.prompt)
        } else {
            json!({"question":question.prompt,"criteria":question.criteria})
        };
        let native = match &question.answer_space {
            AnswerSpace::Noul => json!({"type":"noul","instructions":instructions}),
            AnswerSpace::Choice { options } => {
                json!({"type":"choice","instructions":instructions,"criteria":options.iter().map(|option|(&option.id,&option.description)).collect::<BTreeMap<_,_>>()})
            }
            AnswerSpace::Score { levels } => {
                json!({"type":"score","instructions":instructions,"criteria":levels})
            }
        };
        questions.insert(&question.id, native);
    }
    let evidence = crate::media::Evidence::read(context_toon)?;
    let mut body = json!({"model":model,"state":evidence.text,"questions":questions});
    if !evidence.images.is_empty() {
        body["assets"] = evidence.assets()?;
    }
    Ok(body)
}

/// Normalize only documented primitives. Unknown refusal types fail rather than
/// inventing a native abstention protocol or a successful missing answer.
///
/// # Errors
///
/// - [`ProviderError::Contract`] if the request or resulting outcome is invalid.
/// - [`ProviderError::Wire`] if the response does not match the schema, or its answers
///   do not cover each question once with matching kinds, options, levels, and legend.
/// - Any error from converting native confidence.
#[expect(
    clippy::too_many_lines,
    reason = "pending refactor into smaller functions"
)]
pub fn normalize(
    request: &TurnRequest,
    response: Value,
    profile_name: &str,
) -> Result<TurnOutcome, ProviderError> {
    request.validate()?;
    let response: NativeResponse =
        serde_json::from_value(response).map_err(|_| ProviderError::Wire)?;
    if response.answers.len() != request.questions.len()
        || response
            .answers
            .keys()
            .any(|id| !request.questions.iter().any(|q| &q.id == id))
    {
        return Err(ProviderError::Wire);
    }
    let identity = ProviderIdentity {
        provider: profile_name.to_owned(),
        model: response.model,
        upstream_provider: None,
        request_id: None,
    };
    let mut answers = response.answers;
    let mut outcomes = Vec::with_capacity(request.questions.len());
    for question in &request.questions {
        let native = answers.remove(&question.id).ok_or(ProviderError::Wire)?;
        let (answer, native_confidence, selected_confidence) =
            match (native, &question.answer_space) {
                (
                    NativeAnswer::Choice {
                        choice,
                        probabilities,
                        confidence,
                        answer_confidence,
                    },
                    AnswerSpace::Choice { options },
                ) => {
                    if probabilities.len() != options.len() {
                        return Err(ProviderError::Wire);
                    }
                    let distribution = options
                        .iter()
                        .map(|option| {
                            Ok(OptionProbability {
                                option_id: option.id.clone(),
                                probability: *probabilities
                                    .get(&option.id)
                                    .ok_or(ProviderError::Wire)?,
                            })
                        })
                        .collect::<Result<Vec<_>, ProviderError>>()?;
                    (
                        Answer::Choice {
                            selected: choice,
                            distribution,
                        },
                        confidence,
                        answer_confidence,
                    )
                }
                (
                    NativeAnswer::Noul {
                        noul,
                        confidence,
                        answer_confidence,
                    },
                    AnswerSpace::Noul,
                ) => (
                    Answer::Noul { probability: noul },
                    confidence,
                    answer_confidence,
                ),
                (
                    NativeAnswer::Score {
                        score,
                        probabilities,
                        legend,
                        confidence,
                        answer_confidence,
                    },
                    AnswerSpace::Score { levels },
                ) => {
                    if probabilities.len() != levels.len() {
                        return Err(ProviderError::Wire);
                    }
                    if let Some(legend) = legend {
                        if legend.len() != levels.len() {
                            return Err(ProviderError::Wire);
                        }
                        for (index, level) in levels.iter().enumerate() {
                            let expected =
                                serde_json::to_value(level).map_err(|_| ProviderError::Wire)?;
                            if legend.get(&index.to_string()) != Some(&expected) {
                                return Err(ProviderError::Wire);
                            }
                        }
                    }
                    let probabilities = (0..levels.len())
                        .map(|index| {
                            probabilities
                                .get(&index.to_string())
                                .copied()
                                .ok_or(ProviderError::Wire)
                        })
                        .collect::<Result<Vec<_>, _>>()?;
                    (
                        Answer::Score {
                            value: score,
                            probabilities,
                        },
                        confidence,
                        answer_confidence,
                    )
                }
                _ => return Err(ProviderError::Wire),
            };
        outcomes.push(QuestionOutcome::Answered {
            question_id: question.id.clone(),
            answer,
            confidence: super::confidence::native_confidence(
                native_confidence,
                selected_confidence,
                &identity,
            )?,
        });
    }
    let usage = match response.usage {
        Some(usage) => Usage {
            input_tokens: usage.input_tokens,
            output_tokens: usage.output_tokens,
            exact: usage.input_tokens.is_some() || usage.output_tokens.is_some(),
            cost: None,
        },
        None => Usage::default(),
    };
    let outcome = TurnOutcome {
        version: CONTRACT_VERSION,
        provider: identity,
        outcomes,
        usage,
    };
    outcome.validate(request)?;
    Ok(outcome)
}

/// Send a turn to a `SystemOne` provider and normalize its response.
///
/// # Errors
///
/// - [`ProviderError::Protocol`] if the profile does not use `SystemOne`.
/// - [`ProviderError::Limit`] if the profile limits are invalid or the input exceeds them.
/// - Any error from [`crate::provider::wire::body`] or [`crate::provider::wire::normalize`].
/// - [`ProviderError::Transport`] if the request fails or the response is not JSON.
pub async fn evaluate(
    transport: &HttpTransport,
    settings: &ResolvedSettings,
    request: &TurnRequest,
    context_toon: &str,
    counter: &impl TokenCounter,
    cancel: &CancellationToken,
) -> Result<TurnOutcome, ProviderError> {
    let profile = &settings.snapshot.profile;
    if profile.protocol != Protocol::SystemOne {
        return Err(ProviderError::Protocol);
    }
    ResolvedLimits::for_profile(profile)?.assess(context_toon, request, counter)?;
    let body = crate::provider::wire::body(profile, request, context_toon)?;
    let response = transport
        .post_with_headers(
            profile,
            profile
                .connection
                .request_path
                .as_deref()
                .unwrap_or("v1/systemone"),
            &body,
            settings.credential.as_ref(),
            &settings.headers,
            cancel,
        )
        .await?;
    crate::provider::wire::normalize(
        request,
        response.json()?,
        &settings.snapshot.profile_name,
        profile.connection.binding.unwrap_or_default(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ConnectionSettings;
    use crate::config::ModelSettings;
    use crate::{
        config::{ExecutionSettings, OutputFormat, resolve::SettingsSnapshot},
        limits::count::ByteCounter,
        transport::test_server::MockServer,
    };

    fn request() -> TurnRequest {
        let mut request: TurnRequest = serde_json::from_str(include_str!(
            "../../../fixtures/contracts/decision-request.json"
        ))
        .unwrap();
        request.questions.truncate(3);
        request
    }

    fn response() -> Value {
        serde_json::from_str(include_str!(
            "../../../fixtures/providers/typesafe/response.json"
        ))
        .unwrap()
    }

    fn settings(base_url: String) -> ResolvedSettings {
        SettingsSnapshot {
            format_version: 2,
            profile_name: "local-laya".into(),
            profile: crate::config::ProviderProfile {
                connection: ConnectionSettings::default(),
                limits: ModelSettings::default(),
                protocol: Protocol::SystemOne,
                base_url,
                model: "english".into(),
                api_key_env: None,
                stored_credential: false,
                context_budget_tokens: Some(4096),
                context_window_tokens: None,
                reserved_output_tokens: None,
                max_attempts: None,
                attempt_timeout_seconds: None,
            },
            execution: ExecutionSettings::default(),
            output: OutputFormat::Json,
            store: "/unused".into(),
        }
        .with_credentials(|_| panic!("local profiles do not read credentials"))
        .unwrap()
    }

    #[tokio::test]
    async fn loopback_preserves_native_request_state_and_all_primitive_semantics() {
        let mut server = MockServer::start().await;
        let settings = settings(format!("{}/prefix", server.url));
        let transport = HttpTransport::new(&settings.snapshot.execution).unwrap();
        let body = serde_json::to_string(&response()).unwrap();
        let context = "evidence: \"quotes, colons: and 🦀\"\nvalue: null";
        let request = request();
        let cancel = CancellationToken::new();
        let (result, received) = tokio::join!(
            evaluate(
                &transport,
                &settings,
                &request,
                context,
                &ByteCounter,
                &cancel
            ),
            async {
                let received = server.received().await;
                server.respond(
                    format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    )
                    .into_bytes(),
                );
                received
            }
        );
        let received = String::from_utf8(received).unwrap();
        assert!(received.starts_with("POST /prefix/v1/systemone HTTP/1.1"));
        assert!(!received.to_ascii_lowercase().contains("authorization:"));
        let body: Value = serde_json::from_str(received.split_once("\r\n\r\n").unwrap().1).unwrap();
        assert_eq!(body["state"], context);
        assert_eq!(
            body["questions"]["quality"]["criteria"][1]["label"],
            "Adequate"
        );
        assert!(body["questions"]["travel"]["instructions"]["criteria"].is_array());
        let result = result.unwrap();
        assert_eq!(result.provider.model, "laya-english-fixture");
        assert!(matches!(
            result.outcomes[2],
            QuestionOutcome::Answered {
                answer: Answer::Score { value: 1.43, .. },
                ..
            }
        ));
        let QuestionOutcome::Answered {
            confidence: Some(confidence),
            ..
        } = &result.outcomes[0]
        else {
            panic!()
        };
        assert_eq!(confidence.value, Some(0.35));
        assert_eq!(confidence.answer_confidence, Some(0.7));
        assert!(confidence.meaning.contains("local-laya"));
        server.finish().await;
    }

    #[test]
    fn native_request_fixture_matches_portable_conversion() {
        let expected: Value = serde_json::from_str(include_str!(
            "../../../fixtures/providers/typesafe/request.json"
        ))
        .unwrap();
        assert_eq!(
            request_body(&request(), "evidence: fixture", "english").unwrap(),
            expected
        );
    }

    #[test]
    fn malformed_coverage_types_distributions_and_scores_fail_without_fabrication() {
        let request = request();
        let original = response();
        for edit in 0..7 {
            let mut changed = original.clone();
            match edit {
                0 => {
                    changed["answers"].as_object_mut().unwrap().remove("travel");
                }
                1 => {
                    changed["answers"]["foreign"] = changed["answers"]["travel"].clone();
                }
                2 => changed["answers"]["travel"]["choice"] = json!("foreign"),
                3 => changed["answers"]["travel"]["probabilities"]["train"] = json!(0.9),
                4 => changed["answers"]["quality"]["score"] = json!(1.0),
                5 => changed["answers"]["on_time"]["type"] = json!("refusal"),
                _ => changed["answers"]["quality"]["probabilities"]["3"] = json!(0.0),
            }
            assert!(normalize(&request, changed, "fixture").is_err());
        }
    }

    #[test]
    fn answer_confidence_does_not_fill_absent_distribution_confidence() {
        let mut response = response();
        response["answers"]["travel"]
            .as_object_mut()
            .unwrap()
            .remove("confidence");
        let outcome = normalize(&request(), response, "local").unwrap();
        let QuestionOutcome::Answered {
            confidence: Some(confidence),
            ..
        } = &outcome.outcomes[0]
        else {
            panic!()
        };
        assert_eq!(confidence.value, None);
        assert_eq!(confidence.answer_confidence, Some(0.7));
    }

    #[tokio::test]
    async fn native_capacity_fixture_maps_to_transport_capacity() {
        let mut server = MockServer::start().await;
        let body = include_str!("../../../fixtures/providers/typesafe/error.json");
        server.respond(
            format!(
                "HTTP/1.1 400 Invalid\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            )
            .into_bytes(),
        );
        let settings = settings(server.url.clone());
        let transport = HttpTransport::new(&settings.snapshot.execution).unwrap();
        assert_eq!(
            evaluate(
                &transport,
                &settings,
                &request(),
                "state: fixture",
                &ByteCounter,
                &CancellationToken::new()
            )
            .await,
            Err(ProviderError::Transport(
                crate::transport::TransportError::Capacity
            ))
        );
        server.finish().await;
    }
}
