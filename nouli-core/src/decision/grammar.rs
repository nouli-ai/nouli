//! One-line question syntax: `id: prompt [a, b]`, `id: prompt <a, b>`, or `id: prompt`.
//!
//! A trailing `[…]` group makes a choice, `<…>` makes a score with levels in
//! order, and no group makes a yes/no (noul) question. Items are comma-separated
//! strings, bare or double-quoted. Choice option IDs are derived from labels.

use std::collections::HashMap;

use thiserror::Error;

use super::{AnswerSpace, ChoiceOption, ContractError, Criterion, Question};

const MAX_ID: usize = 64;

/// Grammar errors report byte offsets into the line, never the line itself.
#[derive(Debug, Error, PartialEq)]
pub enum GrammarError {
    #[error("a question starts with an ID and a colon, like `transport: …`")]
    MissingId,
    #[error(
        "question ID must be 1-64 characters of a-z, 0-9, `_` or `-`, starting with a letter or digit"
    )]
    InvalidId,
    #[error("question prompt is empty")]
    EmptyPrompt,
    #[error("unterminated quote starting at byte {0}")]
    UnterminatedQuote(usize),
    #[error("invalid escape at byte {0}; only \\\" and \\\\ are allowed")]
    InvalidEscape(usize),
    #[error("unbalanced answer list near byte {0}")]
    Unbalanced(usize),
    #[error("unexpected text after the answer list at byte {0}")]
    TrailingText(usize),
    #[error("empty answer item at byte {0}")]
    EmptyItem(usize),
    #[error("unexpected character at byte {0}; quote items that contain , [ ] < > or \"")]
    UnexpectedCharacter(usize),
    #[error("a choice needs 2 to 255 options")]
    ChoiceCount,
    #[error("a score needs 2 to 10 levels")]
    ScoreCount,
    #[error("choice options {0} and {1} derive the same ID")]
    DuplicateOption(usize, usize),
    #[error("choice option {0} has no letters or digits to derive an ID from")]
    EmptyOptionId(usize),
    #[error(transparent)]
    Contract(#[from] ContractError),
}

/// Parse one question written in the compact grammar.
pub fn parse(line: &str) -> Result<Question, GrammarError> {
    let colon = line.find(':').ok_or(GrammarError::MissingId)?;
    let id = line[..colon].trim();
    if id.is_empty() {
        return Err(GrammarError::MissingId);
    }
    if !valid_id(id) {
        return Err(GrammarError::InvalidId);
    }
    let body_start = colon + 1;
    let body = line[body_start..].trim_end();
    let (prompt, answer_space) = match body.chars().last() {
        Some(close @ (']' | '>')) => {
            let (at, items) = trailing_group(body, body_start, close)?;
            let prompt = body[..at].trim();
            let answer_space = if close == ']' {
                choice(items)?
            } else {
                score(items)?
            };
            (prompt, answer_space)
        }
        _ => (body.trim(), AnswerSpace::Noul),
    };
    if prompt.is_empty() {
        return Err(GrammarError::EmptyPrompt);
    }
    let question = Question {
        id: id.to_owned(),
        prompt: prompt.to_owned(),
        criteria: Vec::new(),
        answer_space,
    };
    question.validate()?;
    Ok(question)
}

/// Write a question in the compact grammar.
///
/// For any question produced by [`parse`], `parse(&format(&q))` returns `q`.
/// Detailed score levels are written by label, and choice option IDs are not
/// written, so other questions may not round-trip exactly.
#[must_use]
pub fn format(question: &Question) -> String {
    let group = |open: char, close: char, items: Vec<&str>| {
        let items: Vec<String> = items.into_iter().map(item).collect();
        format!(" {open}{}{close}", items.join(", "))
    };
    let answers = match &question.answer_space {
        AnswerSpace::Noul => String::new(),
        AnswerSpace::Choice { options } => group(
            '[',
            ']',
            options.iter().map(|o| o.description.as_str()).collect(),
        ),
        AnswerSpace::Score { levels } => group(
            '<',
            '>',
            levels
                .iter()
                .map(|level| match level {
                    Criterion::Text(text) => text.as_str(),
                    Criterion::Detailed { label, .. } => label.as_str(),
                })
                .collect(),
        ),
    };
    format!("{}: {}{answers}", question.id, question.prompt)
}

