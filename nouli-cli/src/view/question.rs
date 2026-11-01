//! Human answer options and selected results inside one decision card.
use super::card::{self, Row};
use crate::render::RenderError;
use nouli_core::{
    decision::{AnswerSpace, Criterion, Question},
    outcome::{Answer, QuestionOutcome},
};
use std::collections::HashMap;

/// Destination for one question's summary, shared by the card and Mermaid views.
pub(super) trait Lines {
    fn mark(&self) -> usize {
        0
    }
    fn accent(&mut self, _: usize, _: crate::palette::Tone) {}
    fn text(&mut self, value: &str);
    fn selected(&mut self, value: &str);
    /// Options in caller order with exact probabilities; `true` marks the selection.
    fn table(&mut self, heading: &str, entries: &[(String, Option<f64>, bool)]);
}

impl Lines for Vec<Row> {
    fn mark(&self) -> usize {
        self.len()
    }
    fn accent(&mut self, start: usize, tone: crate::palette::Tone) {
        let mut prompt = true;
        for row in &mut self[start..] {
            if row.text.is_empty() {
                prompt = false;
            }
            if row.highlighted || prompt {
                row.accent = Some(tone);
            }
        }
    }
    fn text(&mut self, value: &str) {
        card::text(self, "", value);
    }

    fn selected(&mut self, value: &str) {
        card::selected(self, value);
    }

    fn table(&mut self, heading: &str, entries: &[(String, Option<f64>, bool)]) {
        card::table(self, heading, entries);
    }
}

fn levels(levels: &[Criterion]) -> Vec<String> {
    levels
        .iter()
        .enumerate()
        .map(|(index, level)| match level {
            Criterion::Text(text) => format!("{index} {text}"),
            Criterion::Detailed { label, description } => {
                format!("{index} {label}: {description}")
            }
        })
        .collect()
}

