//! Shared wire preparation; model IDs and hosts never select behavior.
use super::ProviderError;
use crate::{
    config::{HttpBinding, ImageEncoding, Protocol, ProviderProfile},
    decision::TurnRequest,
    outcome::TurnOutcome,
};
use serde_json::{Value, json};
/// Build the provider request body for the profile's protocol and image encoding.
///
/// # Errors
///
/// - [`ProviderError::Profile`] if the profile is invalid.
/// - Any error from the protocol's request body builder.
/// - [`ProviderError::Codec`] if image evidence is malformed or an image cannot be prepared.
/// - [`ProviderError::Limit`] with [`LimitError::RequestBytes`](crate::limits::LimitError::RequestBytes)
///   if the body exceeds the profile's request byte limit.
pub fn body(
    profile: &ProviderProfile,
    request: &TurnRequest,
    context: &str,
) -> Result<Value, ProviderError> {
    body_prepared(
        profile,
        request,
        context,
        &crate::media::PreparedImages::default(),
    )
}
pub(crate) fn body_prepared(
    profile: &ProviderProfile,
    request: &TurnRequest,
    context: &str,
    images: &crate::media::PreparedImages,
) -> Result<Value, ProviderError> {
    profile.validate().map_err(|_| ProviderError::Profile)?;
    let model = profile
        .connection
        .wire_model
        .as_deref()
        .unwrap_or(&profile.model);
    request.validate()?;
    crate::decision::nonempty(model, "model")?;
    let evidence = images.evidence(context)?;
    let mut body = match profile.protocol {
        Protocol::OpenaiDecisions => super::openai::body_from_evidence(request, &evidence, model)?,
        Protocol::SystemOne => super::system_one::body_from_evidence(request, &evidence, model),
    };
    let encoding = profile.image_encoding();
    if !evidence.images.is_empty() {
        let prepared = (0..evidence.images.len())
            .map(|index| images.image(&evidence, index, &profile.connection))
            .collect::<Result<Vec<_>, _>>()?;
        match encoding {
            ImageEncoding::Assets => {
                body["assets"] = assets_iter(prepared.iter().map(|image| &image.image));
            }
            ImageEncoding::ImagesBase64 | ImageEncoding::ImagesDataUrls => {
                body["images"] = json!(
                    prepared
                        .iter()
                        .map(|i| if encoding == ImageEncoding::ImagesBase64 {
                            i.image.data_base64.clone()
                        } else {
                            i.image.data_url()
                        })
                        .collect::<Vec<_>>()
                );
            }
            ImageEncoding::StateImageUrls => {
                let mut state = vec![json!(evidence.text)];
                for (original, i) in evidence.images.iter().zip(&prepared) {
                    state.push(json!(format!(
                        "Image {}: {}",
                        original.name, original.reference
                    )));
                    state.push(json!({"type":"image_url","image_url":{"url":i.image.data_url()}}));
                }
                body["state"] = json!(state);
            }
            ImageEncoding::OpenaiContent => {
                fn replace(value: &mut Value, from: &str, to: &str) {
                    match value {
                        Value::String(s) if s == from => *s = to.into(),
                        Value::Array(v) => {
                            for x in v {
                                replace(x, from, to);
                            }
                        }
                        Value::Object(v) => {
                            for x in v.values_mut() {
                                replace(x, from, to);
                            }
                        }
                        _ => {}
                    }
                }
                for (original, prepared) in evidence.images.iter().zip(prepared.iter()) {
                    replace(&mut body, &original.data_url(), &prepared.image.data_url());
                }
            }
        }
    }
    for (key, value) in &profile.connection.extra_body {
        body[key] = value.clone();
    }
    if profile
        .limits
        .max_request_bytes
        .is_some_and(|max| serde_json::to_vec(&body).map_or(true, |bytes| bytes.len() > max))
    {
        return Err(ProviderError::Limit(
            crate::limits::LimitError::RequestBytes,
        ));
    }
    Ok(body)
}
/// Shared asset representation for prepared wire images and the public raw
/// `SystemOne` request-body compatibility entry point.
pub(super) fn assets(images: &[crate::media::Image]) -> Value {
    assets_iter(images.iter())
}
fn assets_iter<'a>(images: impl Iterator<Item = &'a crate::media::Image>) -> Value {
    Value::Object(
        images
            .map(|image| {
                (
                    image.reference.clone(),
                    json!({"type":"image","format":image.format,"data_base64":image.data_base64}),
                )
            })
            .collect(),
    )
}

