//! Portable JSON/TOON data, retaining integer and floating-point representations.

use serde::{Deserialize, Deserializer, Serialize};
use serde_json::{Number, Value};
use thiserror::Error;

/// Text/structured data supported at the engine boundary.
///
/// Integers retain their i64/u64 representation; floating-point numbers must
/// be finite. Images are not an input variant in v1.
///
/// Equality compares numbers by value, so a whole-number float equals the same
/// integer. Stored payloads write `1.0` as `1`, and a value must equal its own
/// stored copy. Only a float is converted, so large integers keep their full
/// width.
#[derive(Debug, Clone, Serialize)]
#[serde(transparent)]
pub struct Data(Value);

impl PartialEq for Data {
    fn eq(&self, other: &Self) -> bool {
        same_value(&self.0, &other.0)
    }
}

fn same_value(left: &Value, right: &Value) -> bool {
    match (left, right) {
        (Value::Number(left), Value::Number(right)) => same_number(left, right),
        (Value::Array(left), Value::Array(right)) => {
            left.len() == right.len() && left.iter().zip(right).all(|(a, b)| same_value(a, b))
        }
        (Value::Object(left), Value::Object(right)) => {
            left.len() == right.len()
                && left
                    .iter()
                    .all(|(key, a)| right.get(key).is_some_and(|b| same_value(a, b)))
        }
        _ => left == right,
    }
}

fn same_number(left: &Number, right: &Number) -> bool {
    match (left.is_f64(), right.is_f64()) {
        (true, false) => {
            crate::codec::canonical_number(left.clone()) == Value::Number(right.clone())
        }
        (false, true) => {
            crate::codec::canonical_number(right.clone()) == Value::Number(left.clone())
        }
        _ => left == right,
    }
}

#[derive(Debug, Error, PartialEq)]
pub enum DataError {
    #[error("number is outside the portable finite numeric domain")]
    UnsupportedNumber,
}

impl Data {
    /// Wraps a JSON value after checking that every number is finite.
    ///
    /// # Errors
    ///
    /// Returns [`DataError::UnsupportedNumber`] if a number is not a finite `f64`.
    pub fn new(value: Value) -> Result<Self, DataError> {
        fn validate(value: &Value) -> Result<(), DataError> {
            match value {
                Value::Number(number) => {
                    let n = number.as_f64().ok_or(DataError::UnsupportedNumber)?;
                    if !n.is_finite() {
                        return Err(DataError::UnsupportedNumber);
                    }
                }
                Value::Array(values) => {
                    for value in values {
                        validate(value)?;
                    }
                }
                Value::Object(values) => {
                    for value in values.values() {
                        validate(value)?;
                    }
                }
                _ => {}
            }
            Ok(())
        }
        validate(&value)?;
        Ok(Self(value))
    }

    #[must_use]
    pub fn value(&self) -> &Value {
        &self.0
    }
}

impl<'de> Deserialize<'de> for Data {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Self::new(Value::deserialize(deserializer)?).map_err(serde::de::Error::custom)
    }
}

/// Explicitly supported state forms. Unknown media tags fail deserialization.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum Input {
    Text { text: String },
    Structured { data: Data },
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn data(value: Value) -> Data {
        Data::new(value).unwrap()
    }

    #[test]
    fn a_value_equals_its_stored_copy() {
        let live = data(json!({"answer": [{"share": 0.0}, {"share": 1.0}, {"share": 0.5}]}));
        let payload = crate::codec::encode_toon(&live).unwrap();
        let stored: Data = serde_json::from_value(
            crate::codec::decode_value(payload.as_bytes(), crate::codec::InputFormat::Toon)
                .unwrap(),
        )
        .unwrap();
        assert_ne!(live.value(), stored.value(), "the number types differ");
        assert_eq!(live, stored);
    }

    #[test]
    fn data_compares_numbers_by_value_and_structure_exactly() {
        assert_eq!(data(json!(1.0)), data(json!(1)));
        assert_eq!(data(json!(1)), data(json!(1.0)));
        assert_eq!(
            data(json!({"a": 1, "b": 2})),
            data(json!({"b": 2.0, "a": 1}))
        );
        assert_ne!(data(json!(0.5)), data(json!(0.25)));
        assert_ne!(data(json!(0.5)), data(json!(0)));
        assert_ne!(data(json!([1, 2])), data(json!([1])));
        assert_ne!(data(json!({"a": 1})), data(json!({"a": 1, "b": 2})));
        assert_ne!(data(json!([])), data(json!({})));
        assert_ne!(data(json!(0)), data(json!("0")));
        // Comparing as floats would round the integer and call these equal.
        let above = 9_007_199_254_740_993_u64;
        assert_ne!(data(json!(above)), data(json!(9_007_199_254_740_992.0)));
    }

    #[test]
    fn portable_data_preserves_null_and_full_width_integers() {
        assert_eq!(Data::new(Value::Null).unwrap().value(), &Value::Null);
        assert_eq!(
            Data::new(json!(u64::MAX)).unwrap().value().as_u64(),
            Some(u64::MAX)
        );
        assert!(Data::new(json!(9_007_199_254_740_991_u64)).is_ok());
        assert_eq!(
            Data::new(json!(i64::MIN)).unwrap().value().as_i64(),
            Some(i64::MIN)
        );
        assert!(serde_json::from_str::<Data>("1e999").is_err());
        assert!(serde_json::from_str::<Input>(r#"{"type":"image","url":"x"}"#).is_err());
    }
}
