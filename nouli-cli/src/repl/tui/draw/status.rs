//! The status row at the bottom of the main column.
use super::{App, Running, chrome::Theme};
use crate::repl::tui::status_line::Field;
use ratatui::{
    Frame,
    layout::Rect,
    text::{Line, Span},
    widgets::Paragraph,
};
use unicode_width::UnicodeWidthStr;

/// How to change the question type, left out when the fields need the room.
const HINT: &str = "Shift-Tab question type";

/// The status row: the chosen fields, and the question type hint at the right.
pub(super) fn draw_status(
    frame: &mut Frame,
    area: Rect,
    app: &App,
    running: Option<&Running>,
    theme: &Theme,
) {
    if app.selection.active {
        // In place of the status, so holding the view moves no text.
        let hints = fit_hints(
            area.width.into(),
            &["Esc/Ctrl-P resume", "drag to select", "copy with terminal"],
        );
        frame.render_widget(Paragraph::new(Span::styled(hints, theme.accent)), area);
        return;
    }
    let mut spans = Vec::new();
    for field in app.status_fields() {
        let Some(text) = app.field_text(*field, running) else {
            continue;
        };
        if spans.is_empty() {
            // A model reads brighter than the provider beside it.
            match text.split_once(" · ") {
                Some((model, provider)) if *field == Field::Model => {
                    spans.push(Span::styled(model.to_owned(), theme.accent));
                    spans.push(Span::styled(format!(" · {provider}"), theme.dim));
                }
                _ => spans.push(Span::styled(text, theme.accent)),
            }
        } else {
            spans.push(Span::styled(format!(" · {text}"), theme.dim));
        }
    }
    let used = spans.iter().map(Span::width).sum::<usize>();
    let hint = (used + HINT.width() < usize::from(area.width)).then_some(HINT);
    let width = u16::try_from(hint.map_or(0, UnicodeWidthStr::width)).unwrap_or(u16::MAX);
    frame.render_widget(
        Paragraph::new(Line::from(spans)),
        Rect {
            width: area.width.saturating_sub(width),
            ..area
        },
    );
    if let Some(hint) = hint {
        let right = Rect {
            x: area.right() - width,
            width,
            ..area
        };
        frame.render_widget(Paragraph::new(Span::styled(hint, theme.dim)), right);
    }
}

fn fit_hints(width: usize, hints: &[&str]) -> String {
    let mut shown = String::new();
    for hint in hints {
        let next = if shown.is_empty() {
            (*hint).to_owned()
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

    #[test]
    fn hints_that_do_not_fit_are_left_out_whole() {
        let hints = ["Esc/Ctrl-P resume", "drag to select"];
        assert_eq!(fit_hints(200, &hints), "Esc/Ctrl-P resume · drag to select");
        assert_eq!(fit_hints(20, &hints), "Esc/Ctrl-P resume");
        assert_eq!(fit_hints(5, &hints), "");
    }
}
