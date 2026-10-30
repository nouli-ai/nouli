//! OpenAI Decisions' native arrays, predicate answers, and per-question refusals.

use std::collections::HashMap;

use serde::Deserialize;
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

use super::{ProviderError, confidence::native_confidence};
use crate::{
    codec,
    config::{Protocol, resolve::ResolvedSettings},
    decision::{AnswerSpace, CONTRACT_VERSION, Criterion, TurnRequest},
    limits::{ResolvedLimits, count::TokenCounter},
    outcome::{Answer, OptionProbability, ProviderIdentity, QuestionOutcome, TurnOutcome, Usage},
    transport::HttpTransport,
};

#[derive(Deserialize)]
struct NativeResponse {
    model: String,
    answers: Vec<NativeAnswer>,
    usage: Option<NativeUsage>,
}
#[derive(Deserialize)]
struct NativeUsage {
    input_tokens: Option<u64>,
    output_tokens: Option<u64>,
}
#[derive(Deserialize)]
struct ChoiceProbability {
    value: String,
    probability: f64,
}
#[derive(Deserialize)]
struct LevelProbability {
    label: String,
    value: u64,
    probability: f64,
}
#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum NativeAnswer {
    Predicate {
        name: String,
        probability: f64,
    },
    Choice {
        name: String,
        choice: String,
        probabilities: Vec<ChoiceProbability>,
        confidence: Option<f64>,
    },
    Score {
        name: String,
        score: f64,
        probabilities: Vec<LevelProbability>,
        confidence: Option<f64>,
    },
    Refusal {
        name: String,
    },
}
impl NativeAnswer {
    fn name(&self) -> &str {
        match self {
            Self::Predicate { name, .. }
            | Self::Choice { name, .. }
            | Self::Score { name, .. }
            | Self::Refusal { name } => name,
        }
    }
}

fn level(criterion: &Criterion) -> (&str, &str) {
    match criterion {
        Criterion::Text(text) => (text, text),
        Criterion::Detailed { label, description } => (label, description),
    }
}

pub fn request_body(
    request: &TurnRequest,
    context_toon: &str,
    model: &str,
) -> Result<Value, ProviderError> {
    request.validate()?;
    crate::decision::nonempty(model, "model")?;
    let evidence = crate::media::Evidence::read(context_toon)?;
    let mut questions = Vec::with_capacity(request.questions.len());
    for question in &request.questions {
        let instructions = if question.criteria.is_empty() {
            question.prompt.clone()
        } else {
            format!(
                "{}\n\nCriteria:\n{}",
                question.prompt,
                codec::encode_toon(&question.criteria)?
            )
        };
        let native = match &question.answer_space {
            AnswerSpace::Noul => {
                json!({"name":question.id,"type":"predicate","instructions":instructions})
            }
            AnswerSpace::Choice { options } => {
                json!({"name":question.id,"type":"choice","instructions":instructions,"choices":options.iter().map(|o|json!({"value":o.id,"description":o.description})).collect::<Vec<_>>()})
            }
            AnswerSpace::Score { levels } => {
                json!({"name":question.id,"type":"score","instructions":instructions,"levels":levels.iter().map(|criterion|{let (label,description)=level(criterion);json!({"label":label,"description":description})}).collect::<Vec<_>>()})
            }
        };
        let mut native = native;
        native["instructions"] = evidence.openai_parts(&instructions);
        questions.push(native);
    }
    // Option descriptions are text-only on this protocol. Their references
    // identify the labelled image in shared input rather than losing it.
    let parts = evidence.openai_parts(&evidence.text);
    let input = if parts.is_array() {
        json!([{"role":"user", "content":parts}])
    } else {
        parts
    };
    Ok(json!({"model":model,"input":input,"questions":questions}))
}

