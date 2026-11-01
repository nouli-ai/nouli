//! Drawing the view: a header, the transcript over the form, the decisions
//! pane beside them when both fit, and a status line.
use super::{App, Graph, Running, form::Kind, form::Row, form::Section};
use crate::render;
use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Padding, Paragraph},
};
use std::ops::Range;
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

/// Turn results are laid out in 74 columns; the transcript keeps room for them.
const TRANSCRIPT_COLUMNS: u16 = 76;
/// A decision card is 36 columns, so the pane holds one with its border and
/// the column that keeps the two apart.
const GRAPH_MIN: u16 = 40;
/// Two cards side by side, as the blend of two sessions draws them.
const GRAPH_MAX: u16 = 80;
/// Rows a list over the transcript shows at once.
const LIST_ROWS: usize = 8;
/// Rows the transcript keeps however tall the form grows.
const TRANSCRIPT_ROWS: u16 = 3;
const SPINNER: [char; 10] = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];
/// The status line's key hints, most useful first.
const HINTS: [&str; 4] = ["Tab question", "@ reference", "↑↓ fields", "/help"];

/// The view's styles. Without color, emphasis is bold alone.
struct Theme {
    accent: Style,
    dim: Style,
    error: Style,
    color: bool,
}

impl Theme {
    fn new(color: bool) -> Self {
        let dim = Style::new().add_modifier(Modifier::DIM);
        let bold = Style::new().add_modifier(Modifier::BOLD);
        if color {
            Self {
                accent: bold,
                dim,
                error: Style::new().fg(Color::Red),
                color,
            }
        } else {
            Self {
                accent: bold,
                dim,
                error: bold,
                color,
            }
        }
    }

    fn kind(&self, kind: Kind, selected: bool) -> Style {
        let mut style = Style::new();
        if self.color {
            style = style.fg(kind.tone().color());
        }
        style.add_modifier(if selected {
            Modifier::BOLD
        } else {
            Modifier::DIM
        })
    }
}

/// The decisions pane's width beside the rest, or `None` when the
/// transcript would be too narrow for both.
fn graph_width(total: u16) -> Option<u16> {
    let room = total.checked_sub(TRANSCRIPT_COLUMNS)?;
    (room >= GRAPH_MIN).then(|| room.min((total / 5 * 2).clamp(GRAPH_MIN, GRAPH_MAX)))
}

fn pane_width(total: u16, preferred: Option<u16>) -> Option<u16> {
    if let Some(width) = preferred
        && total >= 80
    {
        Some(width.clamp(GRAPH_MIN, total - 40))
    } else {
        graph_width(total)
    }
}

pub(super) fn screen(frame: &mut Frame, app: &mut App, running: Option<&Running>) {
    let theme = Theme::new(app.repl.paint.0);
    let [header, body, model, footer] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(0),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .areas(frame.area());
    // Without room for both, the pane shows only while it has the keys.
    app.graph.body = body;
    let (left, graph) = match pane_width(body.width, app.graph.preferred_width) {
        Some(width) => {
            let [left, right] =
                Layout::horizontal([Constraint::Min(0), Constraint::Length(width)]).areas(body);
            (Some(left), Some(right))
        }
        None if app.graph_focused => (None, Some(body)),
        None => (Some(body), None),
    };
    app.graph.divider = if left.is_some() {
        graph.map(|area| Rect { width: 1, ..area })
    } else {
        None
    };
    draw_header(frame, header, app, &theme);
    if let Some(left) = left {
        draw_left(frame, left, app, &theme);
    }
    if let Some(graph) = graph {
        draw_graph(frame, graph, app, &theme);
    }
    draw_model(frame, model, app, running, &theme);
    draw_footer(frame, footer, app, running, &theme);
}

fn draw_header(frame: &mut Frame, area: Rect, app: &mut App, theme: &Theme) {
    let action = if app.selection.active {
        " resume · Esc/F6 "
    } else if app.graph_focused {
        " select text · F6 "
    } else {
        " hold view · F6 "
    };
    let width = u16::try_from(action.width()).unwrap_or(u16::MAX);
    let action_area = (area.width >= width + 20).then(|| Rect {
        x: area.right() - width,
        width,
        ..area
    });
    app.selection.button = action_area.filter(|_| app.captures_mouse());
    let title_area = Rect {
        width: area
            .width
            .saturating_sub(action_area.map_or(0, |area| area.width)),
        ..area
    };
    if let Some(button) = action_area {
        frame.render_widget(Paragraph::new(Span::styled(action, theme.accent)), button);
    }
    let label = render::clean(&app.repl.session.label);
    let title = Line::from(vec![
        Span::styled(" nouli", theme.accent),
        Span::styled(
            format!(" {} · {label}", env!("CARGO_PKG_VERSION")),
            theme.dim,
        ),
    ]);
    frame.render_widget(Paragraph::new(title), title_area);
}