/// Derive a choice option ID from its label: letters and digits of any script
/// are lowercased, and every other run of characters becomes one `_`. The ID is
/// at most `MAX_ID` bytes.
#[must_use]
pub fn option_id(label: &str) -> String {
    let mut id = String::new();
    let mut gap = false;
    for c in label.chars() {
        if c.is_alphanumeric() {
            if gap && !id.is_empty() {
                id.push('_');
            }
            id.extend(c.to_lowercase());
            gap = false;
        } else {
            gap = true;
        }
    }
    let mut end = id.len().min(MAX_ID);
    while !id.is_char_boundary(end) {
        end -= 1;
    }
    id.truncate(end);
    id.trim_end_matches('_').to_owned()
}

fn valid_id(id: &str) -> bool {
    let bytes = id.as_bytes();
    bytes.len() <= MAX_ID
        && (bytes[0].is_ascii_lowercase() || bytes[0].is_ascii_digit())
        && bytes
            .iter()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || *b == b'_' || *b == b'-')
}

fn item(text: &str) -> String {
    let plain = !text.is_empty()
        && text.trim() == text
        && !text.contains([',', '[', ']', '<', '>', '"', '\\']);
    if plain {
        return text.to_owned();
    }
    format!("\"{}\"", text.replace('\\', "\\\\").replace('"', "\\\""))
}

fn choice(labels: Vec<String>) -> Result<AnswerSpace, GrammarError> {
    if !(2..=255).contains(&labels.len()) {
        return Err(GrammarError::ChoiceCount);
    }
    let mut first = HashMap::new();
    let mut options = Vec::with_capacity(labels.len());
    for (index, label) in labels.into_iter().enumerate() {
        let id = option_id(&label);
        if id.is_empty() {
            return Err(GrammarError::EmptyOptionId(index + 1));
        }
        if let Some(earlier) = first.insert(id.clone(), index + 1) {
            return Err(GrammarError::DuplicateOption(earlier, index + 1));
        }
        options.push(ChoiceOption {
            id,
            description: label,
        });
    }
    Ok(AnswerSpace::Choice { options })
}

fn score(labels: Vec<String>) -> Result<AnswerSpace, GrammarError> {
    if !(2..=10).contains(&labels.len()) {
        return Err(GrammarError::ScoreCount);
    }
    Ok(AnswerSpace::Score {
        levels: labels.into_iter().map(Criterion::Text).collect(),
    })
}

/// Find the group that closes the body: the earliest opening bracket whose
/// items parse all the way to the end. Earlier brackets belong to the prompt.
fn trailing_group(
    body: &str,
    offset: usize,
    close: char,
) -> Result<(usize, Vec<String>), GrammarError> {
    let open = if close == ']' { '[' } else { '<' };
    let mut last_error = GrammarError::Unbalanced(offset + body.len() - 1);
    for (at, c) in body.char_indices() {
        if c != open {
            continue;
        }
        match items(body, at + 1, close, offset) {
            Ok((items, end)) if body[end..].trim().is_empty() => return Ok((at, items)),
            Ok((_, end)) => last_error = GrammarError::TrailingText(offset + end),
            Err(error) => last_error = error,
        }
    }
    Err(last_error)
}