pub fn normalize(
    request: &TurnRequest,
    response: Value,
    profile_name: &str,
) -> Result<TurnOutcome, ProviderError> {
    request.validate()?;
    let response: NativeResponse =
        serde_json::from_value(response).map_err(|_| ProviderError::Wire)?;
    if response.answers.len() != request.questions.len() {
        return Err(ProviderError::Wire);
    }
    let mut answers = HashMap::new();
    for answer in response.answers {
        let name = answer.name().to_owned();
        if !request.questions.iter().any(|q| q.id == name) || answers.insert(name, answer).is_some()
        {
            return Err(ProviderError::Wire);
        }
    }
    let identity = ProviderIdentity {
        provider: profile_name.to_owned(),
        model: response.model,
        upstream_provider: None,
        request_id: None,
    };
    let mut outcomes = Vec::with_capacity(request.questions.len());
    for question in &request.questions {
        let native = answers.remove(&question.id).ok_or(ProviderError::Wire)?;
        if matches!(native, NativeAnswer::Refusal { .. }) {
            outcomes.push(QuestionOutcome::Refused {
                question_id: question.id.clone(),
                reason: "Native provider refusal (no reason supplied)".into(),
            });
            continue;
        }
        let (answer, confidence) = match (native, &question.answer_space) {
            (NativeAnswer::Predicate { probability, .. }, AnswerSpace::Noul) => {
                (Answer::Noul { probability }, None)
            }
            (
                NativeAnswer::Choice {
                    choice,
                    probabilities,
                    confidence,
                    ..
                },
                AnswerSpace::Choice { options },
            ) => {
                let mut probabilities_by_id = HashMap::new();
                for entry in probabilities {
                    if probabilities_by_id
                        .insert(entry.value, entry.probability)
                        .is_some()
                    {
                        return Err(ProviderError::Wire);
                    }
                }
                if probabilities_by_id.len() != options.len() {
                    return Err(ProviderError::Wire);
                }
                let distribution = options
                    .iter()
                    .map(|option| {
                        Ok(OptionProbability {
                            option_id: option.id.clone(),
                            probability: *probabilities_by_id
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
                )
            }
            (
                NativeAnswer::Score {
                    score,
                    probabilities,
                    confidence,
                    ..
                },
                AnswerSpace::Score { levels },
            ) => {
                if probabilities.len() != levels.len() {
                    return Err(ProviderError::Wire);
                }
                let mut ordered = vec![None; levels.len()];
                for probability in probabilities {
                    let index =
                        usize::try_from(probability.value).map_err(|_| ProviderError::Wire)?;
                    let criterion = levels.get(index).ok_or(ProviderError::Wire)?;
                    if level(criterion).0 != probability.label
                        || ordered[index].replace(probability.probability).is_some()
                    {
                        return Err(ProviderError::Wire);
                    }
                }
                let probabilities = ordered
                    .into_iter()
                    .map(|p| p.ok_or(ProviderError::Wire))
                    .collect::<Result<Vec<_>, _>>()?;
                (
                    Answer::Score {
                        value: score,
                        probabilities,
                    },
                    confidence,
                )
            }
            _ => return Err(ProviderError::Wire),
        };
        outcomes.push(QuestionOutcome::Answered {
            question_id: question.id.clone(),
            answer,
            confidence: native_confidence(confidence, None, &identity)?,
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

pub async fn evaluate(
    transport: &HttpTransport,
    settings: &ResolvedSettings,
    request: &TurnRequest,
    context_toon: &str,
    counter: &impl TokenCounter,
    cancel: &CancellationToken,
) -> Result<TurnOutcome, ProviderError> {
    let profile = &settings.snapshot.profile;
    if profile.protocol != Protocol::OpenaiDecisions {
        return Err(ProviderError::Protocol);
    }
    ResolvedLimits::for_profile(profile)?.assess(context_toon, request, counter)?;
    let body = request_body(request, context_toon, &profile.model)?;
    let response = transport
        .post(
            profile,
            "v1/decisions",
            &body,
            settings.credential.as_ref(),
            cancel,
        )
        .await?;
    normalize(request, response.json()?, &settings.snapshot.profile_name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        config::{Config, ExecutionSettings, OutputFormat, resolve::SettingsSnapshot},
        limits::count::ByteCounter,
        transport::{TransportError, test_server::MockServer},
    };

    fn request() -> TurnRequest {
        serde_json::from_str(include_str!(
            "../../../fixtures/contracts/decision-request.json"
        ))
        .unwrap()
    }
    fn response() -> Value {
        serde_json::from_str(include_str!(
            "../../../fixtures/providers/openai/response.json"
        ))
        .unwrap()
    }
    fn settings(base_url: String) -> ResolvedSettings {
        let config =
            Config::parse(include_str!("../../../fixtures/config/providers.toml")).unwrap();
        let mut profile = config
            .resolve_snapshot(
                &crate::config::resolve::RuntimeOverrides {
                    selection: crate::history::ModelSelection {
                        provider: Some("openai".into()),
                        model: Some("gpt-6-luna".into()),
                    },
                    ..Default::default()
                },
                &Default::default(),
                &Default::default(),
                std::path::Path::new("/unused"),
            )
            .unwrap()
            .profile;
        profile.base_url = base_url;
        profile.context_budget_tokens = Some(4096);
        SettingsSnapshot {
            profile_name: "direct-openai".into(),
            profile,
            execution: ExecutionSettings::default(),
            output: OutputFormat::Json,
            store: "/unused".into(),
        }
        .with_credentials(|_| Some("fixture-key".into()))
        .unwrap()
    }

    #[tokio::test]
    async fn loopback_uses_only_decisions_and_preserves_answered_and_refused_siblings() {
        let mut server = MockServer::start().await;
        let settings = settings(format!("{}/gateway", server.url));
        let transport = HttpTransport::new(&settings.snapshot.execution).unwrap();
        let wire = serde_json::to_string(&response()).unwrap();
        let request = request();
        let context = "state: \"🦀 quotes, colon: text\"";
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
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{wire}",
                        wire.len()
                    )
                    .into_bytes(),
                );
                received
            }
        );
        let received = String::from_utf8(received).unwrap();
        assert!(received.starts_with("POST /gateway/v1/decisions HTTP/1.1"));
        assert!(
            received
                .to_ascii_lowercase()
                .contains("authorization: bearer fixture-key")
        );
        let body: Value = serde_json::from_str(received.split_once("\r\n\r\n").unwrap().1).unwrap();
        assert_eq!(body["input"], context);
        assert_eq!(body["questions"][1]["type"], "predicate");
        assert_eq!(body["questions"][2]["levels"][1]["label"], "Adequate");
        assert_eq!(
            body["questions"][2]["levels"][1]["description"],
            "Meets minimum requirements"
        );
        assert!(
            body["questions"][0]["instructions"]
                .as_str()
                .unwrap()
                .contains("Prefer reliability")
        );
        let result = result.unwrap();
        assert_eq!(result.provider.model, "gpt-6-luna-fixture");
        assert!(matches!(
            result.outcomes[2],
            QuestionOutcome::Answered {
                answer: Answer::Score { value: 1.43, .. },
                ..
            }
        ));
        assert!(matches!(
            result.outcomes[3],
            QuestionOutcome::Refused { .. }
        ));
        server.finish().await;
    }

    #[test]
    fn request_fixture_and_named_answer_reordering_are_deterministic() {
        let expected: Value = serde_json::from_str(include_str!(
            "../../../fixtures/providers/openai/request.json"
        ))
        .unwrap();
        assert_eq!(
            request_body(&request(), "evidence: fixture", "gpt-6-luna").unwrap(),
            expected
        );
        let original = normalize(&request(), response(), "openai").unwrap();
        let mut reordered = response();
        reordered["answers"].as_array_mut().unwrap().reverse();
        assert_eq!(
            normalize(&request(), reordered, "openai").unwrap(),
            original
        );
    }

    #[test]
    fn missing_duplicate_foreign_untyped_and_mismatched_native_values_fail() {
        for edit in 0..10 {
            let mut changed = response();
            match edit {
                0 => {
                    changed["answers"].as_array_mut().unwrap().pop();
                }
                1 => changed["answers"][3]["name"] = json!("travel"),
                2 => changed["answers"][3]["name"] = json!("foreign"),
                3 => changed["answers"][0]["choice"] = json!(true),
                4 => changed["answers"][2]["score"] = json!(1.0),
                5 => changed["answers"][2]["probabilities"][0]["label"] = json!("wrong scale"),
                6 => changed["answers"][2]["probabilities"][1]["value"] = json!(0),
                7 => {
                    changed["answers"][0]
                        .as_object_mut()
                        .unwrap()
                        .remove("name");
                }
                8 => {
                    changed["answers"][0]["probabilities"]
                        .as_array_mut()
                        .unwrap()
                        .pop();
                }
                _ => changed["answers"][1]["probability"] = json!(1.2),
            }
            assert!(normalize(&request(), changed, "openai").is_err());
        }
    }

    #[tokio::test]
    async fn capacity_authentication_and_transient_errors_remain_distinct() {
        for (status, body, expected) in [
            (
                400,
                include_str!("../../../fixtures/providers/openai/error.json"),
                TransportError::Capacity,
            ),
            (401, "{}", TransportError::Authentication { status: 401 }),
            (
                503,
                "{}",
                TransportError::Status {
                    status: 503,
                    retry_after: None,
                },
            ),
        ] {
            let mut server = MockServer::start().await;
            server.respond(format!("HTTP/1.1 {status} Fixture\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len()).into_bytes());
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
                Err(ProviderError::Transport(expected))
            );
            server.finish().await;
        }
    }
}