fn draw_model(frame: &mut Frame, area: Rect, app: &App, running: Option<&Running>, theme: &Theme) {
    let selection = running.map_or(&app.selected_model, |turn| &turn.model);
    let model = match (&selection.provider, &selection.model) {
        (Some(provider), Some(model)) => {
            format!("{}/{}", render::clean(provider), render::clean(model))
        }
        _ => "Model not configured".into(),
    };
    let activity = match running {
        Some(turn) if turn.cancel.is_cancelled() => "Cancelling".into(),
        Some(turn) => format!("Asking · {}s", turn.started.elapsed().as_secs()),
        None if !app.form.is_empty() || !app.repl.queue.is_empty() => "Draft".into(),
        None => "Ready".into(),
    };
    let model = if running.is_none() {
        app.model.as_deref().unwrap_or(&model)
    } else {
        &model
    };
    let spans = vec![
        Span::styled(format!(" {model}"), theme.accent),
        Span::styled(format!(" · {activity}"), theme.dim),
    ];
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn draw_left(frame: &mut Frame, area: Rect, app: &mut App, theme: &Theme) {
    let (lines, cursor) = composer(app, usize::from(area.width), theme);
    let tall = u16::try_from(lines.len()).unwrap_or(u16::MAX);
    let reserved = if app
        .references
        .picker
        .as_ref()
        .is_some_and(|picker| picker.expanded)
        || app.references.inspected.is_some()
    {
        (area.height / 2).max(TRANSCRIPT_ROWS)
    } else {
        TRANSCRIPT_ROWS
    };
    let height = tall
        .min(area.height.saturating_sub(reserved))
        .max(1)
        .min(area.height);
    let [top, bottom] =
        Layout::vertical([Constraint::Min(0), Constraint::Length(height)]).areas(area);
    app.page = usize::from(top.height).max(1);
    app.transcript_width = usize::from(top.width).max(1);
    // Short terminals skip the bee and keep all the space for output.
    let greeter = if top.height < super::greeter::ROWS {
        Vec::new()
    } else {
        super::greeter::lines(
            top.width,
            theme.color,
            !app.form.is_empty() || !app.repl.queue.is_empty(),
        )
    };
    let rows = transcript(
        &greeter,
        &app.transcript,
        usize::from(top.width),
        usize::from(top.height),
        &mut app.scroll,
    );
    frame.render_widget(Paragraph::new(rows), top);
    if app.references.picker.as_ref().is_some_and(|p| p.expanded)
        || app.references.inspected.is_some()
    {
        draw_references(frame, top, app, theme);
    }
    let list = list(app, usize::from(top.height), theme);
    if !list.is_empty() {
        let height = u16::try_from(list.len())
            .unwrap_or(u16::MAX)
            .min(top.height);
        let area = Rect {
            y: top.y + top.height - height,
            height,
            ..top
        };
        frame.render_widget(Clear, area);
        frame.render_widget(Paragraph::new(list), area);
    }
    let shown = usize::from(bottom.height);
    if shown == 0 {
        return;
    }
    // The form scrolls when it is taller than its space, keeping the cursor in view.
    let mut first = app.composer_top.min(lines.len().saturating_sub(shown));
    if let Some((_, row)) = cursor {
        if row < first {
            first = row;
        } else if row >= first + shown {
            first = row + 1 - shown;
        }
        if app.references.picker.as_ref().is_some_and(|p| !p.expanded) {
            let menu = reference_menu(app, theme)
                .len()
                .min(shown.saturating_sub(1));
            first = first.max((row + menu + 1).saturating_sub(shown));
        }
    }
    app.composer_top = first;
    let window: Vec<_> = lines.into_iter().skip(first).take(shown).collect();
    frame.render_widget(Paragraph::new(window), bottom);
    if let Some((column, row)) = cursor {
        let x = u16::try_from(column).map_or(u16::MAX, |column| bottom.x.saturating_add(column));
        let y = u16::try_from(row - first).map_or(u16::MAX, |row| bottom.y.saturating_add(row));
        frame.set_cursor_position((x.min(bottom.right().saturating_sub(1)), y));
    }
}

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

/// The open list, drawn over the bottom of the transcript: the sessions to
/// resume, or the commands that match the input row. The chosen row stays in
/// view.
fn list(app: &App, height: usize, theme: &Theme) -> Vec<Line<'static>> {
    let rows: Vec<String> = match &app.sessions {
        Some(entries) => entries
            .iter()
            .map(|entry| {
                let turns = if entry.turns == 1 { "turn" } else { "turns" };
                format!(
                    "{}: {}  · {} {turns}",
                    render::clean(&entry.label),
                    render::clean(&render::image_labels(&entry.first)),
                    entry.turns
                )
            })
            .collect(),
        None => app
            .commands_shown()
            .into_iter()
            .map(|(usage, meaning)| format!("{usage:<22}{meaning}"))
            .collect(),
    };
    let Some(last) = rows.len().checked_sub(1) else {
        return vec![];
    };
    let chosen = app.choice.min(last);
    let window = LIST_ROWS.min(height).max(1);
    let first = chosen.saturating_sub(window - 1);
    rows.into_iter()
        .enumerate()
        .skip(first)
        .take(window)
        .map(|(index, row)| {
            if index == chosen {
                Line::from(Span::styled(format!(" › {row}"), theme.accent))
            } else {
                Line::from(Span::styled(format!("   {row}"), theme.dim))
            }
        })
        .collect()
}

fn reference_kind(entry: &nouli_core::storage::decisions::SavedDecision) -> Kind {
    match entry.question.answer_space {
        nouli_core::decision::AnswerSpace::Noul => Kind::Noul,
        nouli_core::decision::AnswerSpace::Choice { .. } => Kind::Choice,
        nouli_core::decision::AnswerSpace::Score { .. } => Kind::Score,
    }
}

