//! Decision pane rendering and following the latest card.
use super::{App, chrome::Theme};
use crate::repl::tui::Graph;
use ratatui::{
    Frame,
    layout::Rect,
    style::Style,
    text::{Line, Span},
    widgets::{Block, Borders, Padding, Paragraph},
};
pub(super) fn draw_graph(frame: &mut Frame, area: Rect, app: &mut App, theme: &Theme) {
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::fmt::Write as _;

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
