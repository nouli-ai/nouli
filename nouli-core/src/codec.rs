//! Strict boundary codecs. Explicit formats never fall through to another grammar.

mod json;
mod serialize;
mod toon;

use std::path::Path;

use serde::{Serialize, de::DeserializeOwned};
use serde_json::Value;
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::{
    decision::{CONTRACT_VERSION, ContractError, TurnRequest},
    workflow::{WorkflowError, WorkflowSpec},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputFormat {
    Json,
    Toon,
}

impl InputFormat {
    pub fn for_path(path: &Path) -> Result<Self, CodecError> {
        match path.extension().and_then(|ext| ext.to_str()) {
            Some("json") => Ok(Self::Json),
            Some("toon") => Ok(Self::Toon),
            _ => Err(CodecError::Format),
        }
    }
}

#[derive(Debug, Error, PartialEq)]
pub enum CodecError {
    #[error("input is not valid UTF-8")]
    Utf8,
    #[error("input format must be json or toon")]
    Format,
    #[error("invalid serialized syntax")]
    Syntax,
    #[error("duplicate object key")]
    DuplicateKey,
    #[error("numeric value is outside the supported domain")]
    Number,
    #[error("request must contain exactly one of questions or nodes")]
    Kind,
    #[error("serialized value does not match the contract")]
    Shape,
    #[error("value cannot be represented losslessly as TOON")]
    Representation,
    #[error(transparent)]
    Decision(#[from] ContractError),
    #[error(transparent)]
    Workflow(#[from] WorkflowError),
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(untagged)]
pub enum Request {
    Turn(TurnRequest),
    Workflow(WorkflowSpec),
}

pub fn decode_value(bytes: &[u8], format: InputFormat) -> Result<Value, CodecError> {
    let input = std::str::from_utf8(bytes).map_err(|_| CodecError::Utf8)?;
    let value = match format {
        InputFormat::Json => json::decode(input)?,
        InputFormat::Toon => toon::decode(input)?,
    };
    Ok(canonical_value(value))
}

/// Decode a contract shape; callers of generic contracts also run their semantic
/// validator. `decode_request` includes turn/workflow semantic shape validation.
pub fn decode<T: DeserializeOwned>(bytes: &[u8], format: InputFormat) -> Result<T, CodecError> {
    serde_json::from_value(decode_value(bytes, format)?).map_err(|_| CodecError::Shape)
}

pub fn decode_request(bytes: &[u8], format: InputFormat) -> Result<Request, CodecError> {
    let mut value = decode_value(bytes, format)?;
    let object = value.as_object_mut().ok_or(CodecError::Kind)?;
    let turn = object.contains_key("questions");
    let workflow = object.contains_key("nodes");
    if turn == workflow {
        return Err(CodecError::Kind);
    }
    object
        .entry("version")
        .or_insert(Value::from(CONTRACT_VERSION));
    if turn {
        let request: TurnRequest = serde_json::from_value(value).map_err(|_| CodecError::Shape)?;
        request.validate()?;
        Ok(Request::Turn(request))
    } else {
        // Embedded turns use the same omitted-version default.
        if let Some(nodes) = object.get_mut("nodes").and_then(Value::as_array_mut) {
            for node in nodes {
                if let Some(turn) = node.get_mut("turn").and_then(Value::as_object_mut) {
                    turn.entry("version")
                        .or_insert(Value::from(CONTRACT_VERSION));
                }
            }
        }
        let request: WorkflowSpec = serde_json::from_value(value).map_err(|_| CodecError::Shape)?;
        request.validate()?;
        Ok(Request::Workflow(request))
    }
}

/// Convert serializable input without allowing non-finite values to become null.
pub fn to_value<T: Serialize>(value: &T) -> Result<Value, CodecError> {
    serialize::checked_value(value)
}

/// Official TOON encoding, checked against a strict decode before use. Numeric
/// spelling and object-key order are canonicalized; array/history order is kept.
pub fn encode_toon<T: Serialize>(value: &T) -> Result<String, CodecError> {
    let value = canonical_value(serialize::checked_value(value)?);
    let encoded = toon_format::encode_default(&value).map_err(|_| CodecError::Representation)?;
    if decode_value(encoded.as_bytes(), InputFormat::Toon)? != value {
        return Err(CodecError::Representation);
    }
    Ok(encoded)
}

pub fn fingerprint<T: Serialize>(value: &T) -> Result<String, CodecError> {
    let value = canonical_value(serialize::checked_value(value)?);
    let bytes = serde_json::to_vec(&value).map_err(|_| CodecError::Shape)?;
    Ok(Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

fn canonical_value(value: Value) -> Value {
    match value {
        Value::Object(map) => {
            let mut entries: Vec<_> = map.into_iter().collect();
            entries.sort_by(|a, b| a.0.cmp(&b.0));
            Value::Object(
                entries
                    .into_iter()
                    .map(|(key, value)| (key, canonical_value(value)))
                    .collect(),
            )
        }
        Value::Array(values) => Value::Array(values.into_iter().map(canonical_value).collect()),
        Value::Number(n) if n.is_f64() => {
            let f = n.as_f64().expect("f64 number");
            if f.fract() == 0.0 && f >= i64::MIN as f64 && f < i64::MAX as f64 {
                Value::from(f as i64)
            } else if f.fract() == 0.0 && f >= 0.0 && f < u64::MAX as f64 {
                Value::from(f as u64)
            } else {
                Value::Number(n)
            }
        }
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_fixtures_are_equivalent_across_formats_and_versions_default() {
        for fixture in [
            include_bytes!("../../fixtures/contracts/decision-request.json").as_slice(),
            include_bytes!("../../fixtures/contracts/workflow.json").as_slice(),
        ] {
            let json = decode_request(fixture, InputFormat::Json).unwrap();
            let toon = encode_toon(&json).unwrap();
            let decoded = decode_request(toon.as_bytes(), InputFormat::Toon).unwrap();
            assert_eq!(json, decoded);
            assert_eq!(fingerprint(&json).unwrap(), fingerprint(&decoded).unwrap());
            let mut value: Value = serde_json::from_slice(fixture).unwrap();
            value.as_object_mut().unwrap().remove("version");
            assert_eq!(
                json,
                decode_request(&serde_json::to_vec(&value).unwrap(), InputFormat::Json).unwrap()
            );
        }
    }

    #[test]
    fn full_width_numbers_strings_and_history_round_trip() {
        let value = serde_json::json!({"min":i64::MIN,"max":u64::MAX,"float":1.43,"large_float":1e30,"tiny":1e-30,"text":"Unicode 🦀, colon: quote \" and\nnewline","numeric_string":"18446744073709551616","null":null});
        let encoded = encode_toon(&value).unwrap();
        assert_eq!(
            canonical_value(value.clone()),
            decode_value(encoded.as_bytes(), InputFormat::Toon).unwrap()
        );
        let history: Value =
            serde_json::from_str(include_str!("../../fixtures/contracts/history.json")).unwrap();
        let encoded = encode_toon(&history).unwrap();
        assert_eq!(
            canonical_value(history),
            decode_value(encoded.as_bytes(), InputFormat::Toon).unwrap()
        );
    }

    #[test]
    fn malformed_inputs_do_not_fall_through_to_another_format() {
        assert_eq!(
            decode_value(&[255], InputFormat::Json),
            Err(CodecError::Utf8)
        );
        assert!(decode_value(b"items[3]: a,b", InputFormat::Toon).is_err());
        assert!(decode_value(b"a:\n b: 2", InputFormat::Toon).is_err());
        assert!(decode_value(b"a: 2", InputFormat::Json).is_err());
        for bytes in [
            br#"{"questions":[],"nodes":[]}"#.as_slice(),
            br#"{"kind":"arbitrary"}"#.as_slice(),
        ] {
            assert_eq!(
                decode_request(bytes, InputFormat::Json),
                Err(CodecError::Kind)
            );
        }
        assert_eq!(
            InputFormat::for_path(Path::new("input.txt")),
            Err(CodecError::Format)
        );
    }

    #[test]
    fn duplicate_and_numeric_errors_are_strict_in_both_formats() {
        assert_eq!(
            decode_value(br#"{"x":1,"x":2}"#, InputFormat::Json),
            Err(CodecError::DuplicateKey)
        );
        assert!(decode_value(b"x: 1\nx: 2", InputFormat::Toon).is_err());
        for token in ["18446744073709551616", "-9223372036854775809", "1e999"] {
            assert_eq!(
                decode_value(format!("{{\"x\":{token}}}").as_bytes(), InputFormat::Json),
                Err(CodecError::Number)
            );
            assert_eq!(
                decode_value(format!("x: {token}").as_bytes(), InputFormat::Toon),
                Err(CodecError::Number)
            );
        }
    }

    #[test]
    fn official_v4_4_decode_conformance_fixtures() {
        for corpus in [
            include_str!("../../fixtures/toon/upstream/primitives.json"),
            include_str!("../../fixtures/toon/upstream/numbers.json"),
            include_str!("../../fixtures/toon/upstream/arrays-tabular.json"),
            include_str!("../../fixtures/toon/upstream/validation-errors.json"),
        ] {
            let corpus: Value = serde_json::from_str(corpus).unwrap();
            for case in corpus["tests"].as_array().unwrap() {
                let input = case["input"].as_str().unwrap();
                let mut options = toon_format::DecodeOptions::new()
                    .with_strict(case["options"]["strict"].as_bool().unwrap_or(true));
                if let Some(indent) = case["options"]["indentSize"].as_u64() {
                    options = options.with_indent(toon_format::Indent::Spaces(indent as usize));
                }
                let decoded = toon_format::decode::<Value>(input, &options);
                if case["shouldError"].as_bool().unwrap_or(false) {
                    assert!(decoded.is_err(), "{}", case["name"]);
                } else {
                    assert_eq!(
                        canonical_value(decoded.unwrap()),
                        canonical_value(case["expected"].clone()),
                        "{}",
                        case["name"]
                    );
                }
            }
        }
    }

    #[test]
    fn encoding_and_fingerprints_reject_nonfinite_values_before_null_coercion() {
        for number in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            assert_eq!(encode_toon(&number), Err(CodecError::Number));
            assert_eq!(fingerprint(&number), Err(CodecError::Number));
            let nested = vec![vec![Some(number)]];
            assert_eq!(encode_toon(&nested), Err(CodecError::Number));
            assert_eq!(fingerprint(&nested), Err(CodecError::Number));
        }
        assert_eq!(encode_toon(&()).unwrap(), "null");
        assert!(fingerprint(&()).is_ok());
    }
}
