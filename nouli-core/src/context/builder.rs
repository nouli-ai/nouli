//! Deterministic model evidence assembly from fixed, declared ancestry.
mod collect;
mod protect;

use super::{
    ContextUpdate,
    projection::{self, ProjectionError},
};
use crate::{
    codec::{self, CodecError},
    data::{Data, DataError, Input},
    decision::TurnRequest,
    history::{
        EvaluationManifest, FinalizedTurn, HistoryError, RecordProjection, Representation,
        SourcePoint, TokenCount, order::canonical_history,
    },
    identity::Id,
    limits::{LimitError, ResolvedLimits, count::TokenCounter},
    outcome::ProviderIdentity,
};
use collect::{EntryCollector, unless_runtime_only};
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
    /// Chooses the next reduction for context that exceeds capacity, or `None`
    /// to stop reducing.
    ///
    /// # Errors
    ///
    /// Returns a [`BuildError`] when the policy cannot choose a reduction; the
    /// builder passes it on unchanged.
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

/// Checks the live request against `limits` and the provider they were resolved for.
fn validate(input: &ContextInput<'_>, limits: &ResolvedLimits) -> Result<(), BuildError> {
    limits.validate_request(input.request, crate::limits::Modality::Text)?;
    if input.provider.provider.trim().is_empty() || input.provider.model != limits.model {
        return Err(BuildError::Provider);
    }
    Ok(())
}

/// Collects every candidate entry in evidence order: each ancestor's context
/// base, decision, and records, then the current base, then the live turn's
/// pending records.
fn collect_entries(input: &ContextInput<'_>) -> Result<Vec<ContextEntry>, BuildError> {
    let ordered = canonical_history(input.history, input.sources)?;
    if ordered.iter().any(|turn| turn.id == input.turn) {
        return Err(HistoryError::DuplicateTurn.into());
    }
    let pending_origin = projection::RevisionOrigin::Pending {
        session: unless_runtime_only(input.runtime_only, input.session),
        token: input.turn,
    };
    let active = projection::project_with_base(
        input.history,
        input.sources,
        input.context_base,
        input.updates,
        &pending_origin,
    )?;
    let mut collector = EntryCollector::new(&active);
    for turn in ordered {
        if let Some(base) = &turn.context_base {
            let current = input
                .context_base
                .is_some_and(|current| current.id == base.id);
            collector.push_base(base, current)?;
        }
        collector.push_turn(turn)?;
    }
    if let Some(base) = input.context_base {
        collector.push_base(base, true)?;
    }
    collector.push_pending(input.updates, &pending_origin)?;
    let entries = collector.into_entries();
    let mut ids = HashSet::new();
    if entries.iter().any(|entry| !ids.insert(entry.id)) {
        return Err(BuildError::History(HistoryError::DuplicateTurn));
    }
    Ok(entries)
}

/// Marks the entries that reduction must keep, then trims protected
/// references to ancestry and evicts replaced facts.
fn protect_entries(
    input: &ContextInput<'_>,
    entries: &mut [ContextEntry],
) -> Result<(), BuildError> {
    let live = data(&(input.request, input.updates))?;
    protect::protect_required(entries, input.required)?;
    protect::protect_linked_images(entries, input.history, live.value());
    protect::protect_dependencies(entries, input.history, live.value());
    protect::collapse_ancestry_references(entries, input.required)?;
    protect::evict_replaced_facts(entries);
    Ok(())
}

/// Fails when protected evidence and live questions alone exceed capacity.
fn ensure_mandatory_fits(
    input: &ContextInput<'_>,
    entries: &[ContextEntry],
    limits: &ResolvedLimits,
    counter: &impl TokenCounter,
) -> Result<(), BuildError> {
    let mandatory = render(input, entries, true)?;
    match limits.assess(&mandatory, input.request, counter) {
        Ok(_) => Ok(()),
        Err(error) if capacity(&error) => Err(BuildError::ProtectedCapacity),
        Err(error) => Err(error.into()),
    }
}

