//! Terminal event dispatch, navigation, and submission.
use super::{
    App, Flow, Running, Status, Step, attachments,
    commands::{SETUP_COMMANDS, takes_argument},
    form::Row,
};
use crate::repl::Queue;
use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use std::{io, time::Instant};

/// Ctrl-P, which holds the view and resumes it.
fn holds_view(key: &KeyEvent) -> bool {
    key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('p')
}

impl<'a> App<'a> {
    pub(super) fn event(
        &mut self,
        event: Event,
        running: &mut Option<Running<'a>>,
    ) -> io::Result<Flow> {
        if let Event::Paste(text) = &event {
            self.picker_paste(text);
        }
        if (self.selection.active || self.overlay.is_some())
            && !matches!(event, Event::Key(_) | Event::Resize(_, _))
        {
            return Ok(Flow::Continue);
        }
        match event {
            Event::Mouse(event) => {
                self.graph_mouse(event);
                Ok(Flow::Continue)
            }
            Event::Resize(_, _) => {
                self.select_text(false);
                self.graph.dragging = false;
                Ok(Flow::Continue)
            }
            Event::Key(key) if key.kind != KeyEventKind::Release => {
                self.graph.dragging = false;
                self.key(key, running)
            }
            Event::Paste(text) => {
                self.graph.dragging = false;
                if running.is_some() {
                    self.status = Some(Status::info("a turn is running · Ctrl-C cancels it"));
                } else {
                    self.consume_drop();
                    self.graph_focused = false;
                    if let Some(paths) = attachments::paths(&text) {
                        self.attach(&paths);
                    } else {
                        self.paste_text(&text);
                    }
                    self.drop_start = None;
                    self.sync_references();
                }
                Ok(Flow::Continue)
            }
            // A resize, like any event, draws the view again.
            _ => Ok(Flow::Continue),
        }
    }

    pub(super) fn key(
        &mut self,
        key: KeyEvent,
        running: &mut Option<Running<'a>>,
    ) -> io::Result<Flow> {
        if self.selection.active {
            return Ok(self.selection_key(&key, running.as_ref()));
        }
        if self.overlay.is_some() {
            self.overlay_key(&key);
            return Ok(Flow::Continue);
        }
        if holds_view(&key) {
            self.select_text(true);
            return Ok(Flow::Continue);
        }
        self.settle_before_key(&key, running.is_some());
        if let Some(flow) = self.global_key(&key, running.as_ref()) {
            return Ok(flow);
        }
        if running.is_none() && self.reference_key(&key) {
            return Ok(Flow::Continue);
        }
        if let Some(flow) = self.list_key(&key, running)? {
            return Ok(flow);
        }
        if self.graph_focused {
            if self.graph_key(&key) {
                return Ok(Flow::Continue);
            }
            // Any other key is for the form.
            self.graph_focused = false;
        }
        self.form_key(&key, running)
    }

    /// Keys while the view is held for native selection: only Esc, Ctrl-P,
    /// and Ctrl-C resume it, and Ctrl-C also cancels a running turn.
    pub(super) fn selection_key(&mut self, key: &KeyEvent, running: Option<&Running>) -> Flow {
        let control_c =
            key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c');
        if key.code == KeyCode::Esc || holds_view(key) || control_c {
            self.select_text(false);
            if control_c && running.is_some() {
                return self.interrupt(running);
            }
        }
        Flow::Continue
    }

    /// Settles a file drop before moving between fields and clears the
    /// Ctrl-D quit warning when another key is pressed.
    pub(super) fn settle_before_key(&mut self, key: &KeyEvent, running: bool) {
        let control = key.modifiers.contains(KeyModifiers::CONTROL);
        if !running
            && (matches!(
                key.code,
                KeyCode::Tab | KeyCode::BackTab | KeyCode::Up | KeyCode::Down
            ) || control && key.code == KeyCode::Char('g'))
        {
            self.consume_drop();
        }
        if !(control && key.code == KeyCode::Char('d')) {
            self.warned = false;
        }
    }

