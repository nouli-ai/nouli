//! Store-wide discovery and fixed, question-specific saved-answer evidence.
use std::collections::HashSet;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use super::{Ledger, StorageError, sql_error, turns::TurnError};
use crate::{
    codec,
    context::{ContextKind, ContextRecord},
    data::{Data, DataError},
    decision::Question,
    history::FinalizedTurn,
    identity::Id,
    media,
    outcome::{ProviderIdentity, QuestionOutcome},
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DecisionSource {
    pub session: Id,
    pub turn: Id,
    pub question: String,
}

/// A lightweight catalog entry: no input images or entire session histories.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SavedDecision {
    pub source: DecisionSource,
    pub session_name: String,
    pub number: i64,
    pub sequence: i64,
    pub question: Question,
    pub outcome: QuestionOutcome,
    pub provider: ProviderIdentity,
}

impl SavedDecision {
    pub fn answered(&self) -> bool {
        matches!(self.outcome, QuestionOutcome::Answered { .. })
    }

    pub fn matches(&self, query: &str) -> bool {
        let searchable = format!(
            "{} {} {} {}",
            self.question.prompt, self.source.question, self.session_name, self.source.turn
        )
        .to_lowercase();
        query
            .split_whitespace()
            .all(|word| searchable.contains(&word.to_lowercase()))
    }
}

#[derive(Debug, Error)]
pub enum DecisionError {
    #[error(transparent)]
    Storage(#[from] StorageError),
    #[error(transparent)]
    Turn(#[from] TurnError),
    #[error(transparent)]
    Codec(#[from] codec::CodecError),
    #[error(transparent)]
    Data(#[from] DataError),
    #[error("the selected saved question or answer is unavailable")]
    Missing,
    #[error("the selected question was refused; choose a saved answer")]
    Refused,
}

fn short_label(prompt: &str) -> String {
    let mut prompt = prompt.to_owned();
    for prefix in ["[image:", "[decision:"] {
        let mut offset = 0;
        while let Some(at) = prompt[offset..].find(prefix) {
            let start = offset + at;
            let Some(end) = prompt[start + prefix.len()..].find(']') else {
                break;
            };
            let end = start + prefix.len() + end;
            if Id::try_from(prompt[start + prefix.len()..end].to_owned()).is_ok() {
                let from = if start > 0 && prompt.as_bytes()[start - 1] == b' ' {
                    start - 1
                } else {
                    start
                };
                prompt.replace_range(from..end + 1, "");
                offset = from;
            } else {
                offset = end + 1;
            }
        }
    }
    let prompt = prompt
        .chars()
        .filter(|c| !c.is_control() || c.is_whitespace())
        .collect::<String>();
    let prompt = prompt.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut label = prompt.chars().take(44).collect::<String>();
    if prompt.chars().count() > 44 {
        label.push('…');
    }
    label
}

impl Ledger {
    /// Return newly published turns across the whole selected local store.
    /// The caller keeps lightweight entries and known IDs, avoiding decoding
    /// historical image payloads on each keystroke or subsequent picker open.
    /// Checking identities, rather than a UUID watermark, includes late workflow
    /// publications whose IDs precede another already-published turn.
    pub fn saved_decisions(
        &self,
        known: &HashSet<Id>,
    ) -> Result<(Vec<Id>, Vec<SavedDecision>), DecisionError> {
        let mut statement = self.connection.prepare(
            "SELECT t.id, COALESCE(s.name,s.id), t.ordinal, t.rowid FROM turns t JOIN sessions s ON s.id=t.session_id ORDER BY t.rowid DESC"
        ).map_err(|error| sql_error("list saved decisions", error))?;
        let rows = statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, i64>(3)?,
                ))
            })
            .map_err(|error| sql_error("read decision catalog", error))?;
        let mut ids = Vec::new();
        let mut entries = Vec::new();
        for row in rows {
            let (id, session_name, ordinal, sequence) =
                row.map_err(|error| sql_error("read decision entry", error))?;
            let id = Id::try_from(id).map_err(|_| StorageError::Payload)?;
            if known.contains(&id) {
                continue;
            }
            let turn = self.turn(id)?;
            ids.push(id);
            for question in &turn.request.questions {
                let outcome = turn
                    .outcome
                    .outcomes
                    .iter()
                    .find(|outcome| outcome.question_id() == question.id)
                    .ok_or(StorageError::Payload)?;
                entries.push(SavedDecision {
                    source: DecisionSource {
                        session: turn.session,
                        turn: id,
                        question: question.id.clone(),
                    },
                    session_name: session_name.clone(),
                    number: ordinal + 1,
                    sequence,
                    question: question.clone(),
                    outcome: outcome.clone(),
                    provider: turn.outcome.provider.clone(),
                });
            }
        }
        Ok((ids, entries))
    }

