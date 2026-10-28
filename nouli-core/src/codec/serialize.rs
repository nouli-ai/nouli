//! Inspect Serde values before JSON's non-finite-to-null conversion can occur.

use serde::Serialize;
use serde_value::Value;

use super::CodecError;

pub(super) fn checked_value<T: Serialize>(input: &T) -> Result<serde_json::Value, CodecError> {
    let value = serde_value::to_value(input).map_err(|_| CodecError::Shape)?;
    let mut pending = vec![(&value, 0)];
    while let Some((value, depth)) = pending.pop() {
        if depth > 128 {
            return Err(CodecError::Representation);
        }
        match value {
            Value::F32(n) if !n.is_finite() => return Err(CodecError::Number),
            Value::F64(n) if !n.is_finite() => return Err(CodecError::Number),
            Value::Option(Some(value)) | Value::Newtype(value) => pending.push((value, depth + 1)),
            Value::Seq(values) => pending.extend(values.iter().map(|v| (v, depth + 1))),
            Value::Map(values) => {
                for (key, value) in values {
                    pending.push((key, depth + 1));
                    pending.push((value, depth + 1));
                }
            }
            _ => {}
        }
    }
    serde_json::to_value(value).map_err(|_| CodecError::Shape)
}