/// `OpenAI`'s native response has no `SystemOne` HTTP envelope or metadata overlay.
pub(super) fn normalize_openai(
    request: &TurnRequest,
    response: Value,
    name: &str,
    _binding: HttpBinding,
) -> Result<TurnOutcome, ProviderError> {
    super::openai::normalize(request, response, name)
}

/// Convert a `SystemOne` response, unwrapping Cloudflare Workers AI envelopes, into a validated turn outcome.
///
/// # Errors
///
/// - [`ProviderError::Wire`] if a Cloudflare envelope is unsuccessful, incomplete, or
///   missing its result, or the response metadata does not match the schema.
/// - Any error from [`crate::provider::system_one::normalize`].
/// - [`ProviderError::Contract`] if the outcome with metadata is invalid.
pub fn normalize(
    request: &TurnRequest,
    mut response: Value,
    name: &str,
    binding: HttpBinding,
) -> Result<TurnOutcome, ProviderError> {
    #[derive(serde::Deserialize)]
    struct Cost {
        cost: Option<f64>,
    }
    #[derive(serde::Deserialize)]
    struct Metadata {
        id: Option<String>,
        provider: Option<String>,
        usage: Option<Cost>,
    }
    if binding == HttpBinding::CloudflareWorkersAi {
        if response.get("success").and_then(Value::as_bool) != Some(true) {
            return Err(ProviderError::Wire);
        }
        response = response.get("result").cloned().ok_or(ProviderError::Wire)?;
        if response.get("answers").is_none() {
            if response.get("state").and_then(Value::as_str) != Some("Completed") {
                return Err(ProviderError::Wire);
            }
            response = response.get("result").cloned().ok_or(ProviderError::Wire)?;
        }
    }
    let metadata: Metadata =
        serde_json::from_value(response.clone()).map_err(|_| ProviderError::Wire)?;
    let mut outcome = super::system_one::normalize(request, response, name)?;
    outcome.provider.request_id = metadata.id;
    outcome.provider.upstream_provider = metadata.provider;
    outcome.usage.cost = metadata.usage.and_then(|u| u.cost);
    // The protocol normalizer already validated the complete outcome. Only
    // fields added by the HTTP binding need checks here, in the original order.
    if let Some(provider) = &outcome.provider.upstream_provider {
        crate::decision::nonempty(provider, "upstream_provider")?;
    }
    if let Some(id) = &outcome.provider.request_id {
        crate::decision::nonempty(id, "provider.request_id")?;
    }
    if outcome
        .usage
        .cost
        .is_some_and(|cost| !cost.is_finite() || cost < 0.0)
    {
        return Err(crate::decision::ContractError::Number("usage.cost").into());
    }
    Ok(outcome)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{ConnectionSettings, ImageEncoding};
    fn request() -> TurnRequest {
        let mut request: TurnRequest = serde_json::from_str(include_str!(
            "../../../fixtures/contracts/decision-request.json"
        ))
        .unwrap();
        request.questions.truncate(3);
        request
    }

    #[test]
    fn an_invalid_profile_is_reported_as_a_profile_error() {
        let mut profile = crate::limits::count::tests::profile(Protocol::SystemOne, "anything");
        profile.model = " ".into();
        assert_eq!(
            body(&profile, &request(), &crate::media::tests::context()),
            Err(ProviderError::Profile)
        );
    }

    #[test]
    fn encodings_and_wire_model_are_explicit_and_independent_of_host_and_name() {
        let request = request();
        let context = crate::media::tests::context();
        let image = &crate::media::Evidence::read(&context).unwrap().images[0];
        for encoding in [
            ImageEncoding::Assets,
            ImageEncoding::ImagesBase64,
            ImageEncoding::ImagesDataUrls,
            ImageEncoding::StateImageUrls,
        ] {
            let mut profile = crate::limits::count::tests::profile(Protocol::SystemOne, "anything");
            profile.connection = ConnectionSettings {
                image_encoding: Some(encoding),
                wire_model: Some("wire-selector".into()),
                ..Default::default()
            };
            let first = body(&profile, &request, &context).unwrap();
            profile.base_url = "https://totally-different.example/proxy".into();
            profile.model = "another-model".into();
            assert_eq!(first, body(&profile, &request, &context).unwrap());
            assert_eq!(first["model"], "wire-selector");
            match encoding {
                ImageEncoding::Assets => assert_eq!(
                    first["assets"][&image.reference]["data_base64"],
                    image.data_base64
                ),
                ImageEncoding::ImagesBase64 => assert_eq!(first["images"][0], image.data_base64),
                ImageEncoding::ImagesDataUrls => assert_eq!(first["images"][0], image.data_url()),
                ImageEncoding::StateImageUrls => {
                    assert_eq!(first["state"][2]["image_url"]["url"], image.data_url());
                }
                ImageEncoding::OpenaiContent => unreachable!(),
            }
        }
    }
    #[test]
    fn cloudflare_binding_unwraps_shared_outcomes_and_preserves_metadata() {
        let request = request();
        let mut response: Value = serde_json::from_str(include_str!(
            "../../../fixtures/providers/typesafe/response.json"
        ))
        .unwrap();
        response["id"] = json!("request-123");
        response["provider"] = json!("upstream");
        response["usage"]["cost"] = json!(0.0123);
        let standard = normalize(
            &request,
            response.clone(),
            "my-connection",
            HttpBinding::Standard,
        )
        .unwrap();
        for result in [
            response.clone(),
            json!({"state":"Completed", "result":response}),
        ] {
            let cf = normalize(
                &request,
                json!({"success":true,"result":result}),
                "my-connection",
                HttpBinding::CloudflareWorkersAi,
            )
            .unwrap();
            assert_eq!(cf, standard);
        }
        assert_eq!(standard.provider.request_id.as_deref(), Some("request-123"));
        assert_eq!(standard.usage.cost, Some(0.0123));
        assert!(
            normalize(
                &request,
                json!({"success":false,"result":response}),
                "edge",
                HttpBinding::CloudflareWorkersAi
            )
            .is_err()
        );
        assert!(
            normalize(
                &request,
                json!({"success":true,"result":{"state":"Running"}}),
                "edge",
                HttpBinding::CloudflareWorkersAi
            )
            .is_err()
        );
    }
    #[test]
    fn openai_transmits_prepared_pixels_and_checks_actual_request_bytes() {
        let mut request = request();
        request.questions[0].prompt = "Inspect [photo #123]".into();
        let context = crate::media::tests::context();
        let mut profile =
            crate::limits::count::tests::profile(Protocol::OpenaiDecisions, "anything");
        profile.connection = ConnectionSettings {
            image_encoding: Some(ImageEncoding::OpenaiContent),
            max_image_dimension: Some(100),
            ..Default::default()
        };
        let original = crate::media::Evidence::read(&context)
            .unwrap()
            .images
            .remove(0);
        let prepared = original.prepare(&profile.connection).unwrap();
        assert_eq!(prepared.dimensions().unwrap(), (100, 100));
        let serialized = body(&profile, &request, &context).unwrap().to_string();
        assert!(serialized.contains(&prepared.data_url()));
        assert!(!serialized.contains(&original.data_url()));
        profile.limits.max_request_bytes = Some(1);
        assert!(matches!(
            body(&profile, &request, &context),
            Err(ProviderError::Limit(
                crate::limits::LimitError::RequestBytes
            ))
        ));
    }
}