fn reference_row(
    app: &App,
    entry: &nouli_core::storage::decisions::SavedDecision,
    selected: bool,
    theme: &Theme,
) -> Vec<Line<'static>> {
    let kind = reference_kind(entry);
    let prompt = render::clean(&render::image_labels(&entry.question.prompt))
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    let latest = app
        .latest_question
        .as_ref()
        .is_some_and(|q| q.turn == entry.source.turn && q.question.id == entry.source.question);
    let answer = render::clean(&render::image_labels(&super::references::answer(entry)));
    let mut lines = vec![
        Line::from(Span::styled(
            format!(" {} {prompt}", if selected { "›" } else { " " }),
            theme.kind(kind, selected),
        )),
        Line::from(vec![
            Span::styled(format!("   {answer}"), theme.kind(kind, selected)),
            Span::styled(
                format!(
                    " · {} / {}{}",
                    render::clean(&entry.session_name),
                    entry.number,
                    if latest { " · included" } else { "" }
                ),
                theme.dim,
            ),
        ]),
    ];
    if app
        .references
        .picker
        .as_ref()
        .is_some_and(|p| p.query.trim_start().starts_with("q_"))
    {
        lines.push(Line::from(Span::styled(
            format!("   {}", render::clean(&entry.source.question)),
            theme.dim,
        )));
    }
    if let Some(excerpt) = app
        .references
        .picker
        .as_ref()
        .and_then(|picker| entry.excerpt(&picker.query))
    {
        lines.push(Line::from(Span::styled(
            format!("   {}", render::clean(&render::image_labels(&excerpt))),
            theme.dim,
        )));
    }
    lines
}

fn reference_result(app: &App, index: usize, selected: bool, theme: &Theme) -> Vec<Line<'static>> {
    if let Some(file) = app.references.file(index) {
        vec![
            Line::from(Span::styled(
                format!(
                    " {} {}",
                    if selected { "›" } else { " " },
                    app.references.file_label(file)
                ),
                if selected { theme.accent } else { Style::new() },
            )),
            Line::from(Span::styled("   Local file", theme.dim)),
        ]
    } else if let Some(entry) = app.references.catalog.get(index) {
        reference_row(app, entry, selected, theme)
    } else {
        vec![]
    }
}

fn reference_menu(app: &App, theme: &Theme) -> Vec<Line<'static>> {
    let Some(picker) = &app.references.picker else {
        return vec![];
    };
    if picker.expanded {
        return vec![];
    }
    let mut rows = vec![Line::from(Span::styled(
        format!(
            "  Saved answers · all local sessions + files · {} matches",
            picker.matches.len()
        ),
        theme.dim,
    ))];
    if picker.matches.is_empty() {
        rows.push(Line::from(Span::styled(
            "  No saved question found · no local file matches · Esc keeps the text",
            theme.dim,
        )));
    }
    for (i, &index) in picker
        .matches
        .iter()
        .take(super::references::COMPACT)
        .enumerate()
    {
        rows.extend(reference_result(app, index, i == picker.choice, theme));
    }
    let browse = picker.choice == picker.matches.len().min(super::references::COMPACT);
    rows.push(Line::from(Span::styled(
        format!(
            " {} Browse history / files…",
            if browse { "›" } else { " " }
        ),
        if browse { theme.accent } else { theme.dim },
    )));
    rows
}

fn draw_references(frame: &mut Frame, area: Rect, app: &mut App, theme: &Theme) {
    if area.height == 0 || area.width == 0 {
        return;
    }
    frame.render_widget(Clear, area);
    let title = if app.references.inspected.is_some() {
        " Reference source · Ctrl-O · Esc closes "
    } else {
        " Saved answers · all local sessions + files "
    };
    let block = Block::new()
        .borders(Borders::ALL)
        .border_style(theme.dim)
        .title(title);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if let Some(index) = app.references.inspected {
        if let Some(binding) = app.references.bindings.get(index) {
            frame.render_widget(
                Paragraph::new(inspected_reference(binding))
                    .wrap(ratatui::widgets::Wrap { trim: false }),
                inner,
            );
        }
        return;
    }
    let Some(picker) = &app.references.picker else {
        return;
    };
    let wide = inner.width >= 80;
    let (list_area, preview_area) = reference_areas(inner);
    frame.render_widget(
        Paragraph::new(reference_list(app, picker, list_area.height, theme)),
        list_area,
    );
    let Some(preview_area) = preview_area else {
        return;
    };
    if let Some(file) = app.references.selected_file() {
        frame.render_widget(
            Paragraph::new(file_preview(app, file, theme))
                .wrap(ratatui::widgets::Wrap { trim: false }),
            preview_area,
        );
    }
    if let Some(entry) = app.references.selected() {
        let lines = saved_answer_preview(app, entry, theme);
        draw_preview(frame, preview_area, wide, &lines, app, theme);
    }
}

/// What a bound reference in the form points at, as Ctrl-O shows it.
fn inspected_reference(binding: &super::references::Binding) -> Vec<Line<'static>> {
    let snapshot = &binding.snapshot;
    let source = &snapshot.source;
    let mut lines = vec![
        Line::from(render::clean(&binding.label)),
        Line::from(format!("Question: {}", render::clean(&source.question))),
        Line::from(format!("Turn: {}", source.turn)),
        Line::from(format!("Session: {}", source.session)),
        Line::from(format!(
            "Model: {}",
            render::clean(
                snapshot
                    .provider
                    .as_ref()
                    .map_or("", |provider| provider.model.as_str())
            )
        )),
        Line::from("Backspace in the form removes the reference."),
    ];
    if let (Some(question), Some(outcome)) = (&snapshot.question, &snapshot.outcome) {
        lines.insert(
            1,
            Line::from(render::clean(&render::image_labels(&question.prompt))),
        );
        lines.insert(
            2,
            Line::from(render::clean(&render::image_labels(
                &super::references::describe_answer(question, outcome),
            ))),
        );
    }
    let names = snapshot
        .images
        .iter()
        .flatten()
        .filter_map(|image| image.record.as_image().ok().flatten())
        .map(|image| render::clean(&image.name))
        .collect::<Vec<_>>();
    if !names.is_empty() {
        lines.push(Line::from(format!("Images: {}", names.join(", "))));
    }
    lines
}

