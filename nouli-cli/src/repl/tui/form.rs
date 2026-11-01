//! The question form: the input row for the next fact, the question, and its
//! options or levels, edited one row at a time. Queued facts live in the
//! loop's own queue.
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use nouli_core::decision::{AnswerSpace, ChoiceOption, ContractError, Criterion, Question};
use unicode_segmentation::UnicodeSegmentation;

pub(super) use crate::palette::Tone as Kind;

/// The parts of the form that Tab moves between.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Section {
    State,
    Question,
    /// A choice's options or a score's levels; a yes/no question has none.
    Options,
}

/// The row being edited.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Row {
    /// Where a new fact or a `/` command is typed.
    Input,
    Question,
    /// An option or level, by its place in the list.
    Option(usize),
}

impl Row {
    pub(super) fn section(self) -> Section {
        match self {
            Row::Input => Section::State,
            Row::Question => Section::Question,
            Row::Option(_) => Section::Options,
        }
    }
}

/// Why the form cannot be sent yet, and the section to fix.
#[derive(Debug, PartialEq, Eq)]
pub(super) struct Problem {
    pub message: String,
    pub section: Section,
}

/// Only the focused option can be empty: leaving one drops it.
#[derive(Debug)]
pub(super) struct Form {
    pub kind: Kind,
    pub input: String,
    /// Queued facts below the input row, which moves through them. A fact
    /// being edited is held in `input` until the row is left.
    pub rows_below: usize,
    /// Literal question text. Identifiers are generated when sending.
    pub question: String,
    pub options: Vec<String>,
    pub row: Row,
    /// A byte offset into the focused row's text, on a grapheme boundary.
    pub cursor: usize,
}

impl Default for Form {
    fn default() -> Self {
        Self {
            kind: Kind::Noul,
            input: String::new(),
            rows_below: 0,
            question: String::new(),
            options: Vec::new(),
            row: Row::Input,
            cursor: 0,
        }
    }
}

impl Form {
    /// The focused row's text.
    pub(super) fn text(&self) -> &str {
        self.row_text(self.row)
    }

    pub(super) fn row_text(&self, row: Row) -> &str {
        match row {
            Row::Input => &self.input,
            Row::Question => &self.question,
            Row::Option(index) => self.options.get(index).map_or("", String::as_str),
        }
    }

    fn text_mut(&mut self) -> Option<&mut String> {
        match self.row {
            Row::Input => Some(&mut self.input),
            Row::Question => Some(&mut self.question),
            Row::Option(index) => self.options.get_mut(index),
        }
    }

    /// Applies an editing key to the focused row; false for any other key.
    pub(super) fn edit(&mut self, key: &KeyEvent) -> bool {
        let mut cursor = self.cursor;
        let edited = self
            .text_mut()
            .is_some_and(|text| edit(text, &mut cursor, key));
        self.cursor = cursor;
        edited
    }

    /// Insert literal pasted text, preserving paragraphs and code indentation.
    pub(super) fn paste(&mut self, pasted: &str) {
        let normalized = pasted
            .replace("\r\n", "\n")
            .replace('\r', "\n")
            .replace('\t', "    ");
        let pasted: String = normalized
            .chars()
            .filter_map(|c| match c {
                '\n' => Some('\n'),
                c if c.is_control() => None,
                c => Some(c),
            })
            .collect();
        let mut cursor = self.cursor;
        if let Some(text) = self.text_mut() {
            text.insert_str(cursor, &pasted);
            cursor += pasted.len();
        }
        self.cursor = cursor;
    }

    pub(super) fn remove_range(&mut self, range: std::ops::Range<usize>) {
        let start = range.start;
        if let Some(text) = self.text_mut() {
            text.replace_range(range, "");
            self.cursor = start;
        }
    }

    pub(super) fn add_option(&mut self) {
        self.options.push(String::new());
        self.go(Row::Option(self.options.len() - 1));
    }

