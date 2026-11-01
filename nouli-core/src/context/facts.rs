//! Facts: caller evidence stored as numbered, replaceable context records.
//!
//! A fact is an evidence record whose key is `fact:<record id>`. Replacing it
//! supersedes the record with one that keeps the key, so the earlier text stays
//! in history while the fact keeps its number.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use super::{ContextKind, ContextRecord, ContextUpdate, projection};
use crate::{
    data::Data,
    identity::{Id, SessionSelector},
    storage::{Ledger, turns::TurnError},
};

/// Key prefix shared by every fact record.
pub const KEY_PREFIX: &str = "fact:";

#[derive(Debug, Error, PartialEq, Eq)]
pub enum FactError {
    #[error("fact text is empty")]
    Empty,
}

/// An active fact at a session's head.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Fact {
    /// 1-based position in context order.
    pub number: usize,
    /// The records holding the current text. Replacing the fact changes them.
    /// There is more than one only when branches that replaced the same fact
    /// were blended: each record is then an alternative text.
    pub records: Vec<Id>,
    /// The stable `fact:<id>` key shared by every version of this fact.
    pub key: String,
    /// The current text. Conflicting alternatives are joined with ` | `.
    pub text: String,
    /// The text of each record, in `records` order, when the fact has more
    /// than one; empty otherwise.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub alternatives: Vec<String>,
    /// The turn that saved the current text (the first alternative's).
    pub turn: Id,
    /// The session that saved the current text, when it is not the listed
    /// session itself: the fact came from a branch or blend source.
    pub source_session: Option<Id>,
    /// The text before the latest replacement, when the fact was replaced.
    #[serde(default)]
    pub previous: Option<String>,
}

fn record(id: Id, key: String, text: &str) -> Result<ContextRecord, FactError> {
    let text = text.trim();
    if text.is_empty() {
        return Err(FactError::Empty);
    }
    Ok(ContextRecord {
        id,
        key,
        value: Data::new(serde_json::Value::String(text.to_owned()))
            .map_err(|_| FactError::Empty)?,
        kind: ContextKind::Evidence,
    })
}

/// Appends a new fact. The text is trimmed; empty text is rejected.
pub fn append(text: &str) -> Result<ContextUpdate, FactError> {
    append_with(Id::generate(), text)
}

/// Appends a fact under a chosen record ID. Callers that replay a request use
/// IDs derived from the request, so the repeated request has the same intent.
pub fn append_with(id: Id, text: &str) -> Result<ContextUpdate, FactError> {
    Ok(ContextUpdate::Append {
        record: record(id, format!("{KEY_PREFIX}{id}"), text)?,
    })
}

/// Replaces a fact's text, keeping its key and number. The earlier version
/// stays in the ledger. A fact with conflicting alternatives has them all
/// superseded.
pub fn replace(fact: &Fact, text: &str) -> Result<ContextUpdate, FactError> {
    replace_with(Id::generate(), fact, text)
}

/// Replaces a fact with a record of a chosen ID; see [`append_with`].
pub fn replace_with(id: Id, fact: &Fact, text: &str) -> Result<ContextUpdate, FactError> {
    Ok(ContextUpdate::Supersede {
        targets: fact.records.clone(),
        replacement: record(id, fact.key.clone(), text)?,
        pinned: None,
    })
}