    /// Keys that act wherever the focus is: Ctrl-C, Ctrl-D, Ctrl-G, and
    /// Shift-Tab, which cycles the answer type. `None` passes the key on.
    pub(super) fn global_key(&mut self, key: &KeyEvent, running: Option<&Running>) -> Option<Flow> {
        let control = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Char('c')
                if control && running.is_none() && self.references.picker.is_some() =>
            {
                self.references.close();
                return Some(Flow::Continue);
            }
            KeyCode::Char('c') if control => return Some(self.interrupt(running)),
            KeyCode::Char('d') if control => return Some(self.end_of_input(running.is_some())),
            KeyCode::Char('l') if control && running.is_none() => {
                self.references.close();
                self.sessions = None;
                self.graph_focused = false;
                self.open_model_picker("");
                return Some(Flow::Continue);
            }
            KeyCode::Char('g') if control => {
                self.references.close();
                self.sessions = None;
                self.graph_focused = !self.graph_focused;
                return Some(Flow::Continue);
            }
            _ => {}
        }
        if running.is_none()
            && (key.code == KeyCode::BackTab
                || key.code == KeyCode::Tab && key.modifiers.contains(KeyModifiers::SHIFT))
        {
            self.references.close();
            self.sessions = None;
            self.hidden = true;
            self.graph_focused = false;
            self.form.next_kind();
            self.forget_retry();
            return Some(Flow::Continue);
        }
        None
    }

    /// Keys for the form: scrolling the transcript, sending, moving between
    /// rows, and editing. While a turn runs, only scrolling works.
    pub(super) fn form_key(
        &mut self,
        key: &KeyEvent,
        running: &mut Option<Running<'a>>,
    ) -> io::Result<Flow> {
        let control = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::PageUp => self.scroll = self.scroll.saturating_add(self.page),
            KeyCode::PageDown => self.scroll = self.scroll.saturating_sub(self.page),
            _ if running.is_some() => {
                self.status = Some(Status::info("a turn is running · Ctrl-C cancels it"));
            }
            KeyCode::Char('s') if control => *running = self.send_form(false)?,
            KeyCode::Enter => {
                if !self.prepare_input() {
                    return Ok(Flow::Continue);
                }
                return self.enter(running);
            }
            KeyCode::Esc => self.status = None,
            KeyCode::Tab => {
                self.settle_context();
                self.form.tab();
            }
            KeyCode::Up => self.up(),
            KeyCode::Down => self.down(),
            KeyCode::Backspace if self.form.row == Row::Input && self.form.input.is_empty() => {
                self.reopen_fact();
            }
            KeyCode::Backspace if self.form.cursor == 0 => {
                self.form.remove_empty();
            }
            _ => self.type_key(key),
        }
        Ok(Flow::Continue)
    }

    /// Edits the focused row and, when the key changed it, tracks a file
    /// path being typed or dropped, clears the status and the list choice,
    /// and looks for an @ reference.
    pub(super) fn type_key(&mut self, key: &KeyEvent) {
        let start = self.form.cursor;
        if !self.edit_form(key) {
            return;
        }
        if self.form.row == Row::Input && self.form.input.is_empty() {
            self.literal_input = false;
        }
        if matches!(key.code, KeyCode::Char('/' | '~' | '\'' | '"' | 'f'))
            && self.drop_start.is_none()
        {
            self.drop_start = Some((self.form.row, start, Instant::now()));
        }
        if let Some((row, _, time)) = &mut self.drop_start
            && *row == self.form.row
        {
            *time = Instant::now();
        }
        self.status = None;
        self.choice = 0;
        self.hidden = false;
        self.sync_references();
    }

    /// Keys for a list over the transcript: the sessions to resume, or the
    /// commands that match the input row. `None` leaves the key to the form.
    pub(super) fn list_key(
        &mut self,
        key: &KeyEvent,
        running: &mut Option<Running<'a>>,
    ) -> io::Result<Option<Flow>> {
        let rows = match &self.sessions {
            Some(entries) => entries.len(),
            None => self.commands_shown().len(),
        };
        if rows == 0 || running.is_some() {
            return Ok(None);
        }
        if self.sessions.is_none()
            && key.code == KeyCode::Tab
            && key.modifiers.contains(KeyModifiers::SHIFT)
        {
            return Ok(None);
        }
        match key.code {
            KeyCode::Up => self.choice = self.choice.saturating_sub(1),
            KeyCode::Down => self.choice = (self.choice + 1).min(rows - 1),
            KeyCode::Esc if self.sessions.is_some() => self.sessions = None,
            KeyCode::Esc => self.hidden = true,
            KeyCode::Enter if self.sessions.is_some() => self.resume()?,
            KeyCode::Tab | KeyCode::Enter if self.sessions.is_none() => {
                let Some(&(usage, _)) = self.commands_shown().get(self.choice.min(rows - 1)) else {
                    return Ok(None);
                };
                let name = usage.split([' ', ',']).next().unwrap_or(usage);
                self.form.input = if usage
                    .split_once(' ')
                    .is_some_and(|(command, _)| !command.ends_with(','))
                {
                    format!("{name} ")
                } else {
                    name.to_owned()
                };
                self.form.cursor = self.form.input.len();
                self.choice = 0;
                // Enter runs a command that needs nothing more.
                if key.code == KeyCode::Enter && !takes_argument(usage) {
                    return self.submit(running).map(Some);
                }
            }
            // The sessions list keeps the keys until it closes.
            _ if self.sessions.is_some() => {}
            _ => return Ok(None),
        }
        Ok(Some(Flow::Continue))
    }

    /// Backspace on an empty input row: the row above comes back to edit.
    fn reopen_fact(&mut self) {
        if let Some(above) = self.input_index().checked_sub(1) {
            self.open_row(above);
        }
    }

    fn input_index(&self) -> usize {
        self.form.input_index(self.repl.queue.facts.len())
    }

    /// Whether the arrows can leave the input row for another Context row. A
    /// command being typed is not a row yet.
    fn walks_rows(&self) -> bool {
        self.form.row == Row::Input
            && (self.literal_input || !self.form.input.trim_start().starts_with('/'))
    }

    /// Queues the input row's text where the row stands; false if it is blank.
    fn queue_input(&mut self) -> bool {
        if self.form.input.trim().is_empty() {
            self.form.input.clear();
            return false;
        }
        let line = std::mem::take(&mut self.form.input);
        let index = self.input_index();
        self.repl.queue.facts.insert(index, line);
        true
    }

    /// Takes the queued fact at `index` into the input row to edit it.
    fn open_row(&mut self, index: usize) {
        let fact = self.repl.queue.facts.remove(index);
        self.form.rows_below = self.repl.queue.facts.len() - index;
        self.form.cursor = fact.len();
        self.form.input = fact;
        // A queued fact stays literal: never a command, a file path being
        // typed, or a search for a reference.
        self.literal_input = true;
        self.drop_start = None;
        self.sync_references();
        self.references.close();
    }

    /// Up: the Context row above, or the box above from the first row.
    fn up(&mut self) {
        match self.input_index().checked_sub(1) {
            Some(above) if self.walks_rows() => {
                self.queue_input();
                self.open_row(above);
            }
            _ => {
                self.settle_context();
                self.form.up();
            }
        }
    }

    /// Down: the Context row below, or the next box from the last one.
    fn down(&mut self) {
        if self.form.rows_below > 0 && self.walks_rows() {
            let index = self.input_index();
            let below = index + usize::from(self.queue_input());
            self.open_row(below);
        } else {
            self.settle_context();
            self.form.down();
        }
    }

    /// Puts an edited Context row back, leaving the input row at the end.
    /// Text being typed for the first time stays a draft in the input row.
    pub(super) fn settle_context(&mut self) {
        if self.form.rows_below > 0 || self.literal_input {
            self.queue_input();
            self.form.rows_below = 0;
            self.form.cursor = self.form.input.len();
            self.literal_input = false;
        }
    }

    /// The form changed, so sending it no longer repeats what failed.
    fn forget_retry(&mut self) {
        self.status.take_if(|status| status.retry);
    }

    /// Ctrl-C: cancels the running turn. Otherwise, as at the line prompt,
    /// it clears the row, then everything queued or typed, and then quits.
    pub(super) fn interrupt(&mut self, running: Option<&Running>) -> Flow {
        if let Some(turn) = running {
            turn.cancel.cancel();
            return Flow::Continue;
        }
        if self.sessions.take().is_some() {
            return Flow::Continue;
        }
        if !self.form.text().is_empty() {
            self.form.clear_row();
            if self.form.row == Row::Input {
                self.literal_input = false;
            }
            self.forget_retry();
            return Flow::Continue;
        }
        if self.repl.queue.is_empty() && self.form.is_empty() {
            return Flow::Quit;
        }
        self.repl.queue = Queue::default();
        self.form.clear();
        self.images.clear();
        self.literal_input = false;
        self.failed_drop = None;
        self.status = Some(Status::info("^C  queue cleared"));
        Flow::Continue
    }

    /// Ctrl-D: deletes forward in a row with text. Otherwise it quits,
    /// warning once when something is queued or typed. A running turn keeps
    /// the form as it was sent.
    pub(super) fn end_of_input(&mut self, running: bool) -> Flow {
        if running {
            self.status = Some(Status::info("a turn is running · Ctrl-C cancels it"));
            return Flow::Continue;
        }
        if !self.form.text().is_empty() {
            let delete = KeyEvent::new(KeyCode::Delete, KeyModifiers::NONE);
            self.edit_form(&delete);
            if self.form.row == Row::Input && self.form.input.is_empty() {
                self.literal_input = false;
            }
            self.sync_references();
            self.forget_retry();
            return Flow::Continue;
        }
        if !(self.warned || (self.repl.queue.is_empty() && self.form.is_empty())) {
            self.warned = true;
            self.status = Some(Status::info(
                "unsent input; press Ctrl-D again to quit and discard it",
            ));
            return Flow::Continue;
        }
        Flow::Quit
    }

    pub(super) fn enter(&mut self, running: &mut Option<Running<'a>>) -> io::Result<Flow> {
        if self.form.text().trim().is_empty() {
            self.settle_context();
            self.form.tab();
            return Ok(Flow::Continue);
        }
        if let Row::Option(index) = self.form.row {
            self.form.enter_option(index);
            return Ok(Flow::Continue);
        }
        // Context only takes rows and commands; sending is from Question.
        if self.form.row == Row::Input {
            return self.submit(running);
        }
        *running = self.send_form(false)?;
        Ok(Flow::Continue)
    }

    /// Enter on the input row: the line is queued as a fact, or runs as a
    /// command. Blank rows are handled by `enter` as section navigation.
    pub(super) fn submit(&mut self, running: &mut Option<Running<'a>>) -> io::Result<Flow> {
        let line = self.form.input.trim().to_owned();
        if line.is_empty() {
            return Ok(Flow::Continue);
        }
        self.form.input.clear();
        self.form.cursor = 0;
        if std::mem::take(&mut self.literal_input) {
            let index = self.input_index();
            self.repl.queue.facts.insert(index, line);
            return Ok(Flow::Continue);
        }
        if SETUP_COMMANDS.contains(&line.split_whitespace().next().unwrap_or("")) {
            if running.is_some() {
                self.status = Some(Status::error("wait for the active request"));
            } else {
                self.pending_setup = Some(line);
            }
            return Ok(Flow::Continue);
        }
        match line.split_whitespace().next().unwrap_or("") {
            "/help" => {
                self.open_help();
                return Ok(Flow::Continue);
            }
            "/statusline" => {
                self.open_status_line();
                return Ok(Flow::Continue);
            }
            "/model" => {
                let query = line.strip_prefix("/model").unwrap_or("").trim().to_owned();
                self.open_model_picker(&query);
                return Ok(Flow::Continue);
            }
            "/sessions" => {
                self.echo(&line);
                self.open_sessions();
                return Ok(Flow::Continue);
            }
            _ => {}
        }
        if !line.starts_with('/') {
            let index = self.input_index();
            self.repl.queue.facts.insert(index, line);
            return Ok(Flow::Continue);
        }
        let session = self.repl.session.id;
        let session_label = self.repl.session.label.clone();
        let step = self.repl.handle(&line)?;
        match &step {
            Step::Send {
                question,
                ephemeral,
            } => self.echo_turn(question, *ephemeral),
            _ => self.echo(&line),
        }
        if line == "/clear" {
            self.form.clear();
            self.images.clear();
            self.literal_input = false;
        }
        self.drain();
        if self.repl.session.id != session {
            self.refresh_model();
            self.refresh_previous_question();
            self.refresh_graph();
        } else if self.repl.session.label != session_label {
            self.refresh_graph();
        }
        match step {
            Step::Continue => {}
            Step::Send {
                question,
                ephemeral,
            } => *running = self.launch(&question, ephemeral, false)?,
            Step::Quit => return Ok(Flow::Quit),
        }
        Ok(Flow::Continue)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repl::tui::{tests::*, *};

    #[test]
    fn setup_commands_suspend_submission_without_losing_the_question_draft() {
        for command in ["/connect", "/login", "/logout"] {
            let settings = settings();
            let mut app = app(&settings);
            app.form.question = "Keep this unsent question".into();
            app.form.options = vec!["One".into(), "Two".into()];
            app.form.input = command.into();
            let transcript = app.transcript.len();
            app.submit(&mut None).unwrap();
            assert_eq!(app.pending_setup.as_deref(), Some(command));
            assert_eq!(app.form.question, "Keep this unsent question");
            assert_eq!(app.form.options, ["One", "Two"]);
            assert_eq!(app.transcript.len(), transcript);
            assert!(app.repl.queue.question.is_none());
        }
    }

    #[test]
    fn blank_enter_moves_sections_without_sending_and_ctrl_s_still_sends() {
        for kind in [Kind::Noul, Kind::Choice, Kind::Score] {
            let settings = settings();
            let mut app = app(&settings);
            app.form.kind = kind;
            app.form.input = "  ".into();
            app.form.cursor = 2;
            let mut running = None;
            app.key(
                KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
                &mut running,
            )
            .unwrap();
            assert_eq!(app.form.row, Row::Question);
            app.key(
                KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
                &mut running,
            )
            .unwrap();
            if kind == Kind::Noul {
                assert_eq!(app.form.row, Row::Input);
            } else {
                assert_eq!(app.form.row, Row::Option(0));
                app.key(
                    KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
                    &mut running,
                )
                .unwrap();
                assert_eq!(app.form.row, Row::Input);
                assert_eq!(app.form.options, [] as [String; 0]);
            }
            assert!(running.is_none());
            assert!(app.status.is_none());
            assert_eq!(app.repl.queue.facts, [] as [String; 0]);
            app.form.question = "Which fits?".into();
            if kind != Kind::Noul {
                app.form.options = vec!["First".into(), "Second".into()];
                app.form.go(Row::Option(1));
                app.key(
                    KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
                    &mut running,
                )
                .unwrap();
                assert_eq!(app.form.row, Row::Option(2));
                app.key(
                    KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
                    &mut running,
                )
                .unwrap();
                assert_eq!(app.form.row, Row::Input);
                assert_eq!(app.form.options, ["First", "Second"]);
                assert!(running.is_none());
            }
            let expected = app.form.build_question().unwrap();
            app.key(
                KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL),
                &mut running,
            )
            .unwrap();
            let draft = draft_from(running.unwrap());
            assert_eq!(draft.request.turn.questions, [expected]);
        }
    }

    #[test]
    fn enter_in_context_adds_a_row_and_never_sends() {
        let settings = settings();
        let mut app = app(&settings);
        app.form.question = "Will it rain?".into();
        app.form.kind = Kind::Noul;
        typed(&mut app, "One");
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE);
        typed(&mut app, "Two");
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE);
        assert_eq!(app.repl.queue.facts, ["One", "Two"]);
        assert_eq!(app.form.row, Row::Input);
        assert_eq!(app.form.input, "");
    }

    #[test]
    fn backspace_on_an_empty_context_row_reopens_the_one_above() {
        let settings = settings();
        let mut app = app(&settings);
        app.repl.queue.facts = vec!["One".into(), "Two".into()];
        press(&mut app, KeyCode::Backspace, KeyModifiers::NONE);
        assert_eq!(app.repl.queue.facts, ["One"]);
        assert_eq!((app.form.input.as_str(), app.form.cursor), ("Two", 3));
        press(&mut app, KeyCode::Backspace, KeyModifiers::NONE);
        assert_eq!(app.form.input, "Tw", "then it edits as usual");
        assert_eq!(app.repl.queue.facts, ["One"]);
    }

    #[test]
    fn arrows_walk_context_rows_and_leave_after_the_last() {
        let settings = settings();
        let mut app = app(&settings);
        app.repl.queue.facts = vec!["One".into(), "Two".into()];
        press(&mut app, KeyCode::Up, KeyModifiers::NONE);
        assert_eq!(app.form.input, "Two");
        press(&mut app, KeyCode::Up, KeyModifiers::NONE);
        assert_eq!((app.form.input.as_str(), app.form.cursor), ("One", 3));
        assert_eq!(app.repl.queue.facts, ["Two"], "the rest stays queued");
        // Editing keeps the row where it was.
        typed(&mut app, "!");
        press(&mut app, KeyCode::Down, KeyModifiers::NONE);
        assert_eq!(app.form.input, "Two");
        assert_eq!(app.repl.queue.facts, ["One!"]);
        // Down after the last row is the next box, with every row back.
        press(&mut app, KeyCode::Down, KeyModifiers::NONE);
        assert_eq!(app.form.row, Row::Question);
        assert_eq!(app.repl.queue.facts, ["One!", "Two"]);
        assert_eq!(app.form.input, "");
        // Coming back lands on the empty row at the end.
        press(&mut app, KeyCode::Up, KeyModifiers::NONE);
        assert_eq!((app.form.row, app.form.rows_below), (Row::Input, 0));
    }

    #[test]
    fn up_from_the_first_row_changes_box_and_keeps_the_order() {
        let settings = settings();
        let mut app = app(&settings);
        app.repl.queue.facts = vec!["One".into(), "Two".into()];
        press(&mut app, KeyCode::Up, KeyModifiers::NONE);
        press(&mut app, KeyCode::Up, KeyModifiers::NONE);
        press(&mut app, KeyCode::Up, KeyModifiers::NONE);
        assert_eq!(app.form.row, Row::Question);
        assert_eq!(app.repl.queue.facts, ["One", "Two"]);
    }

    #[test]
    fn a_row_emptied_while_walking_is_dropped() {
        let settings = settings();
        let mut app = app(&settings);
        app.repl.queue.facts = vec!["One".into(), "Two".into(), "Three".into()];
        press(&mut app, KeyCode::Up, KeyModifiers::NONE);
        press(&mut app, KeyCode::Up, KeyModifiers::NONE);
        app.form.clear_row();
        press(&mut app, KeyCode::Down, KeyModifiers::NONE);
        assert_eq!(app.form.input, "Three");
        press(&mut app, KeyCode::Tab, KeyModifiers::NONE);
        assert_eq!(app.repl.queue.facts, ["One", "Three"]);
    }

    #[test]
    fn enter_on_a_middle_row_opens_a_new_row_after_it() {
        let settings = settings();
        let mut app = app(&settings);
        app.repl.queue.facts = vec!["One".into(), "Three".into()];
        press(&mut app, KeyCode::Up, KeyModifiers::NONE);
        press(&mut app, KeyCode::Up, KeyModifiers::NONE);
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE);
        typed(&mut app, "Two");
        press(&mut app, KeyCode::Tab, KeyModifiers::NONE);
        assert_eq!(app.repl.queue.facts, ["One", "Two", "Three"]);
    }

    #[test]
    fn a_walked_row_stays_literal_and_a_typed_command_does_not_walk() {
        let settings = settings();
        let mut app = app(&settings);
        app.repl.queue.facts = vec!["/quit".into(), "Two".into()];
        press(&mut app, KeyCode::Up, KeyModifiers::NONE);
        press(&mut app, KeyCode::Up, KeyModifiers::NONE);
        assert_eq!(app.form.input, "/quit");
        assert_eq!(app.commands_shown(), [] as [(&str, &str); 0]);
        assert!(matches!(
            press(&mut app, KeyCode::Enter, KeyModifiers::NONE),
            Flow::Continue
        ));
        assert_eq!(app.repl.queue.facts, ["/quit", "Two"]);
        press(&mut app, KeyCode::Tab, KeyModifiers::NONE);
        press(&mut app, KeyCode::Tab, KeyModifiers::NONE);
        app.hidden = true;
        typed(&mut app, "/mo");
        app.hidden = true;
        press(&mut app, KeyCode::Up, KeyModifiers::NONE);
        assert_eq!(app.form.input, "/mo", "half a command is not queued");
    }

    #[test]
    fn sending_with_a_row_open_keeps_it_in_place() {
        let settings = settings();
        let mut app = app(&settings);
        app.repl.queue.facts = vec!["One".into(), "Two".into()];
        press(&mut app, KeyCode::Up, KeyModifiers::NONE);
        press(&mut app, KeyCode::Up, KeyModifiers::NONE);
        app.settle_context();
        assert_eq!(app.repl.queue.facts, ["One", "Two"]);
        assert_eq!((app.form.input.as_str(), app.form.rows_below), ("", 0));
    }

    #[test]
    fn a_reopened_fact_is_literal_text_and_never_a_command() {
        let settings = settings();
        let mut app = app(&settings);
        app.repl.queue.facts = vec!["// header".into()];
        press(&mut app, KeyCode::Backspace, KeyModifiers::NONE);
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE);
        assert_eq!(app.repl.queue.facts, ["// header"], "the evidence stays");
        assert_eq!(transcript(&app), "");
        // Imported text cannot run as a command either.
        app.repl.queue.facts = vec!["keep me".into(), "/cl".into()];
        app.form.question = "A draft".into();
        press(&mut app, KeyCode::Backspace, KeyModifiers::NONE);
        assert_eq!(app.commands_shown(), [] as [(&str, &str); 0]);
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE);
        assert_eq!(app.repl.queue.facts, ["keep me", "/cl"]);
        assert_eq!(app.form.question, "A draft");
    }

    #[test]
    fn a_reopened_fact_with_an_at_sign_opens_no_search_and_can_be_requeued() {
        let settings = settings();
        let mut app = app(&settings);
        app.repl.queue.facts = vec!["ping @bob".into()];
        press(&mut app, KeyCode::Backspace, KeyModifiers::NONE);
        assert!(app.references.picker.is_none());
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE);
        assert_eq!(app.repl.queue.facts, ["ping @bob"]);
    }

    #[test]
    fn a_reopened_path_is_not_taken_for_a_file_drop() {
        let settings = settings();
        let mut app = app(&settings);
        let path = std::env::temp_dir().join(format!("nouli-reopen-{}", Id::generate()));
        std::fs::write(&path, "FILE CONTENTS").unwrap();
        app.repl.queue.facts = vec![path.display().to_string()];
        typed(&mut app, "f");
        press(&mut app, KeyCode::Backspace, KeyModifiers::NONE);
        press(&mut app, KeyCode::Backspace, KeyModifiers::NONE);
        app.consume_drop();
        assert_eq!(app.form.input, path.display().to_string());
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn deleting_a_whole_literal_row_with_ctrl_d_restores_commands() {
        let settings = settings();
        let mut app = app(&settings);
        app.repl.queue.facts = vec!["x".into()];
        press(&mut app, KeyCode::Backspace, KeyModifiers::NONE);
        press(&mut app, KeyCode::Home, KeyModifiers::NONE);
        press(&mut app, KeyCode::Char('d'), KeyModifiers::CONTROL);
        assert!(!app.literal_input);
    }

    #[test]
    fn help_opens_the_dialog_whatever_follows_the_command() {
        let settings = settings();
        let mut app = app(&settings);
        typed(&mut app, "/help me");
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE);
        assert!(app.overlay.is_some());
        assert_eq!(transcript(&app), "");
    }

    #[test]
    fn changing_the_form_after_a_failure_takes_back_the_retry() {
        let keys = [
            (KeyCode::Char('c'), KeyModifiers::CONTROL),
            (KeyCode::Char('d'), KeyModifiers::CONTROL),
            (KeyCode::BackTab, KeyModifiers::NONE),
        ];
        for (code, modifiers) in keys {
            let settings = settings();
            let mut app = app(&settings);
            app.form.input = "A retry".into();
            app.status = Some(Status::failed("The provider did not answer"));
            press(&mut app, code, modifiers);
            assert!(app.status.is_none(), "{code:?}");
            app.form.input = "Other text".into();
            app.status = Some(Status::error("Cannot read that file"));
            press(&mut app, code, modifiers);
            assert!(app.status.is_some(), "a plain error stays: {code:?}");
        }
    }

    #[test]
    fn function_keys_do_nothing_and_ctrl_s_still_sends() {
        let settings = settings();
        let mut app = app(&settings);
        app.form.question = "Will it rain?".into();
        for number in 1..=12 {
            press(&mut app, KeyCode::F(number), KeyModifiers::NONE);
        }
        assert!(!app.selection.active);
        assert_eq!(app.form.question, "Will it rain?");
        assert!(app.status.is_none());
        let mut running = None;
        app.key(
            KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL),
            &mut running,
        )
        .unwrap();
        assert!(running.is_some());
    }

    #[test]
    fn shift_tab_switches_type_even_when_a_list_is_open() {
        let settings = settings();
        let mut app = app(&settings);
        typed(&mut app, "/");
        assert_ne!(app.commands_shown(), [] as [(&str, &str); 0]);
        press(&mut app, KeyCode::BackTab, KeyModifiers::NONE);
        assert_eq!(app.form.kind, Kind::Choice);
        assert_eq!(app.commands_shown(), [] as [(&str, &str); 0]);
        app.sessions = Some(vec![]);
        press(&mut app, KeyCode::Tab, KeyModifiers::SHIFT);
        assert!(app.sessions.is_none());
        assert_eq!(app.form.kind, Kind::Score);
        assert_eq!(app.form.input, "/", "mode changes keep typed input");
    }

    #[test]
    fn shift_tab_changes_types_globally_and_tab_moves_between_fields() {
        let settings = settings();
        let mut app = app(&settings);
        let none = KeyModifiers::NONE;
        press(&mut app, KeyCode::F(2), none);
        assert_eq!(app.form.kind, Kind::Noul, "only Shift-Tab changes types");
        press(&mut app, KeyCode::BackTab, none);
        assert_eq!((app.form.kind, app.form.row), (Kind::Choice, Row::Input));
        press(&mut app, KeyCode::Tab, KeyModifiers::SHIFT);
        assert_eq!((app.form.kind, app.form.row), (Kind::Score, Row::Input));
        press(&mut app, KeyCode::Tab, none);
        assert_eq!((app.form.kind, app.form.row), (Kind::Score, Row::Question));
        press(&mut app, KeyCode::Tab, none);
        assert_eq!((app.form.kind, app.form.row), (Kind::Score, Row::Option(0)));
        press(&mut app, KeyCode::Up, none);
        press(&mut app, KeyCode::Up, none);
        assert_eq!(app.form.row, Row::Input);
        // The graph pane takes the arrows until Esc gives them back.
        press(&mut app, KeyCode::Char('g'), KeyModifiers::CONTROL);
        press(&mut app, KeyCode::Down, none);
        assert_eq!((app.form.row, app.graph.follow), (Row::Input, false));
        press(&mut app, KeyCode::Tab, KeyModifiers::SHIFT);
        assert_eq!(app.form.kind, Kind::Noul);
        assert!(!app.graph_focused);
        press(&mut app, KeyCode::Esc, none);
        assert!(!app.graph_focused);
        press(&mut app, KeyCode::Down, none);
        assert_eq!(app.form.row, Row::Question);
    }

    #[test]
    fn the_input_row_queues_facts_and_runs_commands_like_the_line_prompt() {
        let settings = settings();
        let mut app = app(&settings);
        typed(&mut app, "The train is late.");
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE);
        assert_eq!(app.repl.queue.facts, ["The train is late."]);
        assert_eq!(app.form.input, "");
        assert!(app.transcript.is_empty(), "a queued fact shows in State");
        typed(&mut app, "/nonsense");
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE);
        typed(&mut app, "/replace 1 The train is cancelled.");
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE);
        typed(&mut app, "?broken");
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE);
        let shown = transcript(&app);
        assert!(
            shown.contains("s › /nonsense\n  unknown command /nonsense"),
            "{shown}"
        );
        assert!(shown.contains("queued: replace fact 1"), "{shown}");
        assert!(app.repl.queue.facts.contains(&"?broken".to_owned()));
        assert_eq!(app.repl.queue.replaces.len(), 1);
        // An empty line moves to Question without changing the queue.
        let queued = app.repl.queue.facts.len();
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE);
        assert_eq!(app.form.row, Row::Question);
        assert_eq!(app.repl.queue.facts.len(), queued);
        press(&mut app, KeyCode::Tab, KeyModifiers::NONE);
        typed(&mut app, "/quit");
        let flow = press(&mut app, KeyCode::Enter, KeyModifiers::NONE);
        assert!(matches!(flow, Flow::Quit));
    }

    #[test]
    fn a_question_that_cannot_be_sent_says_why_and_goes_where_to_fix_it() {
        let settings = settings();
        let mut app = app(&settings);
        press(&mut app, KeyCode::Tab, KeyModifiers::NONE);
        press(&mut app, KeyCode::Char('s'), KeyModifiers::CONTROL);
        let status = app.status.as_ref().unwrap();
        assert!(status.error && status.text.contains("Write a question"));
        assert_eq!(app.form.row, Row::Question);
        // An incomplete list focuses its editor.
        app.form.question = "rain: Will it rain?".into();
        press(&mut app, KeyCode::BackTab, KeyModifiers::NONE);
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE);
        assert_eq!(app.form.row, Row::Option(0));
        typed(&mut app, "Yes");
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE);
        press(&mut app, KeyCode::Char('s'), KeyModifiers::CONTROL);
        let status = app.status.as_ref().unwrap();
        assert!(status.text.contains("at least two"), "{}", status.text);
        assert_eq!(app.form.row, Row::Option(1), "ready for the second option");
        assert!(app.transcript.is_empty(), "nothing was sent");
    }

    #[test]
    fn ctrl_c_clears_the_row_then_everything_and_then_quits() {
        let settings = settings();
        let mut app = app(&settings);
        let control = KeyModifiers::CONTROL;
        app.repl.queue.facts.push("queued".into());
        typed(&mut app, "typing");
        assert!(matches!(
            press(&mut app, KeyCode::Char('c'), control),
            Flow::Continue
        ));
        assert_eq!(app.form.input, "");
        assert_eq!(app.repl.queue.facts.len(), 1);
        assert!(matches!(
            press(&mut app, KeyCode::Char('c'), control),
            Flow::Continue
        ));
        assert!(app.repl.queue.is_empty());
        assert!(matches!(
            press(&mut app, KeyCode::Char('c'), control),
            Flow::Quit
        ));
    }

    #[test]
    fn ctrl_d_deletes_forward_and_warns_once_before_discarding_input() {
        let settings = settings();
        let mut app = app(&settings);
        let control = KeyModifiers::CONTROL;
        typed(&mut app, "ab");
        press(&mut app, KeyCode::Home, KeyModifiers::NONE);
        press(&mut app, KeyCode::Char('d'), control);
        assert_eq!(app.form.input, "b");
        app.form.clear_row();
        app.repl.queue.facts.push("queued".into());
        assert!(matches!(
            press(&mut app, KeyCode::Char('d'), control),
            Flow::Continue
        ));
        assert!(app.warned);
        assert!(matches!(
            press(&mut app, KeyCode::Char('d'), control),
            Flow::Quit
        ));
    }

    #[test]
    fn a_running_turn_takes_only_ctrl_c_and_scrolling() {
        let settings = settings();
        let mut app = app(&settings);
        let cancel = CancellationToken::new();
        let mut running = Some(Running {
            turn: Box::pin(std::future::pending()),
            cancel: cancel.clone(),
            ephemeral: false,
            from_form: true,
            started: Instant::now(),
            model: ModelSelection::default(),
        });
        app.form.input = "sent".into();
        let key = |code, modifiers| KeyEvent::new(code, modifiers);
        app.key(key(KeyCode::Char('x'), KeyModifiers::NONE), &mut running)
            .unwrap();
        app.key(key(KeyCode::Char('d'), KeyModifiers::CONTROL), &mut running)
            .unwrap();
        assert_eq!(app.form.input, "sent", "the form stays as it was sent");
        assert!(
            app.status
                .as_ref()
                .unwrap()
                .text
                .contains("a turn is running")
        );
        app.key(key(KeyCode::PageUp, KeyModifiers::NONE), &mut running)
            .unwrap();
        assert_eq!(app.scroll, app.page);
        let flow = app
            .key(key(KeyCode::Char('c'), KeyModifiers::CONTROL), &mut running)
            .unwrap();
        assert!(matches!(flow, Flow::Continue));
        assert!(cancel.is_cancelled());
        assert!(running.is_some(), "the turn ends itself once cancelled");
    }
}
