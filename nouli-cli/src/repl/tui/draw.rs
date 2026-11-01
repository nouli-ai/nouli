//! Drawing orchestration for the header, transcript, form, and decision pane.
mod chrome;
mod composer;
mod graph;
mod references;
mod wrap;
use super::{App, Running};
use chrome::{Theme, draw_footer, draw_header, draw_left, draw_model, pane_width};
use graph::draw_graph;
use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
};
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
