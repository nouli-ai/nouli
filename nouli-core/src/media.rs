//! Saved image evidence and conversion from context references to native media.
//! Payloads stay in the ledger; providers receive images instead of base64 text.
use base64::{Engine, prelude::BASE64_STANDARD};
use image::ImageDecoder;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    io::Cursor,
    sync::{Arc, Mutex, OnceLock},
};

use crate::{codec, codec::InputFormat, context::evidence};

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Image {
    pub reference: String,
    pub name: String,
    pub format: String,
    pub data_base64: String,
}

pub struct Evidence {
    pub text: String,
    pub images: Vec<Image>,
}

/// Plan-local image preparation shared by budgeting and native wire construction.
/// Clones share the same cache; create a fresh value for each new context plan.
/// No prepared bytes or cache state are serialized into the ledger.
#[derive(Clone, Default)]
pub struct PreparedImages {
    inner: Arc<Mutex<PreparationCache>>,
}
#[derive(Default)]
struct PreparationCache {
    context: Option<(String, Arc<Evidence>)>,
    settings: Option<crate::config::ConnectionSettings>,
    images: Vec<CachedImage>,
    #[cfg(test)]
    preparations: usize,
}
struct CachedImage {
    original: Arc<Evidence>,
    index: usize,
    prepared: Arc<PreparedImage>,
}
/// A connection-specific derivative and its lazily cached pixel dimensions.
pub struct PreparedImage {
    pub(crate) image: Image,
    dimensions: OnceLock<Result<(u32, u32), ()>>,
}
impl PreparedImage {
    /// Read cached derivative bytes. Use the current evidence for display names.
    #[must_use]
    pub fn image(&self) -> &Image {
        &self.image
    }
    /// Read cached dimensions, decoding the header once when preparation did not resize.
    ///
    /// # Errors
    ///
    /// Returns [`codec::CodecError::Shape`] if the image header cannot be decoded.
    pub fn dimensions(&self) -> Result<(u32, u32), codec::CodecError> {
        (*self
            .dimensions
            .get_or_init(|| self.image.dimensions().map_err(|_| ())))
        .map_err(|()| codec::CodecError::Shape)
    }
}
impl PreparedImages {
    // Only complete entries are published. A panic cannot leave a partial image
    // in the cache, so a poisoned lock can safely retain its completed entries.
    fn cache(&self) -> std::sync::MutexGuard<'_, PreparationCache> {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
    /// Read context evidence once for identical serialized input.
    ///
    /// # Errors
    ///
    /// Returns any [`Evidence::read`] error.
    pub fn evidence(&self, context: &str) -> Result<Arc<Evidence>, codec::CodecError> {
        let mut cache = self.cache();
        if let Some((previous, evidence)) = &cache.context
            && previous == context
        {
            return Ok(evidence.clone());
        }
        let evidence = Arc::new(Evidence::read(context)?);
        cache.context = Some((context.to_owned(), evidence.clone()));
        Ok(evidence)
    }
    /// Reuse a derivative only for identical source bytes/reference and connection settings.
    /// The current evidence retains its own display name independently of cached pixels.
    ///
    /// # Errors
    ///
    /// Returns [`codec::CodecError::Shape`] for an invalid index or any image-preparation error.
    pub fn image(
        &self,
        evidence: &Arc<Evidence>,
        index: usize,
        settings: &crate::config::ConnectionSettings,
    ) -> Result<Arc<PreparedImage>, codec::CodecError> {
        let mut cache = self.cache();
        if cache.settings.as_ref() != Some(settings) {
            cache.images.clear();
            cache.settings = Some(settings.clone());
        }
        let image = evidence.images.get(index).ok_or(codec::CodecError::Shape)?;
        if let Some(existing) = cache.images.iter().find(|entry| {
            let original = &entry.original.images[entry.index];
            original.reference == image.reference
                && original.format == image.format
                && original.data_base64 == image.data_base64
        }) {
            return Ok(existing.prepared.clone());
        }
        #[cfg(test)]
        {
            cache.preparations += 1;
        }
        let (image, dimensions) = image.prepare_with_dimensions(settings)?;
        let prepared = Arc::new(PreparedImage {
            image,
            dimensions: OnceLock::new(),
        });
        if let Some(dimensions) = dimensions {
            let _ = prepared.dimensions.set(Ok(dimensions));
        }
        cache.images.push(CachedImage {
            original: evidence.clone(),
            index,
            prepared: prepared.clone(),
        });
        Ok(prepared)
    }
    #[cfg(test)]
    pub(crate) fn preparations(&self) -> usize {
        self.cache().preparations
    }
}