    /// Empties the focused row.
    pub(super) fn clear_row(&mut self) {
        if let Some(text) = self.text_mut() {
            text.clear();
        }
        self.cursor = 0;
    }

    /// Where the input row sits among `facts` queued facts.
    pub(super) fn input_index(&self, facts: usize) -> usize {
        facts.saturating_sub(self.rows_below)
    }

    /// Nothing typed: no new fact, question, or option.
    pub(super) fn is_empty(&self) -> bool {
        self.input.trim().is_empty()
            && self.question.trim().is_empty()
            && self.options.iter().all(|option| option.trim().is_empty())
    }

    /// Empties everything but the answer type, back on the input row.
    pub(super) fn clear(&mut self) {
        *self = Self {
            kind: self.kind,
            ..Self::default()
        };
    }

    /// A saved turn took the queue, and the question when it came from the
    /// form. The answer type stays for the next question.
    pub(super) fn sent(&mut self, from_form: bool) {
        if from_form {
            self.question.clear();
            self.options.clear();
            self.rows_below = 0;
            self.row = Row::Input;
            self.cursor = self.input.len();
        }
    }

    /// Moves to `target` with the cursor at its end, dropping the row it
    /// leaves when that is an empty option.
    pub(super) fn go(&mut self, mut target: Row) {
        if target != self.row
            && let Row::Option(index) = self.row
            && self
                .options
                .get(index)
                .is_some_and(|option| option.trim().is_empty())
        {
            self.options.remove(index);
            if let Row::Option(after) = target
                && after > index
            {
                target = Row::Option(after - 1);
            }
        }
        self.row = target;
        self.cursor = self.text().len();
    }

    /// Moves into a section: State's input row, the question, or the last
    /// option, adding an empty one when there are none.
    pub(super) fn focus(&mut self, section: Section) {
        match section {
            Section::State => self.go(Row::Input),
            Section::Question => self.go(Row::Question),
            Section::Options if self.kind == Kind::Noul => self.go(Row::Question),
            Section::Options => {
                if self.options.is_empty() {
                    self.go(Row::Question);
                    self.options.push(String::new());
                }
                self.go(Row::Option(self.options.len() - 1));
            }
        }
    }

    /// Where Tab goes from the focused row: State, then the question, then
    /// its options or levels, then State.
    pub(super) fn next_section(&self) -> Section {
        match self.row.section() {
            Section::State => Section::Question,
            Section::Question if self.kind != Kind::Noul => Section::Options,
            Section::Question | Section::Options => Section::State,
        }
    }

    /// Tab: the next section, starting at the first option.
    pub(super) fn tab(&mut self) {
        if self.row == Row::Question && self.kind != Kind::Noul && !self.options.is_empty() {
            self.go(Row::Option(0));
        } else {
            self.focus(self.next_section());
        }
    }

    /// Shift-Tab: the next answer type. A yes/no question has no options, so
    /// the cursor leaves them; they come back with a choice or a score.
    pub(super) fn next_kind(&mut self) {
        self.kind = self.kind.next();
        if self.kind == Kind::Noul && matches!(self.row, Row::Option(_)) {
            self.go(Row::Question);
        }
    }

    /// Up: the section above, wrapping, and row by row within a list.
    pub(super) fn up(&mut self) {
        match self.row {
            Row::Input if self.kind == Kind::Noul => self.go(Row::Question),
            Row::Input => self.focus(Section::Options),
            Row::Question => self.go(Row::Input),
            Row::Option(0) => self.go(Row::Question),
            Row::Option(index) => self.go(Row::Option(index - 1)),
        }
    }

    /// Down: the section below, wrapping, and row by row within a list.
    pub(super) fn down(&mut self) {
        match self.row {
            Row::Input => self.go(Row::Question),
            Row::Question => self.tab(),
            Row::Option(index) if index + 1 < self.options.len() => {
                self.go(Row::Option(index + 1));
            }
            Row::Option(_) => self.go(Row::Input),
        }
    }

