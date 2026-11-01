//! Context, question, and answer-list form rows with their cursor positions.
use super::{App, chrome::Theme, references::reference_menu, wrap::wrap};
use crate::{
    render,
    repl::tui::form::{Kind, Row, Section},
};
use ratatui::{
    style::Style,
    text::{Line, Span},
};
use unicode_width::UnicodeWidthStr;
/// The form's lines, gathered with the cursor's column and row in them.
struct Rows {
    lines: Vec<Line<'static>>,
    cursor: Option<(usize, usize)>,
    width: usize,
    menu: Vec<Line<'static>>,
}

impl Rows {
    /// A section's heading: its title in a rule across the width.
    fn rule(&mut self, title: Vec<Span<'static>>, theme: &Theme) {
        let used = 4 + title.iter().map(Span::width).sum::<usize>();
        let mut spans = vec![Span::styled("── ", theme.dim)];
        spans.extend(title);
        let rest = "─".repeat(self.width.saturating_sub(used));
        spans.push(Span::styled(format!(" {rest}"), theme.dim));
        self.lines.push(Line::from(spans));
    }

    /// A row: its gutter, then its text wrapped under itself, or the
    /// placeholder when there is none. `cursor` is a byte offset into `text`.
    fn push(
        &mut self,
        gutter: Span<'static>,
        text: &str,
        placeholder: Span<'static>,
        style: Style,
        cursor: Option<usize>,
    ) {
        let (text, position) = render::image_label_projection(text, cursor.unwrap_or(0));
        let text = text.as_str();
        let cursor = cursor.map(|_| position);
        let indent = gutter.width();
        let room = self.width.saturating_sub(indent).max(1);
        let first = self.lines.len();
        if text.is_empty() {
            self.lines.push(Line::from(vec![gutter, placeholder]));
            if cursor.is_some() {
                self.cursor = Some((indent, first));
                self.lines.append(&mut self.menu);
            }
            return;
        }
        let rows = wrap(text, room);
        for (index, range) in rows.iter().enumerate() {
            let lead = if index == 0 {
                gutter.clone()
            } else {
                Span::raw(" ".repeat(indent))
            };
            let part = Span::styled(text[range.clone()].to_owned(), style);
            self.lines.push(Line::from(vec![lead, part]));
        }
        let Some(cursor) = cursor else { return };
        let row = rows
            .iter()
            .rposition(|range| range.start <= cursor)
            .unwrap_or(0);
        let column = text[rows[row].start..cursor].width();
        self.cursor = Some(if column < room {
            (indent + column, first + row)
        } else {
            // At the end of a full last row, the cursor starts the next one.
            self.lines.push(Line::from(" ".repeat(indent)));
            (indent, first + row + 1)
        });
        self.lines.append(&mut self.menu);
    }
}

