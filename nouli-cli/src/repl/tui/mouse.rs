//! Mouse interaction is scoped to the existing graph focus. Outside it the
//! terminal retains native selection; no global mouse reporting is enabled.
use super::App;
use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};

impl App<'_> {
    pub(super) fn graph_mouse(&mut self, event: MouseEvent) {
        if !self.graph_focused {
            return;
        }
        match event.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                self.graph.dragging = self.graph.divider.is_some_and(|divider| {
                    event.column.abs_diff(divider.x) <= 1
                        && event.row >= divider.y
                        && event.row < divider.bottom()
                });
            }
            MouseEventKind::Drag(MouseButton::Left) if self.graph.dragging => {
                let body = self.graph.body;
                if body.width >= 80 {
                    self.graph.preferred_width = Some(
                        body.right()
                            .saturating_sub(event.column)
                            .clamp(40, body.width - 40),
                    );
                }
            }
            MouseEventKind::Up(MouseButton::Left) => self.graph.dragging = false,
            MouseEventKind::ScrollUp => {
                self.graph.y = self.graph.y.saturating_sub(3);
                self.graph.follow = false;
            }
            MouseEventKind::ScrollDown => {
                self.graph.y = self.graph.y.saturating_add(3);
                self.graph.follow = false;
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyModifiers;
    use ratatui::layout::Rect;
    #[test]
    fn only_a_focused_divider_drag_changes_width_and_clamps_both_panes() {
        let settings = crate::settings::Settings::for_test(std::env::temp_dir());
        let mut app = super::super::tests::app(&settings);
        app.graph.body = Rect::new(0, 1, 200, 30);
        app.graph.divider = Some(Rect::new(120, 1, 1, 30));
        let event = |kind, column| MouseEvent {
            kind,
            column,
            row: 10,
            modifiers: KeyModifiers::NONE,
        };
        app.graph_mouse(event(MouseEventKind::Down(MouseButton::Left), 120));
        app.graph_mouse(event(MouseEventKind::Drag(MouseButton::Left), 80));
        assert!(app.graph.preferred_width.is_none());
        app.graph_focused = true;
        app.graph_mouse(event(MouseEventKind::Down(MouseButton::Left), 120));
        app.graph_mouse(event(MouseEventKind::Drag(MouseButton::Left), 80));
        assert_eq!(app.graph.preferred_width, Some(120));
        app.graph_mouse(event(MouseEventKind::Drag(MouseButton::Left), 0));
        assert_eq!(app.graph.preferred_width, Some(160));
        app.graph_mouse(event(MouseEventKind::Drag(MouseButton::Left), 220));
        assert_eq!(app.graph.preferred_width, Some(40));
        app.graph_mouse(event(MouseEventKind::Up(MouseButton::Left), 120));
        app.graph_mouse(event(MouseEventKind::Drag(MouseButton::Left), 80));
        assert_eq!(app.graph.preferred_width, Some(40));
    }
}