/// The browser's result list and, when there is room, its preview beside or
/// below it.
fn reference_areas(inner: Rect) -> (Rect, Option<Rect>) {
    if inner.width >= 80 {
        let [left, right] =
            Layout::horizontal([Constraint::Percentage(50), Constraint::Percentage(50)])
                .areas(inner);
        (left, Some(right))
    } else if inner.height >= 6 {
        let [list, preview] = Layout::vertical([
            Constraint::Length((inner.height / 3).clamp(2, 6)),
            Constraint::Min(1),
        ])
        .areas(inner);
        (list, Some(preview))
    } else {
        (inner, None)
    }
}

/// The page of results around the choice that fits `height` rows.
fn reference_list(
    app: &App,
    picker: &super::references::Picker,
    height: u16,
    theme: &Theme,
) -> Vec<Line<'static>> {
    let size = (usize::from(height) / 4).max(1);
    let first = picker.choice.saturating_sub(size - 1);
    let mut lines = Vec::new();
    if picker.matches.is_empty() {
        lines.push(Line::from(
            "No saved question found · Esc returns to the draft",
        ));
    }
    for (i, index) in picker.matches.iter().enumerate().skip(first).take(size) {
        lines.extend(reference_result(app, *index, i == picker.choice, theme));
        lines.push(Line::from(""));
    }
    lines
}

fn file_preview(app: &App, file: &std::path::Path, theme: &Theme) -> Vec<Line<'static>> {
    vec![
        Line::from(Span::styled(app.references.file_label(file), theme.accent)),
        Line::from(""),
        Line::from("File in the launch folder"),
        Line::from("Enter adds its contents to this field."),
    ]
}

/// A saved answer's question, answer, distribution, saved input state, and
/// provenance.
fn saved_answer_preview(
    app: &App,
    entry: &nouli_core::storage::decisions::SavedDecision,
    theme: &Theme,
) -> Vec<Line<'static>> {
    let kind = reference_kind(entry);
    let mut lines = vec![
        Line::from(Span::styled(
            render::clean(&render::image_labels(&entry.question.prompt)),
            theme.kind(kind, true),
        )),
        Line::from(""),
        Line::from(Span::styled(
            render::clean(&render::image_labels(&super::references::answer(entry))),
            theme.kind(kind, true),
        )),
    ];
    lines.extend(answer_distribution(entry));
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "Saved input state · PgUp/PgDn scroll",
        theme.accent,
    )));
    if let Some(preview) = app
        .references
        .preview
        .as_ref()
        .filter(|preview| preview.source == entry.source)
    {
        for line in &preview.lines {
            lines.push(Line::from(render::clean(&render::image_labels(line))));
        }
    } else {
        lines.push(Line::from("Loading saved state…"));
    }
    lines.extend([
        Line::from(""),
        Line::from(format!(
            "Question {}",
            render::clean(&entry.source.question)
        )),
        Line::from(format!("Turn {}", entry.source.turn)),
        Line::from(format!(
            "{} · {}",
            render::clean(&entry.provider.provider),
            render::clean(&entry.provider.model)
        )),
    ]);
    lines
}

/// One line per option or level with its probability; none for a yes/no
/// question or an outcome without an answer.
fn answer_distribution(
    entry: &nouli_core::storage::decisions::SavedDecision,
) -> Vec<Line<'static>> {
    use nouli_core::{
        decision::AnswerSpace,
        outcome::{Answer, QuestionOutcome},
    };
    let mut lines = Vec::new();
    match &entry.outcome {
        QuestionOutcome::Answered {
            answer: answer @ Answer::Choice { .. },
            ..
        } => {
            if let AnswerSpace::Choice { options } = &entry.question.answer_space {
                for option in options {
                    if let Some(probability) = answer.probability_of(&option.id) {
                        lines.push(Line::from(format!(
                            "{}  {probability:.2}",
                            render::image_labels(&render::clean(&option.description)),
                        )));
                    }
                }
            }
        }
        QuestionOutcome::Answered {
            answer: Answer::Score { probabilities, .. },
            ..
        } => {
            if let AnswerSpace::Score { levels } = &entry.question.answer_space {
                for (level, p) in levels.iter().zip(probabilities) {
                    let label = level.description();
                    lines.push(Line::from(format!(
                        "{}  {p:.2}",
                        render::image_labels(&render::clean(label))
                    )));
                }
            }
        }
        _ => {}
    }
    lines
}

