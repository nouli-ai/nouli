//! The models a provider says it offers.
//!
//! Providers do not report which models can answer decisions, so a listed
//! model is an option to try. Only models that plainly cannot answer in text
//! are left out, and a provider's own error covers the rest.
use crate::{
    config::{HttpBinding, resolve::ResolvedSettings},
    transport::{HttpTransport, TransportError},
};
use serde_json::Value;
use std::collections::HashSet;
use thiserror::Error;
use tokio_util::sync::CancellationToken;

const PATH: &str = "v1/models";
/// Names of models for speech, images, moderation, and embeddings.
const NOT_TEXT: [&str; 10] = [
    "tts",
    "whisper",
    "transcribe",
    "realtime",
    "moderation",
    "embedding",
    "sora",
    "dall-e",
    "gpt-image",
    "-audio",
];

#[derive(Debug, Error)]
pub enum ModelListError {
    #[error("this connection does not list its models")]
    Unsupported,
    #[error("provider model list does not match its documented schema")]
    Wire,
    #[error(transparent)]
    Transport(#[from] TransportError),
}

/// Fetches the model IDs the selected provider lists, in the provider's order.
///
/// # Errors
///
/// - [`ModelListError::Unsupported`] for a binding with no model list.
/// - [`ModelListError::Transport`] if the request fails.
/// - [`ModelListError::Wire`] if the response has no recognizable list.
pub async fn list(
    transport: &HttpTransport,
    settings: &ResolvedSettings,
    cancel: &CancellationToken,
) -> Result<Vec<String>, ModelListError> {
    let profile = &settings.snapshot.profile;
    if profile.connection.binding == Some(HttpBinding::CloudflareWorkersAi) {
        return Err(ModelListError::Unsupported);
    }
    let response = transport
        .get_with_headers(
            profile,
            PATH,
            settings.credential.as_ref(),
            &settings.headers,
            cancel,
        )
        .await?;
    parse(&response.json()?)
}

/// The IDs in `{"data": [{"id": …}]}` or `{"models": [{"name": …}]}`.
fn parse(body: &Value) -> Result<Vec<String>, ModelListError> {
    let entries = body
        .get("data")
        .or_else(|| body.get("models"))
        .and_then(Value::as_array)
        .ok_or(ModelListError::Wire)?;
    let mut seen = HashSet::new();
    Ok(entries
        .iter()
        .filter(|entry| answers_in_text(entry))
        .filter_map(|entry| entry.get("id").or_else(|| entry.get("name")))
        .filter_map(Value::as_str)
        .filter(|id| !id.trim().is_empty() && !NOT_TEXT.iter().any(|name| id.contains(name)))
        .filter(|id| seen.insert(*id))
        .map(str::to_owned)
        .collect())
}

/// False only for an entry that lists output types without text.
fn answers_in_text(entry: &Value) -> bool {
    entry
        .pointer("/architecture/output_modalities")
        .and_then(Value::as_array)
        .is_none_or(|types| types.iter().any(|kind| kind == "text"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn both_list_shapes_parse_in_order() {
        let openai = json!({"object": "list", "data": [{"id": "b"}, {"id": "a"}]});
        assert_eq!(parse(&openai).unwrap(), ["b", "a"]);
        let typesafe = json!({"models": [{"name": "jev-latest", "release_date": "2026-01-01"}]});
        assert_eq!(parse(&typesafe).unwrap(), ["jev-latest"]);
    }

    #[test]
    fn models_that_cannot_answer_in_text_are_left_out() {
        let body = json!({"data": [
            {"id": "gpt-5.6-luna"},
            {"id": "gpt-4o-mini-tts"},
            {"id": "gpt-realtime-2"},
            {"id": "omni-moderation-latest"},
            {"id": "text-embedding-3-large"},
            {"id": "maker/picture", "architecture": {"output_modalities": ["image"]}},
            {"id": "maker/words", "architecture": {"output_modalities": ["text", "image"]}},
        ]});
        assert_eq!(parse(&body).unwrap(), ["gpt-5.6-luna", "maker/words"]);
    }

    #[test]
    fn repeated_and_blank_ids_are_dropped_and_other_bodies_are_rejected() {
        let body = json!({"data": [{"id": "a"}, {"id": "a"}, {"id": " "}, {"object": "x"}]});
        assert_eq!(parse(&body).unwrap(), ["a"]);
        assert!(matches!(
            parse(&json!({"error": "no"})),
            Err(ModelListError::Wire)
        ));
    }
}
