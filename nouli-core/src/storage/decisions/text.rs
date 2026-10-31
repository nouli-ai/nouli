//! Readable saved conversation content for local literal search and previews.
use super::DecisionError;
use crate::{
    codec,
    context::ContextRecord,
    data::Input,
    decision::{AnswerSpace, Criterion, Question},
    history::FinalizedTurn,
    media,
    outcome::{Answer, QuestionOutcome},
};
use serde_json::{Value, json};

fn readable(value: &Value, include_state: bool) -> Value {
    if let Some(image) = value
        .get("nouli_image")
        .filter(|image| image.is_object() && image["data_base64"].is_string())
    {
        return json!(format!(
            "Image: {}",
            image.get("name").and_then(Value::as_str).unwrap_or("image")
        ));
    }
    if let Some(reference) = value
        .get("nouli_decision_reference")
        .filter(|reference| reference["source"].is_object())
    {
        let mut shown = json!({"Referenced question":reference["question"]["prompt"],"Saved answer":reference["outcome"],"Label":reference["label"]});
        if include_state && let Some(state) = reference.get("input_state") {
            shown["Saved input state"] = readable(state, true);
        }
        return shown;
    }
    match value {
        Value::Object(map) => Value::Object(
            map.iter()
                .filter(|(key, _)| key.as_str() != "data_base64")
                .map(|(key, value)| (key.clone(), readable(value, include_state)))
                .collect(),
        ),
        Value::Array(values) => Value::Array(
            values
                .iter()
                .map(|value| readable(value, include_state))
                .collect(),
        ),
        _ => value.clone(),
    }
}

fn add(value: &Value, lines: &mut Vec<String>) {
    append(value, lines, false);
}
fn add_state(value: &Value, lines: &mut Vec<String>) {
    append(value, lines, true);
}
fn append(value: &Value, lines: &mut Vec<String>, include_state: bool) {
    let value = readable(value, include_state);
    if let Some(text) = value.as_str() {
        lines.extend(text.lines().map(str::to_owned));
    } else if let Ok(text) = serde_json::to_string_pretty(&value) {
        lines.extend(text.lines().map(str::to_owned));
    }
}

fn input(value: &Input, lines: &mut Vec<String>) {
    match value {
        Input::Text { text } => lines.extend(text.lines().map(str::to_owned)),
        Input::Structured { data } => add(data.value(), lines),
    }
}

fn record(record: &ContextRecord, lines: &mut Vec<String>) {
    add(record.value.value(), lines);
}
fn criterion(value: &Criterion, lines: &mut Vec<String>) {
    match value {
        Criterion::Text(text) => lines.push(text.clone()),
        Criterion::Detailed { label, description } => {
            lines.push(label.clone());
            lines.push(description.clone());
        }
    }
}

pub(super) fn conversation(
    turn: &FinalizedTurn,
    question: &Question,
    outcome: &QuestionOutcome,
) -> Vec<String> {
    let mut lines = Vec::new();
    for value in &turn.request.state {
        input(value, &mut lines);
    }
    if let Some(base) = &turn.context_base {
        for value in &base.state {
            input(value, &mut lines);
        }
        for update in &base.updates {
            if let Some(value) = update.new_record() {
                record(value, &mut lines);
            }
        }
    }
    for update in &turn.updates {
        if let Some(value) = update.new_record() {
            record(value, &mut lines);
        }
    }
    lines.push(question.prompt.clone());
    for value in &question.criteria {
        criterion(value, &mut lines);
    }
    match &question.answer_space {
        AnswerSpace::Noul => {}
        AnswerSpace::Choice { options } => {
            lines.extend(options.iter().map(|option| option.description.clone()))
        }
        AnswerSpace::Score { levels } => {
            for value in levels {
                criterion(value, &mut lines);
            }
        }
    }
    match outcome {
        QuestionOutcome::Refused { reason, .. } => lines.push(format!("Refused: {reason}")),
        QuestionOutcome::Answered { answer, .. } => match answer {
            Answer::Noul { probability } => lines.push(format!("P(yes)={probability:.2}")),
            Answer::Score { value, .. } => lines.push(format!("Score {value:.2}")),
            Answer::Choice { selected, .. } => {
                if let AnswerSpace::Choice { options } = &question.answer_space
                    && let Some(option) = options.iter().find(|option| option.id == *selected)
                {
                    lines.push(format!("Selected: {}", option.description));
                }
            }
        },
    }
    lines
        .into_iter()
        .map(|line| super::visible_text(&line))
        .filter(|line| !line.is_empty())
        .collect()
}

