//! Opaque generated identities and unambiguous case-sensitive session names.

use std::fmt;

use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

#[derive(Debug, Error, PartialEq)]
pub enum IdentityError {
    #[error("identifier must be a UUIDv7")]
    InvalidId,
    #[error("session name must match [A-Za-z][A-Za-z0-9._-]{{0,63}} and not be ID-shaped")]
    InvalidName,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct Id(Uuid);

impl Id {
    pub fn generate() -> Self {
        Self(Uuid::now_v7())
    }
}

impl TryFrom<String> for Id {
    type Error = IdentityError;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        let id = Uuid::parse_str(&value).map_err(|_| IdentityError::InvalidId)?;
        if id.get_version_num() != 7 || id.get_variant() != uuid::Variant::RFC4122 {
            return Err(IdentityError::InvalidId);
        }
        Ok(Self(id))
    }
}

impl From<Id> for String {
    fn from(value: Id) -> Self {
        value.0.to_string()
    }
}

impl fmt::Display for Id {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct SessionName(String);

impl SessionName {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for SessionName {
    type Error = IdentityError;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        if !(1..=64).contains(&value.len())
            || !value.as_bytes()[0].is_ascii_alphabetic()
            || !value
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
            || Uuid::parse_str(&value).is_ok()
        {
            return Err(IdentityError::InvalidName);
        }
        Ok(Self(value))
    }
}

impl From<SessionName> for String {
    fn from(value: SessionName) -> Self {
        value.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum SessionSelector {
    Id(Id),
    Name(SessionName),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_ids_are_v7_and_round_trip() {
        let first = Id::generate();
        let second = Id::generate();
        assert_ne!(first, second);
        assert_eq!(
            first,
            serde_json::from_str(&serde_json::to_string(&first).unwrap()).unwrap()
        );
        assert!(Id::try_from("550e8400-e29b-41d4-a716-446655440000".to_owned()).is_err());
    }

    #[test]
    fn validates_names_without_case_folding_or_id_ambiguity() {
        for name in ["a", "Alpha._-9", &"A".repeat(64)] {
            SessionName::try_from(name.to_owned()).unwrap();
        }
        for name in [
            "",
            "1bad",
            "a b",
            "é",
            &"A".repeat(65),
            "aaaaaaaa-aaaa-7aaa-8aaa-aaaaaaaaaaaa",
        ] {
            assert!(SessionName::try_from(name.to_owned()).is_err());
        }
        assert_ne!(
            SessionName::try_from("Foo".to_owned()).unwrap(),
            SessionName::try_from("foo".to_owned()).unwrap()
        );
    }
}
