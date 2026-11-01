//! Protection passes that decide which collected entries must survive reduction,
//! and the clean-ups that follow from that protection.
use super::{BuildError, ContextEntry, EntryKind, RequiredItem};
use crate::{
    context::{
        evidence::{self, DecisionReference},
        facts,
    },
    data::Data,
    history::{FinalizedTurn, Representation},
    identity::Id,
};
use serde::Deserialize;
use serde_json::Value;
use std::collections::HashSet;

/// Whether any string inside the value contains the reference.
fn mentions(value: &Value, reference: &str) -> bool {
    match value {
        Value::String(text) => text.contains(reference),
        Value::Array(values) => values.iter().any(|value| mentions(value, reference)),
        Value::Object(values) => values.values().any(|value| mentions(value, reference)),
        _ => false,
    }
}

/// The saved-answer reference in a collected entry's record, if any.
///
/// A record keyed and wrapped as a reference that does not parse is not a
/// usable binding, so it is protected only as ordinary context.
fn decision_reference(entry: &ContextEntry) -> Option<DecisionReference> {
    let value = evidence::decision_reference_value_of(&entry.value.value()["record"])?;
    DecisionReference::deserialize(value)
        .inspect_err(|error| {
            tracing::warn!(record = %entry.id, %error, "ignoring a malformed saved-answer reference");
        })
        .ok()
}

fn is_protected_decision(entry: &ContextEntry) -> bool {
    entry.protected && entry.kind == EntryKind::Decision
}

fn protected_decisions(entries: &[ContextEntry]) -> HashSet<Id> {
    entries
        .iter()
        .filter(|entry| is_protected_decision(entry))
        .map(|entry| entry.id)
        .collect()
}

/// Records that the given saved turns actually sent to the model.
fn records_used_by(history: &[FinalizedTurn], turns: &HashSet<Id>) -> HashSet<Id> {
    history
        .iter()
        .filter(|turn| turns.contains(&turn.id))
        .flat_map(|turn| &turn.manifest.records)
        .filter(|record| record.representation != Representation::Omitted)
        .map(|record| record.record)
        .collect()
}

/// Marks the entries in `ids` protected, reporting whether any changed.
fn protect_all(entries: &mut [ContextEntry], ids: &HashSet<Id>) -> bool {
    let mut changed = false;
    for entry in entries {
        if ids.contains(&entry.id) && !entry.protected {
            entry.protected = true;
            changed = true;
        }
    }
    changed
}

/// Resolves an RFC 6901 pointer within a required entry, rejecting pointers
/// into a refused decision outcome.
fn pointer<'a>(
    value: &'a Value,
    path: &str,
    record: Id,
    decision: bool,
) -> Result<&'a Value, BuildError> {
    if !path.is_empty() && !path.starts_with('/') {
        return Err(BuildError::InvalidPointer);
    }
    let mut chars = path.chars();
    while let Some(c) = chars.next() {
        if c == '~' && !matches!(chars.next(), Some('0' | '1')) {
            return Err(BuildError::InvalidPointer);
        }
    }
    let parts: Vec<_> = path.split('/').skip(1).collect();
    if decision && refused_outcome(value, &parts) {
        return Err(BuildError::RequiredRefused {
            record,
            pointer: path.into(),
        });
    }
    let mut current = value;
    for component in parts {
        let key = component.replace("~1", "/").replace("~0", "~");
        current = match current {
            Value::Array(values) => canonical_index(&key).and_then(|i| values.get(i)),
            Value::Object(values) => values.get(&key),
            _ => None,
        }
        .ok_or_else(|| BuildError::RequiredMissing {
            record,
            pointer: Some(path.into()),
        })?;
    }
    Ok(current)
}

/// Parses an array index written without leading zeros or signs.
fn canonical_index(text: &str) -> Option<usize> {
    text.parse::<usize>()
        .ok()
        .filter(|index| index.to_string() == text)
}

/// Whether pointer `parts` address a refused outcome of a decision.
fn refused_outcome(value: &Value, parts: &[&str]) -> bool {
    if parts.len() < 3 || parts[0] != "outcome" || parts[1] != "outcomes" {
        return false;
    }
    canonical_index(parts[2])
        .and_then(|index| {
            value
                .get("outcome")?
                .get("outcomes")?
                .get(index)?
                .get("status")
        })
        .and_then(Value::as_str)
        == Some("refused")
}

/// Protects every explicitly required entry after checking its pointer.
pub(super) fn protect_required(
    entries: &mut [ContextEntry],
    required: &[RequiredItem],
) -> Result<(), BuildError> {
    for required in required {
        let entry = entries
            .iter_mut()
            .find(|entry| entry.id == required.record)
            .ok_or_else(|| BuildError::RequiredMissing {
                record: required.record,
                pointer: required.pointer.clone(),
            })?;
        if let Some(path) = &required.pointer {
            pointer(
                entry.value.value(),
                path,
                entry.id,
                entry.kind == EntryKind::Decision,
            )?;
        }
        entry.protected = true;
    }
    Ok(())
}

