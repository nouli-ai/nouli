//! Request-kind detection and v1 normalization above the serialization boundary.
use super::{CONTRACT_VERSION, TurnRequest, grammar};
use crate::{
    codec::{CodecError, to_value},
    workflow::WorkflowSpec,
};
use serde::Serialize;
use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(untagged)]
pub enum Request {
    Turn(TurnRequest),
    Workflow(WorkflowSpec),
}

/// Normalize omitted v1 fields and validate a previously decoded request value.
///
/// # Errors
///
/// - [`CodecError::Kind`] if the value is not an object or does not contain exactly one of
///   `questions` and `nodes`.
/// - [`CodecError::Question`] if a workflow node's compact-grammar question does not parse.
/// - [`CodecError::Shape`] if the value does not match the turn or workflow contract.
/// - [`CodecError::Decision`] or [`CodecError::Workflow`] if the request fails semantic validation.
pub fn request_from_value(mut value: Value) -> Result<Request, CodecError> {
    let object = value.as_object_mut().ok_or(CodecError::Kind)?;
    let turn = object.contains_key("questions");
    let workflow = object.contains_key("nodes");
    if turn == workflow {
        return Err(CodecError::Kind);
    }
    object
        .entry("version")
        .or_insert(Value::from(CONTRACT_VERSION));
    if turn {
        let request: TurnRequest = serde_json::from_value(value).map_err(|_| CodecError::Shape)?;
        request.validate()?;
        Ok(Request::Turn(request))
    } else {
        // Embedded turns use the same omitted-version default, and may write a
        // question as a compact-grammar string.
        if let Some(nodes) = object.get_mut("nodes").and_then(Value::as_array_mut) {
            for node in nodes {
                let id = node
                    .get("id")
                    .and_then(Value::as_str)
                    .unwrap_or("?")
                    .to_owned();
                if let Some(turn) = node.get_mut("turn").and_then(Value::as_object_mut) {
                    turn.entry("version")
                        .or_insert(Value::from(CONTRACT_VERSION));
                    expand_questions(&id, turn)?;
                }
            }
        }
        let request: WorkflowSpec = serde_json::from_value(value).map_err(|_| CodecError::Shape)?;
        request.validate()?;
        Ok(Request::Workflow(request))
    }
}

/// Fills in the v1 `version` of the `turn` object in an evaluation request
/// value, so a request that wraps a turn may omit it as a bare turn does.
///
/// # Errors
///
/// Returns [`CodecError::Shape`] if the value has no `turn` object.
pub fn default_turn_version(value: &mut Value) -> Result<(), CodecError> {
    let turn = value
        .get_mut("turn")
        .and_then(Value::as_object_mut)
        .ok_or(CodecError::Shape)?;
    turn.entry("version")
        .or_insert(Value::from(CONTRACT_VERSION));
    Ok(())
}

/// Replaces each compact-grammar string in a turn's `questions` with the full
/// question it parses to; objects pass through for the contract to check.
fn expand_questions(
    node: &str,
    turn: &mut serde_json::Map<String, Value>,
) -> Result<(), CodecError> {
    let Some(questions) = turn.get_mut("questions").and_then(Value::as_array_mut) else {
        return Ok(());
    };
    for (index, question) in questions.iter_mut().enumerate() {
        if let Value::String(line) = question {
            let parsed = grammar::parse(line).map_err(|source| CodecError::Question {
                node: node.to_owned(),
                index,
                source,
            })?;
            *question = to_value(&parsed)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codec::{InputFormat, decode_request};
    #[test]
    fn a_wrapped_turn_defaults_its_version() {
        let mut value = serde_json::json!({"turn": {"questions": []}});
        default_turn_version(&mut value).unwrap();
        assert_eq!(value["turn"]["version"], CONTRACT_VERSION);
        let mut explicit = serde_json::json!({"turn": {"version": 1, "questions": []}});
        default_turn_version(&mut explicit).unwrap();
        assert_eq!(explicit["turn"]["version"], 1);
        assert_eq!(
            default_turn_version(&mut serde_json::json!({"turn": 3})),
            Err(CodecError::Shape)
        );
    }

    #[test]
    fn workflow_questions_may_be_compact_strings_or_objects() {
        let full = include_bytes!("../../../fixtures/contracts/workflow.json");
        let expected = decode_request(full, InputFormat::Json).unwrap();
        let mut value: Value = serde_json::from_slice(full).unwrap();
        // Write the first node's questions in the grammar instead.
        let questions = value["nodes"][0]["turn"]["questions"].clone();
        let lines: Vec<Value> = questions
            .as_array()
            .unwrap()
            .iter()
            .map(|question| {
                let parsed: crate::decision::Question =
                    serde_json::from_value(question.clone()).unwrap();
                Value::String(grammar::format(&parsed))
            })
            .collect();
        value["nodes"][0]["turn"]["questions"] = Value::Array(lines);
        let Request::Workflow(mixed) = request_from_value(value.clone()).unwrap() else {
            panic!("workflow")
        };
        // Formatting and parsing keep a question's meaning, so the compact node
        // matches the full one wherever the grammar can write the question.
        let Request::Workflow(expected) = expected else {
            panic!("workflow")
        };
        assert_eq!(mixed.nodes[1], expected.nodes[1]);
        assert_eq!(
            mixed.nodes[0].turn.questions.len(),
            expected.nodes[0].turn.questions.len()
        );
        // A bad string names its node and position, and not its text.
        value["nodes"][1]["id"] = Value::String("second".into());
        value["nodes"][1]["turn"]["questions"][1] = Value::String("SECRET no id".into());
        let error = request_from_value(value).unwrap_err();
        let CodecError::Question { node, index, .. } = &error else {
            panic!("{error:?}")
        };
        assert_eq!((node.as_str(), *index), ("second", 1));
        assert!(!error.to_string().contains("SECRET"));
    }
}
