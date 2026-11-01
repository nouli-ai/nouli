//! Saved-answer and file reference menus and previews.
use super::{App, chrome::Theme, wrap::wrap_line};
use crate::{render, repl::tui::form::Kind};
use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    style::Style,
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Padding, Paragraph},
};
fn reference_kind(entry: &nouli_core::storage::decisions::SavedDecision) -> Kind {
    Kind::from(&entry.question.answer_space)
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
    let answer = render::clean(&render::image_labels(
        &crate::repl::tui::references::answer(entry),
    ));
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

pub(super) fn reference_menu(app: &App, theme: &Theme) -> Vec<Line<'static>> {
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
        .take(crate::repl::tui::references::COMPACT)
        .enumerate()
    {
        rows.extend(reference_result(app, index, i == picker.choice, theme));
    }
    let browse = picker.choice
        == picker
            .matches
            .len()
            .min(crate::repl::tui::references::COMPACT);
    rows.push(Line::from(Span::styled(
        format!(
            " {} Browse history / files…",
            if browse { "›" } else { " " }
        ),
        if browse { theme.accent } else { theme.dim },
    )));
    rows
}

pub(super) fn draw_references(frame: &mut Frame, area: Rect, app: &mut App, theme: &Theme) {
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
fn inspected_reference(binding: &crate::repl::tui::references::Binding) -> Vec<Line<'static>> {
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
                &crate::repl::tui::references::describe_answer(question, outcome),
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
    picker: &crate::repl::tui::references::Picker,
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
            render::clean(&render::image_labels(
                &crate::repl::tui::references::answer(entry),
            )),
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

#[cfg(test)]
mod tests {
    use crate::repl::tui::draw::tests::{app, shown};
    use crate::repl::tui::form::Row;

    #[test]
    fn reference_suggestions_stay_visible_below_a_scrolled_field() {
        use nouli_core::{
            history::FinalizedTurn,
            storage::decisions::{DecisionSource, SavedDecision},
        };
        let settings = crate::settings::Settings::for_test(std::env::temp_dir());
        let mut app = app(&settings);
        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../../../../../fixtures/contracts/history.json"
        ))
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
        app.references.picker = Some(crate::repl::tui::references::Picker {
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
        app.references.preview = Some(crate::repl::tui::references::Preview {
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
}
