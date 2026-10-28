//! Duplicate-aware JSON decoding and lexical integer overflow checks.

use std::{cell::Cell, fmt};

use serde::de::{DeserializeSeed, MapAccess, SeqAccess, Visitor};
use serde_json::{Map, Value};

use super::CodecError;

struct Seed<'a>(&'a Cell<bool>);
struct JsonVisitor<'a>(&'a Cell<bool>);

impl<'de> DeserializeSeed<'de> for Seed<'_> {
    type Value = Value;
    fn deserialize<D: serde::Deserializer<'de>>(self, de: D) -> Result<Value, D::Error> {
        de.deserialize_any(JsonVisitor(self.0))
    }
}

impl<'de> Visitor<'de> for JsonVisitor<'_> {
    type Value = Value;
    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("a JSON value")
    }
    fn visit_bool<E: serde::de::Error>(self, v: bool) -> Result<Value, E> {
        Ok(Value::Bool(v))
    }
    fn visit_i64<E: serde::de::Error>(self, v: i64) -> Result<Value, E> {
        Ok(Value::from(v))
    }
    fn visit_u64<E: serde::de::Error>(self, v: u64) -> Result<Value, E> {
        Ok(Value::from(v))
    }
    fn visit_f64<E: serde::de::Error>(self, v: f64) -> Result<Value, E> {
        serde_json::Number::from_f64(v)
            .map(Value::Number)
            .ok_or_else(|| E::custom("nonfinite number"))
    }
    fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<Value, E> {
        Ok(Value::String(v.to_owned()))
    }
    fn visit_string<E: serde::de::Error>(self, v: String) -> Result<Value, E> {
        Ok(Value::String(v))
    }
    fn visit_unit<E: serde::de::Error>(self) -> Result<Value, E> {
        Ok(Value::Null)
    }
    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Value, A::Error> {
        let mut values = Vec::new();
        while let Some(value) = seq.next_element_seed(Seed(self.0))? {
            values.push(value);
        }
        Ok(Value::Array(values))
    }
    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Value, A::Error> {
        let mut values = Map::new();
        while let Some(key) = map.next_key::<String>()? {
            if values.contains_key(&key) {
                self.0.set(true);
                return Err(serde::de::Error::custom("duplicate object key"));
            }
            values.insert(key, map.next_value_seed(Seed(self.0))?);
        }
        Ok(Value::Object(values))
    }
}

pub(super) fn decode(input: &str) -> Result<Value, CodecError> {
    // JSON grammar validation still belongs to serde_json; this pre-pass checks
    // numeric spelling before its out-of-width integer fallback loses precision.
    let mut chars = input.char_indices().peekable();
    while let Some((start, c)) = chars.next() {
        if c == '"' {
            while let Some((_, c)) = chars.next() {
                if c == '\\' {
                    chars.next();
                } else if c == '"' {
                    break;
                }
            }
        } else if c == '-' || c.is_ascii_digit() {
            let mut end = start + c.len_utf8();
            while let Some(&(position, c)) = chars.peek() {
                if c.is_ascii_digit() || matches!(c, '.' | 'e' | 'E' | '+' | '-') {
                    end = position + 1;
                    chars.next();
                } else {
                    break;
                }
            }
            let token = &input[start..end];
            if !token.contains(['.', 'e', 'E']) {
                if token.parse::<i64>().is_err() && token.parse::<u64>().is_err() {
                    return Err(CodecError::Number);
                }
            } else if token.parse::<f64>().is_ok_and(|n| !n.is_finite()) {
                return Err(CodecError::Number);
            }
        }
    }
    let duplicate = Cell::new(false);
    let mut de = serde_json::Deserializer::from_str(input);
    let result = Seed(&duplicate).deserialize(&mut de);
    if duplicate.get() {
        return Err(CodecError::DuplicateKey);
    }
    let value = result.map_err(|_| CodecError::Syntax)?;
    de.end().map_err(|_| CodecError::Syntax)?;
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn duplicate_keys_are_detected_inside_arbitrary_nested_data() {
        assert_eq!(
            decode(r#"{"state":[{"a":1,"\u0061":2}]}"#),
            Err(CodecError::DuplicateKey)
        );
        assert!(decode(r#"{"text":"18446744073709551616"}"#).is_ok());
    }
}
