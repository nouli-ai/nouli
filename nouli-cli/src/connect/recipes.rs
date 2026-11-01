//! Setup shortcut data; runtime connections never consult these recipes.
use nouli_core::config::{ConnectionSettings, HttpBinding, ImageEncoding, Protocol};
pub(super) const SERVICES: &str =
    "openai, typesafe, openrouter, cloudflare, ollama, llama.cpp, custom";
#[derive(Clone)]
pub(super) struct Recipe {
    pub(super) name: &'static str,
    pub(super) url: &'static str,
    pub(super) protocol: Protocol,
    pub(super) models: &'static [&'static str],
    pub(super) connection: ConnectionSettings,
}
// Setup-only suggestions, verified 2026-10-30. They are written as ordinary
// settings and never consulted during resolution or execution.
// https://docs.typesafe.ai/api/system-one and https://docs.typesafe.ai/images
// https://openrouter.ai/docs/guides/community/typesafe-sdk
// https://developers.cloudflare.com/workers-ai/models/clef/
// https://docs.ollama.com/api/systemone
pub(super) fn recipes() -> Vec<Recipe> {
    let plain = ConnectionSettings::default();
    let assets = ConnectionSettings {
        image_encoding: Some(ImageEncoding::Assets),
        headers: std::collections::BTreeMap::from([(
            "TypeSafe-Danger-Zone-Images".into(),
            "true".into(),
        )]),
        min_image_dimension: Some(280),
        max_image_dimension: Some(1568),
        pixel_patch_size: Some(28),
        ..Default::default()
    };
    vec![
        Recipe {
            name: "openai",
            url: "https://api.openai.com",
            protocol: Protocol::OpenaiDecisions,
            models: &["gpt-6-luna"],
            connection: ConnectionSettings {
                image_encoding: Some(ImageEncoding::OpenaiContent),
                pixel_patch_size: Some(32),
                ..plain.clone()
            },
        },
        Recipe {
            name: "typesafe",
            url: "https://api.typesafe.ai",
            protocol: Protocol::SystemOne,
            models: &["jev-latest"],
            connection: assets,
        },
        Recipe {
            name: "openrouter",
            url: "https://openrouter.ai/api",
            protocol: Protocol::SystemOne,
            models: &["typesafe/jev-1.14", "cloudflare/clef"],
            connection: ConnectionSettings {
                image_encoding: Some(ImageEncoding::StateImageUrls),
                extra_body: std::collections::BTreeMap::from([(
                    "provider".into(),
                    serde_json::json!({"allow_fallbacks":false}),
                )]),
                ..plain.clone()
            },
        },
        Recipe {
            name: "cloudflare",
            url: "",
            protocol: Protocol::SystemOne,
            models: &["@cf/cloudflare/clef", "@cf/cloudflare/clef-flash"],
            connection: ConnectionSettings {
                binding: Some(HttpBinding::CloudflareWorkersAi),
                image_encoding: Some(ImageEncoding::ImagesDataUrls),
                ..plain.clone()
            },
        },
        Recipe {
            name: "ollama",
            url: "http://localhost:11434",
            protocol: Protocol::SystemOne,
            models: &["clef", "nimble"],
            connection: ConnectionSettings {
                no_auth: Some(true),
                image_encoding: Some(ImageEncoding::ImagesBase64),
                ..plain.clone()
            },
        },
        Recipe {
            name: "llama.cpp",
            url: "http://localhost:8080",
            protocol: Protocol::SystemOne,
            models: &[],
            connection: ConnectionSettings {
                no_auth: Some(true),
                image_encoding: Some(ImageEncoding::ImagesDataUrls),
                ..plain
            },
        },
    ]
}