pub(super) fn state(turn: &FinalizedTurn) -> Result<Vec<String>, DecisionError> {
    let evidence = media::Evidence::read(&turn.manifest.context_toon)?;
    let value = codec::decode_value(evidence.text.as_bytes(), codec::InputFormat::Toon)?;
    let mut lines = Vec::new();
    for name in ["current_state", "workflow_base"] {
        if let Some(inputs) = value[name].as_array() {
            for value in inputs {
                if let Ok(value) = serde_json::from_value::<Input>(value.clone()) {
                    match value {
                        Input::Text { text } => lines.extend(text.lines().map(str::to_owned)),
                        Input::Structured { data } => add_state(data.value(), &mut lines),
                    }
                } else {
                    add_state(value, &mut lines);
                }
            }
        }
    }
    if let Some(records) = value["records"].as_array() {
        for entry in records {
            if let Some(record) = entry["value"].get("record") {
                if record["key"]
                    .as_str()
                    .is_some_and(|key| key.starts_with("image:"))
                {
                    continue;
                }
                if entry["value"]["active"].as_bool() == Some(false) {
                    lines.push("Historical context (inactive revision):".into());
                }
                add_state(&record["value"], &mut lines);
            } else if let Some(base) = entry["value"].get("workflow_base") {
                add_state(base, &mut lines);
            } else if entry["kind"].as_str() == Some("decision") {
                lines.push("Earlier decision in the saved input:".into());
                if entry["value"]["request"].is_object() {
                    add_state(
                        &json!({"Input":entry["value"]["request"]["state"],"Questions":entry["value"]["request"]["questions"],"Results":entry["value"]["outcome"]["outcomes"]}),
                        &mut lines,
                    );
                } else if entry["value"]["results"].is_array() {
                    add_state(&entry["value"]["results"], &mut lines);
                }
            }
        }
    }
    for image in evidence.images {
        lines.push(format!("Image: {}", image.name));
    }
    if lines.is_empty() {
        add_state(&value, &mut lines);
    }
    Ok(lines)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn readable_state_preserves_values_and_image_names_without_binary_payloads() {
        let value = json!({"budget":250,"accessible":true,"station":null,"photo":{"nouli_image":{"name":"metro.png","data_base64":"BASE64_SENTINEL"}}});
        let mut lines = Vec::new();
        add(&value, &mut lines);
        let text = lines.join("\n");
        assert!(
            text.contains("250")
                && text.contains("true")
                && text.contains("null")
                && text.contains("metro.png")
        );
        assert!(!text.contains("BASE64_SENTINEL"));
    }

    #[test]
    fn saved_input_preview_reads_historical_manifest_state_and_native_image_names() {
        let fixture: Value =
            serde_json::from_str(include_str!("../../../../fixtures/contracts/history.json"))
                .unwrap();
        let mut turn: FinalizedTurn = serde_json::from_value(fixture["turns"][0].clone()).unwrap();
        let image = crate::media::tests::picture(2, 2);
        turn.manifest.context_toon=codec::encode_toon(&json!({
            "current_state":[{"type":"structured","data":{"station":"Namma metro","budget":250,"accessible":true,"delay":null}}],
            "records":[
                {"value":{"active":true,"record":{"key":"fact:one","value":"Prefer rail [late] connections."}}},
                {"value":{"record":{"key":"image:one","value":{"nouli_image":image}}}}
            ]
        })).unwrap();
        let lines = state(&turn).unwrap().join("\n");
        for expected in [
            "Namma metro",
            "250",
            "true",
            "null",
            "rail [late]",
            "Image: photo.png",
        ] {
            assert!(lines.contains(expected), "{lines}");
        }
        assert!(!lines.contains(&image.data_base64));
        assert!(!lines.contains("data_base64"));
        let mut indexed = Vec::new();
        add(&json!({"nouli_image":image}), &mut indexed);
        assert!(indexed.join(" ").contains("photo.png"));
        assert!(!indexed.join(" ").contains(&image.data_base64));
        let mut ordinary = Vec::new();
        add(&json!({"nouli_image":"ordinary caption"}), &mut ordinary);
        assert!(ordinary.join(" ").contains("ordinary caption"));
    }
}
