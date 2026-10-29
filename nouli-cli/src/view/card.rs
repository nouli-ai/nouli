//! Fixed-width decision cards, wrapping by terminal cells and grapheme boundaries.
use crate::render::clean;
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

pub(super) const WIDTH: usize = 36;
pub(super) const STEP: usize = (WIDTH + 4) / 2;
pub(super) const CENTER: usize = WIDTH / 2;
const TEXT: usize = WIDTH - 4;

pub(super) struct Row {
    pub text: String,
    pub highlighted: bool,
}

fn wrap(value: &str, width: usize) -> Vec<String> {
    let mut lines = vec![];
    let mut line = String::new();
    for word in value.split_whitespace() {
        let needed = word.width() + usize::from(!line.is_empty());
        if !line.is_empty() && line.width() + needed > width {
            lines.push(std::mem::take(&mut line));
        }
        if !line.is_empty() {
            line.push(' ');
        }
        for grapheme in word.graphemes(true) {
            // An arbitrarily long Indic conjunct can be a single grapheme.
            // Escape that rare case rather than overflow the card or split it.
            let escaped;
            let fragment = if grapheme.width() > width {
                escaped = grapheme
                    .chars()
                    .flat_map(char::escape_unicode)
                    .collect::<String>();
                escaped.as_str()
            } else {
                grapheme
            };
            for piece in fragment.graphemes(true) {
                if !line.is_empty() && line.width() + piece.width() > width {
                    lines.push(std::mem::take(&mut line));
                }
                line.push_str(piece);
            }
        }
    }
    if !line.is_empty() || lines.is_empty() {
        lines.push(line);
    }
    lines
}

pub(super) fn text(lines: &mut Vec<Row>, prefix: &str, value: &str) {
    append(lines, prefix, value, false);
}

pub(super) fn selected(lines: &mut Vec<Row>, value: &str) {
    append(lines, "▶ ", value, true);
}

fn append(lines: &mut Vec<Row>, prefix: &str, value: &str, highlighted: bool) {
    for (index, line) in wrap(&clean(value), TEXT - prefix.width())
        .into_iter()
        .enumerate()
    {
        lines.push(Row {
            text: format!(
                "{}{line}",
                if index == 0 {
                    prefix.to_owned()
                } else {
                    " ".repeat(prefix.width())
                }
            ),
            highlighted,
        });
    }
}

/// List options in caller order, preserving full labels and exact probabilities.
pub(super) fn table(lines: &mut Vec<Row>, heading: &str, entries: &[(String, Option<f64>, bool)]) {
    let values: Vec<_> = entries
        .iter()
        .map(|(_, probability, _)| {
            probability.map_or_else(|| "n/a".into(), |value| value.to_string())
        })
        .collect();
    let probability_width = values
        .iter()
        .map(String::len)
        .max()
        .unwrap_or(0)
        .max("Probability".len());
    lines.push(Row {
        text: format!(
            "{heading}{}Probability",
            " ".repeat(TEXT - heading.width() - "Probability".len())
        ),
        highlighted: false,
    });
    for ((label, _, highlighted), probability) in entries.iter().zip(values) {
        if probability_width + 8 > TEXT {
            append(
                lines,
                if *highlighted { "▶ " } else { "  " },
                label,
                *highlighted,
            );
            append(lines, "  ", &format!("P={probability}"), *highlighted);
            continue;
        }
        let label_width = TEXT - 3 - probability_width;
        for (index, label) in wrap(&clean(label), label_width).into_iter().enumerate() {
            let value = if index == 0 { probability.as_str() } else { "" };
            let prefix = if index == 0 && *highlighted {
                "▶ "
            } else {
                "  "
            };
            lines.push(Row {
                text: format!(
                    "{prefix}{label}{}{}{}",
                    " ".repeat(label_width - label.width() + 1),
                    " ".repeat(probability_width - value.len()),
                    value
                ),
                highlighted: *highlighted,
            });
        }
    }
}

fn body(value: &str) -> String {
    format!("│ {value}{} │", " ".repeat(TEXT - value.width()))
}