/// Applies `policy` reductions until the rendered context fits `limits`.
fn fit_to_budget(
    input: ContextInput<'_>,
    mut entries: Vec<ContextEntry>,
    limits: &ResolvedLimits,
    counter: &impl TokenCounter,
    policy: &impl ContextPolicy,
) -> Result<ContextPlan, BuildError> {
    loop {
        let context = render(&input, &entries, false)?;
        match limits.assess(&context, input.request, counter) {
            Ok(assessment) => return plan(input, entries, limits, assessment.total, context),
            Err(error) if capacity(&error) => {}
            Err(error) => return Err(error.into()),
        }
        let action = policy.next(&entries)?.ok_or(BuildError::OptionalCapacity)?;
        reduce(&mut entries, action)?;
    }
}

/// Applies one policy reduction to an unprotected entry that is still present.
fn reduce(entries: &mut [ContextEntry], action: ContextAction) -> Result<(), BuildError> {
    let (id, replacement) = match action {
        ContextAction::Compact { record, value } => (record, Some(value)),
        ContextAction::Omit { record } => (record, None),
    };
    let entry = entries
        .iter_mut()
        .find(|entry| {
            entry.id == id && !entry.protected && entry.representation != Representation::Omitted
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
    Ok(())
}

/// Records the fitted context and each entry's projection in a validated plan.
fn plan(
    input: ContextInput<'_>,
    entries: Vec<ContextEntry>,
    limits: &ResolvedLimits,
    input_count: TokenCount,
    context_toon: String,
) -> Result<ContextPlan, BuildError> {
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
        input_count,
        reserved_output_tokens: limits.reserved_output_tokens,
        context_toon,
    };
    manifest.validate()?;
    Ok(ContextPlan { manifest, entries })
}

pub struct ContextBuilder;
impl ContextBuilder {
    /// Assembles the evidence for a turn, reducing optional history with `policy` until it fits
    /// `limits`.
    ///
    /// # Errors
    ///
    /// - [`BuildError::Limit`] if the request violates `limits`, or assessment fails for a reason
    ///   other than capacity.
    /// - [`BuildError::Provider`] if the provider name is blank or the model differs from the
    ///   limits' model.
    /// - [`BuildError::History`] if the history cannot be ordered, already contains the turn,
    ///   yields duplicate entry IDs, or produces an invalid manifest.
    /// - [`BuildError::Projection`] if the context updates cannot be projected.
    /// - [`BuildError::RequiredMissing`], [`BuildError::RequiredRefused`], or
    ///   [`BuildError::InvalidPointer`] if a required item is absent, points into a refused result,
    ///   or has a malformed pointer.
    /// - [`BuildError::ProtectedCapacity`] if protected evidence and live questions alone exceed
    ///   capacity.
    /// - [`BuildError::OptionalCapacity`] if the policy stops before the context fits, or
    ///   [`BuildError::Policy`] if it targets an unknown, protected, or omitted entry or proposes
    ///   an invalid compaction.
    /// - Any error the policy returns, or [`BuildError::Codec`] or [`BuildError::Data`] if evidence
    ///   cannot be encoded.
    pub fn build(
        input: ContextInput<'_>,
        limits: &ResolvedLimits,
        counter: &impl TokenCounter,
        policy: &impl ContextPolicy,
    ) -> Result<ContextPlan, BuildError> {
        validate(&input, limits)?;
        let mut entries = collect_entries(&input)?;
        protect_entries(&input, &mut entries)?;
        ensure_mandatory_fits(&input, &entries, limits, counter)?;
        fit_to_budget(input, entries, limits, counter, policy)
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
            profile: crate::limits::count::tests::profile(Protocol::SystemOne, "fixture"),
            protocol: Protocol::SystemOne,
            model: "offline".into(),
            budget,
            per_question: None,
            reserved_output_tokens: 128,
            max_questions: None,
            max_choices: None,
            max_score_levels: None,
        }
    }
    #[test]
    fn an_ancestry_reference_keeps_its_saved_state_after_a_fact_is_replaced() {
        let mut history = history();
        let old = ContextRecord {
            id: Id::generate(),
            key: format!("{}metro", crate::context::facts::KEY_PREFIX),
            kind: ContextKind::Evidence,
            value: Data::new(serde_json::json!("Namma metro was preferred")).unwrap(),
        };
        history[0].updates.push(ContextUpdate::Append {
            record: old.clone(),
        });
        history[0].manifest.records.push(RecordProjection {
            record: old.id,
            representation: Representation::Full,
            protected: false,
        });
        let source = &history[0];
        let sources = [SourcePoint {
            session: source.session,
            frontier: vec![source.id],
        }];
        let current = ContextRecord {
            id: Id::generate(),
            key: old.key.clone(),
            kind: ContextKind::Evidence,
            value: Data::new(serde_json::json!("Now prefer driving")).unwrap(),
        };
        let id = Id::generate();
        let snapshot=ContextRecord{id,key:format!("decision:{id}"),kind:ContextKind::Evidence,value:Data::new(serde_json::json!({"nouli_decision_reference":{
            "source":{"session":source.session,"turn":source.id,"question":source.request.questions[0].id},
            "reference":format!("[decision:{id}]"),"question":source.request.questions[0],"outcome":source.outcome.outcomes[0],"images":[],
            "input_state":{"records":[{"value":{"record":old}}]}
        }})).unwrap()};
        let updates = [
            ContextUpdate::Supersede {
                targets: vec![old.id],
                replacement: current.clone(),
                pinned: None,
            },
            ContextUpdate::Append { record: snapshot },
        ];
        let required = [
            RequiredItem {
                record: id,
                pointer: None,
            },
            RequiredItem {
                record: current.id,
                pointer: None,
            },
        ];
        let plan = ContextBuilder::build(
            input(&history, &sources, &source.request, &updates, &required),
            &limits(100_000),
            &ByteCounter,
            &FullHistory,
        )
        .unwrap();
        assert_eq!(
            plan.entries
                .iter()
                .find(|e| e.id == old.id)
                .unwrap()
                .representation,
            Representation::Omitted
        );
        let reference = &plan
            .entries
            .iter()
            .find(|e| e.id == id)
            .unwrap()
            .value
            .value()["record"]["value"]["nouli_decision_reference"];
        assert_eq!(reference["included_in_ancestry"], true);
        assert!(reference["input_state"].to_string().contains("Namma metro"));
        assert!(
            plan.manifest.context_toon.contains("Namma metro")
                && plan.manifest.context_toon.contains("Now prefer driving")
        );
    }
    /// A saved-answer reference record citing `question` 0 of `source`.
    fn saved_answer(id: Id, source: &FinalizedTurn, turn: Id) -> ContextRecord {
        ContextRecord {
            id,
            key: format!("decision:{id}"),
            kind: ContextKind::Evidence,
            value: Data::new(serde_json::json!({"nouli_decision_reference":{
                "source":{"session":source.session,"turn":turn,"question":source.request.questions[0].id},
                "reference":format!("[decision:{id}]"),
                "question":source.request.questions[0],
                "outcome":source.outcome.outcomes[0],
                "images":[]
            }}))
            .unwrap(),
        }
    }
    /// Builds with a generous budget and no reduction, expecting success.
    fn build_full(input: ContextInput<'_>) -> ContextPlan {
        ContextBuilder::build(input, &limits(100_000), &ByteCounter, &FullHistory).unwrap()
    }
    fn entry(plan: &ContextPlan, id: Id) -> &ContextEntry {
        plan.entries.iter().find(|entry| entry.id == id).unwrap()
    }
    fn decision_reference(plan: &ContextPlan, id: Id) -> &serde_json::Value {
        &entry(plan, id).value.value()["record"]["value"]["nouli_decision_reference"]
    }
    #[test]
    fn selected_answers_are_required_across_sessions_and_followups_and_deduplicate_ancestry() {
        let mut history = history();
        let original = history[0].clone();
        let sources = [SourcePoint {
            session: original.session,
            frontier: vec![original.id],
        }];
        let id = Id::generate();
        let required = [RequiredItem {
            record: id,
            pointer: None,
        }];
        let updates = [ContextUpdate::Append {
            record: saved_answer(id, &original, original.id),
        }];
        let plan = build_full(input(
            &history,
            &sources,
            &original.request,
            &updates,
            &required,
        ));
        assert!(entry(&plan, original.id).protected);
        let binding = decision_reference(&plan, id);
        assert_eq!(binding["included_in_ancestry"], true);
        assert!(
            binding.get("outcome").is_none(),
            "the already-included answer is not duplicated"
        );
        let pointed = [RequiredItem {
            record: id,
            pointer: Some("/record/value/nouli_decision_reference/outcome".into()),
        }];
        let pointed_plan = build_full(input(
            &history,
            &sources,
            &original.request,
            &updates,
            &pointed,
        ));
        assert!(decision_reference(&pointed_plan, id)["outcome"].is_object());
        // An independent source stays a question-specific snapshot, without
        // importing or mutating the other session's ancestry or active facts.
        let record = saved_answer(id, &original, Id::generate());
        let updates = [ContextUpdate::Append {
            record: record.clone(),
        }];
        let plan = build_full(input(
            &history,
            &sources,
            &original.request,
            &updates,
            &required,
        ));
        assert!(decision_reference(&plan, id).get("outcome").is_some());
        // The next fresh question may have no literal token. Its required
        // preceding decision still retains this used reference dependency.
        history[0].updates.push(ContextUpdate::Append { record });
        history[0].manifest.records.push(RecordProjection {
            record: id,
            representation: Representation::Full,
            protected: true,
        });
        let required = [RequiredItem {
            record: original.id,
            pointer: None,
        }];
        let plan = build_full(input(&history, &sources, &original.request, &[], &required));
        assert!(entry(&plan, id).protected);
        assert!(matches!(
            ContextBuilder::build(
                input(&history, &sources, &original.request, &[], &required),
                &limits(129),
                &ByteCounter,
                &FullHistory
            ),
            Err(BuildError::ProtectedCapacity)
        ));
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
                &crate::limits::count::tests::profile(Protocol::SystemOne, "offline"),
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
    fn required_image_bytes_cannot_be_evicted_to_leave_only_a_reference() {
        let history = history();
        let request = history[0].request.clone();
        let id = Id::generate();
        let mut picture = crate::media::tests::picture(1568, 1568);
        picture.reference = "[photo]".into();
        let updates = [ContextUpdate::Append {
            record: ContextRecord {
                id,
                key: format!("image:{id}"),
                kind: ContextKind::Evidence,
                value: Data::new(serde_json::json!({"nouli_image":picture})).unwrap(),
            },
        }];
        let required = [RequiredItem {
            record: id,
            pointer: None,
        }];
        assert!(matches!(
            ContextBuilder::build(
                input(&history, &[], &request, &updates, &required),
                &limits(4096),
                &ByteCounter,
                &crate::context::compact::Staged
            ),
            Err(BuildError::ProtectedCapacity)
        ));
        let mut history = history;
        history[0].updates.extend(updates);
        let sources = [SourcePoint {
            session: history[0].session,
            frontier: vec![history[0].id],
        }];
        let mut request = request;
        request.questions[0].prompt = "Look again at [photo].".into();
        assert!(matches!(
            ContextBuilder::build(
                input(&history, &sources, &request, &[], &[]),
                &limits(4096),
                &ByteCounter,
                &crate::context::compact::Staged
            ),
            Err(BuildError::ProtectedCapacity)
        ));
        history[0].request.questions[0].prompt = "Inspect [photo].".into();
        request.questions[0].prompt = "Does the preceding judgment still hold?".into();
        let required = [RequiredItem {
            record: history[0].id,
            pointer: None,
        }];
        assert!(
            matches!(
                ContextBuilder::build(
                    input(&history, &sources, &request, &[], &required),
                    &limits(4096),
                    &ByteCounter,
                    &crate::context::compact::Staged
                ),
                Err(BuildError::ProtectedCapacity)
            ),
            "a required prior question retains its image evidence"
        );
        history[0].request.questions[0].prompt = "Is this diagram useful?".into();
        history[0].manifest.records.push(RecordProjection {
            record: id,
            representation: Representation::Full,
            protected: false,
        });
        assert!(
            matches!(
                ContextBuilder::build(
                    input(&history, &sources, &request, &[], &required),
                    &limits(4096),
                    &ByteCounter,
                    &crate::context::compact::Staged
                ),
                Err(BuildError::ProtectedCapacity)
            ),
            "images used by the saved question remain available even without a literal reference in its prompt"
        );
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
            _profile: &crate::config::ProviderProfile,
            context: &str,
            questions: &[crate::decision::Question],
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
        struct RemoveAll;
        impl ContextPolicy for RemoveAll {
            fn next(&self, entries: &[ContextEntry]) -> Result<Option<ContextAction>, BuildError> {
                Ok(Some(ContextAction::Omit {
                    record: entries[0].id,
                }))
            }
        }
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
