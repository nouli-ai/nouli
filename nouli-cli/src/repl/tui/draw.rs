//! Drawing orchestration for the header, transcript, form, and decision pane.
mod activity;
mod chrome;
mod composer;
mod graph;
mod overlay;
mod picker;
mod references;
mod status;
mod wrap;
use super::{App, Running};
use activity::{activity, activity_rows, draw_activity};
use chrome::{Theme, draw_header, draw_left, pane_width};
use graph::draw_graph;
use overlay::draw_overlay;
use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
};
use status::draw_status;
pub(super) fn screen(frame: &mut Frame, app: &mut App, running: Option<&Running>) {
    let theme = Theme::new(app.repl.paint.0);
    let area = frame.area();
    app.graph.body = area;
    // The decisions pane takes the full height beside everything else.
    let (main, side) = match pane_width(area.width, app.graph.preferred_width) {
        Some(width) => {
            let [main, side] =
                Layout::horizontal([Constraint::Min(0), Constraint::Length(width)]).areas(area);
            // A column of gutter keeps the rules off the divider.
            let main = Rect {
                width: main.width.saturating_sub(1),
                ..main
            };
            (main, Some(side))
        }
        None => (area, None),
    };
    let doing = activity(app, running, &theme);
    let [header, body, doing_area, status] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(0),
        Constraint::Length(activity_rows(doing.as_ref(), main.width)),
        Constraint::Length(1),
    ])
    .areas(main);
    // Without room for both, the pane shows only while it has the keys.
    let (left, graph) = match side {
        Some(side) => (Some(body), Some(side)),
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
        draw_left(frame, left, app, running.is_some(), &theme);
    }
    if let Some(graph) = graph {
        draw_graph(frame, graph, app, &theme);
    }
    if let Some(doing) = doing {
        draw_activity(frame, doing_area, &doing, &theme);
    }
    draw_status(frame, status, app, running, &theme);
    draw_overlay(frame, app, &theme);
}

#[cfg(test)]
mod tests {
    use super::*;
    pub(super) use crate::repl::tui::tests::app;
    use ratatui::{Terminal, backend::TestBackend};

    pub(super) fn shown(app: &mut App, width: u16, height: u16) -> Vec<String> {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal.draw(|frame| screen(frame, app, None)).unwrap();
        let buffer = terminal.backend().buffer();
        (0..height)
            .map(|y| (0..width).map(|x| buffer[(x, y)].symbol()).collect())
            .collect()
    }
}
