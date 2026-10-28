//! Append-only context lifecycle contracts; active revisions are branch-local.

pub mod builder;
pub mod projection;

use std::collections::HashSet;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{data::Data, identity::Id};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContextKind {
    Evidence,
    Commitment,
}

/// A caller record is distinct from a provider's model judgment.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContextRecord {
    pub id: Id,
    pub key: String,
    pub value: Data,
    pub kind: ContextKind,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum PinTarget {
    New { record: ContextRecord },
    Existing { target: Id },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum ContextUpdate {
    Append {
        record: ContextRecord,
    },
    Pin {
        target: PinTarget,
    },
    /// `pinned: None` inherits retention from any replaced record.
    Supersede {
        targets: Vec<Id>,
        replacement: ContextRecord,
        pinned: Option<bool>,
    },
    Unpin {
        target: Id,
    },
}

#[derive(Debug, Error, PartialEq)]
pub enum ContextError {
    #[error("context record key is empty")]
    EmptyKey,
    #[error("supersession must target distinct existing records and cannot target itself")]
    InvalidTargets,
    #[error("context record is outside the selected history")]
    UnavailableRecord,
    #[error("context target is no longer active")]
    InactiveRecord,
    #[error("context record identity already exists")]
    DuplicateRecord,
    #[error("independent active replacements require explicit resolution")]
    ConflictingRevisions,
}

impl ContextRecord {
    pub fn validate(&self) -> Result<(), ContextError> {
        if self.key.trim().is_empty() {
            return Err(ContextError::EmptyKey);
        }
        Ok(())
    }
}

impl ContextUpdate {
    pub fn new_record(&self) -> Option<&ContextRecord> {
        match self {
            Self::Append { record }
            | Self::Pin {
                target: PinTarget::New { record },
            } => Some(record),
            Self::Supersede { replacement, .. } => Some(replacement),
            _ => None,
        }
    }

    /// Validates shape only. Reachability, active targets, and inherited retention
    /// are validated against the selected branch by the context projector.
    pub fn validate(&self) -> Result<(), ContextError> {
        if let Some(record) = self.new_record() {
            record.validate()?;
        }
        if let Self::Supersede {
            targets,
            replacement,
            ..
        } = self
            && (targets.is_empty()
                || targets.contains(&replacement.id)
                || targets.iter().collect::<HashSet<_>>().len() != targets.len())
        {
            return Err(ContextError::InvalidTargets);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn updates_preserve_explicit_targets_and_retention_intent() {
        let record = ContextRecord {
            id: Id::generate(),
            key: "budget".into(),
            value: Data::new(serde_json::json!(8)).unwrap(),
            kind: ContextKind::Commitment,
        };
        let update = ContextUpdate::Supersede {
            targets: vec![Id::generate()],
            replacement: record.clone(),
            pinned: None,
        };
        update.validate().unwrap();
        assert_eq!(
            update,
            serde_json::from_str(&serde_json::to_string(&update).unwrap()).unwrap()
        );
        assert_eq!(
            ContextUpdate::Supersede {
                targets: vec![record.id],
                replacement: record,
                pinned: None
            }
            .validate(),
            Err(ContextError::InvalidTargets)
        );
    }
}