pub(super) fn frame(title: &str, content: Vec<Row>) -> Vec<Row> {
    let title = wrap(&clean(title), WIDTH - 6);
    let mut rows = vec![Row {
        text: format!(
            "┌─ {} {}┐",
            title[0],
            "─".repeat(WIDTH - title[0].width() - 5)
        ),
        highlighted: false,
    }];
    rows.extend(title.iter().skip(1).map(|line| Row {
        text: body(line),
        highlighted: false,
    }));
    if title.len() > 1 {
        rows.push(Row {
            text: body(""),
            highlighted: false,
        });
    }
    rows.extend(content.into_iter().map(|line| Row {
        text: body(&line.text),
        highlighted: line.highlighted,
    }));
    rows.push(Row {
        text: format!("└{}┘", "─".repeat(WIDTH - 2)),
        highlighted: false,
    });
    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn long_questions_and_words_wrap_without_losing_text() {
        let question =
            "Which transport should I choose given the meeting and preparation requirements?";
        let mut content = vec![];
        text(&mut content, "Q: ", question);
        assert_eq!(
            content
                .iter()
                .map(|line| line.text[3..].trim())
                .collect::<Vec<_>>()
                .join(" "),
            question
        );
        let word = "x".repeat(100);
        assert_eq!(wrap(&word, 12).concat(), word);
        assert!(content.iter().all(|line| line.text.width() <= TEXT));
    }

    #[test]
    fn unicode_cards_align_and_keep_combining_characters_and_emoji_together() {
        let phrase = "界 e\u{301} 👩‍💻 ".repeat(18);
        let mut content = vec![];
        text(&mut content, "Q: ", &phrase);
        let rows = frame("Unicode card", content);
        assert!(rows.iter().all(|line| line.text.width() == WIDTH));
        assert_eq!(
            rows.iter()
                .map(|line| line.text.matches("👩‍💻").count())
                .sum::<usize>(),
            18
        );
        assert_eq!(
            rows.iter()
                .map(|line| line.text.matches("e\u{301}").count())
                .sum::<usize>(),
            18
        );
    }

    #[test]
    fn long_headers_and_terminal_controls_stay_inside_the_frame() {
        let mut content = vec![];
        text(&mut content, "Q: ", "Question\n\u{1b}[31m\t\r?");
        let rows = frame(&"SessionName".repeat(8), content);
        assert!(rows.iter().all(|line| line.text.width() == WIDTH));
        assert!(
            !rows
                .iter()
                .map(|row| row.text.as_str())
                .collect::<String>()
                .contains('\u{1b}')
        );
        assert!(
            rows.iter()
                .map(|row| row.text.as_str())
                .collect::<String>()
                .contains("\\u{1b}")
        );
    }

    #[test]
    fn oversized_graphemes_cannot_overflow_a_card() {
        let word = format!("{}क", "क्".repeat(40));
        assert_eq!(word.graphemes(true).count(), 1);
        assert!(word.width() > TEXT);
        let mut content = vec![];
        text(&mut content, "Q: ", &word);
        let rows = frame("Large conjunct", content);
        assert!(rows.iter().all(|line| line.text.width() == WIDTH));
        let expected = word
            .chars()
            .flat_map(char::escape_unicode)
            .collect::<String>();
        assert_eq!(wrap(&word, 20).concat(), expected);
    }

    #[test]
    fn selected_options_wrap_and_keep_every_continuation_highlighted() {
        let label = "界 e\u{301} 👩‍💻 ".repeat(12);
        let mut content = vec![];
        table(
            &mut content,
            "Choices",
            &[
                (label.clone(), Some(0.0), true),
                ("Other".into(), Some(1.0), false),
            ],
        );
        let rows = frame("Selection", content);
        assert!(rows.iter().all(|row| row.text.width() == WIDTH));
        let selected: Vec<_> = rows.iter().filter(|row| row.highlighted).collect();
        assert!(selected.len() > 1);
        assert_eq!(
            selected
                .iter()
                .map(|row| row.text.matches("👩‍💻").count())
                .sum::<usize>(),
            12
        );
        assert_eq!(
            selected
                .iter()
                .map(|row| row.text.matches('▶').count())
                .sum::<usize>(),
            1
        );
    }

    #[test]
    fn long_numeric_probabilities_do_not_break_table_width() {
        let mut content = vec![];
        table(
            &mut content,
            "Choices",
            &[("Tiny".into(), Some(1e-100), true)],
        );
        let number = (1e-100_f64).to_string();
        let joined = content
            .iter()
            .map(|row| row.text.trim())
            .collect::<String>();
        assert!(joined.replace(' ', "").contains(&number));
        assert!(
            frame("Tiny probability", content)
                .iter()
                .all(|row| row.text.width() == WIDTH)
        );
    }
}