pub(super) type Composer = (Vec<Line<'static>>, Option<(usize, usize)>);

/// The form row that has the keys, if the form has them, and the cursor in it.
#[derive(Clone, Copy)]
struct Focus {
    row: Option<Row>,
    cursor: usize,
}

impl Focus {
    /// The cursor's offset when `row` has the keys.
    fn at(self, row: Row) -> Option<usize> {
        (self.row == Some(row)).then_some(self.cursor)
    }

    fn section(self) -> Option<Section> {
        self.row.map(Row::section)
    }

    /// `on` while the keys are in `part`, else `off`.
    fn style(self, part: Section, on: Style, off: Style) -> Style {
        if self.section() == Some(part) {
            on
        } else {
            off
        }
    }
}

/// The form as lines, with the cursor's column and row when it has the keys.
pub(super) fn composer(app: &App, width: usize, theme: &Theme) -> Composer {
    let focus = Focus {
        row: (!app.graph_focused).then_some(app.form.row),
        cursor: app.form.cursor,
    };
    let mut rows = Rows {
        lines: Vec::new(),
        cursor: None,
        width,
        menu: reference_menu(app, theme),
    };
    context_section(&mut rows, app, focus, theme);
    question_section(&mut rows, app, focus, theme);
    if app.form.kind != Kind::Noul {
        answers_section(&mut rows, app, focus, theme);
    }
    rows.lines.push(send_hint(app, theme));
    (rows.lines, rows.cursor)
}

/// The context heading, the latest question and queued context, then the
/// input row.
fn context_section(rows: &mut Rows, app: &App, focus: Focus, theme: &Theme) {
    let queue = &app.repl.queue;
    let none = || Span::raw("");
    let mut heading = vec![Span::styled(
        "Context",
        focus.style(Section::State, theme.accent, Style::new()),
    )];
    let queued = queue.len();
    if queued > 0 {
        heading.push(Span::styled(format!(" · {queued} queued"), theme.dim));
    }
    if app.latest_question.is_some() {
        heading.push(Span::styled(" · latest question included", theme.dim));
    }
    rows.rule(heading, theme);
    if let Some(latest) = &app.latest_question {
        rows.push(
            Span::styled("  ↳ ", theme.dim),
            &render::clean(&latest.question.prompt),
            none(),
            theme.dim,
            None,
        );
    }
    for (index, fact) in queue.facts.iter().enumerate() {
        let gutter = Span::styled("  + ", theme.dim);
        rows.push(
            gutter,
            fact,
            none(),
            Style::new(),
            focus.at(Row::Fact(index)),
        );
    }
    for (number, text) in &queue.replaces {
        let replacement = format!("{number}: {}", render::clean(text));
        rows.push(
            Span::styled("  ~ ", theme.dim),
            &replacement,
            none(),
            theme.dim,
            None,
        );
    }
    // A failed turn left it, and an empty line sends it again.
    if let Some(question) = &queue.question {
        let question = render::clean(question);
        rows.push(
            Span::styled("    ", theme.dim),
            &question,
            none(),
            theme.dim,
            None,
        );
    }
    let placeholder = if queue.question.is_some() {
        "Add context or retry"
    } else {
        "Add context or drop a file"
    };
    rows.push(
        Span::styled("  › ", focus.style(Section::State, theme.accent, theme.dim)),
        &app.form.input,
        Span::styled(placeholder, theme.dim),
        Style::new(),
        focus.at(Row::Input),
    );
}

/// The question heading with its kinds, then the question, or the previous
/// one dimmed while the field is blank.
fn question_section(rows: &mut Rows, app: &App, focus: Focus, theme: &Theme) {
    let form = &app.form;
    let mut heading = vec![
        Span::styled("Question", theme.kind(form.kind, true)),
        Span::styled(" ·", theme.dim),
    ];
    for kind in Kind::ALL {
        heading.push(if kind == form.kind {
            Span::styled(format!(" [{}]", kind.name()), theme.kind(kind, true))
        } else {
            Span::styled(format!(" {}", kind.name()), theme.kind(kind, false))
        });
    }
    heading.push(Span::styled(" · Shift-Tab →", theme.dim));
    let previous_prompt = app
        .prior_for_kind()
        .filter(|_| form.question.trim().is_empty())
        .map(|previous| render::clean(&previous.question.prompt));
    rows.rule(heading, theme);
    let question_text = previous_prompt.as_deref().unwrap_or(&form.question);
    let question_style = if previous_prompt.is_some() {
        theme.dim
    } else {
        Style::new()
    };
    rows.push(
        Span::raw("    "),
        question_text,
        Span::styled("Ask a question", theme.dim),
        question_style,
        focus
            .at(Row::Question)
            .map(|cursor| if previous_prompt.is_some() { 0 } else { cursor }),
    );
}

/// A choice's options or a score's levels, numbered, under a heading that
/// says when the previous question's are reused.
fn answers_section(rows: &mut Rows, app: &App, focus: Focus, theme: &Theme) {
    let form = &app.form;
    let (name, item) = match form.kind {
        Kind::Score => ("Levels", "a level"),
        _ => ("Options", "an option"),
    };
    let mut heading = vec![Span::styled(name, theme.kind(form.kind, true))];
    if form.kind == Kind::Score {
        heading.push(Span::styled(" · lowest first", theme.dim));
    }
    if let Some(previous) = app.inherited_answers() {
        let count = match &previous.question.answer_space {
            nouli_core::decision::AnswerSpace::Choice { options } => options.len(),
            nouli_core::decision::AnswerSpace::Score { levels } => levels.len(),
            nouli_core::decision::AnswerSpace::Noul => 0,
        };
        heading.push(Span::styled(
            format!(" · using previous {count}"),
            theme.dim,
        ));
    }
    rows.rule(heading, theme);
    if form.options.is_empty() && app.inherited_answers().is_none() {
        let placeholder = Span::styled(format!("+ Add {item}"), theme.dim);
        rows.push(Span::raw("    "), "", placeholder, Style::new(), None);
    }
    for (index, option) in form.options.iter().enumerate() {
        let gutter = Span::styled(format!("{:>3} ", index + 1), theme.dim);
        let placeholder = Span::styled(item, theme.dim);
        rows.push(
            gutter,
            option,
            placeholder,
            Style::new(),
            focus.at(Row::Option(index)),
        );
    }
}

/// What Enter does from the focused row.
fn send_hint(app: &App, theme: &Theme) -> Line<'static> {
    Line::from(Span::styled(
        match app.form.row {
            _ if app.references.picker.is_some() => "  Enter inserts reference · Esc closes",
            Row::Option(_) if app.option_enter => "  Press Enter again to send",
            Row::Option(_) => "  Enter next item · Enter twice to send",
            _ => "  Press Enter to send",
        },
        theme.kind(app.form.kind, true),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repl::tui::draw::tests::{app, shown};

    #[test]
    fn inherited_answers_are_visible_without_becoming_an_unsent_draft() {
        for (kind, message) in [
            (Kind::Choice, "Options · using previous 2"),
            (Kind::Score, "Levels · lowest first · using previous 2"),
        ] {
            let settings = crate::settings::Settings::for_test(std::env::temp_dir());
            let mut app = app(&settings);
            app.form.kind = kind;
            app.previous_question = Some(crate::repl::tui::tests::previous(kind));
            let screen = shown(&mut app, 140, 24).join("\n");
            assert!(screen.contains(message), "{screen}");
            assert!(!screen.contains("previous question"), "{screen}");
            assert!(screen.contains("Press Enter to send"), "{screen}");
            assert!(
                screen.contains("Which destination suits this trip?"),
                "{screen}"
            );
            assert!(screen.contains("Ready"), "{screen}");
            assert!(app.form.is_empty());
            app.form.options = vec!["A replacement".into()];
            assert!(!shown(&mut app, 140, 24).join("\n").contains(message));
            app.form.kind = Kind::Noul;
            let screen = shown(&mut app, 140, 24).join("\n");
            assert!(!screen.contains("using previous"), "{screen}");
        }
    }

    #[test]
    fn latest_context_is_visible_and_image_metadata_is_hidden() {
        let settings = crate::settings::Settings::for_test(std::env::temp_dir());
        let mut app = app(&settings);
        app.latest_question = Some(crate::repl::tui::tests::previous(Kind::Noul));
        let reference = nouli_core::context::evidence::marker::image_marker(
            nouli_core::identity::Id::generate(),
        );
        app.form.input = format!("holiday photo.png {reference}");
        app.form.cursor = app.form.input.len();
        let screen = shown(&mut app, 140, 24).join("\n");
        assert!(
            screen.contains("Context · latest question included"),
            "{screen}"
        );
        assert!(
            screen.contains("↳ Which destination suits this trip?"),
            "{screen}"
        );
        assert!(screen.contains("holiday photo.png"), "{screen}");
        assert!(
            !screen.contains(nouli_core::context::evidence::marker::IMAGE_MARKER_PREFIX),
            "{screen}"
        );
        assert!(
            app.form.input.contains(&reference),
            "native media reference stays intact"
        );
    }

    #[test]
    fn a_question_a_failed_turn_left_waits_in_state() {
        let settings = crate::settings::Settings::for_test(std::env::temp_dir());
        let mut app = app(&settings);
        app.repl.queue.facts.push("The train is late.".into());
        app.repl.queue.question = Some("late: Will it be late?".into());
        let screen = shown(&mut app, 140, 20).join("\n");
        assert!(screen.contains("Context · 2 queued"), "{screen}");
        assert!(screen.contains("  + The train is late."), "{screen}");
        assert!(screen.contains("    late: Will it be late?"), "{screen}");
        assert!(screen.contains("  › Add context or retry"), "{screen}");
    }

    #[test]
    fn the_form_shows_its_sections_and_which_answer_type_is_chosen() {
        let settings = crate::settings::Settings::for_test(std::env::temp_dir());
        let mut app = app(&settings);
        let screen = shown(&mut app, 140, 20).join("\n");
        assert!(screen.contains("── Context"), "{screen}");
        assert!(
            screen
                .lines()
                .any(|line| line.contains("── Question") && line.contains("Shift-Tab →")),
            "{screen}"
        );
        assert!(
            screen.contains("── Question · [Yes / No] Choose one Score"),
            "{screen}"
        );
        assert!(!screen.contains("Options"), "{screen}");
        assert!(
            screen.contains("│ decisions · Ctrl-G to resize"),
            "{screen}"
        );
        app.form.kind = Kind::Score;
        app.form.options = vec!["low".into(), "high".into()];
        let screen = shown(&mut app, 140, 20).join("\n");
        assert!(
            screen.contains("── Question · Yes / No Choose one [Score]"),
            "{screen}"
        );
        assert!(screen.contains("── Levels · lowest first"), "{screen}");
        assert!(screen.contains("  2 high"), "{screen}");
        app.form.row = Row::Option(1);
        assert!(
            shown(&mut app, 140, 20)
                .join("\n")
                .contains("Enter twice to send")
        );
        app.option_enter = true;
        assert!(
            shown(&mut app, 140, 20)
                .join("\n")
                .contains("Press Enter again to send")
        );
        // Too narrow for both: the pane waits for Ctrl-G.
        let screen = shown(&mut app, 100, 20).join("\n");
        assert!(!screen.contains("decisions · Ctrl-G"), "{screen}");
        app.graph_focused = true;
        let screen = shown(&mut app, 100, 20).join("\n");
        assert!(screen.contains("decisions · Ctrl-G"), "{screen}");
        assert!(!screen.contains("── Context"), "{screen}");
    }

    #[test]
    fn the_cursor_follows_the_text_onto_wrapped_rows() {
        let mut rows = Rows {
            lines: Vec::new(),
            cursor: None,
            width: 14,
            menu: vec![],
        };
        let theme = Theme::new(false);
        rows.push(
            Span::raw("  › "),
            "one two three",
            Span::raw(""),
            theme.dim,
            Some(8),
        );
        assert_eq!(rows.lines.len(), 2);
        assert_eq!(
            rows.cursor,
            Some((4, 1)),
            "after `two `, at the start of `three`"
        );
        // At the end of a full row, the cursor starts a row of its own.
        let mut rows = Rows {
            lines: Vec::new(),
            cursor: None,
            width: 8,
            menu: vec![],
        };
        rows.push(Span::raw("  › "), "abcd", Span::raw(""), theme.dim, Some(4));
        assert_eq!((rows.lines.len(), rows.cursor), (2, Some((4, 1))));
    }
}
