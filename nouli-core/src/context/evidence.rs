//! Saved image and saved-answer evidence records, and the inline markers that
//! bind text to them.
//!
//! Both are ordinary [`ContextRecord`]s identified by a key prefix and a
//! single wrapper field in their value:
//!
//! - an image is keyed `image:…` with value `{"nouli_image": Image}`;
//! - a saved-answer reference is keyed `decision:<id>` with value
//!   `{"nouli_decision_reference": DecisionReference}`.
//!
//! Record values are stored as canonical JSON objects with sorted keys, so the
//! typed forms here serialize to exactly the bytes earlier versions wrote.
pub mod marker;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use thiserror::Error;

use super::{ContextKind, ContextRecord};
use crate::{
    data::{Data, DataError},
    decision::Question,
    identity::Id,
    media::Image,
    outcome::{ProviderIdentity, QuestionOutcome},
};

/// The key prefix of a saved image record.
pub const IMAGE_KEY_PREFIX: &str = "image:";
/// The key prefix of a saved-answer reference record.
pub const DECISION_KEY_PREFIX: &str = "decision:";
/// The value field that wraps a saved image.
pub const IMAGE_FIELD: &str = "nouli_image";
/// The value field that wraps a saved-answer reference.
pub const DECISION_REFERENCE_FIELD: &str = "nouli_decision_reference";

#[derive(Debug, Error)]
pub enum EvidenceError {
    #[error("a saved image record is malformed")]
    Image(#[source] serde_json::Error),
    #[error("a saved-answer reference record is malformed")]
    DecisionReference(#[source] serde_json::Error),
    #[error(transparent)]
    Data(#[from] DataError),
}

/// One finalized question in one saved turn of one session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DecisionSource {
    pub session: Id,
    pub turn: Id,
    pub question: String,
}

/// A saved image copied into a saved-answer snapshot, as an embedded record.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ImageSnapshot {
    pub record: ContextRecord,
}

impl ImageSnapshot {
    /// Embeds `image` under a record keyed by its own marker text.
    #[must_use]
    pub fn new(id: Id, image: &Image) -> Self {
        Self {
            record: ContextRecord {
                id,
                key: format!("{IMAGE_KEY_PREFIX}{}", image.reference),
                value: image_value(image),
                kind: ContextKind::Evidence,
            },
        }
    }
}

/// A fixed snapshot of one saved answer, used as ordinary evidence.
///
/// Every field but `source` and `reference` is optional: a snapshot whose
/// source decision is already in context keeps only its binding
/// (`included_in_ancestry`), and earlier versions wrote `input_references`
/// instead of `input_state`. Field order is the persisted key order.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DecisionReference {
    pub source: DecisionSource,
    /// The `[decision:UUID]` marker that text uses to cite this snapshot.
    pub reference: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub question: Option<Question>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outcome: Option<QuestionOutcome>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<ProviderIdentity>,
    /// Native images from the source turn's saved input.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub images: Option<Vec<ImageSnapshot>>,
    /// The source turn's complete saved input state.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_state: Option<Value>,
    /// The source turn's hand-picked reference inputs, written before
    /// `input_state` replaced them. Read-only legacy content.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_references: Option<Value>,
    /// The human label shown in place of the marker.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// Set when the source decision is already in context, so this
    /// snapshot carries only its binding.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub included_in_ancestry: bool,
}

impl DecisionReference {
    /// A complete snapshot cited by the marker for `id`.
    #[must_use]
    pub fn snapshot(
        id: Id,
        source: DecisionSource,
        question: Question,
        outcome: QuestionOutcome,
        provider: ProviderIdentity,
        images: Vec<ImageSnapshot>,
        input_state: Value,
    ) -> Self {
        Self {
            source,
            reference: marker::decision_marker(id),
            question: Some(question),
            outcome: Some(outcome),
            provider: Some(provider),
            images: Some(images),
            input_state: Some(input_state),
            input_references: None,
            label: None,
            included_in_ancestry: false,
        }
    }

    /// The record value: `{"nouli_decision_reference": self}`.
    ///
    /// # Errors
    ///
    /// Returns [`EvidenceError::Data`] if the saved input state contains a
    /// non-finite number.
    pub fn to_value(&self) -> Result<Data, EvidenceError> {
        let reference = serde_json::to_value(self).map_err(EvidenceError::DecisionReference)?;
        Ok(Data::new(json!({ DECISION_REFERENCE_FIELD: reference }))?)
    }

    /// The `decision:<id>` record holding this snapshot.
    ///
    /// # Errors
    ///
    /// Returns [`EvidenceError::Data`] if the saved input state contains a
    /// non-finite number.
    pub fn record(&self, id: Id) -> Result<ContextRecord, EvidenceError> {
        Ok(ContextRecord {
            id,
            key: decision_key(id),
            value: self.to_value()?,
            kind: ContextKind::Evidence,
        })
    }
}