    /// Backspace at the start of an empty option removes it and moves up;
    /// false when the row is not one to remove.
    pub(super) fn remove_empty(&mut self) -> bool {
        match self.row {
            Row::Option(index) if self.options.get(index).is_some_and(String::is_empty) => {
                self.up();
                true
            }
            _ => false,
        }
    }

    /// Enter on an option: a new option after it, or `true` on an empty one,
    /// which means the list is done.
    pub(super) fn enter_option(&mut self, index: usize) -> bool {
        if self
            .options
            .get(index)
            .is_none_or(|option| option.trim().is_empty())
        {
            return true;
        }
        self.options.insert(index + 1, String::new());
        self.row = Row::Option(index + 1);
        self.cursor = 0;
        false
    }

    /// The options or levels that count: the ones with text.
    pub(super) fn labels(&self) -> impl Iterator<Item = &str> {
        self.options
            .iter()
            .map(|option| option.trim())
            .filter(|option| !option.is_empty())
    }

    /// Build the typed request directly; punctuation is always literal.
    pub(super) fn build_question(&self) -> Result<Question, Problem> {
        self.build_question_with_prompt(&self.question)
    }

    pub(super) fn build_question_with_prompt(&self, prompt: &str) -> Result<Question, Problem> {
        if prompt.trim().is_empty() {
            return Err(Problem {
                message: "Write a question to ask.".into(),
                section: Section::Question,
            });
        }
        let labels: Vec<_> = self.labels().collect();
        let answer_space = match self.kind {
            Kind::Noul => AnswerSpace::Noul,
            Kind::Choice => AnswerSpace::Choice {
                options: labels
                    .iter()
                    .enumerate()
                    .map(|(index, label)| ChoiceOption {
                        id: format!("option_{}", index + 1),
                        description: (*label).to_owned(),
                    })
                    .collect(),
            },
            Kind::Score => AnswerSpace::Score {
                levels: labels
                    .iter()
                    .map(|label| Criterion::Text((*label).to_owned()))
                    .collect(),
            },
        };
        let identity = nouli_core::identity::Id::derived(
            &serde_json::to_string(&(prompt.trim(), &answer_space))
                .expect("question strings serialize"),
        )
        .to_string();
        let question = Question {
            id: format!("q_{}", &identity[identity.len() - 12..]),
            prompt: prompt.trim().to_owned(),
            criteria: Vec::new(),
            answer_space,
        };
        question.validate().map_err(|error| Problem {
            section: Section::Options,
            message: match error {
                ContractError::Cardinality("choice.options") => {
                    "Add at least two options (up to 255).".into()
                }
                ContractError::Cardinality("score.levels") => {
                    "Add at least two levels (up to 10).".into()
                }
                _ => error.to_string(),
            },
        })?;
        Ok(question)
    }
}

/// Applies an editing key to one row's text; false for any other key.
fn edit(text: &mut String, cursor: &mut usize, key: &KeyEvent) -> bool {
    let control = key.modifiers.contains(KeyModifiers::CONTROL);
    let alt = key.modifiers.contains(KeyModifiers::ALT);
    match key.code {
        // Control with Alt is AltGr on some keyboards, which types a character.
        KeyCode::Char(c) if !c.is_control() && control == alt => {
            text.insert(*cursor, c);
            *cursor += c.len_utf8();
        }
        KeyCode::Char('a') if control => *cursor = 0,
        KeyCode::Char('e') if control => *cursor = text.len(),
        KeyCode::Char('u') if control => {
            text.replace_range(..*cursor, "");
            *cursor = 0;
        }
        KeyCode::Char('k') if control => text.truncate(*cursor),
        KeyCode::Char('w') if control => delete_word(text, cursor),
        KeyCode::Backspace if control || alt => delete_word(text, cursor),
        KeyCode::Char('h') if control => backspace(text, cursor),
        KeyCode::Backspace => backspace(text, cursor),
        KeyCode::Delete => {
            if let Some(next) = text[*cursor..].graphemes(true).next() {
                text.replace_range(*cursor..*cursor + next.len(), "");
            }
        }
        KeyCode::Left if control || alt => *cursor = word_start(text, *cursor),
        KeyCode::Right if control || alt => *cursor = word_end(text, *cursor),
        KeyCode::Left => {
            if let Some(previous) = text[..*cursor].graphemes(true).next_back() {
                *cursor -= previous.len();
            }
        }
        KeyCode::Right => {
            if let Some(next) = text[*cursor..].graphemes(true).next() {
                *cursor += next.len();
            }
        }
        KeyCode::Home => *cursor = 0,
        KeyCode::End => *cursor = text.len(),
        _ => return false,
    }
    true
}

