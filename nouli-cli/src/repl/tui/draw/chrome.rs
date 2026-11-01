//! Pane layout, shared styles, and terminal chrome.
use super::{
    App,
    composer::composer,
    references::{draw_references, reference_menu},
    wrap::transcript,
};
use crate::{
    render,
    repl::tui::{
        form::Kind,
        graph::{GRAPH_MAX, GRAPH_MIN},
        overlay::Overlay,
    },
};
use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Clear, Paragraph},
};
use unicode_width::UnicodeWidthStr;
const TRANSCRIPT_COLUMNS: u16 = 76;
const LIST_ROWS: usize = 8;
const TRANSCRIPT_ROWS: u16 = 3;
const SELECT_TEXT: &str = " select text · Ctrl-P ";
/// The view's styles. Without color, emphasis is bold alone.
pub(super) struct Theme {
    pub(super) accent: Style,
    pub(super) dim: Style,
    pub(super) error: Style,
    pub(super) color: bool,
}

impl Theme {
    pub(super) fn new(color: bool) -> Self {
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

    pub(super) fn kind(&self, kind: Kind, selected: bool) -> Style {
        let mut style = Style::new();
        if self.color {
            style = style.fg(kind.color());
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

pub(super) fn pane_width(total: u16, preferred: Option<u16>) -> Option<u16> {
    if let Some(width) = preferred
        && total >= GRAPH_MIN * 2
    {
        Some(width.clamp(GRAPH_MIN, total - GRAPH_MIN))
    } else {
        graph_width(total)
    }
}

pub(super) fn draw_header(frame: &mut Frame, area: Rect, app: &mut App, theme: &Theme) {
    // The mouse belongs to the terminal unless the graph has the keys, so this
    // control only exists while a click can reach it.
    let width = u16::try_from(SELECT_TEXT.width()).unwrap_or(u16::MAX);
    let action_area = (app.captures_mouse() && area.width >= width + 20).then(|| Rect {
        x: area.right() - width,
        width,
        ..area
    });
    app.selection.button = action_area;
    if let Some(button) = action_area {
        frame.render_widget(
            Paragraph::new(Span::styled(SELECT_TEXT, theme.accent)),
            button,
        );
    }
    let label = render::clean(&app.repl.session.label);
    let title = Line::from(vec![
        Span::styled("nouli", theme.accent),
        Span::styled(
            format!(" {} · {label}", env!("CARGO_PKG_VERSION")),
            theme.dim,
        ),
    ]);
    let title_area = Rect {
        width: area
            .width
            .saturating_sub(action_area.map_or(0, |area| area.width)),
        ..area
    };
    frame.render_widget(Paragraph::new(title), title_area);
}

pub(super) fn draw_left(frame: &mut Frame, area: Rect, app: &mut App, busy: bool, theme: &Theme) {
    let (lines, cursor) = composer(app, usize::from(area.width), busy, theme);
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
    let greeter = if top.height < crate::repl::tui::greeter::ROWS {
        Vec::new()
    } else {
        crate::repl::tui::greeter::lines(
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
    let picker_cursor = draw_picker(frame, top, app, theme);
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
    if let Some(position) = picker_cursor {
        frame.set_cursor_position(position);
    } else if let Some((column, row)) = cursor.filter(|_| app.overlay.is_none()) {
        let x = u16::try_from(column).map_or(u16::MAX, |column| bottom.x.saturating_add(column));
        let y = u16::try_from(row - first).map_or(u16::MAX, |row| bottom.y.saturating_add(row));
        frame.set_cursor_position((x.min(bottom.right().saturating_sub(1)), y));
    }
}

/// Draws the open model picker over the bottom of `area`, and says where its
/// cursor goes.
fn draw_picker(frame: &mut Frame, area: Rect, app: &App, theme: &Theme) -> Option<(u16, u16)> {
    let Some(Overlay::Models(picker)) = &app.overlay else {
        return None;
    };
    let (rows, (column, row)) = super::picker::lines(app, picker, usize::from(area.width), theme);
    let height = u16::try_from(rows.len())
        .unwrap_or(u16::MAX)
        .min(area.height);
    let shown = Rect {
        y: area.y + area.height - height,
        height,
        ..area
    };
    frame.render_widget(Clear, shown);
    frame.render_widget(Paragraph::new(rows), shown);
    let column = u16::try_from(column).ok()?;
    let row = u16::try_from(row).ok().filter(|row| *row < height)?;
    Some((
        shown
            .x
            .saturating_add(column)
            .min(shown.right().saturating_sub(1)),
        shown.y + row,
    ))
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repl::tui::draw::tests::{app, shown};

    #[test]
    fn the_select_text_control_shows_only_while_a_click_can_reach_it() {
        let settings = crate::settings::Settings::for_test(std::env::temp_dir());
        let mut app = app(&settings);
        for width in [24, 50, 80, 160] {
            app.select_text(false);
            app.graph_focused = false;
            let normal = shown(&mut app, width, 24);
            assert!(app.selection.button.is_none());
            assert!(!normal[0].contains("Ctrl-P"), "no permanent hint");
            app.graph_focused = true;
            let focused = shown(&mut app, width, 24);
            if width >= 50 {
                assert!(focused[0].contains("select text · Ctrl-P"));
                assert!(app.selection.button.is_some());
            } else {
                assert!(app.selection.button.is_none());
            }
            app.select_text(true);
            let held = shown(&mut app, width, 24);
            assert!(app.selection.button.is_none());
            if width >= 50 {
                assert!(held.last().unwrap().contains("Esc/Ctrl-P resume"));
            }
        }
    }

    #[test]
    fn holding_the_view_moves_no_text() {
        let settings = crate::settings::Settings::for_test(std::env::temp_dir());
        let mut app = app(&settings);
        let before = shown(&mut app, 80, 24);
        app.select_text(true);
        let held = shown(&mut app, 80, 24);
        assert_eq!(before[..23], held[..23]);
    }

    #[test]
    fn the_footer_is_the_model_and_the_question_type_hint() {
        let mut settings = crate::settings::Settings::for_test(std::env::temp_dir());
        settings.config.default_provider = Some("typesafe".into());
        settings.config.default_model = Some("jev-latest".into());
        let mut app = app(&settings);
        let screen = shown(&mut app, 80, 24);
        let footer = screen.last().unwrap();
        assert!(footer.starts_with("jev-latest · typesafe "), "{footer}");
        let joined = screen.join("\n");
        assert!(
            footer.trim_end().ends_with("Shift-Tab question type"),
            "{footer}"
        );
        for gone in ["Enter to send", "@ reference", "/help\n", "Ready"] {
            assert!(!joined.contains(gone), "{gone}: {joined}");
        }
    }

    #[test]
    fn status_fields_follow_the_choice_and_hidden_keeps_errors_visible() {
        use crate::repl::tui::{Status, status_line::Field};
        let settings = crate::settings::Settings::for_test(std::env::temp_dir());
        let mut app = app(&settings);
        let last = |app: &mut App, height| shown(app, 80, height).pop().unwrap();
        app.status_line = vec![Field::Session, Field::Decisions, Field::Latency];
        app.latency = Some(std::time::Duration::from_millis(842));
        let label = app.repl.session.label.clone();
        assert!(
            last(&mut app, 24).starts_with(&format!("{label} · 0 decisions · 842 ms")),
            "{}",
            last(&mut app, 24)
        );
        app.status_line = vec![];
        assert_eq!(last(&mut app, 24).trim(), "Shift-Tab question type");
        app.status = Some(Status::failed("Could not reach model"));
        let screen = shown(&mut app, 80, 24);
        assert!(
            screen[22].starts_with("! Could not reach model"),
            "{screen:?}"
        );
        assert!(
            screen[22].trim_end().ends_with("Ctrl-S retry"),
            "{screen:?}"
        );
    }

    #[test]
    fn a_narrow_footer_keeps_the_fields_and_drops_the_hint() {
        use crate::repl::tui::status_line::Field;
        let settings = crate::settings::Settings::for_test(std::env::temp_dir());
        let mut app = app(&settings);
        app.status_line = vec![Field::Model];
        // The 20 columns of the model, a gap, and the 23 of the hint.
        let footer = shown(&mut app, 44, 24).pop().unwrap();
        assert!(footer.starts_with("Model not configured "), "{footer}");
        assert!(footer.ends_with(" Shift-Tab question type"), "{footer}");
        let footer = shown(&mut app, 43, 24).pop().unwrap();
        assert!(footer.starts_with("Model not configured"), "{footer}");
        assert!(!footer.contains("Shift-Tab"), "{footer}");
    }

    #[test]
    fn a_long_error_wraps_instead_of_being_cut_off() {
        use crate::repl::tui::Status;
        let settings = crate::settings::Settings::for_test(std::env::temp_dir());
        let mut app = app(&settings);
        app.status = Some(Status::error(format!("{}conclusion", "word ".repeat(30))));
        let screen = shown(&mut app, 80, 24).join("\n");
        assert!(screen.contains("conclusion"), "{screen}");
    }

    #[test]
    fn the_decisions_pane_takes_the_full_height_beside_the_footer() {
        let settings = crate::settings::Settings::for_test(std::env::temp_dir());
        let mut app = app(&settings);
        let screen = shown(&mut app, 140, 24);
        let divider = 140 - 56;
        assert!(
            screen
                .iter()
                .all(|row| row.chars().nth(divider) == Some('│')),
            "{screen:?}"
        );
        assert!(
            screen[1].contains("│ decisions · Ctrl-G to resize"),
            "{screen:?}"
        );
        assert!(!screen[0].contains("decisions"), "{screen:?}");
        assert!(
            screen.last().unwrap().contains("Shift-Tab question type │"),
            "{screen:?}"
        );
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
    fn type_controls_keep_distinct_citrus_colors_and_respect_no_color() {
        let theme = Theme::new(true);
        let plain = Theme::new(false);
        for kind in Kind::ALL {
            assert_eq!(theme.kind(kind, true).fg, Some(kind.color()));
            assert_eq!(theme.kind(kind, false).fg, Some(kind.color()));
            assert_eq!(plain.kind(kind, true).fg, None);
            assert!(plain.kind(kind, true).add_modifier.contains(Modifier::BOLD));
        }
        assert_eq!(theme.accent.fg, None, "context and model stay neutral");
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
    fn model_status_is_visible_before_answers_and_refreshes_the_next_selection() {
        let mut settings = crate::settings::Settings::for_test(std::env::temp_dir());
        settings.config.default_provider = Some("typesafe".into());
        settings.config.default_model = Some("jev-latest".into());
        let mut app = app(&settings);
        let screen = shown(&mut app, 140, 24).join("\n");
        assert!(screen.contains("jev-latest · typesafe"), "{screen}");
        app.model = Some("jev-1.14.0 · typesafe".into());
        assert!(
            shown(&mut app, 140, 24)
                .join("\n")
                .contains("jev-1.14.0 · typesafe")
        );
        app.repl.model.model = Some("jev-1.13.0".into());
        app.refresh_model();
        app.form.question = "A draft question?".into();
        let screen = shown(&mut app, 140, 24).join("\n");
        assert!(screen.contains("jev-1.13.0 · typesafe"), "{screen}");
        assert!(!screen.contains("last served:"), "{screen}");
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
    fn small_terminals_keep_a_valid_cursor() {
        let settings = crate::settings::Settings::for_test(std::env::temp_dir());
        let mut app = app(&settings);
        for (width, height) in [(1, 1), (4, 4), (24, 12)] {
            shown(&mut app, width, height);
        }
    }
}