fn decision_key(id: Id) -> String {
    format!("{DECISION_KEY_PREFIX}{id}")
}

fn image_value(image: &Image) -> Data {
    Data::new(json!({ IMAGE_FIELD: image })).expect("image fields are strings, never numbers")
}

/// A new `image:<id>` record holding `image`.
///
/// # Panics
///
/// Never: an image has only string fields, which are always portable data.
#[must_use]
pub fn image_record(id: Id, image: &Image) -> ContextRecord {
    ContextRecord {
        id,
        key: format!("{IMAGE_KEY_PREFIX}{id}"),
        value: image_value(image),
        kind: ContextKind::Evidence,
    }
}

/// A copy of a saved-answer reference record under a new identity, keeping
/// its value (and so its marker text) unchanged. Use it to carry a binding
/// into another history, where record IDs must be new.
#[must_use]
pub fn rebind_decision_record(record: &ContextRecord, id: Id) -> ContextRecord {
    ContextRecord {
        id,
        key: decision_key(id),
        value: record.value.clone(),
        kind: record.kind,
    }
}

/// The fields a saved-answer binding drops when its source decision already
/// supplies them. Saved input state stays: it is the frozen input, not the
/// source's current facts. `input_references` is the legacy form of that
/// input, written before `input_state` replaced it; its content was taken
/// from the source, so it goes too.
const SOURCE_CONTENT: [&str; 4] = ["question", "outcome", "images", "input_references"];

/// Reduces the saved-answer reference in a JSON record (`{"record": …}`) to
/// its binding, marking it `included_in_ancestry`. Edits the stored object
/// in place, so the persisted key order matches what earlier versions wrote.
pub(crate) fn reduce_to_binding(entry: &mut Value) {
    let Some(reference) = entry
        .get_mut("record")
        .and_then(|record| record.get_mut("value"))
        .and_then(|value| value.get_mut(DECISION_REFERENCE_FIELD))
        .and_then(Value::as_object_mut)
    else {
        return;
    };
    for field in SOURCE_CONTENT {
        reference.remove(field);
    }
    reference.insert("included_in_ancestry".into(), Value::Bool(true));
}

/// The wrapped image of a JSON record (`{"key": "image:…", "value":
/// {"nouli_image": …}}`), as saved inputs and manifests hold it.
pub(crate) fn image_value_of(record: &Value) -> Option<&Value> {
    wrapped(record, IMAGE_KEY_PREFIX, IMAGE_FIELD)
}

/// The wrapped saved-answer reference of a JSON record.
pub(crate) fn decision_reference_value_of(record: &Value) -> Option<&Value> {
    wrapped(record, DECISION_KEY_PREFIX, DECISION_REFERENCE_FIELD)
}

/// Whether a JSON record is keyed as a saved image.
pub(crate) fn is_image_record(record: &Value) -> bool {
    record
        .get("key")
        .and_then(Value::as_str)
        .is_some_and(|key| key.starts_with(IMAGE_KEY_PREFIX))
}

fn wrapped<'a>(record: &'a Value, prefix: &str, field: &str) -> Option<&'a Value> {
    record
        .get("key")
        .and_then(Value::as_str)
        .filter(|key| key.starts_with(prefix))?;
    record.get("value")?.get(field)
}

impl ContextRecord {
    /// The saved image this record holds, if it is an image record.
    ///
    /// # Errors
    ///
    /// Returns [`EvidenceError::Image`] if the record is keyed and wrapped as
    /// an image but its fields are malformed.
    pub fn as_image(&self) -> Result<Option<Image>, EvidenceError> {
        self.wrapped(IMAGE_KEY_PREFIX, IMAGE_FIELD)
            .map(|value| Image::deserialize(value).map_err(EvidenceError::Image))
            .transpose()
    }

    /// The saved-answer snapshot this record holds, if it is a reference.
    ///
    /// # Errors
    ///
    /// Returns [`EvidenceError::DecisionReference`] if the record is keyed
    /// and wrapped as a reference but its fields are malformed.
    pub fn as_decision_reference(&self) -> Result<Option<DecisionReference>, EvidenceError> {
        self.wrapped(DECISION_KEY_PREFIX, DECISION_REFERENCE_FIELD)
            .map(|value| {
                DecisionReference::deserialize(value).map_err(EvidenceError::DecisionReference)
            })
            .transpose()
    }

