//! Deterministic model evidence assembly from fixed, declared ancestry.
use super::{
    ContextUpdate,
    projection::{self, ProjectionError},
};
use crate::{
    codec::{self, CodecError},
    data::{Data, DataError, Input},
    decision::{Question, TurnRequest},
    history::{
        EvaluationManifest, FinalizedTurn, HistoryError, RecordProjection, Representation,
        SourcePoint, order::canonical_history,
    },
    identity::Id,
    limits::{LimitError, ResolvedLimits, count::TokenCounter},
    outcome::ProviderIdentity,
};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RequiredItem {
    pub record: Id,
    pub pointer: Option<String>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EntryKind {
    Decision,
    Context,
    Base,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ContextEntry {
    pub id: Id,
    pub kind: EntryKind,
    pub value: Data,
    pub protected: bool,
    pub representation: Representation,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ContextPlan {
    pub manifest: EvaluationManifest,
    pub questions: Vec<Question>,
    pub required: Vec<RequiredItem>,
    pub entries: Vec<ContextEntry>,
}

pub struct ContextInput<'a> {
    pub context_base: Option<&'a crate::history::base::ContextBase>,
    pub runtime_only: bool,
    pub history: &'a [FinalizedTurn],
    pub sources: &'a [SourcePoint],
    pub request: &'a TurnRequest,
    pub updates: &'a [ContextUpdate],
    pub required: &'a [RequiredItem],
    pub workflow_base: &'a [Input],
    pub session: Id,
    pub turn: Id,
    pub provider: ProviderIdentity,
}

#[derive(Debug, Error)]
pub enum BuildError {
    #[error(transparent)]
    History(#[from] HistoryError),
    #[error(transparent)]
    Projection(#[from] ProjectionError),
    #[error(transparent)]
    Codec(#[from] CodecError),
    #[error(transparent)]
    Data(#[from] DataError),
    #[error(transparent)]
    Limit(#[from] LimitError),
    #[error("required record or field is absent")]
    RequiredMissing { record: Id, pointer: Option<String> },
    #[error("required field belongs to a refused result")]
    RequiredRefused { record: Id, pointer: String },
    #[error("required field pointer is invalid")]
    InvalidPointer,
    #[error("protected evidence and live questions exceed capacity")]
    ProtectedCapacity,
    #[error("optional history exceeds capacity under the selected policy")]
    OptionalCapacity,
    #[error("context policy attempted an invalid or protected reduction")]
    Policy,
    #[error("context provider/model does not match resolved limits")]
    Provider,
}

pub enum ContextAction {
    Compact { record: Id, value: Data },
    Omit { record: Id },
}
pub trait ContextPolicy {
    fn next(&self, entries: &[ContextEntry]) -> Result<Option<ContextAction>, BuildError>;
}
#[derive(Debug, Default, Clone, Copy)]
pub struct FullHistory;
impl ContextPolicy for FullHistory {
    fn next(&self, _entries: &[ContextEntry]) -> Result<Option<ContextAction>, BuildError> {
        Ok(None)
    }
}

fn data(value: &impl Serialize) -> Result<Data, BuildError> {
    Ok(Data::new(codec::to_value(value)?)?)
}
#[derive(Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum RevisionSource {
    Finalized { session: Id, turn: Id, index: usize },
    Run { session: Id, run: Id, index: usize },
    Ephemeral { node: Option<String>, index: usize },
    Pending { index: usize },
}
#[derive(Serialize)]
struct ContextMaterial<'a> {
    record: &'a super::ContextRecord,
    active: bool,
    pinned: bool,
    created_session: Option<Id>,
    created_turn: Option<Id>,
    created_run: Option<Id>,
    created_node: Option<String>,
    revision: Option<RevisionSource>,
}
fn context_entry(
    record: &super::ContextRecord,
    active: Option<&super::projection::ActiveRecord>,
    created: &super::projection::RevisionOrigin,
) -> Result<ContextEntry, BuildError> {
    use super::projection::RevisionOrigin;
    let pinned = active.is_some_and(|record| record.pinned);
    let (created_session, created_turn, created_run, created_node) = match created {
        RevisionOrigin::Turn { session, turn } => (Some(*session), Some(*turn), None, None),
        RevisionOrigin::Run { session, run } => (Some(*session), None, Some(*run), None),
        RevisionOrigin::Ephemeral { node } => (None, None, None, node.clone()),
        RevisionOrigin::Pending { .. } => (None, None, None, None),
    };
    let revision = active.map(|record| match &record.origin {
        RevisionOrigin::Turn { session, turn } => RevisionSource::Finalized {
            session: *session,
            turn: *turn,
            index: record.revision,
        },
        RevisionOrigin::Run { session, run } => RevisionSource::Run {
            session: *session,
            run: *run,
            index: record.revision,
        },
        RevisionOrigin::Ephemeral { node } => RevisionSource::Ephemeral {
            node: node.clone(),
            index: record.revision,
        },
        RevisionOrigin::Pending { .. } => RevisionSource::Pending {
            index: record.revision,
        },
    });
    Ok(ContextEntry {
        id: record.id,
        kind: EntryKind::Context,
        protected: pinned,
        representation: Representation::Full,
        value: data(&ContextMaterial {
            record,
            active: active.is_some(),
            pinned,
            created_session,
            created_turn,
            created_run,
            created_node,
            revision,
        })?,
    })
}
fn append_base(
    base: &crate::history::base::ContextBase,
    active: &[super::projection::ActiveRecord],
    protected: bool,
    entries: &mut Vec<ContextEntry>,
    records: &mut HashSet<Id>,
    bases: &mut HashSet<Id>,
) -> Result<(), BuildError> {
    if !bases.insert(base.id) {
        if protected && let Some(entry) = entries.iter_mut().find(|entry| entry.id == base.id) {
            entry.protected = true;
        }
        return Ok(());
    }
    if !base.state.is_empty() {
        entries.push(ContextEntry {id:base.id,kind:EntryKind::Base,protected,representation:Representation::Full,value:data(&serde_json::json!({"workflow_base":base.state,"run":if base.runtime_only {None} else {Some(base.id)},"runtime_only":base.runtime_only}))?});
    }
    let origin = if base.runtime_only {
        super::projection::RevisionOrigin::Ephemeral { node: None }
    } else {
        super::projection::RevisionOrigin::Run {
            session: base.session,
            run: base.id,
        }
    };
    for update in &base.updates {
        if let Some(record) = update.new_record()
            && records.insert(record.id)
        {
            entries.push(context_entry(
                record,
                active.iter().find(|active| active.record.id == record.id),
                &origin,
            )?);
        }
    }
    Ok(())
}
fn pointer<'a>(
    value: &'a serde_json::Value,
    path: &str,
    record: Id,
    decision: bool,
) -> Result<&'a serde_json::Value, BuildError> {
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
    if decision && parts.len() >= 3 && parts[0] == "outcome" && parts[1] == "outcomes" {
        let index = parts[2]
            .parse::<usize>()
            .ok()
            .filter(|i| i.to_string() == parts[2]);
        if index
            .and_then(|index| {
                value
                    .get("outcome")?
                    .get("outcomes")?
                    .get(index)?
                    .get("status")
            })
            .and_then(|status| status.as_str())
            == Some("refused")
        {
            return Err(BuildError::RequiredRefused {
                record,
                pointer: path.into(),
            });
        }
    }
    let mut current = value;
    for component in parts {
        let key = component.replace("~1", "/").replace("~0", "~");
        current = match current {
            serde_json::Value::Array(values) => key
                .parse::<usize>()
                .ok()
                .filter(|i| i.to_string() == key)
                .and_then(|i| values.get(i)),
            serde_json::Value::Object(values) => values.get(&key),
            _ => None,
        }
        .ok_or_else(|| BuildError::RequiredMissing {
            record,
            pointer: Some(path.into()),
        })?;
    }
    Ok(current)
}

#[derive(Serialize)]
struct Evidence<'a> {
    current_state: &'a [Input],
    workflow_base: &'a [Input],
    records: Vec<&'a ContextEntry>,
}
fn render(
    input: &ContextInput<'_>,
    entries: &[ContextEntry],
    mandatory: bool,
) -> Result<String, BuildError> {
    Ok(codec::encode_toon(&Evidence {
        current_state: &input.request.state,
        workflow_base: input.workflow_base,
        records: entries
            .iter()
            .filter(|entry| {
                entry.representation != Representation::Omitted && (!mandatory || entry.protected)
            })
            .collect(),
    })?)
}
fn capacity(error: &LimitError) -> bool {
    matches!(
        error,
        LimitError::TotalCapacity | LimitError::QuestionCapacity
    )
}

fn reported_sources(history: &[FinalizedTurn], sources: &[SourcePoint]) -> Vec<SourcePoint> {
    if sources
        .iter()
        .flat_map(|source| source.frontier.iter())
        .all(|head| {
            history
                .iter()
                .any(|turn| turn.id == *head && !turn.runtime_only)
        })
    {
        return sources.to_vec();
    }
    let mut reported = Vec::new();
    for source in sources {
        for head in &source.frontier {
            if let Some(turn) = history.iter().find(|turn| turn.id == *head) {
                let external = if turn.runtime_only {
                    turn.manifest.sources.clone()
                } else {
                    vec![SourcePoint {
                        session: source.session,
                        frontier: vec![*head],
                    }]
                };
                for source in external {
                    if !reported.contains(&source) {
                        reported.push(source);
                    }
                }
            }
        }
    }
    reported
}

pub struct ContextBuilder;
impl ContextBuilder {
    pub fn build(
        input: ContextInput<'_>,
        limits: &ResolvedLimits,
        counter: &impl TokenCounter,
        policy: &impl ContextPolicy,
    ) -> Result<ContextPlan, BuildError> {
        limits.validate_request(input.request, crate::limits::Modality::Text)?;
        if input.provider.provider.trim().is_empty() || input.provider.model != limits.model {
            return Err(BuildError::Provider);
        }
        let ordered = canonical_history(input.history, input.sources)?;
        if ordered.iter().any(|turn| turn.id == input.turn) {
            return Err(HistoryError::DuplicateTurn.into());
        }
        let pending_origin = projection::RevisionOrigin::Pending {
            session: if input.runtime_only {
                None
            } else {
                Some(input.session)
            },
            token: input.turn,
        };
        let active = projection::project_with_base(
            input.history,
            input.sources,
            input.context_base,
            input.updates,
            pending_origin.clone(),
        )?;
        let mut entries = Vec::new();
        let mut context_ids = HashSet::new();
        let mut base_ids = HashSet::new();
        for turn in ordered {
            if let Some(base) = &turn.context_base {
                append_base(
                    base,
                    &active,
                    input
                        .context_base
                        .is_some_and(|current| current.id == base.id),
                    &mut entries,
                    &mut context_ids,
                    &mut base_ids,
                )?;
            }
            let created = if turn.runtime_only {
                projection::RevisionOrigin::Ephemeral {
                    node: turn.workflow_node.clone(),
                }
            } else {
                projection::RevisionOrigin::Turn {
                    session: turn.session,
                    turn: turn.id,
                }
            };
            #[derive(Serialize)]
            struct Decision<'a> {
                session: Option<Id>,
                turn: Option<Id>,
                #[serde(skip_serializing_if = "Option::is_none")]
                workflow_node: Option<&'a str>,
                request: &'a TurnRequest,
                outcome: &'a crate::outcome::TurnOutcome,
            }
            entries.push(ContextEntry {
                id: turn.id,
                kind: EntryKind::Decision,
                value: data(&Decision {
                    session: if turn.runtime_only {
                        None
                    } else {
                        Some(turn.session)
                    },
                    turn: if turn.runtime_only {
                        None
                    } else {
                        Some(turn.id)
                    },
                    workflow_node: turn.workflow_node.as_deref(),
                    request: &turn.request,
                    outcome: &turn.outcome,
                })?,
                protected: false,
                representation: Representation::Full,
            });
            for update in &turn.updates {
                if let Some(record) = update.new_record() {
                    context_ids.insert(record.id);
                    entries.push(context_entry(
                        record,
                        active.iter().find(|active| active.record.id == record.id),
                        &created,
                    )?);
                }
            }
        }
        if let Some(base) = input.context_base {
            append_base(
                base,
                &active,
                true,
                &mut entries,
                &mut context_ids,
                &mut base_ids,
            )?;
        }
        for update in input.updates {
            if let Some(record) = update.new_record()
                && context_ids.insert(record.id)
            {
                entries.push(context_entry(
                    record,
                    active.iter().find(|active| active.record.id == record.id),
                    &pending_origin,
                )?);
            }
        }
        let mut ids = HashSet::new();
        if entries.iter().any(|entry| !ids.insert(entry.id)) {
            return Err(BuildError::History(HistoryError::DuplicateTurn));
        }
        for required in input.required {
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
        let mandatory = render(&input, &entries, true)?;
        if let Err(error) = limits.assess(&mandatory, input.request, counter) {
            if capacity(&error) {
                return Err(BuildError::ProtectedCapacity);
            }
            return Err(error.into());
        }
        loop {
            let context = render(&input, &entries, false)?;
            match limits.assess(&context, input.request, counter) {
                Ok(assessment) => {
                    let manifest = EvaluationManifest {
                        sources: reported_sources(input.history, input.sources),
                        records: entries
                            .iter()
                            .map(|entry| RecordProjection {
                                record: entry.id,
                                representation: entry.representation,
                                protected: entry.protected,
                            })
                            .collect(),
                        provider: input.provider,
                        budget: limits.budget,
                        input_count: assessment.total,
                        reserved_output_tokens: limits.reserved_output_tokens,
                        context_toon: context,
                    };
                    manifest.validate()?;
                    return Ok(ContextPlan {
                        manifest,
                        questions: input.request.questions.clone(),
                        required: input.required.to_vec(),
                        entries,
                    });
                }
                Err(error) if capacity(&error) => {}
                Err(error) => return Err(error.into()),
            }
            let action = policy.next(&entries)?.ok_or(BuildError::OptionalCapacity)?;
            let (id, replacement) = match action {
                ContextAction::Compact { record, value } => (record, Some(value)),
                ContextAction::Omit { record } => (record, None),
            };
            let entry = entries
                .iter_mut()
                .find(|entry| {
                    entry.id == id
                        && !entry.protected
                        && entry.representation != Representation::Omitted
                })
                .ok_or(BuildError::Policy)?;
            if let Some(value) = replacement {
                if entry.kind != EntryKind::Decision
                    || entry.representation != Representation::Full
                    || codec::encode_toon(&value)?.len() >= codec::encode_toon(&entry.value)?.len()
                {
                    return Err(BuildError::Policy);
                }
                entry.value = value;
                entry.representation = Representation::Compacted;
            } else {
                entry.representation = Representation::Omitted;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        config::Protocol,
        context::{ContextKind, ContextRecord, PinTarget},
        history::CountMethod,
        limits::count::{ByteCounter, InputCounts},
    };
    fn history() -> Vec<FinalizedTurn> {
        let data: serde_json::Value =
            serde_json::from_str(include_str!("../../../fixtures/contracts/history.json")).unwrap();
        vec![serde_json::from_value(data["turns"][0].clone()).unwrap()]
    }
    fn limits(budget: u64) -> ResolvedLimits {
        ResolvedLimits {
            protocol: Protocol::Typesafe,
            model: "offline".into(),
            budget,
            per_question: None,
            reserved_output_tokens: 128,
            capabilities: None,
        }
    }
    fn input<'a>(
        history: &'a [FinalizedTurn],
        sources: &'a [SourcePoint],
        request: &'a TurnRequest,
        updates: &'a [ContextUpdate],
        required: &'a [RequiredItem],
    ) -> ContextInput<'a> {
        ContextInput {
            context_base: None,
            runtime_only: false,
            history,
            sources,
            request,
            updates,
            required,
            workflow_base: &[],
            session: history[0].session,
            turn: Id::generate(),
            provider: history[0].outcome.provider.clone(),
        }
    }
    #[test]
    fn fixed_sources_ignore_siblings_order_and_preserve_unicode_null_and_live_questions() {
        let mut history = history();
        let original = history[0].clone();
        let sources = vec![SourcePoint {
            session: original.session,
            frontier: vec![original.id],
        }];
        let mut sibling = original.clone();
        sibling.id = Id::generate();
        sibling.session = Id::generate();
        sibling.updates.clear();
        sibling.request.state = vec![Input::Text {
            text: "UNRELATED_SIBLING".into(),
        }];
        history.push(sibling);
        let mut request = original.request.clone();
        request.questions[0].prompt = "LIVE_QUESTION_ONLY".into();
        request.state.push(Input::Text {
            text: "quotes: \"🦀\", [brackets]".into(),
        });
        let required = vec![RequiredItem {
            record: original.id,
            pointer: Some("/request/state/1/data/delay".into()),
        }];
        let plan = ContextBuilder::build(
            input(&history, &sources, &request, &[], &required),
            &limits(100_000),
            &ByteCounter,
            &FullHistory,
        )
        .unwrap();
        assert!(!plan.manifest.context_toon.contains("UNRELATED_SIBLING"));
        assert!(!plan.manifest.context_toon.contains("LIVE_QUESTION_ONLY"));
        assert_eq!(plan.questions, request.questions);
        assert_eq!(plan.required, required);
        assert_eq!(plan.manifest.input_count.method, CountMethod::ByteEstimate);
        let decoded: serde_json::Value = codec::decode(
            plan.manifest.context_toon.as_bytes(),
            codec::InputFormat::Toon,
        )
        .unwrap();
        assert!(decoded["records"][0]["value"]["request"]["state"][1]["data"]["delay"].is_null());
        assert!(plan.entries[0].protected);
        assert!(
            plan.entries
                .iter()
                .find(|entry| entry.kind == EntryKind::Context)
                .unwrap()
                .protected
        );
        history.reverse();
        assert_eq!(
            ContextBuilder::build(
                input(&history, &sources, &request, &[], &required),
                &limits(100_000),
                &ByteCounter,
                &FullHistory
            )
            .unwrap(),
            plan
        );
        let direct = ByteCounter
            .count(
                Protocol::Typesafe,
                "offline",
                &plan.manifest.context_toon,
                &request.questions,
            )
            .unwrap();
        assert_eq!(plan.manifest.input_count, direct.total);
    }
    #[test]
    fn required_missing_refused_and_null_are_distinct() {
        let history = history();
        let turn = &history[0];
        let sources = [SourcePoint {
            session: turn.session,
            frontier: vec![turn.id],
        }];
        for (path, refused) in [
            ("/outcome/outcomes/3/answer", true),
            ("/request/state/1/data/absent", false),
        ] {
            let required = [RequiredItem {
                record: turn.id,
                pointer: Some(path.into()),
            }];
            let error = ContextBuilder::build(
                input(&history, &sources, &turn.request, &[], &required),
                &limits(100_000),
                &ByteCounter,
                &FullHistory,
            )
            .unwrap_err();
            assert!(if refused {
                matches!(error, BuildError::RequiredRefused { .. })
            } else {
                matches!(error, BuildError::RequiredMissing { .. })
            });
        }
        let required = [RequiredItem {
            record: Id::generate(),
            pointer: None,
        }];
        assert!(matches!(
            ContextBuilder::build(
                input(&history, &sources, &turn.request, &[], &required),
                &limits(100_000),
                &ByteCounter,
                &FullHistory
            ),
            Err(BuildError::RequiredMissing { .. })
        ));
    }
    #[test]
    fn explicitly_required_retired_records_retain_identity_and_historical_meaning() {
        let mut history = history();
        let original = history[0].clone();
        let old = original.updates[0].new_record().unwrap().clone();
        let replacement = ContextRecord {
            id: Id::generate(),
            value: Data::new(serde_json::json!(12)).unwrap(),
            ..old.clone()
        };
        let mut successor = original.clone();
        successor.id = Id::generate();
        successor.parents = vec![original.id];
        successor.updates = vec![ContextUpdate::Supersede {
            targets: vec![old.id],
            replacement,
            pinned: None,
        }];
        let sources = [SourcePoint {
            session: successor.session,
            frontier: vec![successor.id],
        }];
        history.push(successor);
        let required = [RequiredItem {
            record: old.id,
            pointer: Some("/record/value".into()),
        }];
        let plan = ContextBuilder::build(
            input(&history, &sources, &original.request, &[], &required),
            &limits(100_000),
            &ByteCounter,
            &FullHistory,
        )
        .unwrap();
        let entry = plan
            .entries
            .iter()
            .find(|entry| entry.id == old.id)
            .unwrap();
        assert!(entry.protected);
        assert_eq!(entry.value.value()["active"], false);
        assert_eq!(entry.value.value()["record"]["value"], 8);
    }
    #[test]
    fn current_input_and_workflow_base_are_protected_even_without_saved_history() {
        let history = history();
        let mut request = history[0].request.clone();
        request.state = vec![Input::Text {
            text: "current".repeat(10_000),
        }];
        assert!(matches!(
            ContextBuilder::build(
                input(&history, &[], &request, &[], &[]),
                &limits(4096),
                &ByteCounter,
                &FullHistory
            ),
            Err(BuildError::ProtectedCapacity)
        ));
        request.state.clear();
        let base = [Input::Text {
            text: "frozen workflow base".repeat(10_000),
        }];
        let mut input = input(&history, &[], &request, &[], &[]);
        input.workflow_base = &base;
        assert!(matches!(
            ContextBuilder::build(input, &limits(4096), &ByteCounter, &FullHistory),
            Err(BuildError::ProtectedCapacity)
        ));
    }

    #[test]
    fn caller_data_status_fields_are_not_native_refusals_and_pending_revisions_are_included() {
        let history = history();
        let turn = &history[0];
        let first = ContextRecord {
            id: Id::generate(),
            key: "caller".into(),
            value: Data::new(serde_json::json!({"status":"refused","answer":42})).unwrap(),
            kind: ContextKind::Evidence,
        };
        let second = ContextRecord {
            id: Id::generate(),
            value: Data::new(serde_json::json!({"status":"refused","answer":43})).unwrap(),
            ..first.clone()
        };
        let updates = [
            ContextUpdate::Append {
                record: first.clone(),
            },
            ContextUpdate::Supersede {
                targets: vec![first.id],
                replacement: second.clone(),
                pinned: None,
            },
        ];
        let required = [
            RequiredItem {
                record: first.id,
                pointer: Some("/record/value/answer".into()),
            },
            RequiredItem {
                record: second.id,
                pointer: Some("/record/value/answer".into()),
            },
        ];
        let plan = ContextBuilder::build(
            input(&history, &[], &turn.request, &updates, &required),
            &limits(100_000),
            &ByteCounter,
            &FullHistory,
        )
        .unwrap();
        assert_eq!(plan.entries.len(), 2);
        assert_eq!(plan.entries[0].value.value()["active"], false);
        assert_eq!(plan.entries[1].value.value()["active"], true);
        assert!(plan.entries.iter().all(|entry| entry.protected));
    }

    struct CountRecords;
    impl TokenCounter for CountRecords {
        fn count(
            &self,
            _protocol: Protocol,
            _model: &str,
            context: &str,
            questions: &[Question],
        ) -> Result<InputCounts, LimitError> {
            let value: serde_json::Value =
                codec::decode(context.as_bytes(), codec::InputFormat::Toon)?;
            let tokens = 100 + value["records"].as_array().unwrap().len() as u64 * 100;
            let count = crate::history::TokenCount {
                tokens,
                exact: false,
                method: CountMethod::TokenizerEstimate,
            };
            Ok(InputCounts {
                total: count.clone(),
                per_question: vec![count; questions.len()],
            })
        }
    }
    #[test]
    fn mandatory_and_optional_overflow_differ_and_protected_policy_edits_fail() {
        let mut history = history();
        history[0].updates.clear();
        let turn = history[0].clone();
        let sources = [SourcePoint {
            session: turn.session,
            frontier: vec![turn.id],
        }];
        assert!(matches!(
            ContextBuilder::build(
                input(&history, &sources, &turn.request, &[], &[]),
                &limits(300),
                &CountRecords,
                &FullHistory
            ),
            Err(BuildError::OptionalCapacity)
        ));
        let record = ContextRecord {
            id: Id::generate(),
            key: "pin".into(),
            value: Data::new(serde_json::json!(null)).unwrap(),
            kind: ContextKind::Evidence,
        };
        let updates = [ContextUpdate::Pin {
            target: PinTarget::New { record },
        }];
        assert!(matches!(
            ContextBuilder::build(
                input(&history, &sources, &turn.request, &updates, &[]),
                &limits(300),
                &CountRecords,
                &FullHistory
            ),
            Err(BuildError::ProtectedCapacity)
        ));
        struct RemoveAll;
        impl ContextPolicy for RemoveAll {
            fn next(&self, entries: &[ContextEntry]) -> Result<Option<ContextAction>, BuildError> {
                Ok(Some(ContextAction::Omit {
                    record: entries[0].id,
                }))
            }
        }
        let required = [RequiredItem {
            record: turn.id,
            pointer: None,
        }];
        let extra = [ContextUpdate::Append {
            record: ContextRecord {
                id: Id::generate(),
                key: "optional".into(),
                value: Data::new(serde_json::json!(1)).unwrap(),
                kind: ContextKind::Evidence,
            },
        }];
        let result = ContextBuilder::build(
            input(&history, &sources, &turn.request, &extra, &required),
            &limits(400),
            &CountRecords,
            &RemoveAll,
        );
        assert!(matches!(result, Err(BuildError::Policy)));
    }
}
