//! The row above the status row, which says what is happening now.
use super::{App, Running, chrome::Theme, wrap::wrap_line};
use crate::repl::tui::form::{Kind, Row};
use ratatui::{
    Frame,
    layout::Rect,
    style::Style,
    text::{Line, Span},
    widgets::Paragraph,
};
use unicode_width::UnicodeWidthStr;

const SPINNER: [char; 10] = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];
/// The most rows a long message takes above the status row.
const ACTIVITY_ROWS: usize = 3;

/// What is happening now, for the row above the status row. It takes no room
/// while there is nothing to say.
pub(super) struct Activity {
    line: Line<'static>,
    /// The key that acts on it, at the row's right end.
    action: Option<&'static str>,
}

impl Activity {
    fn note(text: impl Into<String>, style: Style) -> Self {
        Self {
            line: Line::from(Span::styled(text.into(), style)),
            action: None,
        }
    }
}

pub(super) fn activity(app: &App, running: Option<&Running>, theme: &Theme) -> Option<Activity> {
    let tone = theme.kind(Kind::Noul, true);
    Some(match (running, &app.status) {
        (Some(turn), _) if turn.cancel.is_cancelled() => Activity::note("Cancelling…", theme.dim),
        (Some(turn), _) => {
            let elapsed = turn.started.elapsed();
            let frame = (elapsed.as_millis() / 100) as usize % SPINNER.len();
            Activity {
                line: Line::from(Span::styled(
                    format!("{} Asking · {:.1}s", SPINNER[frame], elapsed.as_secs_f32()),
                    tone,
                )),
                action: Some("Ctrl-C cancel"),
            }
        }
        (None, Some(status)) if status.error => Activity {
            line: Line::from(Span::styled(format!("! {}", status.text), theme.error)),
            action: status.retry.then_some(if app.form.row == Row::Question {
                "Enter retry"
            } else {
                "Ctrl-S retry"
            }),
        },
        (None, Some(status)) => Activity::note(status.text.clone(), theme.dim),
        (None, None) if app.references.picker.is_some() => Activity::note(
            if app
                .references
                .picker
                .as_ref()
                .is_some_and(|picker| picker.expanded)
            {
                "↑↓ results · PgUp/PgDn preview · Enter insert · Esc close"
            } else {
                "↑↓ choose · Enter insert · Esc close · Tab field · Shift-Tab type"
            },
            theme.dim,
        ),
        (None, None) if app.sessions.is_some() => Activity::note(
            "↑↓ choose · Enter resumes the session · Esc closes",
            theme.dim,
        ),
        (None, None) if !app.commands_shown().is_empty() => Activity::note(
            "↑↓ choose · Tab completes · Enter runs · Esc hides",
            theme.dim,
        ),
        (None, None) if app.graph_focused => Activity::note(
            if app.graph.divider.is_some() {
                "drag divider to resize · arrows / wheel scroll · Esc returns to form"
            } else {
                "arrows / wheel scroll · Esc returns to form"
            },
            theme.dim,
        ),
        (None, None) if app.option_enter => {
            Activity::note("Press Enter again to send", theme.kind(app.form.kind, true))
        }
        (None, None) => return None,
    })
}

/// The columns beside the activity's action.
fn activity_columns(activity: &Activity, width: u16) -> usize {
    let action = activity.action.map_or(0, |action| action.width() + 1);
    usize::from(width).saturating_sub(action).max(1)
}

pub(super) fn activity_rows(activity: Option<&Activity>, width: u16) -> u16 {
    activity.map_or(0, |activity| {
        let rows = wrap_line(&activity.line, activity_columns(activity, width)).len();
        u16::try_from(rows.min(ACTIVITY_ROWS)).unwrap_or(1)
    })
}

pub(super) fn draw_activity(frame: &mut Frame, area: Rect, activity: &Activity, theme: &Theme) {
    let rows = wrap_line(&activity.line, activity_columns(activity, area.width));
    frame.render_widget(Paragraph::new(rows), area);
    if let Some(action) = activity.action {
        let width = u16::try_from(action.width()).unwrap_or(u16::MAX);
        if area.width > width {
            let right = Rect {
                x: area.right() - width,
                width,
                height: 1,
                ..area
            };
            frame.render_widget(Paragraph::new(Span::styled(action, theme.dim)), right);
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::repl::tui::{Status, draw::tests::shown, form::Row, tests::*};

    #[test]
    fn the_retry_action_names_the_key_that_sends_from_the_focused_row() {
        let settings = settings();
        let mut app = app(&settings);
        app.status = Some(Status::failed("The provider did not answer"));
        let row = |app: &mut crate::repl::tui::App| shown(app, 80, 24)[22].clone();
        assert!(
            row(&mut app).contains("Ctrl-S retry"),
            "Enter never sends here"
        );
        app.form.row = Row::Question;
        assert!(row(&mut app).contains("Enter retry"));
        app.form.options = vec!["One".into()];
        app.form.row = Row::Option(0);
        assert!(row(&mut app).contains("Ctrl-S retry"));
        app.status = Some(Status::error("Cannot read that file"));
        assert!(!row(&mut app).contains("retry"));
    }
}
