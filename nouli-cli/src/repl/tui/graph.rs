//! Visited session graphs and graph navigation.
use super::{App, ansi};
use crate::{inspect::InspectError, render, view::Placement};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use nouli_core::{
    identity::{Id, SessionSelector},
    storage::{Ledger, OpenMode},
};
use ratatui::text::Line;
use std::path::Path;
pub(super) const GRAPH_MIN: u16 = 40;
pub(super) const GRAPH_MAX: u16 = 80;
/// The session's decision graph as `nouli view` draws it, and where the pane
/// shows it.
#[derive(Default)]
pub(super) struct Graph {
    /// Sessions visited in this view. Their histories stay independent.
    pub(super) sessions: Vec<Id>,
    /// The most recently selected session with finalized decisions.
    pub(super) selected: Option<Id>,
    pub(super) lines: Vec<Line<'static>>,
    /// The latest card's row and columns.
    pub(super) latest: Option<Placement>,
    /// Shows the end and the latest card until the pane is scrolled.
    pub(super) follow: bool,
    pub(super) x: usize,
    pub(super) y: usize,
    /// The pane's height at the last draw, for paging.
    pub(super) page: usize,
    /// Why the graph could not be read.
    pub(super) problem: Option<String>,
    pub(super) preferred_width: Option<u16>,
    pub(super) body: ratatui::layout::Rect,
    pub(super) divider: Option<ratatui::layout::Rect>,
    pub(super) dragging: bool,
}

impl Graph {
    pub(super) fn show(&mut self, text: &str, latest: Option<Placement>) {
        let lines: Vec<_> = text.lines().map(ansi::line).collect();
        if self.lines != lines {
            self.lines = lines;
            self.follow = true;
        }
        self.latest = latest;
        self.problem = None;
    }
}

/// The session's decision graph as the text `nouli view` draws, without its
/// header, and the latest card's first row and columns.
pub(super) fn graph_text(
    store: &Path,
    sessions: &[Id],
    selected: Option<Id>,
    current: Id,
    color: bool,
) -> Result<(String, Option<Placement>, Option<Id>), InspectError> {
    let ledger = Ledger::open(store, OpenMode::ReadOnlyExisting)?;
    let exports = sessions
        .iter()
        .map(|id| ledger.export_session(&SessionSelector::Id(*id)))
        .collect::<Result<Vec<_>, _>>()?;
    let exports = crate::view::include_references(&ledger, exports)?;
    let selected = if exports
        .iter()
        .any(|export| export.session.id == current && !export.session.frontier.is_empty())
    {
        Some(current)
    } else {
        selected
    };
    let mut text = Vec::new();
    let latest = crate::view::cards_many(&exports, selected, &mut text, color)?;
    Ok((
        String::from_utf8_lossy(&text).into_owned(),
        latest,
        selected,
    ))
}
impl App<'_> {
    /// Keys that move the decisions pane while it has them; Esc and Tab give
    /// them back to the form. False for keys the pane leaves to the form.
    pub(super) fn graph_key(&mut self, key: &KeyEvent) -> bool {
        let graph = &mut self.graph;
        match key.code {
            KeyCode::Esc | KeyCode::Tab
                if key.code == KeyCode::Esc || !key.modifiers.contains(KeyModifiers::SHIFT) =>
            {
                self.graph_focused = false;
                return true;
            }
            KeyCode::Up => graph.y = graph.y.saturating_sub(1),
            KeyCode::Down => graph.y = graph.y.saturating_add(1),
            KeyCode::PageUp => graph.y = graph.y.saturating_sub(graph.page),
            KeyCode::PageDown => graph.y = graph.y.saturating_add(graph.page),
            KeyCode::Home => graph.y = 0,
            // Drawing stops it at the last page.
            KeyCode::End => graph.y = usize::MAX,
            KeyCode::Left => graph.x = graph.x.saturating_sub(4),
            KeyCode::Right => graph.x = graph.x.saturating_add(4),
            _ => return false,
        }
        graph.follow = false;
        true
    }

    pub(super) fn refresh_graph(&mut self) {
        let store = &self.repl.settings.store;
        let current = self.repl.session.id;
        if !self.graph.sessions.contains(&current) {
            self.graph.sessions.push(current);
        }
        match graph_text(
            store,
            &self.graph.sessions,
            self.graph.selected,
            current,
            self.repl.paint.0,
        ) {
            Ok((text, latest, selected)) => {
                self.graph.selected = selected;
                self.graph.show(&text, latest);
            }
            Err(error) => {
                self.graph.lines.clear();
                self.graph.problem = Some(render::clean(&error.report().message));
            }
        }
    }
}
