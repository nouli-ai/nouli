//! One turn's results as probability bars and changes, for the verbs and the REPL.
//!
//! The layout is fixed: a 2-column indent, an 11-column question ID, a 32-column
//! label field, a 20-cell bar, and the probability to two decimals. Color only
//! styles a row; turning it off never changes the layout.

use super::{RenderError, clean};
use nouli_core::{
    decision::{AnswerSpace, ChoiceOption, Criterion, Question},
    outcome::{Answer, OptionProbability, QuestionOutcome},
};
use std::io::Write;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

/// Bold teal, the product accent, for selected rows, the prompt, and changes.
pub(crate) const TEAL: &str = "\x1b[1;36;38;5;37m";
/// Dim, for headers, metadata, and unselected rows.
pub(crate) const DIM: &str = "\x1b[2m";
pub(crate) const RESET: &str = "\x1b[0m";

const MAX_COLUMNS: usize = 100;
const ID_COLUMNS: usize = 11;
const LABEL_COLUMNS: usize = 32;
const BAR_CELLS: usize = 20;
const EIGHTHS: [char; 8] = [' ', '▏', '▎', '▍', '▌', '▋', '▊', '▉'];
const SPARK: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];
/// A moved answer is worth a marker from this much.
const CHANGE: f64 = 0.05;

/// What a turn did, for the line above its results.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Header {
    /// The saved turn's number, or `None` for a turn that was not saved.
    pub turn: Option<usize>,
    pub facts: usize,
    /// How many of the facts replaced an earlier one.
    pub replaced: usize,
    pub questions: usize,
}

impl Header {
    pub(crate) fn text(&self) -> String {
        let plural = |count: usize, noun: &str| {
            format!("{count} {noun}{}", if count == 1 { "" } else { "s" })
        };
        let mut parts = vec![match self.turn {
            Some(turn) => format!("turn {turn}"),
            None => "(not saved)".into(),
        }];
        let mut facts = plural(self.facts, "fact");
        // What was replaced is a fact about the saved history, which a turn
        // that was not saved does not touch.
        if self.turn.is_some() && self.replaced > 0 {
            facts.push_str(&format!(" ({} replaced, kept in history)", self.replaced));
        }
        parts.push(facts);
        parts.push(plural(self.questions, "question"));
        parts.join(" · ")
    }
}

/// Wraps text in an escape code only when color is on.
#[derive(Clone, Copy)]
pub(crate) struct Paint(pub bool);

impl Paint {
    pub(crate) fn apply(self, code: &str, text: &str) -> String {
        if self.0 && !text.is_empty() {
            format!("{code}{text}{RESET}")
        } else {
            text.to_owned()
        }
    }
}

/// Truncates to `columns` display columns, ending in `…` when cut.
fn fit(text: &str, columns: usize) -> String {
    if text.width() <= columns {
        return text.to_owned();
    }
    let mut result = String::new();
    let mut used = 0;
    for character in text.chars() {
        let width = character.width().unwrap_or(0);
        if used + width >= columns {
            break;
        }
        used += width;
        result.push(character);
    }
    result.push('…');
    result
}

fn pad(text: &str, columns: usize) -> String {
    let text = fit(text, columns);
    let spaces = columns.saturating_sub(text.width());
    format!("{text}{}", " ".repeat(spaces))
}

fn bar(probability: f64) -> String {
    let eighths = (probability.clamp(0.0, 1.0) * (BAR_CELLS * 8) as f64).round() as usize;
    let (full, remainder) = (eighths / 8, eighths % 8);
    let mut bar = "█".repeat(full);
    if full < BAR_CELLS {
        bar.push(EIGHTHS[remainder]);
    }
    pad(&bar, BAR_CELLS)
}

fn percent(probability: Option<f64>) -> String {
    probability.map_or_else(|| "n/a".into(), |value| format!("{value:.2}"))
}

fn moved(before: f64, now: f64) -> bool {
    (before - now).abs() >= CHANGE - 1e-9
}

fn level_label(level: &Criterion) -> &str {
    match level {
        Criterion::Text(text) => text,
        Criterion::Detailed { label, .. } => label,
    }
}

fn option_label(options: &[ChoiceOption], id: &str) -> String {
    options
        .iter()
        .find(|option| option.id == id)
        .map_or(id, |option| option.description.as_str())
        .to_owned()
}

fn chance(distribution: &[OptionProbability], id: &str) -> Option<f64> {
    distribution
        .iter()
        .find(|entry| entry.option_id == id)
        .map(|entry| entry.probability)
}