/// Parse items from `start` up to the matching `close`, returning the byte
/// position just after it.
fn items(
    body: &str,
    start: usize,
    close: char,
    offset: usize,
) -> Result<(Vec<String>, usize), GrammarError> {
    let mut out = Vec::new();
    let mut chars = body[start..]
        .char_indices()
        .map(|(i, c)| (start + i, c))
        .peekable();
    loop {
        while chars.next_if(|(_, c)| c.is_whitespace()).is_some() {}
        let Some(&(at, first)) = chars.peek() else {
            return Err(GrammarError::Unbalanced(offset + start));
        };
        let value = if first == '"' {
            chars.next();
            let mut value = String::new();
            loop {
                match chars.next() {
                    None => return Err(GrammarError::UnterminatedQuote(offset + at)),
                    Some((_, '"')) => break,
                    Some((escape, '\\')) => match chars.next() {
                        Some((_, c @ ('"' | '\\'))) => value.push(c),
                        _ => return Err(GrammarError::InvalidEscape(offset + escape)),
                    },
                    Some((_, c)) => value.push(c),
                }
            }
            if value.is_empty() {
                return Err(GrammarError::EmptyItem(offset + at));
            }
            value
        } else {
            let mut value = String::new();
            while let Some(&(i, c)) = chars.peek() {
                if c == ',' || c == close {
                    break;
                }
                if matches!(c, '[' | ']' | '<' | '>' | '"') {
                    return Err(GrammarError::UnexpectedCharacter(offset + i));
                }
                value.push(c);
                chars.next();
            }
            let value = value.trim();
            if value.is_empty() {
                return Err(GrammarError::EmptyItem(offset + at));
            }
            value.to_owned()
        };
        out.push(value);
        while chars.next_if(|(_, c)| c.is_whitespace()).is_some() {}
        match chars.next() {
            Some((_, ',')) => {}
            Some((i, c)) if c == close => return Ok((out, i + c.len_utf8())),
            Some((i, _)) => return Err(GrammarError::UnexpectedCharacter(offset + i)),
            None => return Err(GrammarError::Unbalanced(offset + start)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn options(q: &Question) -> Vec<(&str, &str)> {
        match &q.answer_space {
            AnswerSpace::Choice { options } => options
                .iter()
                .map(|o| (o.id.as_str(), o.description.as_str()))
                .collect(),
            _ => panic!("not a choice"),
        }
    }

    #[test]
    fn parses_each_answer_type() {
        let q = parse("transport: How should I get there? [train, taxi]").unwrap();
        assert_eq!(q.id, "transport");
        assert_eq!(q.prompt, "How should I get there?");
        assert_eq!(options(&q), [("train", "train"), ("taxi", "taxi")]);

        let q = parse("on_time: Will the train be on time?").unwrap();
        assert_eq!(q.answer_space, AnswerSpace::Noul);
        assert_eq!(q.prompt, "Will the train be on time?");

        let q = parse("fit: How strong is the team? <weak, partial, strong>").unwrap();
        assert_eq!(
            q.answer_space,
            AnswerSpace::Score {
                levels: vec![
                    Criterion::Text("weak".into()),
                    Criterion::Text("partial".into()),
                    Criterion::Text("strong".into()),
                ]
            }
        );
    }

    #[test]
    fn quoted_labels_keep_commas_and_escaped_quotes() {
        let q =
            parse(r#"next: What next? ["Ask, then wait", "Say \"no\"", "Back\\slash"]"#).unwrap();
        assert_eq!(
            options(&q),
            [
                ("ask_then_wait", "Ask, then wait"),
                ("say_no", r#"Say "no""#),
                ("back_slash", r"Back\slash"),
            ]
        );
    }

    #[test]
    fn earlier_brackets_belong_to_the_prompt() {
        let q = parse("pick: Choose [carefully] between <these> [a, b]").unwrap();
        assert_eq!(q.prompt, "Choose [carefully] between <these>");
        assert_eq!(options(&q).len(), 2);
        let q = parse("x: Is 3 < 5?").unwrap();
        assert_eq!(q.answer_space, AnswerSpace::Noul);
    }

    #[test]
    fn rejects_malformed_lines() {
        assert_eq!(parse("no colon here"), Err(GrammarError::MissingId));
        assert_eq!(parse(": prompt"), Err(GrammarError::MissingId));
        assert_eq!(parse("Bad: prompt"), Err(GrammarError::InvalidId));
        assert_eq!(parse("_x: prompt"), Err(GrammarError::InvalidId));
        assert_eq!(
            parse(&format!("{}: prompt", "a".repeat(65))),
            Err(GrammarError::InvalidId)
        );
        assert_eq!(parse("q:   "), Err(GrammarError::EmptyPrompt));
        assert_eq!(parse("q: [a, b]"), Err(GrammarError::EmptyPrompt));
        assert!(matches!(
            parse(r#"q: P? ["a, b]"#),
            Err(GrammarError::UnterminatedQuote(_))
        ));
        assert!(matches!(
            parse(r#"q: P? ["a\n", b]"#),
            Err(GrammarError::InvalidEscape(_))
        ));
        assert!(matches!(
            parse("q: P? [a, , b]"),
            Err(GrammarError::EmptyItem(_))
        ));
        assert!(matches!(
            parse("q: P? a, b]"),
            Err(GrammarError::Unbalanced(_))
        ));
        assert!(matches!(
            parse("q: P? [a, b] then"),
            Ok(Question {
                answer_space: AnswerSpace::Noul,
                ..
            })
        ));
        assert!(matches!(
            parse("q: P? [a <b>, c]"),
            Err(GrammarError::UnexpectedCharacter(_))
        ));
    }

    #[test]
    fn trailing_text_after_a_group_is_rejected() {
        // The line ends with a group, so it must be the answers and nothing may follow it.
        assert!(matches!(
            parse("q: P? [a, b] x]"),
            Err(GrammarError::TrailingText(_))
        ));
        let error = parse("q: P? [a, b] [c]").unwrap_err();
        assert_eq!(error, GrammarError::ChoiceCount);
    }

    #[test]
    fn derives_option_ids_from_labels() {
        assert_eq!(option_id("Book the 40 USD train"), "book_the_40_usd_train");
        assert_eq!(option_id("  --Hello, World!-- "), "hello_world");
        assert_eq!(option_id(&"x".repeat(80)).len(), 64);
        // A cut never splits a multi-byte letter.
        let long = option_id(&"東".repeat(40));
        assert_eq!(long.chars().count(), 21);
        assert!(long.len() <= 64);
        assert_eq!(
            parse("q: P? [Book train, book-train]"),
            Err(GrammarError::DuplicateOption(1, 2))
        );
        assert_eq!(
            parse("q: P? [\"!!\", b]"),
            Err(GrammarError::EmptyOptionId(1))
        );
    }

    #[test]
    fn option_ids_keep_letters_of_any_script_and_stay_distinct() {
        assert_eq!(option_id("東京"), "東京");
        assert_eq!(option_id("Café au lait"), "café_au_lait");
        assert_eq!(option_id("ÉCOLE"), "école");
        assert_eq!(
            option_id("Москва, Санкт-Петербург"),
            "москва_санкт_петербург"
        );
        for line in ["trip: Where? [東京, 大阪]", "shop: Which? [Café, Cafe]"] {
            let question = parse(line).unwrap();
            question.validate().unwrap();
            let AnswerSpace::Choice { options } = &question.answer_space else {
                panic!("a choice")
            };
            assert_ne!(options[0].id, options[1].id, "{line}");
            assert!(options.iter().all(|option| !option.id.is_empty()));
            // The labels survive a round trip through the one-line form.
            assert_eq!(parse(&format(&question)).unwrap(), question);
        }
        let AnswerSpace::Choice { options } =
            parse("trip: Where? [東京, 大阪]").unwrap().answer_space
        else {
            panic!("a choice")
        };
        assert_eq!(
            options.iter().map(|o| o.id.as_str()).collect::<Vec<_>>(),
            ["東京", "大阪"]
        );
    }

    #[test]
    fn enforces_item_counts() {
        assert_eq!(parse("q: P? [a]"), Err(GrammarError::ChoiceCount));
        let many: Vec<String> = (0..256).map(|i| format!("o{i}")).collect();
        assert_eq!(
            parse(&format!("q: P? [{}]", many.join(", "))),
            Err(GrammarError::ChoiceCount)
        );
        assert!(parse(&format!("q: P? [{}]", many[..255].join(", "))).is_ok());
        assert_eq!(parse("q: P? <a>"), Err(GrammarError::ScoreCount));
        let levels: Vec<String> = (0..11).map(|i| format!("l{i}")).collect();
        assert_eq!(
            parse(&format!("q: P? <{}>", levels.join(", "))),
            Err(GrammarError::ScoreCount)
        );
    }

    #[test]
    fn format_round_trips_parsed_questions() {
        for line in [
            "transport: How should I get there? [train, taxi]",
            "on_time: Will it arrive?",
            "fit: Rate it <poor, fair, good>",
            r#"next: What next? ["Ask, then wait", "Say \"no\"", " padded ", plain]"#,
        ] {
            let q = parse(line).unwrap();
            assert_eq!(parse(&format(&q)), Ok(q), "{line}");
        }
        assert_eq!(
            format(&parse("t: Go? [train , taxi]").unwrap()),
            "t: Go? [train, taxi]"
        );
    }

    #[test]
    fn errors_never_echo_the_input() {
        let secret = "q: P? [\"unterminated secret-text]";
        let message = parse(secret).unwrap_err().to_string();
        assert!(!message.contains("secret-text"));
    }
}
