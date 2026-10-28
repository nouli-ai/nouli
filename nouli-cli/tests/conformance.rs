#[path = "support/harness.rs"]
pub mod harness;
#[path = "support/loopback.rs"]
pub mod loopback;
use harness::{Home, report};
use loopback::Server;
use nouli_core::{
    codec::{self, InputFormat},
    data::Input,
    decision::TurnRequest,
    history::{EvaluationRequest, ModelSelection, Representation, SessionSelection},
    identity::SessionSelector,
    outcome::{Answer, QuestionOutcome},
    protocol::{Report, ResultEnvelope},
    storage::{Ledger, OpenMode},
};
use serde_json::{Value, json};
const PROTOCOLS: [&str; 3] = ["typesafe", "openai-decisions", "openrouter-decisions"];
fn food() -> TurnRequest {
    codec::decode(
        include_bytes!("../../fixtures/journeys/food.json"),
        InputFormat::Json,
    )
    .unwrap()
}
fn wire(protocol: &str) -> Value {
    let text = match protocol {
        "typesafe" => include_str!("../../fixtures/journeys/typesafe-food-response.json"),
        "openai-decisions" => include_str!("../../fixtures/journeys/openai-food-response.json"),
        "openrouter-decisions" => {
            include_str!("../../fixtures/journeys/openrouter-food-response.json")
        }
        _ => unreachable!(),
    };
    let mut wire: Value = serde_json::from_str(text).unwrap();
    // A common probabilistic noul value, never a boolean or a thresholded result.
    let available = answer(&mut wire, "available");
    if protocol == "openai-decisions" {
        available["probability"] = json!(0.37);
    } else {
        available["noul"] = json!(0.37);
    }
    wire
}
fn answer<'a>(wire: &'a mut Value, id: &str) -> &'a mut Value {
    if wire["answers"].is_array() {
        wire["answers"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .find(|answer| answer["name"] == id)
            .unwrap()
    } else {
        &mut wire["answers"][id]
    }
}
fn invoke(protocol: &str, response: Value, status: u16) -> (std::process::Output, usize) {
    let server = Server::with_handler(move |_| (status, response.to_string()));
    let home = Home::create();
    home.configure(&server, protocol, false);
    let path = home.0.join("food.json");
    std::fs::write(&path, serde_json::to_vec(&food()).unwrap()).unwrap();
    let output = home.run(&["--input", path.to_str().unwrap(), "--no-session"]);
    (output, server.count())
}
#[test]
fn common_wire_corpus_preserves_probabilities_fractional_expectation_and_native_confidence() {
    for protocol in PROTOCOLS {
        for omit_confidence in [false, true] {
            let mut response = wire(protocol);
            if let Some(answers) = response["answers"].as_array_mut() {
                answers.reverse();
            }
            if omit_confidence {
                for id in ["meal", "quality"] {
                    let answer = answer(&mut response, id).as_object_mut().unwrap();
                    answer.remove("confidence");
                    answer.remove("answer_confidence");
                }
            }
            let actual_model = response["model"].as_str().unwrap().to_owned();
            let (output, calls) = invoke(protocol, response, 200);
            let result = report(output);
            assert_eq!(calls, 1);
            assert_eq!(result.outcome.provider.provider, "local");
            assert_eq!(result.outcome.provider.model, actual_model);
            assert_eq!(result.outcome.usage.input_tokens, Some(100));
            assert!(result.outcome.usage.exact);
            assert!(!result.manifest.input_count.exact);
            let outcomes = &result.outcome.outcomes;
            let QuestionOutcome::Answered {
                question_id,
                answer:
                    Answer::Choice {
                        selected,
                        distribution,
                    },
                confidence,
            } = &outcomes[0]
            else {
                panic!("choice")
            };
            assert_eq!(question_id, "meal");
            assert_eq!(selected, "lentils");
            assert_eq!(distribution[0].option_id, "lentils");
            assert_eq!(distribution[0].probability, 0.7);
            if omit_confidence {
                assert!(confidence.is_none());
            } else {
                let native = confidence.as_ref().unwrap();
                assert_eq!(
                    native.value,
                    Some(if protocol == "openai-decisions" {
                        0.8
                    } else {
                        0.35
                    })
                );
                assert!(native.meaning.contains(&actual_model));
                assert_eq!(
                    native.answer_confidence,
                    if protocol == "typesafe" {
                        Some(0.7)
                    } else {
                        None
                    }
                );
            }
            assert!(matches!(
                outcomes[1],
                QuestionOutcome::Answered {
                    answer: Answer::Noul { probability: 0.37 },
                    ..
                }
            ));
            assert!(
                matches!(&outcomes[2], QuestionOutcome::Answered { answer: Answer::Score { value: 1.43, probabilities }, .. } if probabilities == &[0.0, 0.57, 0.43])
            );
            if protocol == "openrouter-decisions" {
                assert_eq!(
                    result.outcome.provider.upstream_provider.as_deref(),
                    Some("OpenAI")
                );
                assert_eq!(
                    result.outcome.provider.request_id.as_deref(),
                    Some("fixture-router-request")
                );
                assert_eq!(result.outcome.usage.cost, Some(0.0000123));
            }
        }
    }
}
#[test]
fn malformed_coverage_boolean_noul_score_argmax_and_capacity_fail_without_success() {
    for protocol in PROTOCOLS {
        for case in [
            "coverage", "boolean", "argmax", "identity", "capacity", "refusal",
        ] {
            let mut response = wire(protocol);
            match case {
                "coverage" => {
                    if let Some(answers) = response["answers"].as_array_mut() {
                        answers.pop();
                    } else {
                        response["answers"]
                            .as_object_mut()
                            .unwrap()
                            .remove("quality");
                    }
                }
                "boolean" => {
                    let key = if protocol == "openai-decisions" {
                        "probability"
                    } else {
                        "noul"
                    };
                    answer(&mut response, "available")[key] = json!(true);
                }
                "argmax" => answer(&mut response, "quality")["score"] = json!(1),
                "identity" => answer(&mut response, "meal")["choice"] = json!("unknown"),
                "capacity" => response = json!({"error":{"code":"context_length_exceeded"}}),
                "refusal" => {
                    *answer(&mut response, "available") =
                        json!({"name":"available","type":"refusal"});
                }
                _ => unreachable!(),
            }
            let (output, calls) = invoke(
                protocol,
                response,
                if case == "capacity" { 413 } else { 200 },
            );
            assert_eq!(calls, 1);
            if case == "refusal" && protocol == "openai-decisions" {
                let result = report(output);
                assert!(
                    matches!(&result.outcome.outcomes[1], QuestionOutcome::Refused { question_id, .. } if question_id == "available")
                );
            } else {
                assert_eq!(output.status.code(), Some(3), "{protocol}/{case}");
                assert!(output.stderr.is_empty());
                let envelope: ResultEnvelope = serde_json::from_slice(&output.stdout).unwrap();
                assert!(matches!(envelope.result, Report::Error(_)));
            }
        }
    }
}
#[test]
fn every_adapter_receives_compact_selected_outputs_and_preserves_full_original_ledger() {
    for protocol in PROTOCOLS {
        let response = wire(protocol);
        let server = Server::with_handler(move |_| (200, response.to_string()));
        let home = Home::create();
        home.configure(&server, protocol, false);
        let mut turn = food();
        turn.state.push(Input::Text {
            text: "ORIGINAL_EVIDENCE_".repeat(1500),
        });
        let request = EvaluationRequest {
            turn: turn.clone(),
            session: SessionSelection::New { name: None },
            updates: vec![],
            required_records: vec![],
            model: ModelSelection::default(),
            request_id: Some("format-equivalence".into()),
        };
        let json = home.0.join("first.json");
        let toon = home.0.join("first.toon");
        std::fs::write(&json, serde_json::to_vec(&request).unwrap()).unwrap();
        std::fs::write(&toon, codec::encode_toon(&request).unwrap()).unwrap();
        let first = report(home.run(&["--input", json.to_str().unwrap()]));
        let replay = report(home.run(&["--input", toon.to_str().unwrap()]));
        assert!(replay.replayed);
        assert_eq!(replay.turn, first.turn);
        assert_eq!(server.count(), 1);
        let follow = home.0.join("follow.json");
        std::fs::write(&follow, serde_json::to_vec(&food()).unwrap()).unwrap();
        let second = report(home.run(&[
            "--input",
            follow.to_str().unwrap(),
            "--session",
            &first.session.unwrap().to_string(),
            "--context-budget",
            "1800",
        ]));
        assert!(
            second
                .manifest
                .records
                .iter()
                .any(|record| Some(record.record) == first.turn
                    && record.representation == Representation::Compacted)
        );
        let calls = server.calls.lock().unwrap();
        let body = &calls[1];
        let text = body
            .get("state")
            .or_else(|| body.get("input"))
            .unwrap()
            .as_str()
            .unwrap();
        assert!(!text.contains("ORIGINAL_EVIDENCE_"));
        let context = codec::decode_value(text.as_bytes(), InputFormat::Toon).unwrap();
        let compact = context["records"]
            .as_array()
            .unwrap()
            .iter()
            .find(|record| record["id"] == first.turn.unwrap().to_string())
            .unwrap();
        let selected = &compact["value"]["results"][0]["answer"];
        assert_eq!(selected["selected"], "lentils");
        assert_eq!(selected["description"], "Lentil stew");
        assert_eq!(selected["probability"], 0.7);
        assert!(selected.get("distribution").is_none());
        let score = &compact["value"]["results"][2]["answer"];
        assert_eq!(score["value"], 1.43);
        assert_eq!(score["scale"][2]["label"], "Excellent");
        drop(calls);
        let ledger = Ledger::open(&home.store(), OpenMode::ReadOnlyExisting).unwrap();
        let exported = ledger
            .export_session(&SessionSelector::Id(first.session.unwrap()))
            .unwrap();
        assert_eq!(exported.turns.len(), 2);
        assert_eq!(exported.turns[0].request, turn);
        let QuestionOutcome::Answered {
            answer: Answer::Choice { distribution, .. },
            ..
        } = &exported.turns[0].outcome.outcomes[0]
        else {
            panic!("original distribution")
        };
        assert_eq!(distribution.len(), 2);
    }
}
#[test]
fn strict_formats_preserve_numeric_boundaries_and_reject_malformed_toon() {
    let json = br#"{"low":-9223372036854775808,"high":18446744073709551615,"fraction":0.6,"null":null,"text":"true"}"#;
    let value = codec::decode_value(json, InputFormat::Json).unwrap();
    let toon = codec::encode_toon(&value).unwrap();
    assert_eq!(
        codec::decode_value(toon.as_bytes(), InputFormat::Toon).unwrap(),
        value
    );
    for invalid in ["x: 1\nx: 2", "x: NaN", "x: 18446744073709551616", "x[2]: 1"] {
        assert!(codec::decode_value(invalid.as_bytes(), InputFormat::Toon).is_err());
    }
}
