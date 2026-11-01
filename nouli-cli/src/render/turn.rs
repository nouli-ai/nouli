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
use std::fmt::Write as _;
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
            let _ = write!(facts, " ({} replaced, kept in history)", self.replaced);
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

/// The sparkline glyph for a probability.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "the probability is clamped to 0..=1, so the index is 0..=7"
)]
fn spark(probability: f64) -> char {
    SPARK[(probability.clamp(0.0, 1.0) * 7.0).round() as usize]
}

#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss,
    reason = "the probability is clamped to 0..=1 and a bar is a few dozen cells"
)]
fn bar(probability: f64, cells: usize) -> String {
    let eighths = (probability.clamp(0.0, 1.0) * (cells * 8) as f64).round() as usize;
    let (full, remainder) = (eighths / 8, eighths % 8);
    let mut bar = "█".repeat(full);
    if full < cells {
        bar.push(EIGHTHS[remainder]);
    }
    pad(&bar, cells)
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
fn marker(
    paint: Paint,
    used: usize,
    label: Option<&str>,
    probability: f64,
    accent: &str,
    layout: Layout,
) -> String {
    let make = |used| {
        let tail = format!(" {probability:.2}");
        let label = label.map(|label| {
            let spent = used + "   → was ".width() + tail.width();
            fit(
                &clean(&super::image_labels(label)),
                layout.columns.saturating_sub(spent).max(1),
            )
        });
        match label {
            Some(label) => format!("   → was {label}{tail}"),
            None => format!("   → was{tail}"),
        }
    };
    let text = make(used);
    if layout.id_columns == 0 && used + text.width() > layout.columns {
        // TUI colors are parsed per physical line, so style after the newline.
        return format!("\n{}", paint.apply(accent, &format!("  {}", make(2))));
    }
    paint.apply(accent, &text)
}

fn choice_rows(
    paint: Paint,
    question: &Question,
    options: &[ChoiceOption],
    answer: (&str, &[OptionProbability]),
    previous: Option<&QuestionOutcome>,
    rows: &mut Vec<String>,
    layout: Layout,
) {
    let (selected, distribution) = answer;
    let name = id_column(question, layout);
    for (position, option) in options.iter().enumerate() {
        let chosen = option.id == selected;
        let lead = if position == 0 {
            format!("  {name}")
        } else {
            format!("  {}", " ".repeat(layout.id_columns))
        };
        let body = format!(
            "{} {} {}",
            pad(
                &clean(&super::image_labels(&option.description)),
                layout.label_columns
            ),
            bar(
                chance(distribution, &option.id).unwrap_or(0.0),
                layout.bar_cells
            ),
            percent(chance(distribution, &option.id)),
        );
        let mut row = if chosen {
            format!(
                "{lead}{}",
                paint.apply(crate::palette::Tone::Choice.ansi(), &format!(" ▶ {body}"))
            )
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
            let used =
                2 + layout.id_columns + 3 + layout.label_columns + 1 + layout.bar_cells + 1 + 4;
            let label = option_label(options, before);
            row.push_str(&marker(
                paint,
                used,
                if layout.id_columns == 0 && before == selected {
                    None
                } else {
                    Some(&label)
                },
                was,
                crate::palette::Tone::Choice.ansi(),
                layout,
            ));
        }
        rows.push(row);
    }
}

/// The padded question ID, or nothing when the layout has no ID column.
fn id_column(question: &Question, layout: Layout) -> String {
    if layout.id_columns == 0 {
        String::new()
    } else {
        pad(&clean(&question.id), layout.id_columns)
    }
}

/// The shared parts of a yes/no or score question's answer rows.
struct AnswerLine<'a> {
    paint: Paint,
    layout: Layout,
    /// The indent and question ID before the answer.
    lead: &'a str,
    /// The answer space's color.
    accent: &'a str,
}

impl AnswerLine<'_> {
    /// One accented row, ending in a change marker when the answer moved from `was`.
    fn row(&self, body: &str, was: Option<f64>) -> String {
        let mut row = format!("{}{}", self.lead, self.paint.apply(self.accent, body));
        if let Some(was) = was {
            row.push_str(&marker(
                self.paint,
                self.lead.width() + body.width(),
                None,
                was,
                self.accent,
                self.layout,
            ));
        }
        row
    }
}