impl Evidence {
    /// Extract saved images from TOON context, replacing each with its reference.
    ///
    /// # Errors
    ///
    /// - Any [`codec::CodecError`] from decoding or re-encoding the context.
    /// - [`codec::CodecError::Shape`] if an image record is malformed, empty, or not PNG or JPEG.
    /// - [`codec::CodecError::ImageReferenceConflict`] if two images share a reference but
    ///   differ in format or bytes.
    pub fn read(context: &str) -> Result<Self, codec::CodecError> {
        fn extract(value: &mut Value, images: &mut Vec<Image>) -> Result<(), codec::CodecError> {
            if let Some(image) = value.get("record").and_then(evidence::image_value_of) {
                let image: Image =
                    serde_json::from_value(image.clone()).map_err(|_| codec::CodecError::Shape)?;
                if image.reference.is_empty()
                    || image.data_base64.is_empty()
                    || !matches!(image.format.as_str(), "png" | "jpeg")
                {
                    return Err(codec::CodecError::Shape);
                }
                value["record"]["value"] = json!(image.reference);
                if let Some(existing) = images
                    .iter()
                    .find(|existing| existing.reference == image.reference)
                {
                    if existing.data_base64 != image.data_base64 || existing.format != image.format
                    {
                        return Err(codec::CodecError::ImageReferenceConflict);
                    }
                } else {
                    images.push(image);
                }
            } else {
                match value {
                    Value::Object(map) => {
                        for child in map.values_mut() {
                            extract(child, images)?;
                        }
                    }
                    Value::Array(array) => {
                        for child in array {
                            extract(child, images)?;
                        }
                    }
                    _ => {}
                }
            }
            Ok(())
        }
        if !context.contains(evidence::IMAGE_FIELD) {
            return Ok(Self {
                text: context.into(),
                images: vec![],
            });
        }
        let mut value = codec::decode_value(context.as_bytes(), InputFormat::Toon)?;
        let mut images = Vec::new();
        extract(&mut value, &mut images)?;
        let text = if images.is_empty() {
            context.to_owned()
        } else {
            codec::encode_toon(&value)?
        };
        Ok(Self { text, images })
    }

    /// `OpenAI` reads ordered text/image parts. Other providers use references.
    #[must_use]
    pub fn openai_parts(&self, text: &str) -> Value {
        let mut rest = text;
        let mut parts = Vec::new();
        while let Some((at, image)) = self
            .images
            .iter()
            .filter_map(|image| rest.find(&image.reference).map(|at| (at, image)))
            .min_by_key(|(at, _)| *at)
        {
            if at > 0 {
                parts.push(json!({"type":"input_text", "text":&rest[..at]}));
            }
            parts.push(json!({"type":"input_text", "text":format!("Image {}: ", image.reference)}));
            parts.push(json!({"type":"input_image", "image_url":image.data_url()}));
            rest = &rest[at + image.reference.len()..];
        }
        if parts.is_empty() {
            return json!(text);
        }
        if !rest.is_empty() {
            parts.push(json!({"type":"input_text", "text":rest}));
        }
        json!(parts)
    }
}

impl Image {
    #[must_use]
    pub fn data_url(&self) -> String {
        format!("data:image/{};base64,{}", self.format, self.data_base64)
    }

    fn bytes(&self) -> Result<Vec<u8>, codec::CodecError> {
        if self.data_base64.len() > (16 * 1024 * 1024_usize).div_ceil(3) * 4 {
            return Err(codec::CodecError::Shape);
        }
        BASE64_STANDARD
            .decode(&self.data_base64)
            .map_err(|_| codec::CodecError::Shape)
    }

