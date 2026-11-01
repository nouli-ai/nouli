//! Native terminal selection by default; mouse reporting only for graph focus.
use super::App;
use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::{Position, Rect};

#[derive(Default)]
pub(super) struct Selection {
    pub(super) active: bool,
    pub(super) button: Option<Rect>,
    drawn: bool,
}

impl Selection {
    pub(super) fn set_active(&mut self, active: bool) {
        self.active = active;
        self.drawn = false;
    }

    pub(super) fn should_draw(&self) -> bool {
        !self.active || !self.drawn
    }

    pub(super) fn did_draw(&mut self) {
        self.drawn = self.active;
    }
}

impl App<'_> {
    pub(super) fn captures_mouse(&self) -> bool {
        self.graph_focused && !self.selection.active
    }

    pub(super) fn select_text(&mut self, active: bool) {
        self.selection.set_active(active);
        self.graph.dragging = false;
        self.option_enter = false;
    }

    pub(super) fn graph_mouse(&mut self, event: MouseEvent) {
        if !self.captures_mouse() {
            return;
        }
        if matches!(event.kind, MouseEventKind::Down(MouseButton::Left))
            && self
                .selection
                .button
                .is_some_and(|area| area.contains(Position::new(event.column, event.row)))
        {
            self.select_text(true);
            return;
        }
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
    use crossterm::event::{Event, KeyCode, KeyEvent};
    use ratatui::layout::Rect;

    #[test]
    fn mouse_capture_follows_graph_focus_and_copy_mode() {
        let settings = crate::settings::Settings::for_test(std::env::temp_dir());
        let mut app = super::super::tests::app(&settings);
        let mut running = None;
        let mut press = |code, modifiers| {
            app.event(Event::Key(KeyEvent::new(code, modifiers)), &mut running)
                .unwrap();
            app.captures_mouse()
        };
        assert!(!press(KeyCode::Null, KeyModifiers::NONE));
        assert!(press(KeyCode::Char('g'), KeyModifiers::CONTROL));
        assert!(!press(KeyCode::F(6), KeyModifiers::NONE));
        assert!(press(KeyCode::Esc, KeyModifiers::NONE));
        assert!(!press(KeyCode::Tab, KeyModifiers::NONE));
        assert!(press(KeyCode::Char('g'), KeyModifiers::CONTROL));
        assert!(!press(KeyCode::Esc, KeyModifiers::NONE));
    }

    #[test]
    fn selection_holds_the_view_until_resumed() {
        let mut selection = Selection::default();
        assert!(selection.should_draw());
        selection.did_draw();
        assert!(selection.should_draw(), "normal drawing continues");
        selection.set_active(true);
        assert!(selection.should_draw(), "show selection instructions once");
        selection.did_draw();
        assert!(
            !selection.should_draw(),
            "background updates must not erase native selection"
        );
        selection.set_active(false);
        assert!(selection.should_draw());
    }

    #[test]
    fn a_completed_turn_is_recorded_without_repainting_native_selection() {
        let settings = crate::settings::Settings::for_test(std::env::temp_dir());
        let mut app = super::super::tests::app(&settings);
        app.form.input = "next draft".into();
        app.select_text(true);
        app.selection.did_draw();
        app.finish(
            nouli_core::protocol::ResultEnvelope::new(crate::repl::tests::saved_turn()),
            &[],
            false,
            false,
        )
        .unwrap();
        assert!(app.selection.active);
        assert!(!app.selection.should_draw());
        assert_ne!(app.transcript, [] as [ratatui::prelude::Line<'_>; 0]);
        assert_eq!(app.form.input, "next draft");
        app.select_text(false);
        assert!(
            app.selection.should_draw(),
            "show the completed result on resume"
        );
    }

    #[test]
    fn selection_keeps_drafts_focus_lists_and_paste_unchanged() {
        let settings = crate::settings::Settings::for_test(std::env::temp_dir());
        let mut app = super::super::tests::app(&settings);
        app.form.input = "keep this context".into();
        app.form.question = "Keep this question?".into();
        app.form.cursor = 4;
        app.graph_focused = true;
        app.sessions = Some(vec![]);
        app.option_enter = true;
        let mut running = None;
        let key = |code| Event::Key(KeyEvent::new(code, KeyModifiers::NONE));
        app.event(key(KeyCode::F(6)), &mut running).unwrap();
        assert!(app.selection.active);
        assert!(!app.option_enter);
        for event in [
            key(KeyCode::Char('x')),
            key(KeyCode::Enter),
            key(KeyCode::Tab),
            Event::Paste("do not insert".into()),
        ] {
            app.event(event, &mut running).unwrap();
        }
        app.event(key(KeyCode::Esc), &mut running).unwrap();
        assert!(!app.selection.active);
        assert_eq!(app.form.input, "keep this context");
        assert_eq!(app.form.question, "Keep this question?");
        assert_eq!(app.form.cursor, 4);
        assert!(app.graph_focused);
        assert!(app.sessions.is_some());
        assert!(running.is_none());
    }

    #[test]
    fn copy_mode_control_c_keeps_idle_input_and_still_cancels_running_turns() {
        let settings = crate::settings::Settings::for_test(std::env::temp_dir());
        let mut app = super::super::tests::app(&settings);
        app.form.input = "preserve this draft".into();
        let key = Event::Key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL));
        app.select_text(true);
        assert!(matches!(
            app.event(key.clone(), &mut None).unwrap(),
            super::super::Flow::Continue
        ));
        assert_eq!(app.form.input, "preserve this draft");
        assert!(!app.selection.active);
        let cancel = tokio_util::sync::CancellationToken::new();
        let mut running = Some(super::super::Running {
            turn: Box::pin(std::future::pending()),
            cancel: cancel.clone(),
            ephemeral: false,
            from_form: false,
            started: std::time::Instant::now(),
            model: nouli_core::history::ModelSelection::default(),
        });
        app.select_text(true);
        app.event(key, &mut running).unwrap();
        assert!(cancel.is_cancelled());
        assert_eq!(app.form.input, "preserve this draft");
    }

    #[test]
    fn header_selection_click_and_resize_preserve_divider_width() {
        let settings = crate::settings::Settings::for_test(std::env::temp_dir());
        let mut app = super::super::tests::app(&settings);
        app.graph_focused = true;
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(160, 30)).unwrap();
        terminal
            .draw(|frame| super::super::draw::screen(frame, &mut app, None))
            .unwrap();
        let button = app.selection.button.unwrap();
        app.graph.preferred_width = Some(70);
        app.graph.dragging = true;
        app.event(
            Event::Mouse(MouseEvent {
                kind: MouseEventKind::Down(MouseButton::Left),
                column: button.x + 1,
                row: button.y,
                modifiers: KeyModifiers::NONE,
            }),
            &mut None,
        )
        .unwrap();
        assert!(app.selection.active);
        assert!(!app.graph.dragging);
        app.event(Event::Resize(80, 24), &mut None).unwrap();
        assert!(!app.selection.active);
        assert_eq!(app.graph.preferred_width, Some(70));
    }
    #[test]
    fn divider_drag_requires_graph_focus_and_clamps_both_panes() {
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
        assert_eq!(app.graph.preferred_width, None);
        assert!(!app.graph.dragging);
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