fn question_rows(
    paint: Paint,
    question: &Question,
    outcome: Option<&QuestionOutcome>,
    previous: Option<&QuestionOutcome>,
    rows: &mut Vec<String>,
    layout: Layout,
) {
    let lead = format!("  {}   ", id_column(question, layout));
    let answer = match outcome {
        Some(QuestionOutcome::Answered { answer, .. }) => answer,
        Some(QuestionOutcome::Refused { reason, .. }) => {
            let text = fit(
                &clean(reason),
                layout.columns.saturating_sub(lead.width() + 9).max(1),
            );
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
    let line = AnswerLine {
        paint,
        layout,
        lead: &lead,
        accent: crate::palette::Tone::for_space(&question.answer_space).ansi(),
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
            layout,
        ),
        (AnswerSpace::Noul, Answer::Noul { probability }) => {
            rows.push(noul_row(&line, *probability, earlier));
        }
        (
            AnswerSpace::Score { levels },
            Answer::Score {
                value,
                probabilities,
            },
        ) => rows.extend(score_rows(&line, levels, (*value, probabilities), earlier)),
        _ => rows.push(format!(
            "{lead}{}",
            paint.apply(DIM, "answer does not match the question")
        )),
    }
}

/// A yes/no answer's probability bar.
fn noul_row(line: &AnswerLine, probability: f64, earlier: Option<&Answer>) -> String {
    // "yes" fills the label field, so the bar lines up with a choice's.
    let body = format!(
        "yes{}{} {}",
        " ".repeat(line.layout.label_columns.saturating_sub(2)),
        bar(probability, line.layout.bar_cells),
        percent(Some(probability))
    );
    let was = match earlier {
        Some(Answer::Noul { probability: was }) if moved(*was, probability) => Some(*was),
        _ => None,
    };
    line.row(&body, was)
}

/// A score answer's expected value and sparkline, then its per-level probabilities.
fn score_rows(
    line: &AnswerLine,
    levels: &[Criterion],
    (value, probabilities): (f64, &[f64]),
    earlier: Option<&Answer>,
) -> [String; 2] {
    let sparkline: String = probabilities
        .iter()
        .map(|probability| spark(*probability))
        .collect();
    let body = format!(
        "expected {value:.2} of {}   {sparkline}",
        levels.len().saturating_sub(1)
    );
    let was = match earlier {
        Some(Answer::Score {
            value: was,
            probabilities: before,
        }) if before.len() == probabilities.len() && moved(*was, value) => Some(*was),
        _ => None,
    };
    let detail = levels
        .iter()
        .zip(probabilities)
        .map(|(level, probability)| {
            format!(
                "{} {probability:.2}",
                clean(&super::image_labels(level_label(level)))
            )
        })
        .collect::<Vec<_>>()
        .join(" · ");
    let layout = line.layout;
    let detail = line.paint.apply(
        DIM,
        &format!(
            "  {}{}",
            " ".repeat(layout.id_columns + 3),
            fit(
                &detail,
                layout.columns.saturating_sub(layout.id_columns + 5)
            )
        ),
    );
    [line.row(&body, was), detail]
}

#[derive(Clone, Copy)]
struct Layout {
    id_columns: usize,
    bar_cells: usize,
    label_columns: usize,
    columns: usize,
}

pub(crate) fn write<'a>(
    sink: &mut impl Write,
    header: &Header,
    questions: &[Question],
    outcomes: &[QuestionOutcome],
    previous: impl Fn(&str) -> Option<&'a QuestionOutcome>,
    color: bool,
) -> Result<(), RenderError> {
    write_layout(
        sink,
        header,
        questions,
        outcomes,
        previous,
        color,
        Layout {
            id_columns: ID_COLUMNS,
            bar_cells: BAR_CELLS,
            label_columns: LABEL_COLUMNS,
            columns: MAX_COLUMNS,
        },
    )
}

pub(crate) fn write_compact<'a>(
    sink: &mut impl Write,
    header: &Header,
    questions: &[Question],
    outcomes: &[QuestionOutcome],
    previous: impl Fn(&str) -> Option<&'a QuestionOutcome>,
    color: bool,
    columns: usize,
) -> Result<(), RenderError> {
    let bar_cells = columns.saturating_sub(30).clamp(1, BAR_CELLS);
    write_layout(
        sink,
        header,
        questions,
        outcomes,
        previous,
        color,
        Layout {
            id_columns: 0,
            bar_cells,
            label_columns: columns
                .saturating_sub(26 + bar_cells)
                .clamp(1, LABEL_COLUMNS),
            columns,
        },
    )
}