/// The change marker, shortened so the whole line stays within the column limit.
fn marker(paint: Paint, used: usize, label: Option<&str>, probability: f64) -> String {
    let tail = format!(" {probability:.2}");
    let label = label.map(|label| {
        let spent = used + "   → was ".width() + tail.width();
        fit(&clean(label), MAX_COLUMNS.saturating_sub(spent).max(1))
    });
    let text = match label {
        Some(label) => format!("   → was {label}{tail}"),
        None => format!("   → was{tail}"),
    };
    paint.apply(TEAL, &text)
}

fn choice_rows(
    paint: Paint,
    question: &Question,
    options: &[ChoiceOption],
    answer: (&str, &[OptionProbability]),
    previous: Option<&QuestionOutcome>,
    rows: &mut Vec<String>,
) {
    let (selected, distribution) = answer;
    let name = pad(&clean(&question.id), ID_COLUMNS);
    for (position, option) in options.iter().enumerate() {
        let chosen = option.id == selected;
        let lead = if position == 0 {
            format!("  {name}")
        } else {
            format!("  {}", " ".repeat(ID_COLUMNS))
        };
        let body = format!(
            "{} {} {}",
            pad(&clean(&option.description), LABEL_COLUMNS),
            bar(chance(distribution, &option.id).unwrap_or(0.0)),
            percent(chance(distribution, &option.id)),
        );
        let mut row = if chosen {
            format!("{lead}{}", paint.apply(TEAL, &format!(" ▶ {body}")))
        } else {
            format!("{lead}   {}", paint.apply(DIM, &body))
        };
        if chosen
            && let Some(QuestionOutcome::Answered {
                answer:
                    Answer::Choice {
                        selected: before,
                        distribution: earlier,
                    },
                ..
            }) = previous
            && let Some(was) = chance(earlier, before)
            && options.iter().any(|option| &option.id == before)
            && let Some(now) = chance(distribution, selected)
            && (before != selected || moved(was, now))
        {
            let used = 2 + ID_COLUMNS + 3 + LABEL_COLUMNS + 1 + BAR_CELLS + 1 + 4;
            row.push_str(&marker(
                paint,
                used,
                Some(&option_label(options, before)),
                was,
            ));
        }
        rows.push(row);
    }
}

fn question_rows(
    paint: Paint,
    question: &Question,
    outcome: Option<&QuestionOutcome>,
    previous: Option<&QuestionOutcome>,
    rows: &mut Vec<String>,
) {
    let name = pad(&clean(&question.id), ID_COLUMNS);
    let lead = format!("  {name}   ");
    let used = lead.width();
    let (answer, ..) = match outcome {
        Some(QuestionOutcome::Answered { answer, .. }) => (answer, ()),
        Some(QuestionOutcome::Refused { reason, .. }) => {
            let text = fit(&clean(reason), MAX_COLUMNS.saturating_sub(used + 9).max(1));
            rows.push(format!(
                "{lead}{}",
                paint.apply(DIM, &format!("refused: {text}"))
            ));
            return;
        }
        None => {
            rows.push(format!("{lead}{}", paint.apply(DIM, "no result")));
            return;
        }
    };
    let earlier = match previous {
        Some(QuestionOutcome::Answered { answer, .. }) => Some(answer),
        _ => None,
    };
    match (&question.answer_space, answer) {
        (
            AnswerSpace::Choice { options },
            Answer::Choice {
                selected,
                distribution,
            },
        ) => choice_rows(
            paint,
            question,
            options,
            (selected, distribution),
            previous,
            rows,
        ),
        (AnswerSpace::Noul, Answer::Noul { probability }) => {
            // "yes" fills the label field, so the bar lines up with a choice's.
            let body = format!(
                "yes{}{} {}",
                " ".repeat(LABEL_COLUMNS - 2),
                bar(*probability),
                percent(Some(*probability))
            );
            let mut row = format!("{lead}{}", paint.apply(TEAL, &body));
            if let Some(Answer::Noul { probability: was }) = earlier
                && moved(*was, *probability)
            {
                row.push_str(&marker(paint, used + body.width(), None, *was));
            }
            rows.push(row);
        }
        (
            AnswerSpace::Score { levels },
            Answer::Score {
                value,
                probabilities,
            },
        ) => {
            let sparkline: String = probabilities
                .iter()
                .map(|probability| {
                    SPARK[((probability.clamp(0.0, 1.0) * 7.0).round() as usize).min(7)]
                })
                .collect();
            let body = format!(
                "expected {value:.2} of {}   {sparkline}",
                levels.len().saturating_sub(1)
            );
            let mut row = format!("{lead}{}", paint.apply(TEAL, &body));
            if let Some(Answer::Score {
                value: was,
                probabilities: before,
            }) = earlier
                && before.len() == probabilities.len()
                && moved(*was, *value)
            {
                row.push_str(&marker(paint, used + body.width(), None, *was));
            }
            rows.push(row);
            let detail = levels
                .iter()
                .zip(probabilities)
                .map(|(level, probability)| {
                    format!("{} {probability:.2}", clean(level_label(level)))
                })
                .collect::<Vec<_>>()
                .join(" · ");
            rows.push(paint.apply(
                DIM,
                &format!(
                    "  {}{}",
                    " ".repeat(ID_COLUMNS + 3),
                    fit(&detail, MAX_COLUMNS.saturating_sub(ID_COLUMNS + 5))
                ),
            ));
        }
        _ => rows.push(format!(
            "{lead}{}",
            paint.apply(DIM, "answer does not match the question")
        )),
    }
}

