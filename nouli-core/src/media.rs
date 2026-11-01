//! Saved image evidence and conversion from context references to native media.
//! Payloads stay in the ledger; providers receive images instead of base64 text.
use base64::{Engine, prelude::BASE64_STANDARD};
use image::ImageDecoder;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::io::Cursor;

use crate::{codec, codec::InputFormat};

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

impl Evidence {
    pub fn read(context: &str) -> Result<Self, codec::CodecError> {
        if !context.contains("nouli_image") {
            return Ok(Self {
                text: context.into(),
                images: vec![],
            });
        }
        let mut value = codec::decode_value(context.as_bytes(), InputFormat::Toon)?;
        let mut images = Vec::new();
        fn extract(value: &mut Value, images: &mut Vec<Image>) -> Result<(), codec::CodecError> {
            if let Some(image) = value
                .get("record")
                .filter(|record| {
                    record
                        .get("key")
                        .and_then(Value::as_str)
                        .is_some_and(|key| key.starts_with("image:"))
                })
                .and_then(|record| record.get("value"))
                .and_then(|value| value.get("nouli_image"))
            {
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
        extract(&mut value, &mut images)?;
        let text = if images.is_empty() {
            context.to_owned()
        } else {
            codec::encode_toon(&value)?
        };
        Ok(Self { text, images })
    }

    pub fn assets(&self) -> Result<Value, codec::CodecError> {
        let images = &self.images;
        Ok(Value::Object(
            images
                .iter()
                .map(|image| {
                    (
                        image.reference.clone(),
                        json!({
                            "type":"image", "format":image.format, "data_base64":image.data_base64
                        }),
                    )
                })
                .collect(),
        ))
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

    pub fn dimensions(&self) -> Result<(u32, u32), codec::CodecError> {
        image::ImageReader::new(Cursor::new(self.bytes()?))
            .with_guessed_format()
            .map_err(|_| codec::CodecError::Shape)?
            .into_dimensions()
            .map_err(|_| codec::CodecError::Shape)
    }

    /// Fit pixels within explicit inclusive bounds, padding extreme aspect ratios.
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

    pub fn prepare(
        &self,
        settings: &crate::config::ConnectionSettings,
    ) -> Result<Self, codec::CodecError> {
        if settings.min_image_dimension.is_none() && settings.max_image_dimension.is_none() {
            return Ok(self.clone());
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
        Ok(Self {
            data_base64: BASE64_STANDARD.encode(output.into_inner()),
            format: "png".into(),
            ..self.clone()
        })
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
        assert_eq!(evidence.assets().unwrap()["[photo #123]"]["format"], "png");
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
                {"record":{"key":"image:a","value":{"nouli_image":a}}},
                {"record":{"key":"image:b","value":{"nouli_image":other}}}
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
        segment.extend_from_slice(&((exif.len() + 2) as u16).to_be_bytes());
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
}