    fn wrapped(&self, prefix: &str, field: &str) -> Option<&Value> {
        if self.key.starts_with(prefix) {
            self.value.value().get(field)
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::history::FinalizedTurn;

    fn turn() -> FinalizedTurn {
        let fixture: Value =
            serde_json::from_str(include_str!("../../../fixtures/contracts/history.json")).unwrap();
        serde_json::from_value(fixture["turns"][0].clone()).unwrap()
    }

    fn id(text: &str) -> Id {
        Id::try_from(text.to_owned()).unwrap()
    }

    fn image() -> Image {
        Image {
            reference: "[image:0190f0a0-0000-7000-8000-00000000000a]".into(),
            name: "旧 \"photo\".png".into(),
            format: "png".into(),
            data_base64: "iVBORw0KGgo=".into(),
        }
    }

    /// Today's writer, verbatim: `storage::decisions::reference_record`
    /// before this module existed, with `Ledger::decision_reference`'s label.
    fn legacy_reference(
        turn: &FinalizedTurn,
        source: &DecisionSource,
        id: Id,
        image_id: Id,
        image: &Image,
        input: &Value,
    ) -> ContextRecord {
        let question = &turn.request.questions[0];
        let outcome = &turn.outcome.outcomes[0];
        let images = vec![serde_json::json!({"record":{
            "id":image_id,"key":format!("image:{}",image.reference),
            "value":{"nouli_image":image},"kind":"evidence"
        }})];
        let mut value = serde_json::json!({"nouli_decision_reference":{
            "source":source,"reference":format!("[decision:{id}]"),
            "question":question,"outcome":outcome,"provider":turn.outcome.provider,
            "images":images,"input_state":input
        }});
        value["nouli_decision_reference"]["label"] = serde_json::json!("@Where? · trip / 1");
        ContextRecord {
            id,
            key: format!("decision:{id}"),
            kind: ContextKind::Evidence,
            value: Data::new(value).unwrap(),
        }
    }

    fn fixed() -> (FinalizedTurn, DecisionSource, Id, Id, Value) {
        let turn = turn();
        let source = DecisionSource {
            session: turn.session,
            turn: turn.id,
            question: turn.request.questions[0].id.clone(),
        };
        let input = json!({"current_state":[{"type":"text","text":"Rail [late]"}],"budget":250,"ratio":0.25,"ok":null});
        (
            turn,
            source,
            id("0190f0a0-0000-7000-8000-000000000001"),
            id("0190f0a0-0000-7000-8000-000000000002"),
            input,
        )
    }

    fn typed(
        turn: &FinalizedTurn,
        source: &DecisionSource,
        id: Id,
        image_id: Id,
        input: &Value,
    ) -> ContextRecord {
        let mut reference = DecisionReference::snapshot(
            id,
            source.clone(),
            turn.request.questions[0].clone(),
            turn.outcome.outcomes[0].clone(),
            turn.outcome.provider.clone(),
            vec![ImageSnapshot::new(image_id, &image())],
            input.clone(),
        );
        reference.label = Some("@Where? · trip / 1".into());
        reference.record(id).unwrap()
    }

    #[test]
    fn decision_records_serialize_to_exactly_the_legacy_bytes() {
        let (turn, source, id, image_id, input) = fixed();
        let legacy = legacy_reference(&turn, &source, id, image_id, &image(), &input);
        let record = typed(&turn, &source, id, image_id, &input);
        assert_eq!(
            serde_json::to_string(&record).unwrap(),
            serde_json::to_string(&legacy).unwrap()
        );
        assert_eq!(record, legacy);
        // The persisted text of the fixed golden record, captured from the
        // legacy writer: a format change must fail here.
        let golden = include_str!("../../../fixtures/contracts/decision-reference-record.json");
        assert_eq!(serde_json::to_string(&record).unwrap(), golden.trim_end());
    }

    #[test]
    fn image_records_serialize_to_exactly_the_legacy_bytes() {
        let id = id("0190f0a0-0000-7000-8000-000000000003");
        let legacy = ContextRecord {
            id,
            key: format!("image:{id}"),
            kind: ContextKind::Evidence,
            value: Data::new(serde_json::json!({"nouli_image":image()})).unwrap(),
        };
        let record = image_record(id, &image());
        assert_eq!(
            serde_json::to_string(&record).unwrap(),
            serde_json::to_string(&legacy).unwrap()
        );
        assert_eq!(
            serde_json::to_string(&record).unwrap(),
            r#"{"id":"0190f0a0-0000-7000-8000-000000000003","key":"image:0190f0a0-0000-7000-8000-000000000003","value":{"nouli_image":{"reference":"[image:0190f0a0-0000-7000-8000-00000000000a]","name":"旧 \"photo\".png","format":"png","data_base64":"iVBORw0KGgo="}},"kind":"evidence"}"#
        );
        let parsed = record.as_image().unwrap().unwrap();
        assert_eq!(parsed.reference, image().reference);
        assert_eq!(parsed.data_base64, image().data_base64);
    }

    #[test]
    fn todays_records_parse_and_round_trip_unchanged() {
        let (turn, source, id, image_id, input) = fixed();
        let legacy = legacy_reference(&turn, &source, id, image_id, &image(), &input);
        let reference = legacy.as_decision_reference().unwrap().unwrap();
        assert_eq!(reference.source, source);
        assert_eq!(reference.reference, format!("[decision:{id}]"));
        assert_eq!(reference.label.as_deref(), Some("@Where? · trip / 1"));
        assert_eq!(
            reference.question.as_ref(),
            Some(&turn.request.questions[0])
        );
        assert_eq!(reference.input_state.as_ref(), Some(&input));
        assert_eq!(reference.record(id).unwrap(), legacy);
        // A record written before saved input state, and a binding reduced
        // by an ancestry, both survive a round trip.
        for value in [
            json!({"nouli_decision_reference":{"source":source,"reference":format!("[decision:{id}]"),
                "question":turn.request.questions[0],"outcome":turn.outcome.outcomes[0],
                "provider":turn.outcome.provider,"images":[],"input_references":[{"id":"x"}]}}),
            json!({"nouli_decision_reference":{"source":source,"reference":format!("[decision:{id}]"),
                "provider":turn.outcome.provider,"input_state":input,"label":"@x","included_in_ancestry":true}}),
            json!({"nouli_decision_reference":{"source":source,"reference":format!("[decision:{id}]")}}),
        ] {
            let record = ContextRecord {
                id,
                key: format!("decision:{id}"),
                kind: ContextKind::Evidence,
                value: Data::new(value).unwrap(),
            };
            let parsed = record.as_decision_reference().unwrap().unwrap();
            assert_eq!(parsed.record(id).unwrap(), record);
        }
    }

    #[test]
    fn binding_reduction_drops_source_content_but_keeps_frozen_input_state() {
        let (turn, source, id, image_id, input) = fixed();
        let record = typed(&turn, &source, id, image_id, &input);
        let mut legacy = record.value.value().clone();
        legacy[DECISION_REFERENCE_FIELD]["input_references"] = json!([]);
        let reduce = |value: Value| {
            let mut entry = json!({ "record": { "key": record.key, "value": value } });
            reduce_to_binding(&mut entry);
            entry["record"]["value"][DECISION_REFERENCE_FIELD].clone()
        };
        let binding = reduce(record.value.value().clone());
        // The in-place removal order that manifests have always persisted.
        assert_eq!(
            binding.as_object().unwrap().keys().collect::<Vec<_>>(),
            [
                "source",
                "reference",
                "label",
                "input_state",
                "provider",
                "included_in_ancestry"
            ]
        );
        assert_eq!(binding["input_state"], input);
        assert_eq!(reduce(legacy), binding);
        let mut other = json!({"record":{"key":"fact:x","value":"text"}});
        reduce_to_binding(&mut other);
        assert_eq!(other, json!({"record":{"key":"fact:x","value":"text"}}));
    }

    #[test]
    fn accessors_require_both_the_key_prefix_and_the_wrapper() {
        let (turn, source, id, image_id, input) = fixed();
        let mut record = typed(&turn, &source, id, image_id, &input);
        assert!(record.as_image().unwrap().is_none());
        record.key = "fact:other".into();
        assert!(record.as_decision_reference().unwrap().is_none());
        record.key = format!("decision:{id}");
        record.value =
            Data::new(json!({"nouli_decision_reference":{"reference":"[decision:x]"}})).unwrap();
        assert!(matches!(
            record.as_decision_reference(),
            Err(EvidenceError::DecisionReference(_))
        ));
        let mut image = image_record(image_id, &image());
        image.value = Data::new(json!({"nouli_image":{"name":"x"}})).unwrap();
        assert!(matches!(image.as_image(), Err(EvidenceError::Image(_))));
        image.value = Data::new(json!("caption")).unwrap();
        assert!(image.as_image().unwrap().is_none());
    }

    #[test]
    fn rebinding_keeps_the_value_and_marker_under_a_new_identity() {
        let (turn, source, id, image_id, input) = fixed();
        let record = typed(&turn, &source, id, image_id, &input);
        let other = Id::generate();
        let copy = rebind_decision_record(&record, other);
        assert_eq!(copy.id, other);
        assert_eq!(copy.key, format!("decision:{other}"));
        assert_eq!(copy.value, record.value);
        assert_eq!(
            copy.as_decision_reference().unwrap().unwrap().reference,
            format!("[decision:{id}]")
        );
    }
}