impl Ledger {
    /// The active facts at the session's head. They follow the order in which
    /// each fact was first added, so a replacement keeps its predecessor's
    /// number. A blend lists the facts of every source.
    pub fn facts(&self, selector: &SessionSelector) -> Result<Vec<Fact>, TurnError> {
        let session = self.session(selector)?;
        let sources = self.context_sources(session.id)?;
        let history = self.ancestry(&sources)?;
        let mut rank: HashMap<&str, usize> = HashMap::new();
        let mut versions: HashMap<&str, Vec<&str>> = HashMap::new();
        for turn in &history {
            for update in &turn.updates {
                if let Some(record) = update.new_record()
                    && record.key.starts_with(KEY_PREFIX)
                {
                    let next = rank.len();
                    rank.entry(record.key.as_str()).or_insert(next);
                    if let Some(text) = record.value.value().as_str() {
                        versions.entry(record.key.as_str()).or_default().push(text);
                    }
                }
            }
        }
        let active = projection::project(&history, &sources, &[], session.id, session.id)?;
        // Group the active versions by key; blended branches that replaced the
        // same fact leave several, which stay together as one numbered fact.
        let mut groups: Vec<(&str, Vec<&projection::ActiveRecord>)> = Vec::new();
        for record in active.iter().filter(|active| {
            active.record.kind == ContextKind::Evidence
                && active.record.key.starts_with(KEY_PREFIX)
                && active.record.value.value().is_string()
        }) {
            match groups
                .iter_mut()
                .find(|(key, _)| *key == record.record.key.as_str())
            {
                Some((_, members)) => members.push(record),
                None => groups.push((record.record.key.as_str(), vec![record])),
            }
        }
        let mut facts: Vec<_> = groups
            .into_iter()
            .filter_map(|(key, members)| {
                let texts: Vec<String> = members
                    .iter()
                    .filter_map(|active| active.record.value.value().as_str().map(str::to_owned))
                    .collect();
                let projection::RevisionOrigin::Turn {
                    session: origin_session,
                    turn,
                } = &members[0].origin
                else {
                    return None;
                };
                let (origin_session, turn) = (*origin_session, *turn);
                let previous = if members.len() == 1 {
                    versions
                        .get(key)
                        .and_then(|texts| texts.iter().rev().nth(1))
                        .map(|text| (*text).to_owned())
                } else {
                    None
                };
                Some((
                    rank.get(key).copied()?,
                    Fact {
                        number: 0,
                        records: members.iter().map(|active| active.record.id).collect(),
                        key: key.to_owned(),
                        text: texts.join(" | "),
                        alternatives: if texts.len() > 1 { texts } else { Vec::new() },
                        turn,
                        source_session: (origin_session != session.id).then_some(origin_session),
                        previous,
                    },
                ))
            })
            .collect();
        facts.sort_by_key(|(rank, _)| *rank);
        Ok(facts
            .into_iter()
            .enumerate()
            .map(|(index, (_, mut fact))| {
                fact.number = index + 1;
                fact
            })
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::history::StandingChanges;
    use crate::{
        history::{FinalizedTurn, ModelSelection, SourceSelection},
        storage::{OpenMode, turns::AttemptInput},
    };

    struct Store(std::path::PathBuf);
    impl Store {
        fn new() -> Self {
            Self(std::env::temp_dir().join(format!("nouli-facts-{}", Id::generate())))
        }
        fn open(&self) -> Ledger {
            Ledger::open(&self.0, OpenMode::ReadWriteCreate).unwrap()
        }
    }
    impl Drop for Store {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn commit(ledger: &mut Ledger, session: Id, updates: Vec<ContextUpdate>) -> FinalizedTurn {
        let data: serde_json::Value =
            serde_json::from_str(include_str!("../../../fixtures/contracts/history.json")).unwrap();
        let mut fixture: FinalizedTurn = serde_json::from_value(data["turns"][0].clone()).unwrap();
        let guard = ledger.session_guard(session).unwrap();
        fixture.manifest.sources = ledger.context_sources(session).unwrap();
        let attempt = ledger
            .prepare_attempt(
                &guard,
                AttemptInput {
                    request: fixture.request,
                    updates,
                    standing: StandingChanges::default(),
                    manifest: fixture.manifest,
                    request_id: None,
                },
            )
            .unwrap();
        ledger
            .finalize_attempt(&guard, attempt.id, fixture.outcome)
            .unwrap()
    }
    fn texts(facts: &[Fact]) -> Vec<(usize, &str)> {
        facts
            .iter()
            .map(|fact| (fact.number, fact.text.as_str()))
            .collect()
    }
    fn list(ledger: &Ledger, session: Id) -> Vec<Fact> {
        ledger.facts(&SessionSelector::Id(session)).unwrap()
    }
    fn session(ledger: &mut Ledger) -> Id {
        ledger
            .create_session(None, ModelSelection::default())
            .unwrap()
            .id
    }

    #[test]
    fn append_and_replace_build_keyed_evidence_updates_and_reject_empty_text() {
        let ContextUpdate::Append { record } = append("  budget is 40 USD ").unwrap() else {
            panic!("append must append");
        };
        assert_eq!(record.kind, ContextKind::Evidence);
        assert_eq!(record.key, format!("fact:{}", record.id));
        assert_eq!(record.value.value(), "budget is 40 USD");
        assert_eq!(append(""), Err(FactError::Empty));
        assert_eq!(append(" \n\t"), Err(FactError::Empty));
        let fact = Fact {
            number: 1,
            records: vec![record.id],
            key: record.key.clone(),
            text: "budget is 40 USD".into(),
            alternatives: Vec::new(),
            turn: Id::generate(),
            source_session: None,
            previous: None,
        };
        assert_eq!(replace(&fact, "  "), Err(FactError::Empty));
        let ContextUpdate::Supersede {
            targets,
            replacement,
            pinned,
        } = replace(&fact, "budget is 60 USD").unwrap()
        else {
            panic!("replace must supersede");
        };
        assert_eq!(targets, vec![record.id]);
        assert_eq!(replacement.key, record.key);
        assert_ne!(replacement.id, record.id);
        assert_eq!(pinned, None);
    }

    #[test]
    fn facts_are_numbered_across_turns_and_a_replacement_keeps_its_number() {
        let store = Store::new();
        let mut ledger = store.open();
        let id = session(&mut ledger);
        commit(
            &mut ledger,
            id,
            vec![append("one").unwrap(), append("two").unwrap()],
        );
        let second = commit(&mut ledger, id, vec![append("three").unwrap()]);
        let facts = list(&ledger, id);
        assert_eq!(texts(&facts), [(1, "one"), (2, "two"), (3, "three")]);
        assert_eq!(facts[2].turn, second.id);
        assert!(facts.iter().all(|fact| fact.source_session.is_none()));
        let replaced = commit(&mut ledger, id, vec![replace(&facts[1], "TWO").unwrap()]);
        let after = list(&ledger, id);
        assert_eq!(texts(&after), [(1, "one"), (2, "TWO"), (3, "three")]);
        assert_eq!(after[1].key, facts[1].key);
        assert_ne!(after[1].records, facts[1].records);
        assert_eq!(after[1].turn, replaced.id);
        assert_eq!(after[1].previous.as_deref(), Some("two"));
        assert_eq!(after[0].previous, None);
        // The original text is still in the first turn's history.
        let history = ledger
            .ancestry(&ledger.context_sources(id).unwrap())
            .unwrap();
        let originals: Vec<_> = history
            .iter()
            .flat_map(|turn| &turn.updates)
            .filter_map(|update| update.new_record())
            .map(|record| record.value.value().as_str().unwrap().to_owned())
            .collect();
        assert_eq!(originals, ["one", "two", "three", "TWO"]);
        // Replacing again targets the current version.
        commit(&mut ledger, id, vec![replace(&after[1], "Two").unwrap()]);
        assert_eq!(
            texts(&list(&ledger, id)),
            [(1, "one"), (2, "Two"), (3, "three")]
        );
    }

    #[test]
    fn a_blend_lists_both_sources_facts_and_a_branch_sees_only_its_frontier() {
        let store = Store::new();
        let mut ledger = store.open();
        let left = session(&mut ledger);
        let right = session(&mut ledger);
        let first = commit(&mut ledger, left, vec![append("left one").unwrap()]);
        commit(&mut ledger, left, vec![append("left two").unwrap()]);
        commit(&mut ledger, right, vec![append("right one").unwrap()]);
        let select = |session: Id, frontier: Option<Vec<Id>>| SourceSelection {
            session: SessionSelector::Id(session),
            frontier,
        };
        let older = ledger
            .create_session_from(
                None,
                ModelSelection::default(),
                &[select(left, Some(vec![first.id]))],
            )
            .unwrap();
        let facts = list(&ledger, older.id);
        assert_eq!(texts(&facts), [(1, "left one")]);
        assert_eq!(facts[0].source_session, Some(left));
        let blend = ledger
            .create_session_from(
                None,
                ModelSelection::default(),
                &[select(left, None), select(right, None)],
            )
            .unwrap();
        commit(&mut ledger, blend.id, vec![append("blend own").unwrap()]);
        let facts = list(&ledger, blend.id);
        assert_eq!(
            texts(&facts),
            [
                (1, "left one"),
                (2, "left two"),
                (3, "right one"),
                (4, "blend own")
            ]
        );
        assert_eq!(facts[0].source_session, Some(left));
        assert_eq!(facts[2].source_session, Some(right));
        assert_eq!(facts[3].source_session, None);
        // Replacing a source's fact from the blend takes ownership of the new text.
        commit(
            &mut ledger,
            blend.id,
            vec![replace(&facts[2], "RIGHT").unwrap()],
        );
        let facts = list(&ledger, blend.id);
        assert_eq!(facts[2].text, "RIGHT");
        assert_eq!(facts[2].source_session, None);
        assert_eq!(texts(&list(&ledger, right)), [(1, "right one")]);
    }

    #[test]
    fn blended_branches_that_replaced_one_fact_list_it_once_and_one_replacement_settles_it() {
        let store = Store::new();
        let mut ledger = store.open();
        let root = session(&mut ledger);
        commit(
            &mut ledger,
            root,
            vec![append("one").unwrap(), append("two").unwrap()],
        );
        let select = |session: Id| SourceSelection {
            session: SessionSelector::Id(session),
            frontier: None,
        };
        let base = list(&ledger, root);
        let mut branches = Vec::new();
        for text in ["cheap", "dear"] {
            let branch = ledger
                .create_session_from(None, ModelSelection::default(), &[select(root)])
                .unwrap();
            commit(
                &mut ledger,
                branch.id,
                vec![replace(&base[0], text).unwrap()],
            );
            branches.push(branch.id);
        }
        let blend = ledger
            .create_session_from(
                None,
                ModelSelection::default(),
                &[select(branches[0]), select(branches[1])],
            )
            .unwrap();
        // Listing does not fail: the fact appears once with both alternatives.
        let facts = list(&ledger, blend.id);
        assert_eq!(facts.len(), 2);
        assert_eq!(facts[0].number, 1);
        assert_eq!(facts[0].records.len(), 2);
        assert_eq!(facts[0].alternatives, ["cheap", "dear"]);
        assert_eq!(facts[0].text, "cheap | dear");
        assert_eq!(facts[0].previous, None);
        assert_eq!(facts[1].records.len(), 1);
        assert!(facts[1].alternatives.is_empty());
        // A plain append is allowed and keeps both alternatives.
        commit(&mut ledger, blend.id, vec![append("three").unwrap()]);
        let facts = list(&ledger, blend.id);
        assert_eq!(facts.len(), 3);
        assert_eq!(facts[0].alternatives, ["cheap", "dear"]);
        // One replacement supersedes every alternative.
        let update = replace(&facts[0], "settled").unwrap();
        let ContextUpdate::Supersede { targets, .. } = &update else {
            panic!("replace must supersede");
        };
        assert_eq!(targets, &facts[0].records);
        commit(&mut ledger, blend.id, vec![update]);
        let facts = list(&ledger, blend.id);
        assert_eq!(texts(&facts), [(1, "settled"), (2, "two"), (3, "three")]);
        assert_eq!(facts[0].records.len(), 1);
        assert!(facts[0].alternatives.is_empty());
    }

    #[test]
    fn other_records_are_not_facts() {
        let store = Store::new();
        let mut ledger = store.open();
        let id = session(&mut ledger);
        let other = ContextRecord {
            id: Id::generate(),
            key: "budget".into(),
            value: Data::new(serde_json::json!("not a fact")).unwrap(),
            kind: ContextKind::Evidence,
        };
        commit(
            &mut ledger,
            id,
            vec![
                ContextUpdate::Append { record: other },
                append("real").unwrap(),
            ],
        );
        assert_eq!(texts(&list(&ledger, id)), [(1, "real")]);
    }
}
