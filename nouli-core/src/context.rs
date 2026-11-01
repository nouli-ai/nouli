//! Append-only context lifecycle contracts; active revisions are branch-local.

pub mod builder;
pub mod compact;
mod decision_entry;
pub mod evidence;
pub mod facts;
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

/// Check each update's shape and reject duplicate identities among newly created records.
///
/// # Errors
///
/// Returns any [`ContextUpdate::validate`] error before checking record identities,
/// or [`ContextError::DuplicateRecord`] when two updates create the same record.
pub fn validate_updates(updates: &[ContextUpdate]) -> Result<(), ContextError> {
    for update in updates {
        update.validate()?;
    }
    let mut ids = HashSet::new();
    for update in updates {
        if let Some(record) = update.new_record()
            && !ids.insert(record.id)
        {
            return Err(ContextError::DuplicateRecord);
        }
    }
    Ok(())
}

impl ContextRecord {
    /// Checks that the record has a non-blank key.
    ///
    /// # Errors
    ///
    /// Returns [`ContextError::EmptyKey`] if the key is blank.
    pub fn validate(&self) -> Result<(), ContextError> {
        if self.key.trim().is_empty() {
            return Err(ContextError::EmptyKey);
        }
        Ok(())
    }
}

impl ContextUpdate {
    #[must_use]
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
    ///
    /// # Errors
    ///
    /// - [`ContextError::EmptyKey`] if a new record has a blank key.
    /// - [`ContextError::InvalidTargets`] if a supersession has no targets, repeats a target, or
    ///   targets its own replacement.
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
    #[test]
    fn shared_validation_checks_shapes_before_duplicate_record_identities() {
        let record = ContextRecord {
            id: Id::generate(),
            key: "budget".into(),
            value: Data::new(serde_json::json!(8)).unwrap(),
            kind: ContextKind::Commitment,
        };
        let append = ContextUpdate::Append {
            record: record.clone(),
        };
        let pin = ContextUpdate::Pin {
            target: PinTarget::New {
                record: record.clone(),
            },
        };
        assert_eq!(
            validate_updates(&[append.clone(), pin.clone()]),
            Err(ContextError::DuplicateRecord)
        );
        let mut invalid = record.clone();
        invalid.key.clear();
        assert_eq!(
            validate_updates(&[append, pin, ContextUpdate::Append { record: invalid }]),
            Err(ContextError::EmptyKey)
        );
        validate_updates(&[
            ContextUpdate::Unpin { target: record.id },
            ContextUpdate::Unpin { target: record.id },
        ])
        .unwrap();
        let mut workflow: crate::workflow::WorkflowSpec =
            serde_json::from_str(include_str!("../../fixtures/contracts/workflow.json")).unwrap();
        workflow.nodes[0].updates = vec![
            ContextUpdate::Append {
                record: record.clone()
            };
            2
        ];
        assert_eq!(
            workflow.validate(),
            Err(crate::workflow::WorkflowError::Context(
                ContextError::DuplicateRecord
            ))
        );
        workflow.nodes[0].updates.clear();
        let request = crate::workflow::WorkflowRequest {
            workflow,
            session: crate::history::SessionSelection::New { name: None },
            updates: vec![ContextUpdate::Append { record }; 2],
            required_records: vec![],
            request_id: None,
        };
        assert!(matches!(
            request.validate(),
            Err(crate::workflow::RequestError::History(
                crate::history::HistoryError::Context(ContextError::DuplicateRecord)
            ))
        ));
    }
}