    /// Read the image's pixel dimensions from its header.
    ///
    /// # Errors
    ///
    /// Returns [`codec::CodecError::Shape`] if the payload exceeds 16 MiB, is not valid
    /// base64, or its format or dimensions cannot be read.
    pub fn dimensions(&self) -> Result<(u32, u32), codec::CodecError> {
        image::ImageReader::new(Cursor::new(self.bytes()?))
            .with_guessed_format()
            .map_err(|_| codec::CodecError::Shape)?
            .into_dimensions()
            .map_err(|_| codec::CodecError::Shape)
    }

    /// Fit pixels within explicit inclusive bounds, padding extreme aspect ratios.
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "scaled sides are rounded and clamped to 1..=maximum, a u32"
    )]
    fn image_size(width: u32, height: u32, minimum: u32, maximum: u32) -> (u32, u32, u32, u32) {
        let longest = f64::from(width.max(height).max(1));
        let shortest = f64::from(width.min(height).max(1));
        let scale = if longest > f64::from(maximum) {
            f64::from(maximum) / longest
        } else if shortest < f64::from(minimum) {
            (f64::from(minimum) / shortest).min(f64::from(maximum) / longest)
        } else {
            1.0
        };
        let width = (f64::from(width) * scale)
            .round()
            .clamp(1.0, f64::from(maximum)) as u32;
        let height = (f64::from(height) * scale)
            .round()
            .clamp(1.0, f64::from(maximum)) as u32;
        (width, height, width.max(minimum), height.max(minimum))
    }

    /// Resize and pad the image to the configured dimension bounds, re-encoding it as PNG.
    ///
    /// # Errors
    ///
    /// Returns [`codec::CodecError::Shape`] if the payload exceeds 16 MiB, cannot be decoded,
    /// or the re-encoded PNG exceeds 16 MiB. Returns the image unchanged, without
    /// decoding, when no bounds are configured.
    pub fn prepare(
        &self,
        settings: &crate::config::ConnectionSettings,
    ) -> Result<Self, codec::CodecError> {
        Ok(self.prepare_with_dimensions(settings)?.0)
    }
    fn prepare_with_dimensions(
        &self,
        settings: &crate::config::ConnectionSettings,
    ) -> Result<(Self, Option<(u32, u32)>), codec::CodecError> {
        if settings.min_image_dimension.is_none() && settings.max_image_dimension.is_none() {
            return Ok((self.clone(), None));
        }
        let reader = image::ImageReader::new(Cursor::new(self.bytes()?))
            .with_guessed_format()
            .map_err(|_| codec::CodecError::Shape)?;
        let mut decoder = reader
            .into_decoder()
            .map_err(|_| codec::CodecError::Shape)?;
        let orientation = decoder
            .orientation()
            .map_err(|_| codec::CodecError::Shape)?;
        let mut image =
            image::DynamicImage::from_decoder(decoder).map_err(|_| codec::CodecError::Shape)?;
        image.apply_orientation(orientation);
        let (width, height, canvas_width, canvas_height) = Self::image_size(
            image.width(),
            image.height(),
            settings.min_image_dimension.unwrap_or(1),
            settings.max_image_dimension.unwrap_or(16384),
        );
        if (width, height) != (image.width(), image.height()) {
            image = image.resize_exact(width, height, image::imageops::FilterType::Lanczos3);
        }
        let mut canvas = image::RgbaImage::from_pixel(
            canvas_width,
            canvas_height,
            image::Rgba([255, 255, 255, 255]),
        );
        image::imageops::overlay(
            &mut canvas,
            &image.to_rgba8(),
            i64::from((canvas_width - width) / 2),
            i64::from((canvas_height - height) / 2),
        );
        let mut output = Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(canvas)
            .write_to(&mut output, image::ImageFormat::Png)
            .map_err(|_| codec::CodecError::Shape)?;
        if output.get_ref().len() > 16 * 1024 * 1024 {
            return Err(codec::CodecError::Shape);
        }
        Ok((
            Self {
                data_base64: BASE64_STANDARD.encode(output.into_inner()),
                format: "png".into(),
                ..self.clone()
            },
            Some((canvas_width, canvas_height)),
        ))
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) fn picture(width: u32, height: u32) -> Image {
        let mut bytes = Cursor::new(Vec::new());
        image::DynamicImage::new_rgb8(width, height)
            .write_to(&mut bytes, image::ImageFormat::Png)
            .unwrap();
        Image {
            reference: "[photo #123]".into(),
            name: "photo.png".into(),
            format: "png".into(),
            data_base64: BASE64_STANDARD.encode(bytes.into_inner()),
        }
    }

    pub(crate) fn context() -> String {
        codec::encode_toon(&json!({"records":[{"record":{"key":"image:fixture","value":{"nouli_image":picture(280, 280)}}}]})).unwrap()
    }

    #[test]
    fn payload_is_removed_from_text_and_references_become_native_image_parts() {
        let evidence = Evidence::read(&context()).unwrap();
        assert!(!evidence.text.contains("data_base64"));
        assert_eq!(evidence.images[0].format, "png");
        let parts = evidence.openai_parts("Compare [photo #123] with this");
        assert_eq!(parts[0]["text"], "Compare ");
        assert_eq!(parts[2]["type"], "input_image");
        assert_eq!(parts[3]["text"], " with this");
    }

    #[test]
    fn ordinary_structured_fields_are_not_treated_as_image_records() {
        let context = codec::encode_toon(
            &json!({"current_state":[{"type":"structured","data":{"nouli_image":"caption"}}]}),
        )
        .unwrap();
        let evidence = Evidence::read(&context).unwrap();
        assert!(evidence.images.is_empty());
        assert!(evidence.text.contains("caption"));
    }

    #[test]
    fn equal_image_references_deduplicate_only_when_the_pixels_match() {
        let a = picture(2, 2);
        let b = picture(3, 3);
        let context = |other: &Image| {
            codec::encode_toon(&json!({"records":[
                {"record":evidence::image_record(crate::identity::Id::generate(), &a)},
                {"record":evidence::image_record(crate::identity::Id::generate(), other)}
            ]}))
            .unwrap()
        };
        assert_eq!(Evidence::read(&context(&a)).unwrap().images.len(), 1);
        assert!(matches!(
            Evidence::read(&context(&b)),
            Err(codec::CodecError::ImageReferenceConflict)
        ));
        let report = crate::engine::EngineError::Limit(crate::limits::LimitError::Codec(
            codec::CodecError::ImageReferenceConflict,
        ))
        .report();
        assert_eq!(report.code, "image_reference_conflict");
        assert!(report.message.contains("different bytes"));
    }

    #[test]
    fn native_requests_send_pixels_and_keep_question_and_option_references() {
        use crate::{
            decision::{AnswerSpace, ChoiceOption, Question, TurnRequest},
            provider,
        };
        let request = TurnRequest {
            version: 1,
            state: vec![],
            questions: vec![Question {
                id: "photo".into(),
                prompt: "Compare [photo #123] with the options.".into(),
                criteria: vec![],
                answer_space: AnswerSpace::Choice {
                    options: vec![
                        ChoiceOption {
                            id: "first".into(),
                            description: "[photo #123]".into(),
                        },
                        ChoiceOption {
                            id: "second".into(),
                            description: "Something else".into(),
                        },
                    ],
                },
            }],
        };
        let direct =
            provider::system_one::request_body(&request, &context(), "jev-latest").unwrap();
        assert!(direct["assets"]["[photo #123]"]["data_base64"].is_string());
        assert!(!direct["state"].as_str().unwrap().contains("aGVsbG8="));
        assert_eq!(
            direct["questions"]["photo"]["criteria"]["first"],
            "[photo #123]"
        );
        let openai = provider::openai::request_body(&request, &context(), "gpt-6-luna").unwrap();
        assert_eq!(openai["input"][0]["role"], "user");
        assert!(
            openai["input"][0]["content"]
                .as_array()
                .unwrap()
                .iter()
                .any(|part| part["type"] == "input_image")
        );
        assert!(
            openai["questions"][0]["instructions"]
                .as_array()
                .unwrap()
                .iter()
                .any(|part| part["type"] == "input_image")
        );
        assert_eq!(
            openai["questions"][0]["choices"][0]["description"],
            "[photo #123]"
        );
        let jev = provider::wire::body(
            &crate::limits::count::tests::profile(
                crate::config::Protocol::SystemOne,
                "typesafe/jev-1.14",
            ),
            &request,
            &context(),
        )
        .unwrap();
        assert!(jev["assets"].is_object());
        let router = provider::wire::body(
            &crate::limits::count::tests::profile(
                crate::config::Protocol::SystemOne,
                "cloudflare/clef",
            ),
            &request,
            &context(),
        )
        .unwrap();
        assert!(router.get("assets").is_none());
        assert!(
            router["state"]
                .as_array()
                .unwrap()
                .iter()
                .any(|part| part["type"] == "image_url")
        );
    }

    #[test]
    fn jev_derivatives_fit_screenshots_and_panoramas_without_changing_saved_bytes() {
        for (width, height, expected) in [
            (1912, 1044, (1568, 856)),
            (4000, 16, (1568, 280)),
            (100, 100, (280, 280)),
        ] {
            let original = picture(width, height);
            let bytes = original.data_base64.clone();
            let prepared = original
                .prepare(&crate::config::ConnectionSettings {
                    min_image_dimension: Some(280),
                    max_image_dimension: Some(1568),
                    ..Default::default()
                })
                .unwrap();
            assert_eq!(prepared.dimensions().unwrap(), expected);
            assert_eq!(original.data_base64, bytes);
            assert_eq!(prepared.format, "png");
        }
    }

    #[test]
    fn jev_applies_exif_orientation_before_preparing_pixels() {
        let mut jpeg = Cursor::new(Vec::new());
        image::DynamicImage::new_rgb8(300, 600)
            .write_to(&mut jpeg, image::ImageFormat::Jpeg)
            .unwrap();
        let mut bytes = jpeg.into_inner();
        let exif = b"Exif\0\0II\x2a\0\x08\0\0\0\x01\0\x12\x01\x03\0\x01\0\0\0\x06\0\0\0\0\0\0\0";
        let mut segment = vec![0xff, 0xe1];
        segment.extend_from_slice(&u16::try_from(exif.len() + 2).unwrap().to_be_bytes());
        segment.extend_from_slice(exif);
        bytes.splice(2..2, segment);
        let picture = Image {
            data_base64: BASE64_STANDARD.encode(bytes),
            format: "jpeg".into(),
            ..picture(300, 600)
        };
        assert_eq!(
            picture
                .prepare(&crate::config::ConnectionSettings {
                    min_image_dimension: Some(280),
                    max_image_dimension: Some(1568),
                    ..Default::default()
                })
                .unwrap()
                .dimensions()
                .unwrap(),
            (600, 300)
        );
    }
    #[test]
    fn preparation_cache_respects_settings_bytes_and_current_display_names() {
        use crate::{
            config::{ImageEncoding, Protocol},
            provider::wire,
        };
        let mut profile = crate::limits::count::tests::profile(Protocol::SystemOne, "fixture");
        profile.connection.image_encoding = Some(ImageEncoding::StateImageUrls);
        let mut request: crate::decision::TurnRequest = serde_json::from_str(include_str!(
            "../../fixtures/contracts/decision-request.json"
        ))
        .unwrap();
        request.questions.truncate(3);
        let images = PreparedImages::default();
        let context = context();
        wire::body_prepared(&profile, &request, &context, &images).unwrap();
        let mut value = codec::decode_value(context.as_bytes(), InputFormat::Toon).unwrap();
        value["records"][0]["record"]["value"]["nouli_image"]["name"] = json!("renamed.png");
        let renamed = codec::encode_toon(&value).unwrap();
        let body = wire::body_prepared(&profile, &request, &renamed, &images).unwrap();
        assert_eq!(images.preparations(), 1);
        assert_eq!(body["state"][1], "Image renamed.png: [photo #123]");
        profile.connection.max_image_dimension = Some(512);
        wire::body_prepared(&profile, &request, &renamed, &images).unwrap();
        assert_eq!(images.preparations(), 2);
        value["records"][0]["record"]["value"]["nouli_image"] =
            serde_json::to_value(picture(281, 280)).unwrap();
        let changed = codec::encode_toon(&value).unwrap();
        wire::body_prepared(&profile, &request, &changed, &images).unwrap();
        assert_eq!(images.preparations(), 3);
    }
}