/// Writes a turn's header and one block of rows per question. `previous` gives
/// the last answer to a question ID, when there is one.
fn write_layout<'a>(
    sink: &mut impl Write,
    header: &Header,
    questions: &[Question],
    outcomes: &[QuestionOutcome],
    previous: impl Fn(&str) -> Option<&'a QuestionOutcome>,
    color: bool,
    layout: Layout,
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
        question_rows(
            paint,
            question,
            outcome,
            previous(&question.id),
            &mut rows,
            layout,
        );
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
        assert_eq!(bar(0.0, BAR_CELLS), " ".repeat(20));
        assert_eq!(bar(1.0, BAR_CELLS), "█".repeat(20));
        assert_eq!(
            bar(0.5, BAR_CELLS),
            format!("{}{}", "█".repeat(10), " ".repeat(10))
        );
        assert_eq!(bar(0.00625, BAR_CELLS), format!("▏{}", " ".repeat(19)));
        // 0.99 * 160 = 158.4 eighths: 19 full cells and 6 eighths.
        assert_eq!(bar(0.99, BAR_CELLS), format!("{}▊", "█".repeat(19)));
        // Rounding up to a whole cell never indexes past the eighths.
        assert_eq!(bar(0.99999, BAR_CELLS), "█".repeat(20));
        assert_eq!(bar(7.0, BAR_CELLS), "█".repeat(20));
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
        for tone in [
            crate::palette::Tone::Noul,
            crate::palette::Tone::Choice,
            crate::palette::Tone::Score,
        ] {
            assert!(on.contains(tone.ansi()), "missing {tone:?} accent");
        }
        assert!(on.contains(crate::palette::Tone::Choice.ansi()) && on.contains(DIM));
        assert_eq!(plain(&on), off);
        // Selected rows and changes keep the question type color.
        let taxi = on.lines().find(|line| line.contains("taxi")).unwrap();
        assert!(taxi.contains(&format!("{} ▶ taxi", crate::palette::Tone::Choice.ansi())));
        assert!(taxi.ends_with(&format!(
            "{}   → was train 1.00{RESET}",
            crate::palette::Tone::Choice.ansi()
        )));
        let train = on.lines().find(|line| line.contains("0.01")).unwrap();
        assert!(train.contains(DIM) && !train.contains(TEAL));
    }

    #[test]
    fn compact_results_hide_ids_and_keep_same_choice_changes_on_one_line() {
        let questions = [choice(
            "q_f089742effff",
            &[("a", "Hawaii"), ("b", "San Diego")],
        )];
        let now = [picked("q_f089742effff", "b", &[("a", 0.23), ("b", 0.77)])];
        let was = picked("q_f089742effff", "b", &[("a", 0.38), ("b", 0.62)]);
        for columns in [60, 79, 100] {
            let mut bytes = Vec::new();
            write_compact(
                &mut bytes,
                &header(Some(2)),
                &questions,
                &now,
                |_| Some(&was),
                false,
                columns,
            )
            .unwrap();
            let text = String::from_utf8(bytes).unwrap();
            assert!(!text.contains("q_f089") && !text.contains('…'), "{text}");
            assert!(text.contains("→ was 0.62"), "{text}");
            assert!(!text.contains("was San Diego"), "{text}");
            assert!(text.lines().all(|line| line.width() <= columns), "{text}");
            assert_eq!(text.lines().count(), 3, "{text}");
        }
        // Tiny layouts must not underflow while padding a yes/no label.
        let mut bytes = Vec::new();
        write_compact(
            &mut bytes,
            &header(Some(1)),
            &[noul("q_internal")],
            &[chance_of("q_internal", 0.9)],
            |_| None,
            false,
            20,
        )
        .unwrap();
        assert!(!String::from_utf8(bytes).unwrap().contains("q_internal"));
    }

    #[test]
    fn compact_changes_fit_narrow_screens_for_all_answer_types() {
        let levels = ["A", "B", "C", "D", "E", "F", "G", "H", "I", "J"];
        let questions = [
            choice("internal-choice", &[("a", "First"), ("b", "Second")]),
            noul("internal-noul"),
            score("internal-score", &levels),
        ];
        let now = [
            picked("internal-choice", "b", &[("a", 0.1), ("b", 0.9)]),
            chance_of("internal-noul", 0.9),
            scored(
                "internal-score",
                9.0,
                &[0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0],
            ),
        ];
        let was = [
            picked("internal-choice", "a", &[("a", 0.9), ("b", 0.1)]),
            chance_of("internal-noul", 0.1),
            scored(
                "internal-score",
                0.0,
                &[1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0],
            ),
        ];
        for columns in [40, 46, 60, 79] {
            let mut bytes = Vec::new();
            write_compact(
                &mut bytes,
                &header(Some(2)),
                &questions,
                &now,
                |id| was.iter().find(|outcome| outcome.question_id() == id),
                false,
                columns,
            )
            .unwrap();
            let text = String::from_utf8(bytes).unwrap();
            assert!(!text.contains("internal-"), "{text}");
            assert_eq!(text.matches("→ was").count(), 3, "{text}");
            assert!(
                text.lines().all(|line| line.width() <= columns),
                "width {columns}: {text}"
            );
            let mut colored = Vec::new();
            write_compact(
                &mut colored,
                &header(Some(2)),
                &questions,
                &now,
                |id| was.iter().find(|outcome| outcome.question_id() == id),
                true,
                columns,
            )
            .unwrap();
            let colored = String::from_utf8(colored).unwrap();
            assert_eq!(plain(&colored), text);
            if columns == 40 {
                assert!(
                    colored.contains(&format!(
                        "\n{}     → was",
                        crate::palette::Tone::Score.ansi()
                    )),
                    "{colored:?}"
                );
            }
        }
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
