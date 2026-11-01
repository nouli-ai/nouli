//! Terminal event dispatch, navigation, and submission.
use super::{
    App, Flow, Running, Status, Step, attachments,
    commands::{SETUP_COMMANDS, takes_argument},
    form::{Row, Section},
};
use crate::repl::Queue;
use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use std::{io, time::Instant};

impl<'a> App<'a> {
    pub(super) fn event(
        &mut self,
        event: Event,
        running: &mut Option<Running<'a>>,
    ) -> io::Result<Flow> {
        if self.selection.active && !matches!(event, Event::Key(_) | Event::Resize(_, _)) {
            return Ok(Flow::Continue);
        }
        match event {
            Event::Mouse(event) => {
                if !matches!(event.kind, crossterm::event::MouseEventKind::Moved) {
                    self.option_enter = false;
                }
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
                self.option_enter = false;
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
        if key.code == KeyCode::F(6) {
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

    /// Keys while the view is held for native selection: only Esc, F6, and
    /// Ctrl-C resume it, and Ctrl-C also cancels a running turn.
    pub(super) fn selection_key(&mut self, key: &KeyEvent, running: Option<&Running>) -> Flow {
        let control_c =
            key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c');
        if matches!(key.code, KeyCode::Esc | KeyCode::F(6)) || control_c {
            self.select_text(false);
            if control_c && running.is_some() {
                return self.interrupt(running);
            }
        }
        Flow::Continue
    }

    /// Ends what a key interrupts: a pending second Enter in a list, a file
    /// drop still being typed before moving between fields, and the Ctrl-D
    /// quit warning.
    pub(super) fn settle_before_key(&mut self, key: &KeyEvent, running: bool) {
        let control = key.modifiers.contains(KeyModifiers::CONTROL);
        if key.code != KeyCode::Enter {
            self.option_enter = false;
        }
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
            self.form.next_kind(&mut self.repl.queue.facts);
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
            KeyCode::F(5) => *running = self.send_form(false)?,
            KeyCode::Enter => {
                if !self.prepare_input() {
                    return Ok(Flow::Continue);
                }
                return self.enter(running);
            }
            KeyCode::Esc => self.status = None,
            KeyCode::Tab => self.form.tab(&mut self.repl.queue.facts),
            KeyCode::Up => self.form.up(&mut self.repl.queue.facts),
            KeyCode::Down => self.form.down(&mut self.repl.queue.facts),
            KeyCode::Backspace if self.form.cursor == 0 => {
                self.form.remove_empty(&mut self.repl.queue.facts);
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
        let facts = &mut self.repl.queue.facts;
        if !self.form.text(facts).is_empty() {
            self.form.clear_row(facts);
            if self.form.row == Row::Input {
                self.literal_input = false;
            }
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
        if !self.form.text(&self.repl.queue.facts).is_empty() {
            let delete = KeyEvent::new(KeyCode::Delete, KeyModifiers::NONE);
            self.edit_form(&delete);
            self.sync_references();
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
        if let Row::Option(index) = self.form.row {
            if std::mem::take(&mut self.option_enter) {
                *running = self.send_form(false)?;
            } else {
                self.form.enter_option(index);
                self.option_enter = true;
            }
            return Ok(Flow::Continue);
        }
        if self.form.row == Row::Input {
            let command = self.form.input.trim_start().starts_with('/') && !self.literal_input;
            // Before the first question, Enter moves context into the queue.
            if command || (self.form.question.trim().is_empty() && self.prior_for_kind().is_none())
            {
                return self.submit(running);
            }
        }
        *running = self.send_form(false)?;
        Ok(Flow::Continue)
    }

    /// Enter on the input row: the line goes to the loop as a fact, a `?`
    /// question, or a command, as it would at the line prompt. An empty line
    /// moves on to the question, unless a failed turn left its question to
    /// send again.
    pub(super) fn submit(&mut self, running: &mut Option<Running<'a>>) -> io::Result<Flow> {
        let line = self.form.input.trim().to_owned();
        if line.is_empty() && self.repl.queue.question.is_none() {
            self.form
                .focus(Section::Question, &mut self.repl.queue.facts);
            return Ok(Flow::Continue);
        }
        self.form.input.clear();
        self.form.cursor = 0;
        if std::mem::take(&mut self.literal_input) && !line.is_empty() {
            self.repl.queue.facts.push(line);
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
        match line.as_str() {
            "/help" => {
                self.echo(&line);
                self.help();
                return Ok(Flow::Continue);
            }
            "/sessions" => {
                self.echo(&line);
                self.open_sessions();
                return Ok(Flow::Continue);
            }
            _ => {}
        }
        let fact = !line.is_empty() && !line.starts_with('/');
        if fact {
            self.repl.queue.facts.push(line);
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
            // A queued fact shows in State until a turn sends it.
            _ if fact => {}
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
        for command in ["/connect", "/model", "/login", "/logout"] {
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
    fn option_and_level_lists_need_two_consecutive_enters_to_send() {
        for kind in [Kind::Choice, Kind::Score] {
            let settings = settings();
            let mut app = app(&settings);
            app.form.kind = kind;
            app.form.question = "Which fits?".into();
            app.form.options = vec!["First".into(), "Second".into()];
            app.form.row = Row::Option(1);
            app.form.cursor = "Second".len();
            press(&mut app, KeyCode::Enter, KeyModifiers::NONE);
            assert_eq!(app.form.row, Row::Option(2));
            assert!(app.option_enter);
            typed(&mut app, "Third");
            assert!(!app.option_enter, "typing continues the list");
            press(&mut app, KeyCode::Enter, KeyModifiers::NONE);
            assert_eq!(app.form.row, Row::Option(3));
            // Tab navigates away and cancels the pending confirmation.
            press(&mut app, KeyCode::Tab, KeyModifiers::NONE);
            assert!(!app.option_enter);
            press(&mut app, KeyCode::Tab, KeyModifiers::NONE);
            press(&mut app, KeyCode::Tab, KeyModifiers::NONE);
            assert_eq!(app.form.row, Row::Option(0));
            press(&mut app, KeyCode::Enter, KeyModifiers::NONE);
            app.event(Event::Paste("Another".into()), &mut None)
                .unwrap();
            assert!(!app.option_enter, "pasting also continues the list");
            press(&mut app, KeyCode::Enter, KeyModifiers::NONE);
            let expected = app.form.build_question().unwrap();
            let mut running = None;
            app.key(
                KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
                &mut running,
            )
            .unwrap();
            let draft = draft_from(running.unwrap());
            assert_eq!(draft.request.turn.questions, [expected]);
            assert!(!app.option_enter);
        }
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
        typed(&mut app, "/help");
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE);
        let shown = transcript(&app);
        assert!(shown.contains("Shift-Tab"), "{shown}");
        assert!(shown.contains("/blend A B as NAME"), "{shown}");
        // An empty line moves on to the question.
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE);
        assert_eq!(app.form.row, Row::Question);
        press(&mut app, KeyCode::Up, KeyModifiers::NONE);
        typed(&mut app, "/quit");
        let flow = press(&mut app, KeyCode::Enter, KeyModifiers::NONE);
        assert!(matches!(flow, Flow::Quit));
    }

    #[test]
    fn a_question_that_cannot_be_sent_says_why_and_goes_where_to_fix_it() {
        let settings = settings();
        let mut app = app(&settings);
        press(&mut app, KeyCode::Tab, KeyModifiers::NONE);
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE);
        let status = app.status.as_ref().unwrap();
        assert!(status.error && status.text.contains("Write a question"));
        assert_eq!(app.form.row, Row::Question);
        // An incomplete list focuses its editor; Tab adds another item.
        app.form.question = "rain: Will it rain?".into();
        press(&mut app, KeyCode::BackTab, KeyModifiers::NONE);
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE);
        assert_eq!(app.form.row, Row::Option(0));
        typed(&mut app, "Yes");
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE);
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE);
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
        app.form.clear_row(&mut app.repl.queue.facts);
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
