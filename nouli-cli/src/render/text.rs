//! Human decision meaning, without terminal control sequences or model explanations.
use super::RenderError;
use nouli_core::{
    decision::{AnswerSpace, Criterion, Question},
    outcome::{Answer, Confidence, ProviderIdentity, QuestionOutcome},
    protocol::{ProjectedOutput, Report, ResultEnvelope},
    workflow::reference::{ReferenceValue, ResultField},
};
use std::io::Write;
pub(crate) fn clean(value: &str) -> String {
    value
        .chars()
        .flat_map(|character| {
            if character.is_control() {
                character.escape_default().collect::<Vec<_>>()
            } else {
                vec![character]
            }
        })
        .collect()
}
fn criterion(value: &Criterion) -> String {
    match value {
        Criterion::Text(text) => clean(text),
        Criterion::Detailed { label, description } => {
            format!("{} ({})", clean(label), clean(description))
        }
    }
}
fn scale(question: Option<&Question>) -> String {
    match question.map(|question| &question.answer_space) {
        Some(AnswerSpace::Score { levels }) => format!(
            "0..{} [{}]",
            levels.len().saturating_sub(1),
            levels
                .iter()
                .enumerate()
                .map(|(index, level)| format!("{index}: {}", criterion(level)))
                .collect::<Vec<_>>()
                .join("; ")
        ),
        _ => "scale unavailable in report".into(),
    }
}
fn choice(id: &str, question: Option<&Question>) -> String {
    let meaning = question.and_then(|question| match &question.answer_space {
        AnswerSpace::Choice { options } => options
            .iter()
            .find(|option| option.id == id)
            .map(|option| option.description.as_str()),
        _ => None,
    });
    match meaning {
        Some(meaning) => format!("{} ({})", clean(id), clean(meaning)),
        None => clean(id),
    }
}
fn confidence(
    sink: &mut impl Write,
    value: Option<&Confidence>,
    provider: Option<&ProviderIdentity>,
) -> Result<(), RenderError> {
    if let Some(value) = value {
        let provenance = provider
            .map(|provider| format!("{}/{}", clean(&provider.provider), clean(&provider.model)))
            .unwrap_or_else(|| "provider not recorded".into());
        write!(
            sink,
            "  Native confidence ({provenance}, {}): ",
            clean(&value.meaning)
        )?;
        match value.value {
            Some(value) => write!(sink, "value={value}")?,
            None => write!(sink, "value unavailable")?,
        };
        if let Some(probability) = value.answer_confidence {
            write!(sink, ", answer confidence={probability}")?;
        }
        writeln!(sink)?;
    }
    Ok(())
}
fn answer(
    sink: &mut impl Write,
    value: &Answer,
    question: Option<&Question>,
) -> Result<(), RenderError> {
    match value {
        Answer::Choice {
            selected,
            distribution,
        } => {
            let probability = distribution
                .iter()
                .find(|entry| entry.option_id == *selected)
                .map(|entry| entry.probability);
            write!(sink, "choice: {}", choice(selected, question))?;
            if let Some(probability) = probability {
                write!(sink, ", P(selected)={probability}")?;
            }
            writeln!(sink)?;
        }
        Answer::Noul { probability } => writeln!(sink, "noul: P(yes)={probability}")?,
        Answer::Score { value, .. } => {
            writeln!(sink, "expected score: {value}, scale {}", scale(question))?
        }
    }
    Ok(())
}
fn outcome(
    sink: &mut impl Write,
    value: &QuestionOutcome,
    question: Option<&Question>,
    provider: Option<&ProviderIdentity>,
) -> Result<(), RenderError> {
    match value {
        QuestionOutcome::Refused { reason, .. } => writeln!(sink, "refused: {}", clean(reason))?,
        QuestionOutcome::Answered {
            answer: value,
            confidence: certainty,
            ..
        } => {
            answer(sink, value, question)?;
            confidence(sink, certainty.as_ref(), provider)?;
        }
    }
    Ok(())
}
fn projection(sink: &mut impl Write, output: &ProjectedOutput) -> Result<(), RenderError> {
    write!(
        sink,
        "{}{}: ",
        clean(&output.name),
        if output.defaulted { " (default)" } else { "" }
    )?;
    let ReferenceValue::Present { value } = &output.value else {
        if let ReferenceValue::Absent { reason } = &output.value {
            writeln!(
                sink,
                "unavailable ({reason:?}){}",
                if output.required_missing {
                    ", required"
                } else {
                    ", optional"
                }
            )?;
        }
        return Ok(());
    };
    if output.defaulted {
        writeln!(sink, "{}", clean(&serde_json::to_string(value.value())?))?;
        return Ok(());
    }
    let question = output.question.as_ref();
    let reference = output.reference.as_ref();
    if reference.is_some_and(|reference| reference.pointer.as_ref().is_none_or(String::is_empty)) {
        match reference.map(|reference| reference.field) {
            Some(ResultField::Outcome) => {
                let value = serde_json::from_value::<QuestionOutcome>(value.value().clone())
                    .map_err(|_| RenderError::Protocol)?;
                return outcome(sink, &value, question, output.provider.as_ref());
            }
            Some(ResultField::Answer) => {
                let value = serde_json::from_value::<Answer>(value.value().clone())
                    .map_err(|_| RenderError::Protocol)?;
                answer(sink, &value, question)?;
                return confidence(sink, output.confidence.as_ref(), output.provider.as_ref());
            }
            Some(ResultField::Score) => {
                writeln!(
                    sink,
                    "expected score: {}, scale {}",
                    value.value(),
                    scale(question)
                )?;
                return confidence(sink, output.confidence.as_ref(), output.provider.as_ref());
            }
            Some(ResultField::Probability)
                if question
                    .is_some_and(|question| matches!(question.answer_space, AnswerSpace::Noul)) =>
            {
                writeln!(sink, "noul: P(yes)={}", value.value())?;
                return confidence(sink, output.confidence.as_ref(), output.provider.as_ref());
            }
            Some(ResultField::Selected | ResultField::Probability) => {
                if let Some(selected) = &output.selected_choice {
                    writeln!(
                        sink,
                        "choice: {}, P(selected)={}",
                        choice(&selected.id, question),
                        selected.probability
                    )?;
                    return confidence(sink, output.confidence.as_ref(), output.provider.as_ref());
                }
            }
            Some(ResultField::Confidence) => {
                writeln!(sink, "native confidence value={}", value.value())?;
                return confidence(sink, output.confidence.as_ref(), output.provider.as_ref());
            }
            _ => {}
        }
    }
    writeln!(sink, "{}", clean(&serde_json::to_string(value.value())?))?;
    Ok(())
}
pub(super) fn result(sink: &mut impl Write, envelope: &ResultEnvelope) -> Result<(), RenderError> {
    match &envelope.result {
        Report::Turn(report) => {
            if let Some(session) = report.session {
                writeln!(sink, "Session: {session}")?;
            }
            if let Some(turn) = report.turn {
                writeln!(
                    sink,
                    "Turn: {turn}{}",
                    if report.replayed { " (replayed)" } else { "" }
                )?;
            }
            writeln!(
                sink,
                "Provider: {}/{}",
                clean(&report.outcome.provider.provider),
                clean(&report.outcome.provider.model)
            )?;
            for value in &report.outcome.outcomes {
                let question = report
                    .questions
                    .iter()
                    .find(|question| question.id == value.question_id());
                write!(sink, "{}: ", clean(value.question_id()))?;
                outcome(sink, value, question, Some(&report.outcome.provider))?;
            }
        }
        Report::Run(report) => {
            match report.run {
                Some(run) => writeln!(
                    sink,
                    "Run: {run} ({:?}){}",
                    report.status,
                    if report.replayed { " (replayed)" } else { "" }
                )?,
                None => writeln!(sink, "Ephemeral run ({:?})", report.status)?,
            }
            if let Some(session) = report.session {
                writeln!(sink, "Session: {session}")?;
            }
            for node in &report.nodes {
                write!(sink, "{}: {:?}", clean(&node.node), node.status)?;
                if let Some(turn) = node.turn {
                    write!(sink, " (turn {turn})")?;
                }
                if let Some(provider) = &node.provider {
                    write!(
                        sink,
                        " [{}/{}]",
                        clean(&provider.provider),
                        clean(&provider.model)
                    )?;
                }

                writeln!(sink)?;
                if let Some(error) = &node.error {
                    writeln!(sink, "  {}: {}", clean(&error.code), clean(&error.message))?;
                }
            }
            for output in &report.outputs {
                projection(sink, output)?;
            }
        }
        Report::Session(report) => {
            if let Some(name) = &report.name {
                writeln!(sink, "Created session {}", clean(name.as_str()))?;
            }
            writeln!(sink, "Session: {}", report.session)?;
            for source in &report.sources {
                writeln!(
                    sink,
                    "From: {} @ {}",
                    source.session,
                    source
                        .frontier
                        .iter()
                        .map(ToString::to_string)
                        .collect::<Vec<_>>()
                        .join(", ")
                )?;
            }
        }
        Report::Error(_) => return Err(RenderError::Protocol),
    }
    sink.flush()?;
    Ok(())
}
