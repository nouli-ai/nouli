//! The dialogs, centred at the bottom of the view.
use super::{App, chrome::Theme};
use crate::repl::tui::{
    commands::SHORTCUTS,
    form::Kind,
    overlay::Overlay,
    status_line::{Editor, Field, Focus, PRESETS, preset_of},
};
use ratatui::{
    Frame,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Clear, Padding, Paragraph},
};
use unicode_width::UnicodeWidthStr;

const WIDTH: u16 = 60;
/// The border and padding around a dialog's text.
const FRAME_COLUMNS: u16 = 4;
const FRAME_ROWS: u16 = 2;
const SHORTCUT_COLUMNS: usize = 16;
const PRESETS_ROW: usize = 2;
const FIRST_FIELD_ROW: usize = 5;

pub(super) fn draw_overlay(frame: &mut Frame, app: &mut App, theme: &Theme) {
    let area = frame.area();
    let columns = usize::from(WIDTH.min(area.width).saturating_sub(FRAME_COLUMNS));
    let rows = match &app.overlay {
        Some(Overlay::Help { .. }) => help(columns, theme),
        Some(Overlay::StatusLine(editor)) => status_line(editor, app, columns, theme),
        Some(Overlay::Models(_)) | None => return,
    };
    let body = u16::try_from(rows.len()).unwrap_or(u16::MAX);
    let height = body.saturating_add(FRAME_ROWS).min(area.height);
    let width = WIDTH.min(area.width);
    let dialog = Rect {
        x: area.x + (area.width - width) / 2,
        y: area.bottom() - height,
        width,
        height,
    };
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(theme.dim)
        .padding(Padding::horizontal(1));
    let inner = block.inner(dialog);
    frame.render_widget(Clear, dialog);
    frame.render_widget(block, dialog);
    let page = usize::from(inner.height);
    let first = match &mut app.overlay {
        Some(Overlay::Help { top }) => {
            *top = (*top).min(rows.len().saturating_sub(page));
            *top
        }
        // Whatever has the keys stays in view.
        Some(Overlay::StatusLine(editor)) => {
            (focus_row(editor.focus, rows.len()) + 1).saturating_sub(page)
        }
        Some(Overlay::Models(_)) | None => 0,
    };
    let scroll = u16::try_from(first).unwrap_or(u16::MAX);
    frame.render_widget(Paragraph::new(rows).scroll((scroll, 0)), inner);
}

/// `left` and `right` at the ends of a row `columns` wide.
fn spread(left: Vec<Span<'static>>, right: Span<'static>, columns: usize) -> Line<'static> {
    let used = left.iter().map(Span::width).sum::<usize>() + right.width();
    let mut spans = left;
    spans.push(Span::raw(" ".repeat(columns.saturating_sub(used))));
    spans.push(right);
    Line::from(spans)
}

fn accent(theme: &Theme) -> Style {
    theme.kind(Kind::Noul, true)
}

fn help(columns: usize, theme: &Theme) -> Vec<Line<'static>> {
    let mut rows = vec![
        spread(
            vec![Span::styled("Shortcuts", theme.accent)],
            Span::styled("Esc close", theme.dim),
            columns,
        ),
        Line::default(),
    ];
    for (keys, meaning) in SHORTCUTS {
        rows.push(Line::from(vec![
            Span::styled(format!("{keys:<SHORTCUT_COLUMNS$}"), accent(theme)),
            Span::styled(meaning, theme.dim),
        ]));
    }
    rows.push(Line::default());
    rows.push(Line::from(Span::styled("/statusline →", accent(theme))));
    rows
}

/// The status dialog's row that holds `focus`, out of `rows`.
fn focus_row(focus: Focus, rows: usize) -> usize {
    match focus {
        Focus::Preset(_) => PRESETS_ROW,
        Focus::Field(index) => FIRST_FIELD_ROW + index,
        Focus::Apply => rows.saturating_sub(1),
    }
}

fn status_line(editor: &Editor, app: &App, columns: usize, theme: &Theme) -> Vec<Line<'static>> {
    let mut rows = vec![
        spread(
            vec![Span::styled("Status line", theme.accent)],
            Span::styled("Esc close", theme.dim),
            columns,
        ),
        Line::default(),
        presets(editor, columns, theme),
        Line::default(),
        Line::from(Span::styled("Built-in fields", theme.dim)),
    ];
    for (index, field) in Field::ALL.into_iter().enumerate() {
        rows.push(field_row(editor, app, index, field, columns, theme));
    }
    let preview: Vec<_> = editor
        .fields
        .iter()
        .filter_map(|field| app.field_text(*field, None))
        .collect();
    let preview = if preview.is_empty() {
        "(status line hidden)".to_owned()
    } else {
        preview.join(" · ")
    };
    rows.push(Line::from(Span::styled("─".repeat(columns), theme.dim)));
    rows.push(Line::from(Span::styled(preview, theme.dim)));
    rows.push(Line::default());
    let hint = Span::styled("Space toggle · Esc cancel", theme.dim);
    let apply = Span::styled(
        " Apply ",
        focused(accent(theme), editor.focus == Focus::Apply),
    );
    let hint = if hint.width() + apply.width() < columns {
        vec![hint]
    } else {
        Vec::new()
    };
    rows.push(spread(hint, apply, columns));
    rows
}

fn focused(style: Style, focus: bool) -> Style {
    if focus {
        style.add_modifier(Modifier::REVERSED)
    } else {
        style
    }
}