    /// Snapshot one exact answer as ordinary durable evidence, without blending
    /// source-session ancestry, changing its active facts, or re-evaluating it.
    /// Native image inputs are copied from the immutable saved input manifest.
    pub fn decision_reference(
        &self,
        source: &DecisionSource,
        id: Id,
    ) -> Result<ContextRecord, DecisionError> {
        let turn = self.turn(source.turn)?;
        let mut record = reference_record(&turn, source, id)?;
        let (name,ordinal):(String,i64)=self.connection.query_row(
            "SELECT COALESCE(s.name,s.id),t.ordinal FROM turns t JOIN sessions s ON s.id=t.session_id WHERE t.id=?1",
            [source.turn.to_string()],|row|Ok((row.get(0)?,row.get(1)?)))
            .map_err(|e|sql_error("read reference source label",e))?;
        let prompt = &turn
            .request
            .questions
            .iter()
            .find(|q| q.id == source.question)
            .ok_or(DecisionError::Missing)?
            .prompt;
        let shortened = short_label(prompt);
        let mut value = record.value.value().clone();
        value["nouli_decision_reference"]["label"] =
            serde_json::json!(format!("@{shortened} · {name} / {}", ordinal + 1));
        record.value = Data::new(value)?;
        Ok(record)
    }
}