/// Writes a turn's header and one block of rows per question. `previous` gives
/// the last answer to a question ID, when there is one.
pub(crate) fn write<'a>(
    sink: &mut impl Write,
    header: &Header,
    questions: &[Question],
    outcomes: &[QuestionOutcome],
    previous: impl Fn(&str) -> Option<&'a QuestionOutcome>,
    color: bool,
) -> Result<(), RenderError> {
    let paint = Paint(color);
    writeln!(
        sink,
        "{}",
        paint.apply(DIM, &format!("  {}", header.text()))
    )?;
    let mut rows = Vec::new();
    for question in questions {
        let outcome = outcomes
            .iter()
            .find(|outcome| outcome.question_id() == question.id);
        question_rows(paint, question, outcome, previous(&question.id), &mut rows);
    }
    for row in rows {
        writeln!(sink, "{row}")?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use nouli_core::decision::Question;

    fn choice(id: &str, options: &[(&str, &str)]) -> Question {
        Question {
            id: id.into(),
            prompt: "Which?".into(),
            criteria: vec![],
            answer_space: AnswerSpace::Choice {
                options: options
                    .iter()
                    .map(|(id, description)| ChoiceOption {
                        id: (*id).into(),
                        description: (*description).into(),
                    })
                    .collect(),
            },
        }
    }
    fn noul(id: &str) -> Question {
        Question {
            id: id.into(),
            prompt: "Will it?".into(),
            criteria: vec![],
            answer_space: AnswerSpace::Noul,
        }
    }
    fn score(id: &str, levels: &[&str]) -> Question {
        Question {
            id: id.into(),
            prompt: "How good?".into(),
            criteria: vec![],
            answer_space: AnswerSpace::Score {
                levels: levels
                    .iter()
                    .map(|l| Criterion::Text((*l).into()))
                    .collect(),
            },
        }
    }
    fn picked(id: &str, selected: &str, distribution: &[(&str, f64)]) -> QuestionOutcome {
        QuestionOutcome::Answered {
            question_id: id.into(),
            answer: Answer::Choice {
                selected: selected.into(),
                distribution: distribution
                    .iter()
                    .map(|(option_id, probability)| OptionProbability {
                        option_id: (*option_id).into(),
                        probability: *probability,
                    })
                    .collect(),
            },
            confidence: None,
        }
    }
    fn chance_of(id: &str, probability: f64) -> QuestionOutcome {
        QuestionOutcome::Answered {
            question_id: id.into(),
            answer: Answer::Noul { probability },
            confidence: None,
        }
    }
    fn scored(id: &str, value: f64, probabilities: &[f64]) -> QuestionOutcome {
        QuestionOutcome::Answered {
            question_id: id.into(),
            answer: Answer::Score {
                value,
                probabilities: probabilities.to_vec(),
            },
            confidence: None,
        }
    }
    fn header(turn: Option<usize>) -> Header {
        Header {
            turn,
            facts: 1,
            replaced: 0,
            questions: 2,
        }
    }
    fn render(
        header: &Header,
        questions: &[Question],
        outcomes: &[QuestionOutcome],
        previous: &[QuestionOutcome],
        color: bool,
    ) -> String {
        let mut sink = Vec::new();
        write(
            &mut sink,
            header,
            questions,
            outcomes,
            |id| previous.iter().find(|outcome| outcome.question_id() == id),
            color,
        )
        .unwrap();
        String::from_utf8(sink).unwrap()
    }
    fn plain(text: &str) -> String {
        let mut result = String::new();
        let mut escape = false;
        for character in text.chars() {
            match (escape, character) {
                (false, '\x1b') => escape = true,
                (true, 'm') => escape = false,
                (false, _) => result.push(character),
                _ => {}
            }
        }
        result
    }

    #[test]
    fn headers_match_the_prototype() {
        assert_eq!(
            Header {
                turn: Some(3),
                facts: 1,
                replaced: 1,
                questions: 2,
            }
            .text(),
            "turn 3 · 1 fact (1 replaced, kept in history) · 2 questions"
        );
        assert_eq!(header(None).text(), "(not saved) · 1 fact · 2 questions");
        // An unsaved turn leaves out what only a saved one has.
        assert_eq!(
            Header {
                turn: None,
                facts: 3,
                replaced: 1,
                questions: 3,
            }
            .text(),
            "(not saved) · 3 facts · 3 questions"
        );
        assert_eq!(
            Header {
                turn: Some(1),
                facts: 2,
                questions: 1,
                ..Header::default()
            }
            .text(),
            "turn 1 · 2 facts · 1 question"
        );
    }

    #[test]
    fn bars_use_eighth_blocks_and_always_fill_twenty_cells() {
        assert_eq!(bar(0.0), " ".repeat(20));
        assert_eq!(bar(1.0), "█".repeat(20));
        assert_eq!(bar(0.5), format!("{}{}", "█".repeat(10), " ".repeat(10)));
        assert_eq!(bar(0.00625), format!("▏{}", " ".repeat(19)));
        // 0.99 * 160 = 158.4 eighths: 19 full cells and 6 eighths.
        assert_eq!(bar(0.99), format!("{}▊", "█".repeat(19)));
        // Rounding up to a whole cell never indexes past the eighths.
        assert_eq!(bar(0.99999), "█".repeat(20));
        assert_eq!(bar(7.0), "█".repeat(20));
    }

    fn session() -> (Vec<Question>, Vec<QuestionOutcome>, Vec<QuestionOutcome>) {
        (
            vec![
                choice("transport", &[("train", "train"), ("taxi", "taxi")]),
                noul("on_time"),
                score("fit", &["poor", "fair", "good", "excellent"]),
            ],
            vec![
                picked("transport", "taxi", &[("train", 0.01), ("taxi", 0.99)]),
                chance_of("on_time", 0.13),
                scored("fit", 2.02, &[0.0, 0.24, 0.5, 0.26]),
            ],
            vec![
                picked("transport", "train", &[("train", 1.0), ("taxi", 0.0)]),
                chance_of("on_time", 0.98),
                scored("fit", 2.02, &[0.0, 0.24, 0.5, 0.26]),
            ],
        )
    }

    #[test]
    fn a_saved_turn_shows_each_type_with_changes_since_the_last_answers() {
        let (questions, now, was) = session();
        assert_eq!(
            render(&header(Some(3)), &questions, &now, &was, false),
            "  turn 3 · 1 fact · 2 questions\n\
             \x20 transport     train                            ▎                    0.01\n\
             \x20             ▶ taxi                             ███████████████████▊ 0.99   → was train 1.00\n\
             \x20 on_time       yes                              ██▋                  0.13   → was 0.98\n\
             \x20 fit           expected 2.02 of 3   ▁▃▅▃\n\
             \x20               poor 0.00 · fair 0.24 · good 0.50 · excellent 0.26\n"
        );
    }

    #[test]
    fn an_unchanged_answer_has_no_marker_and_a_small_move_is_not_a_change() {
        let questions = [
            choice("transport", &[("train", "train"), ("taxi", "taxi")]),
            noul("on_time"),
        ];
        let now = [
            picked("transport", "train", &[("train", 0.9), ("taxi", 0.1)]),
            chance_of("on_time", 0.50),
        ];
        let was = [
            picked("transport", "train", &[("train", 0.86), ("taxi", 0.14)]),
            chance_of("on_time", 0.46),
        ];
        let text = render(&header(Some(2)), &questions, &now, &was, false);
        assert!(!text.contains("was"), "{text}");
        // The same probability movement at exactly 0.05 is a change.
        let was = [
            picked("transport", "train", &[("train", 0.85), ("taxi", 0.15)]),
            chance_of("on_time", 0.45),
        ];
        let text = render(&header(Some(2)), &questions, &now, &was, false);
        assert!(text.contains("→ was train 0.85"), "{text}");
        assert!(text.contains("→ was 0.45"), "{text}");
    }

    #[test]
    fn a_different_answer_space_or_missing_previous_answer_is_not_compared() {
        let questions = [choice("pick", &[("a", "a"), ("b", "b")]), noul("sure")];
        let now = [
            picked("pick", "a", &[("a", 0.7), ("b", 0.3)]),
            chance_of("sure", 0.9),
        ];
        // The earlier answers to these IDs had other types, or an option that
        // no longer exists.
        let was = [
            chance_of("pick", 0.1),
            picked("sure", "gone", &[("gone", 1.0)]),
        ];
        assert!(!render(&header(Some(2)), &questions, &now, &was, false).contains("was"));
        let was = [picked("pick", "gone", &[("gone", 1.0)])];
        assert!(!render(&header(Some(2)), &questions, &now, &was, false).contains("was"));
        assert!(!render(&header(Some(1)), &questions, &now, &[], false).contains("was"));
    }

    #[test]
    fn an_unsaved_turn_is_marked_and_a_refusal_shows_its_reason() {
        let questions = [
            noul("private"),
            choice("transport", &[("train", "train"), ("taxi", "taxi")]),
        ];
        let outcomes = [
            QuestionOutcome::Refused {
                question_id: "private".into(),
                reason: "Unavailable\u{1b}[31m information".into(),
            },
            picked("transport", "train", &[("train", 1.0)]),
        ];
        assert_eq!(
            render(&header(None), &questions, &outcomes, &[], false),
            "  (not saved) · 1 fact · 2 questions\n\
             \x20 private       refused: Unavailable\\u{1b}[31m information\n\
             \x20 transport   ▶ train                            ████████████████████ 1.00\n\
             \x20               taxi                                                  n/a\n"
        );
    }

    #[test]
    fn long_labels_ids_and_reasons_keep_every_line_within_the_column_limit() {
        let long = "an extremely long option label that cannot possibly fit in its column";
        let questions = [
            choice(
                "a_very_long_question_identifier",
                &[("x", long), ("y", "short")],
            ),
            noul("n"),
        ];
        let now = [
            picked(
                "a_very_long_question_identifier",
                "x",
                &[("x", 0.6), ("y", 0.4)],
            ),
            QuestionOutcome::Refused {
                question_id: "n".into(),
                reason: "r".repeat(300),
            },
        ];
        let was = [picked(
            "a_very_long_question_identifier",
            "y",
            &[("x", 0.2), ("y", 0.8)],
        )];
        let text = render(&header(Some(9)), &questions, &now, &was, false);
        for line in text.lines() {
            assert!(line.width() <= 100, "{} columns: {line}", line.width());
        }
        assert!(text.contains("a_very_lon…"));
        assert!(text.contains("an extremely long option label …"));
        assert!(text.contains("→ was short 0.80"));
        let narrow = [choice("q", &[("x", "a"), ("y", &"w".repeat(80))])];
        let outcome = [picked("q", "x", &[("x", 0.6), ("y", 0.4)])];
        let before = [picked("q", "y", &[("x", 0.2), ("y", 0.8)])];
        for line in render(&header(Some(1)), &narrow, &outcome, &before, false).lines() {
            assert!(line.width() <= 100, "{} columns: {line}", line.width());
        }
    }

    #[test]
    fn color_styles_rows_but_never_changes_the_layout() {
        let (questions, now, was) = session();
        let off = render(&header(Some(3)), &questions, &now, &was, false);
        let on = render(&header(Some(3)), &questions, &now, &was, true);
        assert!(!off.contains('\x1b'));
        assert!(on.contains(TEAL) && on.contains(DIM));
        assert_eq!(plain(&on), off);
        // Selected rows and change markers are teal; unselected options are dim.
        let taxi = on.lines().find(|line| line.contains("taxi")).unwrap();
        assert!(taxi.contains(&format!("{TEAL} ▶ taxi")));
        assert!(taxi.ends_with(&format!("{TEAL}   → was train 1.00{RESET}")));
        let train = on.lines().find(|line| line.contains("0.01")).unwrap();
        assert!(train.contains(DIM) && !train.contains(TEAL));
    }

    #[test]
    fn output_failures_surface_as_render_errors() {
        struct Broken;
        impl Write for Broken {
            fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
                Err(std::io::Error::other("closed"))
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let (questions, now, was) = session();
        let result = write(
            &mut Broken,
            &header(Some(1)),
            &questions,
            &now,
            |id| was.iter().find(|outcome| outcome.question_id() == id),
            false,
        );
        assert!(matches!(result, Err(RenderError::Write(_))));
    }
}