pub(super) fn append(
    lines: &mut impl Lines,
    question: &Question,
    outcome: &QuestionOutcome,
) -> Result<(), RenderError> {
    let start = lines.mark();
    lines.text(&question.prompt);
    lines.text("");
    match outcome {
        QuestionOutcome::Refused { reason, .. } => {
            let (heading, labels) = match &question.answer_space {
                AnswerSpace::Choice { options } => (
                    "Choices",
                    options
                        .iter()
                        .map(|option| option.description.clone())
                        .collect(),
                ),
                AnswerSpace::Score { levels: rubric } => ("Levels", levels(rubric)),
                AnswerSpace::Noul => ("", vec![]),
            };
            if !labels.is_empty() {
                lines.table(
                    heading,
                    &labels
                        .into_iter()
                        .map(|label| (label, None, false))
                        .collect::<Vec<_>>(),
                );
                lines.text("");
            }
            lines.text(&format!("Refused: {reason}"));
        }
        QuestionOutcome::Answered { answer, .. } => match (&question.answer_space, answer) {
            (
                AnswerSpace::Choice { options },
                Answer::Choice {
                    selected,
                    distribution,
                },
            ) => {
                if !options.iter().any(|option| option.id == *selected) {
                    return Err(RenderError::Protocol);
                }
                let probabilities: HashMap<_, _> = distribution
                    .iter()
                    .map(|entry| (entry.option_id.as_str(), entry.probability))
                    .collect();
                let entries = options
                    .iter()
                    .map(|option| {
                        Ok((
                            option.description.clone(),
                            Some(
                                *probabilities
                                    .get(option.id.as_str())
                                    .ok_or(RenderError::Protocol)?,
                            ),
                            option.id == *selected,
                        ))
                    })
                    .collect::<Result<Vec<_>, RenderError>>()?;
                lines.table("Choices", &entries);
            }
            (AnswerSpace::Noul, Answer::Noul { probability }) => {
                lines.selected(&format!("P(yes)={probability}"));
            }
            (
                AnswerSpace::Score { levels: rubric },
                Answer::Score {
                    value,
                    probabilities,
                },
            ) => {
                if rubric.len() != probabilities.len() {
                    return Err(RenderError::Protocol);
                }
                let entries = levels(rubric)
                    .into_iter()
                    .zip(probabilities)
                    .map(|(level, probability)| (level, Some(*probability), false))
                    .collect::<Vec<_>>();
                lines.table("Levels", &entries);
                lines.text("");
                lines.selected(&format!("Expected score: {value}"));
            }
            _ => return Err(RenderError::Protocol),
        },
    }
    lines.accent(start, crate::palette::Tone::from(&question.answer_space));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(index: usize) -> (Question, QuestionOutcome) {
        let fixture: serde_json::Value =
            serde_json::from_str(include_str!("../../../fixtures/contracts/history.json")).unwrap();
        let turn = &fixture["turns"][0];
        (
            serde_json::from_value(turn["request"]["questions"][index].clone()).unwrap(),
            serde_json::from_value(turn["outcome"]["outcomes"][index].clone()).unwrap(),
        )
    }

    fn rows(question: &Question, outcome: &QuestionOutcome) -> Vec<Row> {
        let mut rows = vec![];
        append(&mut rows, question, outcome).unwrap();
        rows
    }

    #[test]
    fn each_question_keeps_its_type_accent_in_a_mixed_card() {
        let mut rows = vec![];
        for index in 0..3 {
            let (question, outcome) = fixture(index);
            let start = rows.len();
            append(&mut rows, &question, &outcome).unwrap();
            let tone = crate::palette::Tone::from(&question.answer_space);
            assert_eq!(rows[start].accent, Some(tone));
            for row in rows[start..].iter().filter(|row| row.highlighted) {
                assert_eq!(row.accent, Some(tone));
            }
        }
    }

    fn text(rows: &[Row]) -> String {
        rows.iter()
            .map(|row| row.text.as_str())
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn choice_shows_all_options_in_caller_order_with_matching_probabilities() {
        let (question, mut outcome) = fixture(0);
        let QuestionOutcome::Answered {
            answer: Answer::Choice { distribution, .. },
            ..
        } = &mut outcome
        else {
            panic!("choice")
        };
        distribution.reverse();
        let rows = rows(&question, &outcome);
        let output = text(&rows);
        assert!(output.contains("Choices") && output.contains("Probability"));
        assert!(output.find("Rail").unwrap() < output.find("Air").unwrap());
        assert!(
            rows.iter().any(|row| row.text.contains("Rail")
                && row.text.contains("0.7")
                && row.highlighted)
        );
        assert!(
            rows.iter().any(|row| row.text.contains("Air")
                && row.text.contains("0.3")
                && !row.highlighted)
        );
        assert_eq!(output.matches('▶').count(), 1);
    }

    #[test]
    fn choice_highlights_the_explicit_selection_even_when_probabilities_tie() {
        let (question, mut outcome) = fixture(0);
        let QuestionOutcome::Answered {
            answer:
                Answer::Choice {
                    selected,
                    distribution,
                },
            ..
        } = &mut outcome
        else {
            panic!("choice")
        };
        *selected = "flight".into();
        for entry in distribution {
            entry.probability = 0.5;
        }
        let rows = rows(&question, &outcome);
        let selected: Vec<_> = rows.iter().filter(|row| row.highlighted).collect();
        assert_eq!(selected.len(), 1);
        assert!(selected[0].text.contains("▶ Air"));
    }

    #[test]
    fn noul_keeps_the_returned_probability_without_a_binary_selection() {
        let (question, mut outcome) = fixture(1);
        let QuestionOutcome::Answered {
            answer: Answer::Noul { probability },
            ..
        } = &mut outcome
        else {
            panic!("noul")
        };
        *probability = 0.03;
        let rows = rows(&question, &outcome);
        let selected: Vec<_> = rows.iter().filter(|row| row.highlighted).collect();
        assert_eq!(selected.len(), 1);
        assert_eq!(selected[0].text, "▶ P(yes)=0.03");
        assert!(!text(&rows).contains("Choices"));
    }

    #[test]
    fn score_lists_the_full_rubric_and_highlights_only_the_expected_value() {
        let (question, outcome) = fixture(2);
        let rows = rows(&question, &outcome);
        let output = text(&rows);
        for token in [
            "0 Poor",
            "1 Adequate",
            "Meets",
            "minimum",
            "requirements",
            "2 Excellent",
            "0.57",
            "0.43",
        ] {
            assert!(output.contains(token), "{output}");
        }
        let selected: Vec<_> = rows.iter().filter(|row| row.highlighted).collect();
        assert_eq!(selected.len(), 1);
        assert_eq!(selected[0].text, "▶ Expected score: 1.43");
    }

    #[test]
    fn refusal_keeps_the_options_without_inventing_probabilities_or_a_selection() {
        let (question, _) = fixture(0);
        let outcome = QuestionOutcome::Refused {
            question_id: question.id.clone(),
            reason: "Insufficient evidence".into(),
        };
        let rows = rows(&question, &outcome);
        let output = text(&rows);
        assert!(output.contains("Rail") && output.contains("Air"));
        assert_eq!(output.matches("n/a").count(), 2);
        assert!(output.contains("Refused: Insufficient evidence"));
        assert!(!rows.iter().any(|row| row.highlighted));
    }

    #[test]
    fn incomplete_distributions_are_errors() {
        let (question, mut outcome) = fixture(0);
        let QuestionOutcome::Answered {
            answer: Answer::Choice { distribution, .. },
            ..
        } = &mut outcome
        else {
            panic!("choice")
        };
        distribution.pop();
        assert!(matches!(
            append(&mut Vec::<Row>::new(), &question, &outcome),
            Err(RenderError::Protocol)
        ));
    }
}
