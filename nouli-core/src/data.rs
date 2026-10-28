//! Portable JSON/TOON data, retaining integer and floating-point representations.

use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;
use thiserror::Error;

/// Text/structured data supported at the engine boundary.
///
/// Integers retain their i64/u64 representation; floating-point numbers must
/// be finite. Images are not an input variant in v1.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(transparent)]
pub struct Data(Value);

#[derive(Debug, Error, PartialEq)]
pub enum DataError {
    #[error("number is outside the portable finite numeric domain")]
    UnsupportedNumber,
}

impl Data {
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
