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
/// The rule around a heading: its lead, spaces, and shortest end.
const HEADING_COLUMNS: usize = 8;
/// A list needs this many items, so that many rows show while it is edited.
const MINIMUM_ANSWERS: usize = 2;
/// Queued context folds into a count once it has this many rows.
const FOLDED_FACTS: usize = 3;
/// The form's lines, gathered with the cursor's column and row in them.
struct Rows {
    lines: Vec<Line<'static>>,
    cursor: Option<(usize, usize)>,
    width: usize,
    menu: Vec<Line<'static>>,
}

impl Rows {
    /// A section's heading: its title in a rule across the width, with
    /// `controls` at the rule's right end.
    fn rule(&mut self, title: Vec<Span<'static>>, controls: Vec<Span<'static>>, theme: &Theme) {
        let right = spans_width(&controls);
        let used = 4 + spans_width(&title) + if right > 0 { right + 1 } else { 0 };
        let mut spans = vec![Span::styled("── ", theme.dim)];
        spans.extend(title);
        let rest = "─".repeat(self.width.saturating_sub(used));
        spans.push(Span::styled(format!(" {rest}"), theme.dim));
        if right > 0 {
            spans.push(Span::raw(" "));
            spans.extend(controls);
        }
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

fn spans_width(spans: &[Span<'_>]) -> usize {
    spans.iter().map(Span::width).sum()
}

pub(super) type Composer = (Vec<Line<'static>>, Option<(usize, usize)>);

/// The form row that has the keys, if the form has them, the cursor in it,
/// and the section Tab goes to from there.
#[derive(Clone, Copy)]
struct Focus {
    row: Option<Row>,
    cursor: usize,
    next: Option<Section>,
}

impl Focus {
    /// The cursor's offset when `row` has the keys.
    fn at(self, row: Row) -> Option<usize> {
        (self.row == Some(row)).then_some(self.cursor)
    }

    fn active(self, part: Section) -> bool {
        self.row.map(Row::section) == Some(part)
    }

    /// `on` while the keys are in `part`, else `off`.
    fn style(self, part: Section, on: Style, off: Style) -> Style {
        if self.active(part) { on } else { off }
    }

    /// `tab` on the section Tab goes to next, else `plain`, so the hint is
    /// only ever true.
    fn hint(self, part: Section, tab: &'static str, plain: &'static str) -> &'static str {
        if self.next == Some(part) { tab } else { plain }
    }
}

/// The form as lines, with the cursor's column and row when it has the keys.
pub(super) fn composer(app: &App, width: usize, busy: bool, theme: &Theme) -> Composer {
    let focused = !app.graph_focused;
    let focus = Focus {
        row: focused.then_some(app.form.row),
        cursor: app.form.cursor,
        // Open lists and a running turn take Tab, so no hint promises it.
        next: (focused && !busy && app.sessions.is_none() && app.commands_shown().is_empty())
            .then(|| app.form.next_section()),
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
    (rows.lines, rows.cursor)
}

/// The context heading, the latest question and queued context, then the
/// input row. Away from Context, a long list folds into the heading's count.
fn context_section(rows: &mut Rows, app: &App, focus: Focus, theme: &Theme) {
    let queue = &app.repl.queue;
    let none = || Span::raw("");
    let active = focus.active(Section::State);
    let folded = !active && queue.facts.len() >= FOLDED_FACTS;
    let mut heading = vec![Span::styled(
        "Context",
        focus.style(Section::State, theme.accent, theme.dim),
    )];
    let queued = queue.len();
    if queued > 0 {
        heading.push(Span::styled(format!(" · {queued} added"), theme.dim));
    }
    if app.latest_question.is_some() {
        heading.push(Span::styled(" · latest question included", theme.dim));
    }
    rows.rule(heading, Vec::new(), theme);
    if let Some(latest) = &app.latest_question {
        rows.push(
            Span::styled("  ↳ ", theme.dim),
            &render::clean(&latest.question.prompt),
            none(),
            theme.dim,
            None,
        );
    }
    // Facts on either side of the input row, which moves through them.
    let (above, below) = queue
        .facts
        .split_at(app.form.input_index(queue.facts.len()));
    let fact_rows = |rows: &mut Rows, facts: &[String]| {
        if folded {
            return;
        }
        for fact in facts {
            rows.push(
                Span::styled("  + ", theme.dim),
                fact,
                Span::raw(""),
                Style::new(),
                None,
            );
        }
    };
    fact_rows(rows, above);
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
    if folded && app.form.input.is_empty() {
        return;
    }
    let placeholder = if queue.facts.is_empty() {
        focus.hint(
            Section::State,
            "Tab to add context",
            "Add context or drop a file",
        )
    } else {
        "Add context"
    };
    let gutter = if active {
        Span::styled("  › ", theme.accent)
    } else {
        Span::raw("    ")
    };
    rows.push(
        gutter,
        &app.form.input,
        Span::styled(placeholder, theme.dim),
        Style::new(),
        focus.at(Row::Input),
    );
    fact_rows(rows, below);
}

/// The answer types: all three while Question has the keys and they fit, or
/// only the chosen one.
fn kinds(chosen: Kind, room: usize, active: bool, theme: &Theme) -> Vec<Span<'static>> {
    let only = |text| vec![Span::styled(text, theme.kind(chosen, true))];
    if !active {
        return only(format!("[{}]", chosen.name()));
    }
    let mut all = Vec::new();
    for kind in Kind::ALL {
        if !all.is_empty() {
            all.push(Span::raw(" "));
        }
        all.push(if kind == chosen {
            Span::styled(format!("[{}]", kind.name()), theme.kind(kind, true))
        } else {
            Span::styled(kind.name(), theme.kind(kind, false))
        });
    }
    if spans_width(&all) <= room {
        return all;
    }
    only(format!("[{}] ⌄", chosen.name()))
}

/// The question heading with its kinds, then the question, or the previous
/// one dimmed while the field is blank.
fn question_section(rows: &mut Rows, app: &App, focus: Focus, theme: &Theme) {
    let form = &app.form;
    let active = focus.active(Section::Question);
    let heading = vec![Span::styled("Question", theme.kind(form.kind, active))];
    let room = rows
        .width
        .saturating_sub(HEADING_COLUMNS + spans_width(&heading));
    let controls = kinds(form.kind, room, active, theme);
    let previous_prompt = app
        .prior_for_kind()
        .filter(|_| form.question.trim().is_empty())
        .map(|previous| render::clean(&previous.question.prompt));
    rows.rule(heading, controls, theme);
    let question_text = previous_prompt.as_deref().unwrap_or(&form.question);
    let question_style = if previous_prompt.is_some() {
        theme.dim
    } else {
        Style::new()
    };
    rows.push(
        Span::raw("    "),
        question_text,
        Span::styled(
            focus.hint(Section::Question, "Tab to add a question", "Ask a question"),
            theme.dim,
        ),
        question_style,
        focus
            .at(Row::Question)
            .map(|cursor| if previous_prompt.is_some() { 0 } else { cursor }),
    );
}

/// A choice's options or a score's levels, numbered, under a label that says
/// when the previous question's are reused. Rows a list still needs show their
/// placeholders.
fn answers_section(rows: &mut Rows, app: &App, focus: Focus, theme: &Theme) {
    let form = &app.form;
    let active = focus.active(Section::Options);
    let (name, item, tab) = match form.kind {
        Kind::Score => ("Levels · lowest first", "Add a level", "Tab to add levels"),
        _ => ("Options", "Add an option", "Tab to add options"),
    };
    let mut label = vec![
        Span::raw("  "),
        Span::styled(name, theme.kind(form.kind, active)),
    ];
    let inherited = app.inherited_answers();
    if let Some(previous) = inherited {
        let count = match &previous.question.answer_space {
            nouli_core::decision::AnswerSpace::Choice { options } => options.len(),
            nouli_core::decision::AnswerSpace::Score { levels } => levels.len(),
            nouli_core::decision::AnswerSpace::Noul => 0,
        };
        label.push(Span::styled(
            format!(" · using previous {count}"),
            theme.dim,
        ));
    }
    rows.lines.push(Line::from(label));
    for (index, option) in form.options.iter().enumerate() {
        let gutter = Span::styled(format!("{:>3} ", index + 1), theme.dim);
        rows.push(
            gutter,
            option,
            Span::styled(item, theme.dim),
            Style::new(),
            focus.at(Row::Option(index)),
        );
    }
    if inherited.is_some() {
        return;
    }
    if active {
        for index in form.options.len()..MINIMUM_ANSWERS {
            let gutter = Span::styled(format!("{:>3} ", index + 1), theme.dim);
            let placeholder = Span::styled(item, theme.dim);
            rows.push(gutter, "", placeholder, Style::new(), None);
        }
    } else if form.options.is_empty() {
        let hint = focus.hint(Section::Options, tab, item);
        let placeholder = Span::styled(hint, theme.dim);
        let gutter = Span::styled(format!("{:>3} ", 1), theme.dim);
        rows.push(gutter, "", placeholder, Style::new(), None);
    }
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
            assert!(!screen.contains("Enter to send"), "{screen}");
            assert!(
                screen.contains("Which destination suits this trip?"),
                "{screen}"
            );
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
    fn the_input_row_stands_where_it_is_among_the_context_rows() {
        let settings = crate::settings::Settings::for_test(std::env::temp_dir());
        let mut app = app(&settings);
        app.repl.queue.facts = vec!["one".into(), "three".into()];
        app.form.input = "two".into();
        app.form.rows_below = 1;
        app.form.cursor = 3;
        let screen = shown(&mut app, 140, 20).join("\n");
        let at = |text: &str| screen.find(text).unwrap();
        assert!(at("  + one") < at("  › two"), "{screen}");
        assert!(at("  › two") < at("  + three"), "{screen}");
    }

    #[test]
    fn no_hint_promises_tab_while_something_else_takes_it() {
        let settings = crate::settings::Settings::for_test(std::env::temp_dir());
        let mut app = app(&settings);
        app.form.input = "/".into();
        app.form.cursor = 1;
        let screen = shown(&mut app, 140, 24).join("\n");
        assert!(screen.contains("    Ask a question"), "{screen}");
        assert!(!screen.contains("Tab to add"), "{screen}");
        app.form.input.clear();
        app.form.cursor = 0;
        app.sessions = Some(vec![]);
        assert!(!shown(&mut app, 140, 24).join("\n").contains("Tab to add"));
        app.sessions = None;
        let theme = Theme::new(false);
        let (lines, _) = composer(&app, 140, true, &theme);
        let text: String = lines.iter().map(ToString::to_string).collect();
        assert!(
            !text.contains("Tab to add"),
            "a running turn takes Tab: {text}"
        );
    }

    fn rule_of(screen: &str) -> String {
        screen
            .lines()
            .find(|line| line.contains("── Question"))
            .unwrap()
            .to_owned()
    }

    #[test]
    fn hint_text_says_how_to_reach_the_box_tab_goes_to() {
        let settings = crate::settings::Settings::for_test(std::env::temp_dir());
        let mut app = app(&settings);
        let screen = shown(&mut app, 140, 20).join("\n");
        assert!(
            screen.contains("  › Add context or drop a file"),
            "{screen}"
        );
        assert!(screen.contains("    Tab to add a question"), "{screen}");
        app.form.row = Row::Question;
        let screen = shown(&mut app, 140, 20).join("\n");
        assert!(screen.contains("    Tab to add context"), "{screen}");
        assert!(screen.contains("    Ask a question"), "{screen}");
        // With a list, Tab from Question reaches the list and not Context.
        app.form.kind = Kind::Choice;
        let screen = shown(&mut app, 140, 20).join("\n");
        assert!(screen.contains("  1 Tab to add options"), "{screen}");
        assert!(
            screen.contains("    Add context or drop a file"),
            "{screen}"
        );
        assert!(!screen.contains("Tab to add context"), "{screen}");
        app.form.kind = Kind::Score;
        let screen = shown(&mut app, 140, 20).join("\n");
        assert!(screen.contains("  1 Tab to add levels"), "{screen}");
        // Filled boxes keep their text, and the hint goes.
        app.form.question = "Will it rain?".into();
        app.form.row = Row::Input;
        let screen = shown(&mut app, 140, 20).join("\n");
        assert!(!screen.contains("Tab to add a question"), "{screen}");
    }

    #[test]
    fn all_the_answer_types_show_only_while_question_is_active() {
        let settings = crate::settings::Settings::for_test(std::env::temp_dir());
        let mut app = app(&settings);
        let screen = shown(&mut app, 140, 20).join("\n");
        assert!(rule_of(&screen).contains("── [Yes / No] "), "{screen}");
        assert!(!screen.contains("Choose one"), "{screen}");
        app.form.row = Row::Question;
        let screen = shown(&mut app, 140, 20).join("\n");
        assert!(
            rule_of(&screen).contains("── [Yes / No] Choose one Score"),
            "{screen}"
        );
        app.form.kind = Kind::Score;
        app.form.row = Row::Input;
        let screen = shown(&mut app, 140, 20).join("\n");
        assert!(rule_of(&screen).contains("── [Score] "), "{screen}");
    }

    #[test]
    fn headings_stand_out_only_for_the_active_box() {
        let theme = Theme::new(true);
        let settings = crate::settings::Settings::for_test(std::env::temp_dir());
        let mut app = app(&settings);
        let style_of = |app: &mut crate::repl::tui::App, text: &str| {
            let (lines, _) = composer(app, 140, false, &theme);
            lines
                .iter()
                .flat_map(|line| &line.spans)
                .find(|span| span.content == text)
                .unwrap()
                .style
        };
        assert_eq!(style_of(&mut app, "Context"), theme.accent);
        assert_eq!(
            style_of(&mut app, "Question"),
            theme.kind(Kind::Noul, false)
        );
        app.form.row = Row::Question;
        assert_eq!(style_of(&mut app, "Context"), theme.dim);
        assert_eq!(style_of(&mut app, "Question"), theme.kind(Kind::Noul, true));
    }

    #[test]
    fn three_or_more_context_rows_fold_away_from_context() {
        let settings = crate::settings::Settings::for_test(std::env::temp_dir());
        let mut app = app(&settings);
        app.repl.queue.facts = vec!["one".into(), "two".into()];
        app.form.row = Row::Question;
        let screen = shown(&mut app, 140, 20).join("\n");
        assert!(screen.contains("  + one"), "two rows stay: {screen}");
        app.repl.queue.facts.push("three".into());
        let screen = shown(&mut app, 140, 20).join("\n");
        assert!(screen.contains("── Context · 3 added"), "{screen}");
        assert!(!screen.contains("  + one"), "{screen}");
        app.form.row = Row::Input;
        let screen = shown(&mut app, 140, 20).join("\n");
        assert!(screen.contains("  + one"), "back in Context: {screen}");
        app.form.row = Row::Question;
        app.form.input = "still typing".into();
        let screen = shown(&mut app, 140, 20).join("\n");
        assert!(screen.contains("still typing"), "{screen}");
    }

    #[test]
    fn lists_show_two_rows_while_active_and_the_decisions_pane_waits_when_narrow() {
        let settings = crate::settings::Settings::for_test(std::env::temp_dir());
        let mut app = app(&settings);
        app.form.kind = Kind::Choice;
        let screen = shown(&mut app, 140, 20).join("\n");
        assert!(screen.contains("  Options"), "{screen}");
        assert!(!screen.contains("── Options"), "{screen}");
        assert!(screen.contains("  1 Add an option"), "{screen}");
        assert!(!screen.contains("  2 Add an option"), "{screen}");
        app.form.options = vec![String::new()];
        app.form.row = Row::Option(0);
        let screen = shown(&mut app, 140, 20).join("\n");
        assert!(screen.contains("  2 Add an option"), "{screen}");
        app.form.kind = Kind::Score;
        app.form.options = vec!["low".into(), "high".into()];
        app.form.row = Row::Option(1);
        let screen = shown(&mut app, 140, 20).join("\n");
        assert!(screen.contains("  Levels · lowest first"), "{screen}");
        assert!(screen.contains("  2 high"), "{screen}");
        app.option_enter = true;
        assert!(
            shown(&mut app, 140, 20)
                .join("\n")
                .contains("Press Enter again to send")
        );
        assert!(
            shown(&mut app, 140, 20)
                .join("\n")
                .contains("│ decisions · Ctrl-G to resize")
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
    fn narrow_forms_keep_only_the_chosen_answer_type() {
        let settings = crate::settings::Settings::for_test(std::env::temp_dir());
        let mut app = app(&settings);
        app.form.row = Row::Question;
        let screen = shown(&mut app, 40, 20).join("\n");
        assert!(screen.contains("[Yes / No] ⌄"), "{screen}");
        assert!(!screen.contains("Choose one"), "{screen}");
        assert!(
            shown(&mut app, 60, 20)
                .join("\n")
                .contains("Choose one Score"),
            "all three fit"
        );
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