/// The presets in a row, or the one in play when they do not all fit.
fn presets(editor: &Editor, columns: usize, theme: &Theme) -> Line<'static> {
    let chosen = preset_of(&editor.fields);
    let names: usize = PRESETS.iter().map(|(name, _)| name.width()).sum();
    let fits = names + 2 * (PRESETS.len() - 1) <= columns;
    let only = match editor.focus {
        Focus::Preset(index) => index,
        _ => chosen.unwrap_or(0),
    };
    let mut spans = Vec::new();
    for (index, (name, _)) in PRESETS.into_iter().enumerate() {
        if !fits && index != only {
            continue;
        }
        if !spans.is_empty() {
            spans.push(Span::raw("  "));
        }
        let style = if chosen == Some(index) {
            accent(theme).add_modifier(Modifier::UNDERLINED)
        } else {
            theme.dim
        };
        spans.push(Span::styled(
            name,
            focused(style, editor.focus == Focus::Preset(index)),
        ));
    }
    Line::from(spans)
}

fn field_row(
    editor: &Editor,
    app: &App,
    index: usize,
    field: Field,
    columns: usize,
    theme: &Theme,
) -> Line<'static> {
    let on = editor.fields.contains(&field);
    let mark = if on { "[x] " } else { "[ ] " };
    let label = format!("{mark}{}", field.label());
    let style = focused(
        if on { accent(theme) } else { Style::new() },
        editor.focus == Focus::Field(index),
    );
    let sample = app
        .field_text(field, None)
        .unwrap_or_else(|| "—".to_owned());
    let room = columns.saturating_sub(label.width() + 1);
    let sample = if sample.width() > room {
        String::new()
    } else {
        sample
    };
    spread(
        vec![Span::styled(label, style)],
        Span::styled(sample, theme.dim),
        columns,
    )
}

#[cfg(test)]
mod tests {
    use crate::repl::tui::{draw::tests::shown, tests::*};

    #[test]
    fn help_lists_the_shortcuts_without_any_function_key() {
        let settings = settings();
        let mut app = app(&settings);
        app.open_help();
        let screen = shown(&mut app, 100, 30).join("\n");
        for text in [
            "Shortcuts",
            "Esc close",
            "Shift-Tab",
            "Ctrl-P",
            "Ctrl-S",
            "/statusline →",
        ] {
            assert!(screen.contains(text), "{text}: {screen}");
        }
        assert!(!screen.contains("F5") && !screen.contains("F6"), "{screen}");
    }

    #[test]
    fn help_scrolls_to_its_end_in_a_short_view() {
        let settings = settings();
        let mut app = app(&settings);
        app.open_help();
        assert!(
            !shown(&mut app, 100, 12)
                .join("\n")
                .contains("/statusline →")
        );
        for _ in 0..40 {
            press(&mut app, KeyCode::Down, KeyModifiers::NONE);
        }
        assert!(
            shown(&mut app, 100, 12)
                .join("\n")
                .contains("/statusline →")
        );
    }

    #[test]
    fn the_status_dialog_shows_presets_fields_and_a_preview() {
        let mut settings = settings();
        settings.config.default_provider = Some("typesafe".into());
        settings.config.default_model = Some("jev-latest".into());
        let mut app = app(&settings);
        app.open_status_line();
        let screen = shown(&mut app, 100, 30).join("\n");
        for text in [
            "Status line",
            "Minimal  Session  Request  Hidden",
            "Built-in fields",
            "[x] Model",
            "[ ] Last latency",
            "Space toggle · Esc cancel",
            "Apply",
        ] {
            assert!(screen.contains(text), "{text}: {screen}");
        }
        press(&mut app, KeyCode::Left, KeyModifiers::NONE);
        press(&mut app, KeyCode::Char(' '), KeyModifiers::NONE);
        let screen = shown(&mut app, 100, 30).join("\n");
        assert!(screen.contains("(status line hidden)"), "{screen}");
    }

    #[test]
    fn the_control_with_the_keys_stays_in_view_when_the_dialog_does_not_fit() {
        use crate::repl::tui::overlay::Overlay;
        use crate::repl::tui::status_line::{Editor, Field, Focus, PRESETS};
        let settings = settings();
        let mut app = app(&settings);
        let focused = |app: &mut crate::repl::tui::App, focus, text: &str, width, height| {
            let mut editor = Editor::new(&[Field::Model]);
            editor.focus = focus;
            app.overlay = Some(Overlay::StatusLine(editor));
            let screen = shown(app, width, height).join("\n");
            assert!(screen.contains(text), "{text}: {screen}");
        };
        for (index, field) in Field::ALL.into_iter().enumerate() {
            focused(&mut app, Focus::Field(index), field.label(), 80, 12);
        }
        focused(&mut app, Focus::Apply, "Apply", 80, 12);
        focused(&mut app, Focus::Apply, "Apply", 30, 24);
        focused(&mut app, Focus::Preset(0), "Minimal", 80, 12);
        for (index, (name, _)) in PRESETS.into_iter().enumerate() {
            focused(&mut app, Focus::Preset(index), name, 30, 24);
        }
    }

    #[test]
    fn dialogs_fit_tiny_terminals() {
        let settings = settings();
        let mut app = app(&settings);
        for (width, height) in [(1, 1), (4, 4), (24, 12)] {
            app.open_help();
            shown(&mut app, width, height);
            app.open_status_line();
            shown(&mut app, width, height);
        }
    }
}
