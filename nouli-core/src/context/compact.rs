//! Staged deterministic reduction, preserving typed decision meaning.
use super::builder::{BuildError, ContextAction, ContextEntry, ContextPolicy, EntryKind};
use super::decision_entry::DecisionEntry;
use crate::{
    codec,
    data::Data,
    decision::AnswerSpace,
    history::Representation,
    identity::Id,
    outcome::{Answer, Confidence, ProviderIdentity, QuestionOutcome},
};
use serde::{Deserialize, Serialize};

#[derive(Debug, Default, Clone, Copy)]
pub struct Staged;
impl ContextPolicy for Staged {
    fn next(&self, entries: &[ContextEntry]) -> Result<Option<ContextAction>, BuildError> {
        for entry in entries.iter().filter(|entry| {
            !entry.protected
                && entry.kind == EntryKind::Decision
                && entry.representation == Representation::Full
        }) {
            let compact = compact(&entry.value)?;
            if codec::encode_toon(&compact)?.len() < codec::encode_toon(&entry.value)?.len() {
                return Ok(Some(ContextAction::Compact {
                    record: entry.id,
                    value: compact,
                }));
            }
        }
        Ok(entries
            .iter()
            .find(|entry| !entry.protected && entry.representation != Representation::Omitted)
            .map(|entry| ContextAction::Omit { record: entry.id }))
    }
}
#[derive(Serialize)]
struct CompactDecision<'a> {
    session: Option<Id>,
    turn: Option<Id>,
    #[serde(skip_serializing_if = "Option::is_none")]
    workflow_node: Option<&'a str>,
    provider: &'a ProviderIdentity,
    results: Vec<CompactResult<'a>>,
}
#[derive(Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
enum CompactResult<'a> {
    Answered {
        question_id: &'a str,
        prompt: &'a str,
        answer: CompactAnswer<'a>,
        confidence: &'a Option<Confidence>,
    },
    Refused {
        question_id: &'a str,
        prompt: &'a str,
        reason: &'a str,
    },
}
#[derive(Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum CompactAnswer<'a> {
    Choice {
        selected: &'a str,
        description: &'a str,
        probability: f64,
    },
    Noul {
        probability: f64,
    },
    Score {
        value: f64,
        scale: Vec<Scale<'a>>,
    },
}
#[derive(Serialize)]
struct Scale<'a> {
    index: usize,
    label: &'a str,
}