fn backspace(text: &mut String, cursor: &mut usize) {
    if let Some(previous) = text[..*cursor].graphemes(true).next_back() {
        let start = *cursor - previous.len();
        text.replace_range(start..*cursor, "");
        *cursor = start;
    }
}

fn delete_word(text: &mut String, cursor: &mut usize) {
    let start = word_start(text, *cursor);
    text.replace_range(start..*cursor, "");
    *cursor = start;
}

/// The start of the word before `cursor`, past any spaces in between.
pub(super) fn word_start(text: &str, cursor: usize) -> usize {
    let before = text[..cursor].trim_end();
    before
        .char_indices()
        .rev()
        .find(|(_, c)| c.is_whitespace())
        .map_or(0, |(at, c)| at + c.len_utf8())
}

/// The end of the word after `cursor`, past any spaces in between.
fn word_end(text: &str, cursor: usize) -> usize {
    let after = &text[cursor..];
    let word = after.trim_start();
    let skipped = after.len() - word.len();
    cursor + skipped + word.find(char::is_whitespace).unwrap_or(word.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn control(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
    }

    fn typed(form: &mut Form, text: &str) {
        for c in text.chars() {
            assert!(form.edit(&key(KeyCode::Char(c))));
        }
    }

    #[test]
    fn editing_keys_move_and_delete_by_grapheme_and_by_word() {
        let mut form = Form::default();
        typed(&mut form, "the cafe\u{301} is open");
        assert!(form.edit(&control('w')));
        assert_eq!(form.input, "the cafe\u{301} is ");
        for _ in 0..5 {
            form.edit(&key(KeyCode::Left));
        }
        // One step crossed the letter and its accent together.
        assert_eq!(&form.input[form.cursor..], "e\u{301} is ");
        form.edit(&key(KeyCode::Delete));
        assert_eq!(form.input, "the caf is ");
        form.edit(&key(KeyCode::Backspace));
        assert_eq!(form.input, "the ca is ");
        form.edit(&control('u'));
        assert_eq!((form.input.as_str(), form.cursor), (" is ", 0));
        form.edit(&KeyEvent::new(KeyCode::Right, KeyModifiers::ALT));
        assert_eq!(form.cursor, 3);
        form.edit(&control('k'));
        assert_eq!(form.input, " is");
        form.edit(&control('a'));
        assert_eq!(form.cursor, 0);
        assert!(!form.edit(&key(KeyCode::Enter)));
        assert!(!form.edit(&control('g')));
        form.paste("two\nlines\t!\u{7}");
        assert_eq!(form.input, "two\nlines    ! is");
    }

    #[test]
    fn tab_visits_the_sections_and_options_only_for_choices_and_scores() {
        let mut form = Form::default();
        form.tab();
        assert_eq!(form.row, Row::Question);
        // A yes/no question has no options, so Tab goes back to State.
        form.tab();
        assert_eq!(form.row, Row::Input);
        form.next_kind();
        assert_eq!(form.kind, Kind::Choice);
        form.tab();
        form.tab();
        assert_eq!((form.row, form.options.len()), (Row::Option(0), 1));
        typed(&mut form, "Rail");
        form.next_kind();
        assert_eq!((form.kind, form.row), (Kind::Score, Row::Option(0)));
        // Back to yes/no: the cursor leaves the options, which stay for later.
        form.next_kind();
        assert_eq!((form.kind, form.row), (Kind::Noul, Row::Question));
        assert_eq!(form.options, ["Rail"]);
        form.next_kind();
        form.tab();
        assert_eq!(
            (form.row, form.cursor, form.options.len()),
            (Row::Option(0), 4, 1)
        );
        // An empty option left behind is dropped.
        assert!(!form.enter_option(0));
        assert_eq!(form.row, Row::Option(1));
        form.tab();
        assert_eq!((form.row, form.options.len()), (Row::Input, 1));
    }

    #[test]
    fn tab_leaves_a_list_and_starts_at_its_first_row() {
        let mut form = Form {
            kind: Kind::Choice,
            options: vec!["One".into(), "Two".into()],
            row: Row::Option(0),
            ..Form::default()
        };
        form.tab();
        assert_eq!(form.row, Row::Input, "Tab does not walk the list");
        form.tab();
        form.tab();
        assert_eq!(form.row, Row::Option(0));
    }

    #[test]
    fn up_and_down_change_boxes_and_walk_only_within_a_list() {
        let mut form = Form::default();
        form.down();
        assert_eq!(form.row, Row::Question);
        form.down();
        assert_eq!(form.row, Row::Input, "yes/no wraps past the question");
        form.up();
        assert_eq!(form.row, Row::Question);
        form.next_kind();
        form.down();
        assert_eq!((form.row, form.options.len()), (Row::Option(0), 1));
        // Backspace on the empty option removes it and goes back up.
        assert!(form.remove_empty());
        assert_eq!((form.row, form.options.len()), (Row::Question, 0));
        assert!(!form.remove_empty());
        form.options = vec!["One".into(), "Two".into()];
        form.down();
        assert_eq!(form.row, Row::Option(0));
        form.down();
        assert_eq!(form.row, Row::Option(1));
        form.down();
        assert_eq!(form.row, Row::Input);
        form.up();
        assert_eq!(form.row, Row::Option(1), "Up wraps to the list's last row");
        form.up();
        form.up();
        assert_eq!(form.row, Row::Question);
    }

    #[test]
    fn questions_and_options_are_literal_without_ids_or_slug_collisions() {
        for prompt in [
            "What now?",
            "Why: this [a, b]",
            "中文？",
            "Does x > y?",
            "Which <one>",
        ] {
            let mut form = Form {
                question: prompt.into(),
                options: vec!["Rail!".into(), "rail".into(), "東京, [駅]".into()],
                kind: Kind::Choice,
                ..Form::default()
            };
            let question = form.build_question().unwrap();
            assert_eq!(question.prompt, prompt);
            let AnswerSpace::Choice { options } = question.answer_space else {
                panic!()
            };
            assert_eq!(options[0].description, "Rail!");
            assert_ne!(options[0].id, options[1].id);
            assert_eq!(options[2].description, "東京, [駅]");
            form.kind = Kind::Noul;
            assert_eq!(form.build_question().unwrap().prompt, prompt);
        }
    }

    #[test]
    fn validation_only_requires_a_question_and_enough_options() {
        let mut form = Form::default();
        assert_eq!(
            form.build_question().unwrap_err().section,
            Section::Question
        );
        form.question = "What now?".into();
        assert!(form.build_question().is_ok());
        form.kind = Kind::Choice;
        assert_eq!(form.build_question().unwrap_err().section, Section::Options);
        form.options = vec!["a".into(), "b".into()];
        assert!(form.build_question().is_ok());
    }
}