/// Protects saved images that the live turn or a protected decision depends on.
///
/// Explicitly reusing a saved image is current evidence, even when the
/// original file has gone. Required historical decisions retain any image
/// payload their saved questions reference, not just an otherwise unusable
/// filename/token.
pub(super) fn protect_linked_images(
    entries: &mut [ContextEntry],
    history: &[FinalizedTurn],
    live: &Value,
) {
    let used = records_used_by(history, &protected_decisions(entries));
    let mentioned = |reference: &str| {
        mentions(live, reference)
            || entries.iter().any(|prior| {
                is_protected_decision(prior) && mentions(prior.value.value(), reference)
            })
    };
    let linked: HashSet<_> = entries
        .iter()
        .filter(|entry| {
            let record = &entry.value.value()["record"];
            entry.kind == EntryKind::Context
                && evidence::image_value_of(record)
                    .and_then(|image| image["reference"].as_str())
                    .is_some_and(|reference| {
                        !reference.is_empty() && (used.contains(&entry.id) || mentioned(reference))
                    })
        })
        .map(|entry| entry.id)
        .collect();
    protect_all(entries, &linked);
}

/// Protects saved-answer references, and the turns they cite, until no more
/// entries change.
///
/// A saved-answer reference is a dependency, not session ancestry. Keep it
/// (and its native image snapshot) when used by the live draft or a protected
/// saved turn. Walk to a fixed point for references to turns that themselves
/// used references. Never resurrect omitted inputs.
pub(super) fn protect_dependencies(
    entries: &mut [ContextEntry],
    history: &[FinalizedTurn],
    live: &Value,
) {
    loop {
        let used = records_used_by(history, &protected_decisions(entries));
        let protect: HashSet<Id> = entries
            .iter()
            .flat_map(|entry| dependencies(entry, &used, live))
            .collect();
        if !protect_all(entries, &protect) {
            break;
        }
    }
}

/// The entry itself, and any turn it cites, when the entry is in use.
fn dependencies(entry: &ContextEntry, used: &HashSet<Id>, live: &Value) -> Vec<Id> {
    if let Some(reference) = decision_reference(entry) {
        let mentioned = mentions(live, &reference.reference);
        if !(entry.protected || used.contains(&entry.id) || mentioned) {
            return Vec::new();
        }
        vec![entry.id, reference.source.turn]
    } else if evidence::is_image_record(&entry.value.value()["record"]) && used.contains(&entry.id)
    {
        vec![entry.id]
    } else {
        Vec::new()
    }
}

/// Strips a protected saved-answer reference down to its binding when its
/// source decision is already included.
///
/// Within existing ancestry the source decision is already present and now
/// protected. Keep only the explicit binding alongside that source, rather
/// than sending the same answer and images a second time. A reference
/// required through a pointer keeps its full content.
pub(super) fn collapse_ancestry_references(
    entries: &mut [ContextEntry],
    required: &[RequiredItem],
) -> Result<(), BuildError> {
    let decisions = protected_decisions(entries);
    for entry in entries.iter_mut().filter(|entry| entry.protected) {
        let pointed = required
            .iter()
            .any(|required| required.record == entry.id && required.pointer.is_some());
        if pointed {
            continue;
        }
        if decision_reference(entry)
            .is_some_and(|reference| decisions.contains(&reference.source.turn))
        {
            let mut value = entry.value.value().clone();
            evidence::reduce_to_binding(&mut value);
            entry.value = Data::new(value)?;
        }
    }
    Ok(())
}

fn is_replaced_fact(entry: &ContextEntry) -> bool {
    let value = entry.value.value();
    entry.kind == EntryKind::Context
        && value["active"] == false
        && value["record"]["key"]
            .as_str()
            .is_some_and(|key| key.starts_with(facts::KEY_PREFIX))
}

/// Omits replaced facts that nothing protects.
///
/// A replaced fact is evicted from model context so the model sees only the
/// current text. The ledger keeps it, and an explicit requirement still
/// includes it.
pub(super) fn evict_replaced_facts(entries: &mut [ContextEntry]) {
    for entry in entries {
        if !entry.protected && is_replaced_fact(entry) {
            entry.representation = Representation::Omitted;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pointers_reject_malformed_syntax_and_non_canonical_indexes() {
        let record = Id::generate();
        let value = serde_json::json!({
            "outcome": {"outcomes": [{"status": "refused"}]},
            "list": ["first"],
            "a/b": {"~": 1},
        });
        for path in ["outcome", "/bad~2escape"] {
            assert!(matches!(
                pointer(&value, path, record, true),
                Err(BuildError::InvalidPointer)
            ));
        }
        assert!(matches!(
            pointer(&value, "/outcome/outcomes/0/answer", record, true),
            Err(BuildError::RequiredRefused { .. })
        ));
        assert!(matches!(
            pointer(&value, "/outcome/outcomes/00/answer", record, true),
            Err(BuildError::RequiredMissing { .. })
        ));
        assert!(matches!(
            pointer(&value, "/list/01", record, false),
            Err(BuildError::RequiredMissing { .. })
        ));
        assert_eq!(pointer(&value, "/list/0", record, false).unwrap(), "first");
        assert_eq!(pointer(&value, "/a~1b/~0", record, false).unwrap(), 1);
        assert_eq!(pointer(&value, "", record, false).unwrap(), &value);
    }
}