fn compact(value: &Data) -> Result<Data, BuildError> {
    let decision = DecisionEntry::deserialize(value.value()).map_err(|_| BuildError::Policy)?;
    decision
        .outcome
        .validate(&decision.request)
        .map_err(crate::history::HistoryError::from)?;
    let mut results = Vec::new();
    for question in &decision.request.questions {
        let outcome = decision
            .outcome
            .outcome_for(&question.id)
            .ok_or(BuildError::Policy)?;
        match outcome {
            QuestionOutcome::Refused {
                question_id,
                reason,
            } => results.push(CompactResult::Refused {
                question_id,
                prompt: &question.prompt,
                reason,
            }),
            QuestionOutcome::Answered {
                question_id,
                answer,
                confidence,
            } => {
                let answer = match (answer, &question.answer_space) {
                    (Answer::Choice { selected, .. }, AnswerSpace::Choice { options }) => {
                        CompactAnswer::Choice {
                            selected,
                            description: &options
                                .iter()
                                .find(|option| option.id == *selected)
                                .ok_or(BuildError::Policy)?
                                .description,
                            probability: answer.selected_probability().ok_or(BuildError::Policy)?,
                        }
                    }
                    (Answer::Noul { probability }, AnswerSpace::Noul) => CompactAnswer::Noul {
                        probability: *probability,
                    },
                    (Answer::Score { value, .. }, AnswerSpace::Score { levels }) => {
                        CompactAnswer::Score {
                            value: *value,
                            scale: levels
                                .iter()
                                .enumerate()
                                .map(|(index, level)| Scale {
                                    index,
                                    label: level.label(),
                                })
                                .collect(),
                        }
                    }
                    _ => return Err(BuildError::Policy),
                };
                results.push(CompactResult::Answered {
                    question_id,
                    prompt: &question.prompt,
                    answer,
                    confidence,
                });
            }
        }
    }
    let compact = CompactDecision {
        session: decision.session,
        turn: decision.turn,
        workflow_node: decision.workflow_node.as_deref(),
        provider: &decision.outcome.provider,
        results,
    };
    Ok(Data::new(codec::to_value(&compact)?)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::history::StandingChanges;
    use crate::{
        config::Protocol,
        context::builder::{ContextBuilder, ContextInput, RequiredItem},
        history::ModelSelection,
        history::{CountMethod, FinalizedTurn, SourcePoint, TokenCount},
        limits::{
            LimitError, ResolvedLimits,
            count::{InputCounts, TokenCounter},
        },
        storage::{Ledger, OpenMode, turns::AttemptInput},
    };
    fn fixture() -> FinalizedTurn {
        let data: serde_json::Value =
            serde_json::from_str(include_str!("../../../fixtures/contracts/history.json")).unwrap();
        serde_json::from_value(data["turns"][0].clone()).unwrap()
    }
    fn history() -> Vec<FinalizedTurn> {
        let root = fixture();
        let mut next = root.clone();
        next.id = Id::generate();
        next.parents = vec![root.id];
        next.updates.clear();
        vec![root, next]
    }
    fn limits(budget: u64) -> ResolvedLimits {
        ResolvedLimits {
            profile: crate::limits::count::tests::profile(Protocol::SystemOne, "fixture"),
            protocol: Protocol::SystemOne,
            model: "offline".into(),
            budget,
            per_question: None,
            reserved_output_tokens: 0,
            max_questions: None,
            max_choices: None,
            max_score_levels: None,
        }
    }
    struct Weights;
    impl TokenCounter for Weights {
        fn count(
            &self,
            _profile: &crate::config::ProviderProfile,
            context: &str,
            questions: &[crate::decision::Question],
        ) -> Result<InputCounts, LimitError> {
            let context: serde_json::Value =
                codec::decode(context.as_bytes(), codec::InputFormat::Toon)?;
            let mut tokens = 100;
            for entry in context["records"].as_array().unwrap() {
                tokens += if entry["kind"] == "context" {
                    10
                } else if entry["representation"] == "compacted" {
                    50
                } else {
                    100
                };
            }
            let count = TokenCount {
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
    fn build(
        history: &[FinalizedTurn],
        budget: u64,
        required: &[RequiredItem],
    ) -> Result<super::super::builder::ContextPlan, BuildError> {
        let head = history.last().unwrap();
        let sources = [SourcePoint {
            session: head.session,
            frontier: vec![head.id],
        }];
        ContextBuilder::build(
            ContextInput {
                context_base: None,
                runtime_only: false,
                history,
                sources: &sources,
                request: &head.request,
                updates: &[],
                required,
                workflow_base: &[],
                session: Id::generate(),
                turn: Id::generate(),
                provider: head.outcome.provider.clone(),
            },
            &limits(budget),
            &Weights,
            &Staged,
        )
    }
    #[test]
    fn shrinking_precedes_eviction_and_oldest_order_is_stable() {
        let history = history();
        let before = codec::encode_toon(&history).unwrap();
        let one = build(&history, 260, &[]).unwrap();
        assert_eq!(one.entries[0].representation, Representation::Compacted);
        assert_eq!(one.entries[2].representation, Representation::Full);
        assert!(
            one.entries
                .iter()
                .all(|entry| entry.representation != Representation::Omitted)
        );
        let both = build(&history, 210, &[]).unwrap();
        assert_eq!(both.entries[2].representation, Representation::Compacted);
        let evicted = build(&history, 180, &[]).unwrap();
        assert_eq!(evicted.entries[0].representation, Representation::Omitted);
        assert_eq!(evicted.entries[2].representation, Representation::Compacted);
        assert!(evicted.entries[1].protected);
        assert_eq!(evicted.entries[1].representation, Representation::Full);
        let rendered: serde_json::Value = codec::decode(
            evicted.manifest.context_toon.as_bytes(),
            codec::InputFormat::Toon,
        )
        .unwrap();
        assert!(
            rendered["records"]
                .as_array()
                .unwrap()
                .iter()
                .all(|entry| entry["id"] != history[0].id.to_string())
        );
        assert_eq!(codec::encode_toon(&history).unwrap(), before);
        let required = [RequiredItem {
            record: history[0].id,
            pointer: Some("/outcome/outcomes/0/answer/selected".into()),
        }];
        let protected = build(&history, 260, &required).unwrap();
        assert_eq!(protected.entries[0].representation, Representation::Full);
        assert!(protected.entries[0].protected);
        assert_eq!(
            protected.entries[2].representation,
            Representation::Compacted
        );
        assert!(matches!(
            build(&history, 109, &[]),
            Err(BuildError::ProtectedCapacity)
        ));
    }
    #[test]
    fn standalone_evidence_is_evicted_whole_and_retrieval_order_does_not_change_age() {
        let mut history = history();
        let original = history[0].updates[0].new_record().unwrap().clone();
        history[0].updates = vec![crate::context::ContextUpdate::Append {
            record: original.clone(),
        }];
        let plan = build(&history, 105, &[]).unwrap();
        assert_eq!(
            plan.entries
                .iter()
                .find(|entry| entry.id == original.id)
                .unwrap()
                .representation,
            Representation::Omitted
        );
        let normal = build(&history, 180, &[]).unwrap();
        let head = history.last().unwrap().clone();
        history.reverse();
        let sources = [SourcePoint {
            session: head.session,
            frontier: vec![head.id],
        }];
        let reversed = ContextBuilder::build(
            ContextInput {
                context_base: None,
                runtime_only: false,
                history: &history,
                sources: &sources,
                request: &head.request,
                updates: &[],
                required: &[],
                workflow_base: &[],
                session: Id::generate(),
                turn: Id::generate(),
                provider: head.outcome.provider,
            },
            &limits(180),
            &Weights,
            &Staged,
        )
        .unwrap();
        assert_eq!(normal, reversed);
    }

    #[test]
    fn compact_answers_preserve_choice_meaning_noul_endpoints_and_fractional_score_scale() {
        let turn = fixture();
        let value=Data::new(codec::to_value(&serde_json::json!({"session":turn.session,"turn":turn.id,"request":turn.request,"outcome":turn.outcome})).unwrap()).unwrap();
        let result = compact(&value).unwrap();
        let result = result.value();
        assert_eq!(result["results"][0]["answer"]["selected"], "train");
        assert_eq!(result["results"][0]["answer"]["description"], "Rail");
        assert_eq!(result["results"][0]["answer"]["probability"], 0.7);
        assert_eq!(result["results"][1]["answer"]["probability"], 1.0);
        assert!(result["results"][1]["confidence"].is_null());
        assert_eq!(result["results"][2]["answer"]["value"], 1.43);
        assert_eq!(
            result["results"][2]["answer"]["scale"][1],
            serde_json::json!({"index":1,"label":"Adequate"})
        );
        assert!(result["results"][2]["confidence"].is_null());
        assert_eq!(result["results"][3]["status"], "refused");
        assert!(result.get("state").is_none());
        assert!(result["results"][0]["answer"].get("distribution").is_none());
        assert!(
            result["results"][2]["answer"]
                .get("probabilities")
                .is_none()
        );
        let mut zero = value.value().clone();
        zero["outcome"]["outcomes"][1]["answer"]["probability"] = serde_json::json!(0.0);
        assert_eq!(
            compact(&Data::new(zero).unwrap()).unwrap().value()["results"][1]["answer"]["probability"],
            0.0
        );
    }
    #[test]
    fn context_reduction_leaves_durable_originals_and_distributions_unchanged() {
        let root = std::env::temp_dir().join(format!("nouli-compaction-{}", Id::generate()));
        let mut ledger = Ledger::open(&root, OpenMode::ReadWriteCreate).unwrap();
        let session = ledger
            .create_session(None, ModelSelection::default())
            .unwrap();
        let guard = ledger.session_guard(session.id).unwrap();
        let turn = fixture();
        let attempt = ledger
            .prepare_attempt(
                &guard,
                AttemptInput {
                    standing: StandingChanges::default(),
                    request: turn.request,
                    updates: turn.updates,
                    manifest: turn.manifest,
                    request_id: None,
                },
            )
            .unwrap();
        let saved = ledger
            .finalize_attempt(&guard, attempt.id, turn.outcome)
            .unwrap();
        drop(guard);
        drop(ledger);
        let path = root.join("ledger.sqlite3");
        let bytes = std::fs::read(&path).unwrap();
        let ledger = Ledger::open(&root, OpenMode::ReadOnlyExisting).unwrap();
        let sources = ledger.context_sources(session.id).unwrap();
        let history = ledger.ancestry(&sources).unwrap();
        let plan = build(&history, 160, &[]).unwrap();
        assert_eq!(plan.entries[0].representation, Representation::Compacted);
        assert_eq!(ledger.turn(saved.id).unwrap(), saved);
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
        drop(ledger);
        std::fs::remove_dir_all(root).unwrap();
    }
}
