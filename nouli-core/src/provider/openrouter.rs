//! OpenRouter's declared Decisions protocol, metadata, and endpoint.

use serde::Deserialize;
use serde_json::Value;
use tokio_util::sync::CancellationToken;

use super::{ProviderError, typesafe};
use crate::{
    config::{Protocol, resolve::ResolvedSettings},
    decision::TurnRequest,
    limits::{ResolvedLimits, count::TokenCounter},
    outcome::TurnOutcome,
    transport::HttpTransport,
};

#[derive(Deserialize)]
struct RouterMetadata {
    id: Option<String>,
    provider: Option<String>,
    usage: Option<RouterUsage>,
}
#[derive(Deserialize)]
struct RouterUsage {
    cost: Option<f64>,
}

/// Only shared map-based wire conversion is reused. Network path, capability
/// lookup, selected model, and metadata normalization remain router-specific.
pub fn request_body(
    request: &TurnRequest,
    context_toon: &str,
    model: &str,
) -> Result<Value, ProviderError> {
    let mut body = typesafe::request_body_with_images(
        request,
        context_toon,
        model,
        model.contains("typesafe/jev"),
    )?;
    if !model.contains("typesafe/jev") {
        let evidence = crate::media::Evidence::read(context_toon)?;
        if !evidence.images.is_empty() {
            body.as_object_mut()
                .expect("request object")
                .remove("assets");
            let mut state = vec![serde_json::json!(evidence.text)];
            for image in evidence.images {
                state.push(serde_json::json!(format!(
                    "Image {}: {}",
                    image.name, image.reference
                )));
                state.push(
                    serde_json::json!({"type":"image_url","image_url":{"url":image.data_url()}}),
                );
            }
            body["state"] = serde_json::json!(state);
        }
    }
    body["provider"] = serde_json::json!({"allow_fallbacks":false});
    Ok(body)
}

pub fn normalize(
    request: &TurnRequest,
    response: Value,
    profile_name: &str,
) -> Result<TurnOutcome, ProviderError> {
    let metadata: RouterMetadata =
        RouterMetadata::deserialize(&response).map_err(|_| ProviderError::Wire)?;
    let mut outcome = typesafe::normalize(request, response, profile_name)?;
    outcome.provider.upstream_provider = metadata.provider;
    outcome.provider.request_id = metadata.id;
    outcome.usage.cost = metadata.usage.and_then(|usage| usage.cost);
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
    if profile.protocol != Protocol::OpenrouterDecisions {
        return Err(ProviderError::Protocol);
    }
    ResolvedLimits::for_profile(profile)?.assess(context_toon, request, counter)?;
    let body = request_body(request, context_toon, &profile.model)?;
    let response = transport
        .post(
            profile,
            "alpha/decisions",
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
        outcome::QuestionOutcome,
        transport::{TransportError, retry::RetryPolicy, test_server::MockServer},
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
            "../../../fixtures/providers/openrouter/response.json"
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
                        provider: Some("router".into()),
                        model: Some("openai/gpt-6-luna-decisions".into()),
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
            profile_name: "router".into(),
            profile,
            execution: ExecutionSettings::default(),
            output: OutputFormat::Json,
            store: "/unused".into(),
        }
        .with_credentials(|_| Some("fixture-key".into()))
        .unwrap()
    }

    #[tokio::test]
    async fn router_wire_preserves_prefix_model_and_native_metadata() {
        let mut server = MockServer::start().await;
        let settings = settings(format!("{}/proxy/api", server.url));
        let transport = HttpTransport::new(&settings.snapshot.execution).unwrap();
        let wire = serde_json::to_string(&response()).unwrap();
        let request = request();
        let cancel = CancellationToken::new();
        let (result, received) = tokio::join!(
            evaluate(
                &transport,
                &settings,
                &request,
                "evidence: fixture",
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
        assert!(received.starts_with("POST /proxy/api/alpha/decisions HTTP/1.1"));
        assert!(!received.contains("/api/api/"));
        assert!(
            received
                .to_ascii_lowercase()
                .contains("authorization: bearer fixture-key")
        );
        let body: Value = serde_json::from_str(received.split_once("\r\n\r\n").unwrap().1).unwrap();
        let expected: Value = serde_json::from_str(include_str!(
            "../../../fixtures/providers/openrouter/request.json"
        ))
        .unwrap();
        assert_eq!(body, expected);
        assert_eq!(body["provider"]["allow_fallbacks"], false);
        let result = result.unwrap();
        assert_eq!(result.provider.upstream_provider.as_deref(), Some("OpenAI"));
        assert_eq!(
            result.provider.request_id.as_deref(),
            Some("fixture-router-request")
        );
        assert_eq!(result.usage.cost, Some(0.0000123));
        assert!(matches!(
            result.outcomes[2],
            QuestionOutcome::Answered {
                answer: crate::outcome::Answer::Score { value: 1.43, .. },
                ..
            }
        ));
        server.finish().await;
    }

    #[test]
    fn optional_metadata_stays_absent_and_malformed_values_are_errors() {
        let mut absent = response();
        absent.as_object_mut().unwrap().remove("provider");
        absent.as_object_mut().unwrap().remove("id");
        absent["usage"].as_object_mut().unwrap().remove("cost");
        let outcome = normalize(&request(), absent, "router").unwrap();
        assert_eq!(outcome.provider.upstream_provider, None);
        assert_eq!(outcome.provider.request_id, None);
        assert_eq!(outcome.usage.cost, None);
        for edit in 0..5 {
            let mut invalid = response();
            match edit {
                0 => invalid["provider"] = Value::Bool(true),
                1 => invalid["usage"]["cost"] = serde_json::json!(-1.0),
                2 => invalid["id"] = serde_json::json!(""),
                3 => {
                    invalid["answers"].as_object_mut().unwrap().remove("travel");
                }
                _ => invalid["answers"]["travel"]["type"] = serde_json::json!("refusal"),
            }
            assert!(normalize(&request(), invalid, "router").is_err());
        }
    }

    #[tokio::test]
    async fn router_transient_status_uses_shared_policy_without_adapter_retry() {
        let mut server = MockServer::start().await;
        let body = include_str!("../../../fixtures/providers/openrouter/error.json");
        server.respond(
            format!(
                "HTTP/1.1 529 Fixture\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            )
            .into_bytes(),
        );
        let settings = settings(server.url.clone());
        let transport = HttpTransport::new(&settings.snapshot.execution).unwrap();
        let result = evaluate(
            &transport,
            &settings,
            &request(),
            "evidence: fixture",
            &ByteCounter,
            &CancellationToken::new(),
        )
        .await;
        let error = TransportError::Status {
            status: 529,
            retry_after: None,
        };
        assert_eq!(result, Err(ProviderError::Transport(error.clone())));
        assert!(
            RetryPolicy::new(&settings.snapshot.execution)
                .unwrap()
                .next_delay(1, &error, std::time::SystemTime::UNIX_EPOCH)
                .is_some()
        );
        server.finish().await;
    }
}
