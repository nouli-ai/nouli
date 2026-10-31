//! Divider dragging works independently of keyboard focus. Native terminal
//! selection uses the host's modifier (for example Option-drag in iTerm2).
use super::App;
use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};

impl App<'_> {
    pub(super) fn graph_mouse(&mut self, event: MouseEvent) {
        let over_graph = self
            .graph
            .divider
            .map_or(self.graph_focused, |area| event.column >= area.x);
        let overlay = self.references.picker.is_some()
            || self.references.inspected.is_some()
            || self.sessions.is_some()
            || !self.commands_shown().is_empty();
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
            MouseEventKind::ScrollUp if over_graph => {
                self.graph.y = self.graph.y.saturating_sub(3);
                self.graph.follow = false;
            }
            MouseEventKind::ScrollDown if over_graph => {
                self.graph.y = self.graph.y.saturating_add(3);
                self.graph.follow = false;
            }
            MouseEventKind::ScrollUp if !overlay => self.scroll = self.scroll.saturating_add(3),
            MouseEventKind::ScrollDown if !overlay => self.scroll = self.scroll.saturating_sub(3),
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
    fn divider_drag_works_without_graph_focus_and_clamps_both_panes() {
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
        assert_eq!(app.graph.preferred_width, Some(120));
        assert!(!app.graph_focused, "resizing does not move keyboard focus");
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

    #[test]
    fn pointer_wheel_routes_by_pane_and_drag_disarms_double_enter() {
        let settings = crate::settings::Settings::for_test(std::env::temp_dir());
        let mut app = super::super::tests::app(&settings);
        app.graph.body = Rect::new(0, 1, 200, 30);
        app.graph.divider = Some(Rect::new(120, 1, 1, 30));
        app.graph_focused = true;
        let event = |kind, column| MouseEvent {
            kind,
            column,
            row: 10,
            modifiers: KeyModifiers::NONE,
        };
        app.graph_mouse(event(MouseEventKind::ScrollUp, 30));
        assert_eq!(app.scroll, 3);
        assert_eq!(app.graph.y, 0);
        app.graph_mouse(event(MouseEventKind::ScrollDown, 150));
        assert_eq!(app.graph.y, 3);
        app.references.inspected = Some(0);
        app.graph_mouse(event(MouseEventKind::ScrollUp, 30));
        assert_eq!(
            app.scroll, 3,
            "an overlay leaves the underlying transcript still"
        );
        app.option_enter = true;
        app.event(
            crossterm::event::Event::Mouse(event(MouseEventKind::Down(MouseButton::Left), 120)),
            &mut None,
        )
        .unwrap();
        assert!(!app.option_enter);
    }
}