/// Draws the preview wrapped inside its border, scrolled to the browser's
/// preview offset. Stores the page size PgUp/PgDn move by and clamps the
/// stored offset to the wrapped content.
fn draw_preview(
    frame: &mut Frame,
    area: Rect,
    wide: bool,
    lines: &[Line<'static>],
    app: &mut App,
    theme: &Theme,
) {
    let block = Block::new()
        .borders(if wide { Borders::LEFT } else { Borders::TOP })
        .padding(Padding::left(1))
        .border_style(theme.dim);
    let visible = block.inner(area);
    app.references.preview_page = usize::from(visible.height).saturating_sub(1).max(1);
    let wrapped = lines
        .iter()
        .flat_map(|line| wrap_line(line, usize::from(visible.width).max(1)))
        .collect::<Vec<_>>();
    app.references.preview_scroll = app
        .references
        .preview_scroll
        .min(wrapped.len().saturating_sub(usize::from(visible.height)));
    let lines = wrapped
        .into_iter()
        .skip(app.references.preview_scroll)
        .take(usize::from(visible.height))
        .collect::<Vec<_>>();
    frame.render_widget(Paragraph::new(lines).block(block), area);
}

type Composer = (Vec<Line<'static>>, Option<(usize, usize)>);

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
fn composer(app: &App, width: usize, theme: &Theme) -> Composer {
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

/// The transcript's last rows, `scroll` rows up from its end, wrapped to
/// `width`, with `header` as its first rows so it scrolls away with them.
/// Only the lines it shows are wrapped, however long it grows; `scroll`
/// stops at the header's start.
fn transcript(
    header: &[Line<'static>],
    lines: &[Line<'static>],
    width: usize,
    height: usize,
    scroll: &mut usize,
) -> Vec<Line<'static>> {
    let mut rows = Vec::new();
    for line in header.iter().chain(lines).rev() {
        if rows.len() >= height + *scroll {
            break;
        }
        rows.extend(wrap_line(line, width).into_iter().rev());
    }
    *scroll = (*scroll).min(rows.len().saturating_sub(height));
    let mut shown: Vec<_> = rows.into_iter().skip(*scroll).take(height).collect();
    shown.reverse();
    shown
}

/// A styled line cut into rows of at most `width` columns.
fn wrap_line(line: &Line<'static>, width: usize) -> Vec<Line<'static>> {
    if line.width() <= width {
        return vec![line.clone()];
    }
    let text: String = line
        .spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect();
    let mut spans = Vec::with_capacity(line.spans.len());
    let mut start = 0;
    for span in &line.spans {
        let end = start + span.content.len();
        spans.push((start..end, span.style));
        start = end;
    }
    wrap(&text, width)
        .into_iter()
        .map(|row| {
            let parts: Vec<_> = spans
                .iter()
                .filter_map(|(range, style)| {
                    let (from, to) = (range.start.max(row.start), range.end.min(row.end));
                    (from < to).then(|| Span::styled(text[from..to].to_owned(), *style))
                })
                .collect();
            Line::from(parts).style(line.style)
        })
        .collect()
}

/// Byte ranges of `text` for rows of at most `width` columns, breaking after
/// a space where one fits and inside a word longer than a row. The ranges
/// cover the text without gaps, so every cursor offset falls in one of them.
fn wrap(text: &str, width: usize) -> Vec<Range<usize>> {
    let width = width.max(1);
    let mut rows = Vec::new();
    let (mut start, mut used, mut space) = (0, 0, None);
    for (at, grapheme) in text.grapheme_indices(true) {
        if grapheme == "\n" {
            rows.push(start..at);
            start = at + 1;
            used = 0;
            space = None;
            continue;
        }
        let columns = grapheme.width();
        if used + columns > width && at > start {
            let end = space.filter(|&after| after > start).unwrap_or(at);
            rows.push(start..end);
            start = end;
            used = text[start..at].width();
            space = None;
            if used + columns > width && at > start {
                rows.push(start..at);
                start = at;
                used = 0;
            }
        }
        used += columns;
        if grapheme.chars().all(char::is_whitespace) {
            space = Some(at + grapheme.len());
        }
    }
    rows.push(start..text.len());
    rows
}

fn draw_graph(frame: &mut Frame, area: Rect, app: &mut App, theme: &Theme) {
    let title = if app.graph_focused {
        theme.accent
    } else {
        Style::new()
    };
    let block = Block::new()
        .borders(Borders::LEFT)
        .border_style(theme.dim)
        .padding(Padding::left(1))
        .title(Line::from(vec![
            Span::styled(" decisions", title),
            Span::styled(
                if app.graph.divider.is_some() && app.graph_focused {
                    " · drag divider · Ctrl-G"
                } else if app.graph.divider.is_some() {
                    " · Ctrl-G to resize"
                } else {
                    " · Ctrl-G"
                },
                theme.dim,
            ),
        ]));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let graph = &mut app.graph;
    graph.page = usize::from(inner.height).max(1);
    let note = if let Some(problem) = &graph.problem {
        Some(format!("cannot read the decisions: {problem}"))
    } else if graph.lines.is_empty() {
        Some("No decisions yet".to_owned())
    } else {
        None
    };
    if let Some(note) = note {
        frame.render_widget(Paragraph::new(Span::styled(note, theme.dim)), inner);
        return;
    }
    let (x, y) = place(graph, usize::from(inner.width), usize::from(inner.height));
    let visible: Vec<_> = graph
        .lines
        .iter()
        .skip(y)
        .take(usize::from(inner.height))
        .cloned()
        .collect();
    let x = u16::try_from(x).unwrap_or(u16::MAX);
    frame.render_widget(Paragraph::new(visible).scroll((0, x)), inner);
}

/// Where the pane's view of the graph starts. Until it is scrolled, the pane
/// shows the end of the graph and the whole latest card where it fits.
fn place(graph: &mut Graph, width: usize, height: usize) -> (usize, usize) {
    if graph.follow {
        let (row, columns) = graph.latest.clone().unwrap_or((graph.lines.len(), 0..0));
        graph.y = graph.lines.len().saturating_sub(height).min(row);
        graph.x = columns.end.saturating_sub(width).min(columns.start);
    }
    let widest = graph.lines.iter().map(Line::width).max().unwrap_or(0);
    graph.y = graph.y.min(graph.lines.len().saturating_sub(height));
    graph.x = graph.x.min(widest.saturating_sub(width));
    (graph.x, graph.y)
}

fn draw_footer(frame: &mut Frame, area: Rect, app: &App, running: Option<&Running>, theme: &Theme) {
    if app.selection.active {
        let hints = fit_hints(
            area.width.into(),
            &["Esc/F6 resume", "drag to select", "copy with terminal"],
        );
        frame.render_widget(Paragraph::new(Span::styled(hints, theme.accent)), area);
        return;
    }
    let line = match (running, &app.status) {
        (Some(turn), _) if turn.cancel.is_cancelled() => {
            Line::from(Span::styled(" cancelling the turn…", theme.dim))
        }
        (Some(turn), _) => {
            let elapsed = turn.started.elapsed();
            let frame = (elapsed.as_millis() / 100) as usize % SPINNER.len();
            Line::from(vec![
                Span::styled(format!(" {} ", SPINNER[frame]), theme.accent),
                Span::styled(
                    format!("asking the model · {}s · Ctrl-C cancels", elapsed.as_secs()),
                    theme.dim,
                ),
            ])
        }
        (None, Some(status)) => {
            let style = if status.error { theme.error } else { theme.dim };
            Line::from(Span::styled(format!(" {}", status.text), style))
        }
        (None, _) if app.references.picker.is_some() => Line::from(Span::styled(
            if app
                .references
                .picker
                .as_ref()
                .is_some_and(|picker| picker.expanded)
            {
                " ↑↓ results · PgUp/PgDn preview · Enter insert · Esc close"
            } else {
                " ↑↓ choose · Enter insert · Esc close · Tab field · Shift-Tab type"
            },
            theme.dim,
        )),
        (None, None) if app.sessions.is_some() => Line::from(Span::styled(
            " ↑↓ choose · Enter resumes the session · Esc closes",
            theme.dim,
        )),
        (None, None) if !app.commands_shown().is_empty() => Line::from(Span::styled(
            " ↑↓ choose · Tab completes · Enter runs · Esc hides",
            theme.dim,
        )),
        (None, None) if app.graph_focused => Line::from(Span::styled(
            if app.graph.divider.is_some() {
                " drag divider to resize · arrows / wheel scroll · Esc returns to form"
            } else {
                " arrows / wheel scroll · Esc returns to form"
            },
            theme.dim,
        )),
        (None, None) => Line::from(Span::styled(
            focused_hints(area.width.into(), app.form.row),
            theme.dim,
        )),
    };
    frame.render_widget(Paragraph::new(line), area);
}

/// As many key hints as fit in `width` columns, leaving out whole hints.
fn hints(width: usize) -> String {
    fit_hints(width, &HINTS)
}

fn focused_hints(width: usize, row: Row) -> String {
    match row {
        Row::Question => fit_hints(
            width,
            &["Tab next field", "@ reference", "↑↓ fields", "/help"],
        ),
        Row::Option(_) => fit_hints(
            width,
            &["Tab next item", "@ reference", "↑↓ fields", "/help"],
        ),
        Row::Fact(_) => fit_hints(
            width,
            &["↑↓ fields", "@ reference", "Tab question", "/help"],
        ),
        Row::Input => hints(width),
    }
}

fn fit_hints(width: usize, hints: &[&str]) -> String {
    let mut shown = String::new();
    for hint in hints {
        let next = if shown.is_empty() {
            format!(" {hint}")
        } else {
            format!("{shown} · {hint}")
        };
        if next.width() > width {
            break;
        }
        shown = next;
    }
    shown
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repl::tui::tests::app;
    use ratatui::{Terminal, backend::TestBackend};
    use std::fmt::Write as _;

    fn shown(app: &mut App, width: u16, height: u16) -> Vec<String> {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal.draw(|frame| screen(frame, app, None)).unwrap();
        let buffer = terminal.backend().buffer();
        (0..height)
            .map(|y| (0..width).map(|x| buffer[(x, y)].symbol()).collect())
            .collect()
    }

    #[test]
    fn copy_controls_and_escape_hint_fit_narrow_and_wide_views() {
        let settings = crate::settings::Settings::for_test(std::env::temp_dir());
        let mut app = app(&settings);
        for width in [24, 40, 80, 160] {
            app.select_text(false);
            app.graph_focused = false;
            let normal = shown(&mut app, width, 24);
            assert!(app.selection.button.is_none());
            if width >= 40 {
                assert!(normal[0].contains("hold view"));
            }
            app.graph_focused = true;
            let focused = shown(&mut app, width, 24);
            if width >= 40 {
                assert!(focused[0].contains("select text"));
                assert!(app.selection.button.is_some());
            } else {
                assert!(app.selection.button.is_none());
            }
            app.select_text(true);
            let held = shown(&mut app, width, 24);
            assert!(held.last().unwrap().contains("Esc/F6 resume"));
        }
    }

    #[test]
    fn the_bee_heads_the_transcript_and_scrolls_away_with_output() {
        let settings = crate::settings::Settings::for_test(std::env::temp_dir());
        let mut app = app(&settings);
        let screen = shown(&mut app, 80, 24).join("\n");
        assert!(screen.contains(">   <"), "{screen}");
        assert!(screen.contains("Let's make a decision."), "{screen}");
        assert!(screen.contains("── Context"), "{screen}");
        app.form.input = "Some context".into();
        assert!(shown(&mut app, 80, 24).join("\n").contains(">   _"));
        let short = shown(&mut app, 80, 12).join("\n");
        assert!(!short.contains(">   _"), "{short}");
        assert!(short.contains("── Context"), "{short}");
        // Output follows the bee, then scrolls it away like any other row.
        app.transcript.push(Line::from("An answer"));
        let screen = shown(&mut app, 80, 24);
        let bee = screen.iter().position(|row| row.contains(">   _"));
        let answer = screen.iter().position(|row| row.contains("An answer"));
        assert!(bee < answer && bee.is_some(), "{screen:?}");
        app.transcript
            .extend((0..20).map(|n| Line::from(format!("Line {n}"))));
        let screen = shown(&mut app, 80, 24).join("\n");
        assert!(screen.contains("Line 19"), "{screen}");
        assert!(!screen.contains(">   _"), "{screen}");
        app.scroll = 1000;
        let screen = shown(&mut app, 80, 24).join("\n");
        assert!(
            screen.contains(">   _"),
            "scrolling back reaches the bee: {screen}"
        );
    }

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
    fn type_controls_keep_distinct_citrus_colors_and_respect_no_color() {
        let theme = Theme::new(true);
        let plain = Theme::new(false);
        for kind in Kind::ALL {
            assert_eq!(theme.kind(kind, true).fg, Some(kind.tone().color()));
            assert_eq!(theme.kind(kind, false).fg, Some(kind.tone().color()));
            assert_eq!(plain.kind(kind, true).fg, None);
            assert!(plain.kind(kind, true).add_modifier.contains(Modifier::BOLD));
        }
        assert_eq!(theme.accent.fg, None, "context and model stay neutral");
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
    fn the_chosen_row_of_a_list_stays_in_view_when_the_transcript_is_short() {
        let settings = crate::settings::Settings::for_test(std::env::temp_dir());
        let mut app = app(&settings);
        for number in 0..8 {
            app.repl.queue.facts.push(format!("fact {number}"));
        }
        app.form.input = "/".into();
        app.form.cursor = 1;
        app.choice = app
            .commands_shown()
            .iter()
            .position(|(usage, _)| *usage == "/clear")
            .unwrap();
        let screen = shown(&mut app, 80, 16).join("\n");
        assert!(screen.contains(" › /clear"), "{screen}");
    }

    #[test]
    fn reference_suggestions_stay_visible_below_a_scrolled_field() {
        use nouli_core::{
            history::FinalizedTurn,
            storage::decisions::{DecisionSource, SavedDecision},
        };
        let settings = crate::settings::Settings::for_test(std::env::temp_dir());
        let mut app = app(&settings);
        let fixture: serde_json::Value =
            serde_json::from_str(include_str!("../../../../fixtures/contracts/history.json"))
                .unwrap();
        let turn: FinalizedTurn = serde_json::from_value(fixture["turns"][0].clone()).unwrap();
        app.references.catalog = vec![SavedDecision {
            source: DecisionSource {
                session: turn.session,
                turn: turn.id,
                question: turn.request.questions[0].id.clone(),
            },
            session_name: "other-project".into(),
            number: 1,
            sequence: 1,
            question: turn.request.questions[0].clone(),
            outcome: turn.outcome.outcomes[0].clone(),
            provider: turn.outcome.provider,
            text: vec![],
        }];
        for number in 0..12 {
            app.repl.queue.facts.push(format!("fact {number}"));
        }
        app.form.question = "@Which".into();
        app.form.row = Row::Question;
        app.form.cursor = 6;
        app.references.picker = Some(super::super::references::Picker {
            row: Row::Question,
            range: 0..6,
            query: "Which".into(),
            matches: vec![0],
            choice: 0,
            expanded: false,
        });
        let screen = shown(&mut app, 80, 20).join("\n");
        assert!(
            screen.contains("Saved answers · all local sessions"),
            "{screen}"
        );
        assert!(screen.contains("Which route?"), "{screen}");
        assert!(screen.contains("Browse history"), "{screen}");
        assert!(screen.contains("other-project"), "{screen}");
        app.status = Some(crate::repl::tui::Status::error(
            "This question was refused. Choose a saved answer.",
        ));
        let screen = shown(&mut app, 80, 20).join("\n");
        assert!(
            screen.contains("This question was refused"),
            "picker hints must not hide errors: {screen}"
        );
        app.status = None;
        app.references.picker.as_mut().unwrap().expanded = true;
        app.references.preview = Some(super::super::references::Preview {
            source: app.references.catalog[0].source.clone(),
            lines: vec![
                "station: Namma metro".into(),
                "budget: 250".into(),
                "delay: null".into(),
            ],
        });
        for width in [140, 80, 60] {
            app.references.preview_scroll = 0;
            let mut found = false;
            for _ in 0..5 {
                let screen = shown(&mut app, width, 24).join("\n");
                found |= screen.contains("Namma metro") && screen.contains("250");
                app.reference_key(&crossterm::event::KeyEvent::new(
                    crossterm::event::KeyCode::PageDown,
                    crossterm::event::KeyModifiers::NONE,
                ));
            }
            assert!(found, "saved state is inspectable at {width} columns");
            assert_eq!(
                app.references.picker.as_ref().unwrap().choice,
                0,
                "preview scrolling never chooses another source"
            );
        }
    }

    #[test]
    fn the_status_line_leaves_out_the_key_hints_that_do_not_fit() {
        let all = hints(200);
        assert_eq!(all, " Tab question · @ reference · ↑↓ fields · /help");
        let narrow = hints(all.width() - 1);
        assert_eq!(narrow, " Tab question · @ reference · ↑↓ fields");
        assert_eq!(hints(13), " Tab question");
        assert_eq!(hints(12), "");
    }

    #[test]
    fn model_status_is_visible_before_answers_and_refreshes_the_next_selection() {
        let mut settings = crate::settings::Settings::for_test(std::env::temp_dir());
        settings.config.default_provider = Some("typesafe".into());
        settings.config.default_model = Some("jev-latest".into());
        let mut app = app(&settings);
        let screen = shown(&mut app, 140, 24).join("\n");
        assert!(screen.contains("typesafe/jev-latest · Ready"), "{screen}");
        app.model = Some("typesafe/jev-1.14.0".into());
        assert!(
            shown(&mut app, 140, 24)
                .join("\n")
                .contains("typesafe/jev-1.14.0 · Ready")
        );
        app.repl.model.model = Some("jev-1.13.0".into());
        app.refresh_model();
        app.form.question = "A draft question?".into();
        let screen = shown(&mut app, 140, 24).join("\n");
        assert!(screen.contains("typesafe/jev-1.13.0 · Draft"), "{screen}");
        assert!(!screen.contains("last served:"), "{screen}");
        assert!(focused_hints(140, Row::Option(0)).contains("Tab next item"));
        assert!(focused_hints(140, Row::Question).contains("Tab next field"));
    }

    #[test]
    fn the_decisions_pane_sits_beside_the_rest_only_when_both_fit() {
        assert_eq!(graph_width(115), None);
        assert_eq!(graph_width(116), Some(40));
        assert_eq!(graph_width(140), Some(56));
        assert_eq!(graph_width(200), Some(80));
        assert_eq!(graph_width(0), None);
        assert_eq!(pane_width(200, Some(120)), Some(120));
        assert_eq!(pane_width(100, Some(120)), Some(60));
        assert_eq!(pane_width(100, Some(1)), Some(40));
        assert_eq!(pane_width(80, Some(120)), Some(40));
        assert_eq!(pane_width(79, Some(120)), None);
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
    fn small_terminals_keep_a_valid_cursor() {
        let settings = crate::settings::Settings::for_test(std::env::temp_dir());
        let mut app = app(&settings);
        for (width, height) in [(1, 1), (4, 4), (24, 12)] {
            shown(&mut app, width, height);
        }
    }

    #[test]
    fn rows_wrap_after_spaces_and_cover_every_byte() {
        let text = "the quick brown fox";
        let rows = wrap(text, 10);
        let parts: Vec<_> = rows.iter().map(|row| &text[row.clone()]).collect();
        assert_eq!(parts, ["the quick ", "brown fox"]);
        let parts: Vec<_> = wrap("abcdefghijkl", 5)
            .into_iter()
            .map(|row| &"abcdefghijkl"[row])
            .collect();
        assert_eq!(parts, ["abcde", "fghij", "kl"]);
        // Wide characters count two columns each.
        assert_eq!(wrap("東京東京", 5).len(), 2);
        assert!(matches!(wrap("", 5).as_slice(), [row] if *row == (0..0)));
        for width in 1..12 {
            let rows = wrap("a bb ccc dddd eeeee", width);
            assert_eq!(rows.first().unwrap().start, 0);
            assert_eq!(rows.last().unwrap().end, 19);
            assert!(rows.windows(2).all(|pair| pair[0].end == pair[1].start));
        }
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

    #[test]
    fn the_transcript_shows_its_end_and_scrolls_back_to_its_start() {
        let lines: Vec<Line<'static>> = (0..10).map(|n| Line::from(n.to_string())).collect();
        let text = |rows: Vec<Line<'static>>| -> Vec<String> {
            rows.iter().map(ToString::to_string).collect()
        };
        let mut scroll = 0;
        assert_eq!(
            text(transcript(&[], &lines, 20, 3, &mut scroll)),
            ["7", "8", "9"]
        );
        scroll = 2;
        assert_eq!(
            text(transcript(&[], &lines, 20, 3, &mut scroll)),
            ["5", "6", "7"]
        );
        scroll = 100;
        assert_eq!(
            text(transcript(&[], &lines, 20, 3, &mut scroll)),
            ["0", "1", "2"]
        );
        assert_eq!(scroll, 7);
        // A header is the transcript's first rows, and scrolling reaches it.
        let header = [Line::from("h0"), Line::from("h1")];
        scroll = 100;
        assert_eq!(
            text(transcript(&header, &lines, 20, 3, &mut scroll)),
            ["h0", "h1", "0"]
        );
        assert_eq!(scroll, 9);
        let long = [Line::from("aaaa bbbb cccc")];
        let mut scroll = 0;
        assert_eq!(
            text(transcript(&[], &long, 5, 3, &mut scroll)),
            ["aaaa ", "bbbb ", "cccc"]
        );
    }

    #[test]
    fn the_pane_keeps_the_latest_card_in_view_until_it_is_scrolled() {
        let mut graph = Graph::default();
        let mut text = String::new();
        for row in 0..30 {
            writeln!(text, "┌─ a{row} ─┐{}┌─ b ─┐", " ".repeat(30)).unwrap();
        }
        writeln!(text, "{}┌─ c [latest] ─┐", " ".repeat(20)).unwrap();
        graph.show(&text, Some((30, 20..36)));
        // The latest card is the last row, and it fits from column 0.
        assert_eq!(place(&mut graph, 40, 10), (0, 21));
        // In a narrower pane, the view moves right just enough to show it.
        assert_eq!(place(&mut graph, 30, 10), (6, 21));
        graph.follow = false;
        graph.y = 0;
        graph.x = 1000;
        assert_eq!(place(&mut graph, 30, 10), (16, 0));
    }
}