fn reference_record(
    turn: &FinalizedTurn,
    source: &DecisionSource,
    id: Id,
) -> Result<ContextRecord, DecisionError> {
    if turn.runtime_only || turn.session != source.session || turn.id != source.turn {
        return Err(DecisionError::Missing);
    }
    let question = turn
        .request
        .questions
        .iter()
        .find(|q| q.id == source.question)
        .ok_or(DecisionError::Missing)?;
    let outcome = turn
        .outcome
        .outcomes
        .iter()
        .find(|o| o.question_id() == source.question)
        .ok_or(DecisionError::Missing)?;
    if !matches!(outcome, QuestionOutcome::Answered { .. }) {
        return Err(DecisionError::Refused);
    }
    // The manifest is the actual saved input, including images from earlier
    // turns and other references, even when their original files are gone.
    let evidence = media::Evidence::read(&turn.manifest.context_toon)?;
    let input = codec::decode_value(evidence.text.as_bytes(), codec::InputFormat::Toon)?;
    let records = input.get("records").and_then(serde_json::Value::as_array);
    let references = records
        .into_iter()
        .flatten()
        .filter(|entry| entry["value"]["record"]["value"]["nouli_decision_reference"].is_object())
        .cloned()
        .collect::<Vec<_>>();
    let referenced_turns = references
        .iter()
        .filter_map(|entry| {
            entry["value"]["record"]["value"]["nouli_decision_reference"]["source"]["turn"].as_str()
        })
        .collect::<HashSet<_>>();
    let inputs = references
        .iter()
        .chain(records.into_iter().flatten().filter(|entry| {
            entry["id"]
                .as_str()
                .is_some_and(|id| referenced_turns.contains(id))
        }))
        .cloned()
        .collect::<Vec<_>>();
    let images = evidence.images;
    let images = images
        .into_iter()
        .map(|image| {
            serde_json::json!({"record":{
                "id":Id::generate(),"key":format!("image:{}",image.reference),
                "value":{"nouli_image":image},"kind":"evidence"
            }})
        })
        .collect::<Vec<_>>();
    Ok(ContextRecord {
        id,
        key: format!("decision:{id}"),
        kind: ContextKind::Evidence,
        value: Data::new(serde_json::json!({"nouli_decision_reference":{
            "source":source,"reference":format!("[decision:{id}]"),
            "question":question,"outcome":outcome,"provider":turn.outcome.provider,
            "images":images,"input_references":inputs
        }}))?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn turn() -> FinalizedTurn {
        let fixture: serde_json::Value =
            serde_json::from_str(include_str!("../../../fixtures/contracts/history.json")).unwrap();
        serde_json::from_value(fixture["turns"][0].clone()).unwrap()
    }

    #[test]
    fn labels_hide_complete_media_and_decision_markers_before_shortening() {
        let image = Id::generate();
        let decision = Id::generate();
        for tag in [format!("[image:{image}]"), format!("[decision:{decision}]")] {
            let label = short_label(&format!(
                "photo.png {tag} This is a long question about the photo and prior answer"
            ));
            assert!(label.starts_with("photo.png This"));
            assert!(!label.contains('['));
            assert!(!label.contains(&image.to_string()));
            assert!(label.ends_with('…'));
        }
    }

    #[test]
    fn global_catalog_keeps_independent_sessions_versions_and_late_publications() {
        use crate::{
            history::ModelSelection,
            storage::{OpenMode, turns::AttemptInput},
        };
        let path = std::env::temp_dir().join(format!("nouli-decision-catalog-{}", Id::generate()));
        let mut ledger = Ledger::open(&path, OpenMode::ReadWriteCreate).unwrap();
        let mut saved = Vec::new();
        for name in ["first-project", "other-project"] {
            let session = ledger
                .create_session(
                    Some(name.to_owned().try_into().unwrap()),
                    ModelSelection::default(),
                )
                .unwrap();
            let guard = ledger.session_guard(session.id).unwrap();
            for _ in 0..2 {
                let template = turn();
                let attempt = ledger
                    .prepare_attempt(
                        &guard,
                        AttemptInput {
                            request: template.request,
                            updates: vec![],
                            standing: Default::default(),
                            manifest: template.manifest,
                            request_id: None,
                        },
                    )
                    .unwrap();
                saved.push(
                    ledger
                        .finalize_attempt(&guard, attempt.id, template.outcome)
                        .unwrap(),
                );
            }
        }
        let (known, catalog) = ledger.saved_decisions(&HashSet::new()).unwrap();
        assert_eq!(known.len(), 4);
        assert!(catalog.iter().any(|e| e.session_name == "first-project"));
        assert!(catalog.iter().any(|e| e.session_name == "other-project"));
        let q = &saved[0].request.questions[0].id;
        assert_eq!(
            catalog.iter().filter(|e| e.source.question == *q).count(),
            4
        );
        assert!(
            catalog
                .iter()
                .filter(|e| e.source.question == *q)
                .all(|e| e.matches(q))
        );
        let mut seen = known.iter().copied().collect::<HashSet<_>>();
        assert!(ledger.saved_decisions(&seen).unwrap().1.is_empty());
        // A missing older ID must be found too; a greatest-ID watermark would
        // skip delayed workflow publications and imported historical turns.
        seen.remove(&saved[0].id);
        assert_eq!(ledger.saved_decisions(&seen).unwrap().0, vec![saved[0].id]);
        let entry = catalog
            .iter()
            .find(|e| e.source.turn == saved[0].id && e.answered())
            .unwrap();
        let record = ledger
            .decision_reference(&entry.source, Id::generate())
            .unwrap();
        assert!(
            record.value.value()["nouli_decision_reference"]["label"]
                .as_str()
                .unwrap()
                .contains("first-project")
        );
        assert_eq!(ledger.turn(saved[0].id).unwrap(), saved[0]);
        drop(ledger);
        let reopened = Ledger::open(&path, OpenMode::ReadOnlyExisting).unwrap();
        assert_eq!(
            reopened.saved_decisions(&HashSet::new()).unwrap().1.len(),
            catalog.len()
        );
        drop(reopened);
        std::fs::remove_dir_all(path).unwrap();
    }

    #[test]
    fn a_reference_keeps_exact_question_answer_and_provenance_without_mutating_source() {
        let turn = turn();
        let before = turn.clone();
        let source = DecisionSource {
            session: turn.session,
            turn: turn.id,
            question: turn.request.questions[0].id.clone(),
        };
        let record = reference_record(&turn, &source, Id::generate()).unwrap();
        let value = &record.value.value()["nouli_decision_reference"];
        assert_eq!(
            value["question"],
            serde_json::to_value(&turn.request.questions[0]).unwrap()
        );
        assert_eq!(
            value["outcome"],
            serde_json::to_value(&turn.outcome.outcomes[0]).unwrap()
        );
        assert_eq!(value["source"]["turn"], turn.id.to_string());
        assert_eq!(turn, before);
        let mut invalid = source.clone();
        invalid.session = Id::generate();
        assert!(matches!(
            reference_record(&turn, &invalid, Id::generate()),
            Err(DecisionError::Missing)
        ));
        let mut refused = turn.clone();
        refused.outcome.outcomes[0] = QuestionOutcome::Refused {
            question_id: source.question.clone(),
            reason: "No".into(),
        };
        assert!(matches!(
            reference_record(&refused, &source, Id::generate()),
            Err(DecisionError::Refused)
        ));
    }

    #[test]
    fn snapshot_preserves_native_image_bytes_from_saved_input_without_source_files() {
        use base64::{Engine, prelude::BASE64_STANDARD};
        let mut turn = turn();
        let mut png = std::io::Cursor::new(Vec::new());
        image::RgbImage::new(2, 2)
            .write_to(&mut png, image::ImageFormat::Png)
            .unwrap();
        let original = png.into_inner();
        let reference = format!("[image:{}]", Id::generate());
        let image = media::Image {
            reference: reference.clone(),
            name: "旧 image.png".into(),
            format: "png".into(),
            data_base64: BASE64_STANDARD.encode(&original),
        };
        turn.manifest.context_toon=codec::encode_toon(&serde_json::json!({"records":[{"value":{"record":{
            "id":Id::generate(),"key":"image:source","kind":"evidence","value":{"nouli_image":image}
        }}}]})).unwrap();
        let source = DecisionSource {
            session: turn.session,
            turn: turn.id,
            question: turn.request.questions[0].id.clone(),
        };
        let record = reference_record(&turn, &source, Id::generate()).unwrap();
        let native = media::Evidence::read(
            &codec::encode_toon(&serde_json::json!({"records":[{"value":{"record":record}}]}))
                .unwrap(),
        )
        .unwrap();
        assert_eq!(native.images.len(), 1);
        assert_eq!(native.images[0].reference, reference);
        assert_eq!(
            BASE64_STANDARD
                .decode(&native.images[0].data_base64)
                .unwrap(),
            original
        );
        assert!(!native.text.contains(&BASE64_STANDARD.encode(&original)));
    }
}
