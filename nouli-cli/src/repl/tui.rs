//! The full-screen loop: the transcript, the question form under it, and the
//! session's decisions beside them. Like the line loop, it is a thin layer
//! over `Repl`, whose printed output fills the transcript. It runs each turn
//! while it keeps drawing, and Ctrl-C, which arrives as a key, cancels it.
mod ansi;
pub(crate) mod attachments;
mod draw;
mod form;
mod greeter;
mod mouse;
mod references;
mod sessions;
mod turn;

use super::{HELP, Queue, Repl, Step};
use crate::{
    inspect::InspectError,
    render::{
        self,
        turn::{DIM, TEAL},
    },
    turns::Draft,
    view::Placement,
};
use crossterm::{
    event::{
        DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
        Event, EventStream, KeyCode, KeyEvent, KeyEventKind, KeyModifiers,
    },
    execute,
};
use form::{Form, Kind, Row, Section};
use futures_util::StreamExt;
use nouli_core::{
    context::{ContextUpdate, evidence::marker::MarkerKind},
    decision::{AnswerSpace, Question},
    history::ModelSelection,
    identity::{Id, SessionSelector},
    protocol::{Report, ResultEnvelope},
    storage::{Ledger, OpenMode},
};
use ratatui::{DefaultTerminal, text::Line};
use std::{
    io,
    path::Path,
    pin::Pin,
    time::{Duration, Instant},
};
use tokio::{runtime::Runtime, time::MissedTickBehavior};
use tokio_util::sync::CancellationToken;

/// Commands handled by [`App::setup_command`], outside the turn loop.
const SETUP_COMMANDS: [&str; 4] = ["/connect", "/login", "/logout", "/model"];

/// The keys and TUI-only commands, for `/help` before the commands the line
/// loop lists. Setup commands prompt interactively, so only the TUI offers them.
const KEYS: [(&str, &str); 18] = [
    (
        "Mouse drag",
        "select text normally; Ctrl-G enables divider dragging and mouse scrolling",
    ),
    (
        "F6 / hold view",
        "hold the view for native drag-selection and terminal Copy; Esc or F6 resumes",
    ),
    ("Tab", "next field or option"),
    (
        "Shift-Tab",
        "next answer type: Yes / No → Choose one → Score",
    ),
    ("Up, Down", "the row above or below"),
    ("@", "find a saved answer or file in the launch folder"),
    ("Ctrl-O", "inspect the nearby saved reference"),
    (
        "Enter",
        "send; in Options/Levels, add an item then Enter again to send",
    ),
    ("Ctrl-S / F5", "send the question from any field"),
    (
        "Drop / paste files",
        "insert text, PDF contents, or an image into the focused field",
    ),
    ("PgUp, PgDn", "scroll the transcript"),
    (
        "Ctrl-G",
        "focus decisions for scrolling and divider dragging; Esc returns to text selection",
    ),
    ("Ctrl-C", "cancel, clear input, or quit"),
    ("/sessions", "resume an earlier session"),
    ("/connect", "configure a connection"),
    ("/login [NAME]", "store an API key"),
    ("/logout [NAME]", "remove a stored credential"),
    ("/model", "choose a connection and model"),
];

/// The slash commands, for the list that opens as the input row starts with
/// `/`: each one's usage and meaning, as `/help` lists them.
fn commands() -> impl Iterator<Item = (&'static str, &'static str)> {
    [("/help", "the keys and commands")].into_iter().chain(
        KEYS.into_iter()
            .chain(HELP)
            .filter(|(usage, _)| usage.starts_with('/')),
    )
}

/// The commands that start with what the input row holds, while it is still
/// a command name.
fn matching(input: &str) -> Vec<(&'static str, &'static str)> {
    if !input.starts_with('/') || input.contains(char::is_whitespace) {
        return vec![];
    }
    commands()
        .filter(|(usage, _)| usage.starts_with(input))
        .collect()
}

/// Whether a usage such as `/use NAME` needs more after the command name.
fn takes_argument(usage: &str) -> bool {
    usage
        .split_once(' ')
        .is_some_and(|(name, argument)| !name.ends_with(',') && !argument.starts_with('['))
}

/// Runs the view on the terminal until the person leaves, then puts the
/// terminal back as it was.
pub(super) fn run(repl: Repl<'_, Vec<u8>>, runtime: &Runtime) -> io::Result<()> {
    let mut app = App::new(repl);
    let mut screen = Screen::enter()?;
    let result = app.run(&mut screen.terminal, runtime);
    let restored = screen.leave();
    result.and(restored)
}

/// The terminal in raw mode on the alternate screen, with bracketed paste,
/// restored when left or dropped.
struct Screen {
    terminal: DefaultTerminal,
    entered: bool,
}

impl Screen {
    fn enter() -> io::Result<Self> {
        let terminal = match ratatui::try_init() {
            Ok(terminal) => terminal,
            Err(error) => {
                // Raw mode may already be on.
                if let Err(restore) = ratatui::try_restore() {
                    tracing::warn!(error = %restore, "cannot restore the terminal");
                }
                return Err(error);
            }
        };
        let screen = Self {
            terminal,
            entered: true,
        };
        execute!(io::stdout(), EnableBracketedPaste, DisableMouseCapture)?;
        Ok(screen)
    }

    fn leave(&mut self) -> io::Result<()> {
        if !std::mem::replace(&mut self.entered, false) {
            return Ok(());
        }
        let paste = execute!(io::stdout(), DisableBracketedPaste, DisableMouseCapture);
        ratatui::try_restore().and(paste)
    }
}

impl Drop for Screen {
    fn drop(&mut self) {
        if let Err(error) = self.leave() {
            tracing::warn!(%error, "cannot restore the terminal");
        }
    }
}

/// A turn on its way. It gives back the draft it sent when it ends.
struct Running<'a> {
    turn: Pin<Box<dyn Future<Output = (ResultEnvelope, Draft)> + 'a>>,
    cancel: CancellationToken,
    ephemeral: bool,
    /// Sent from the form, which empties once the turn is saved.
    from_form: bool,
    started: Instant,
    model: ModelSelection,
}

/// A line for the status bar.
struct Status {
    text: String,
    error: bool,
}

impl Status {
    fn info(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            error: false,
        }
    }

    fn error(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            error: true,
        }
    }
}

/// The session's decision graph as `nouli view` draws it, and where the pane
/// shows it.
#[derive(Default)]
struct Graph {
    /// Sessions visited in this view. Their histories stay independent.
    sessions: Vec<Id>,
    /// The most recently selected session with finalized decisions.
    selected: Option<Id>,
    lines: Vec<Line<'static>>,
    /// The latest card's row and columns.
    latest: Option<Placement>,
    /// Shows the end and the latest card until the pane is scrolled.
    follow: bool,
    x: usize,
    y: usize,
    /// The pane's height at the last draw, for paging.
    page: usize,
    /// Why the graph could not be read.
    problem: Option<String>,
    preferred_width: Option<u16>,
    body: ratatui::layout::Rect,
    divider: Option<ratatui::layout::Rect>,
    dragging: bool,
}

impl Graph {
    fn show(&mut self, text: &str, latest: Option<Placement>) {
        let lines: Vec<_> = text.lines().map(ansi::line).collect();
        if self.lines != lines {
            self.lines = lines;
            self.follow = true;
        }
        self.latest = latest;
        self.problem = None;
    }
}

enum Flow {
    Continue,
    Quit,
}

/// What woke the loop.
enum Wake {
    Input(Option<io::Result<Event>>),
    Done(Box<(ResultEnvelope, Draft)>),
    Tick,
}

#[derive(Clone)]
struct PreviousQuestion {
    turn: Id,
    question: Question,
}

impl PreviousQuestion {
    fn kind(&self) -> Kind {
        Kind::from(&self.question.answer_space)
    }
}

#[expect(
    clippy::struct_excessive_bools,
    reason = "the flags track independent pieces of interface state"
)]
struct App<'a> {
    repl: Repl<'a, Vec<u8>>,
    pending_setup: Option<String>,
    form: Form,
    transcript: Vec<Line<'static>>,
    /// How far the transcript is scrolled up from its end, in rows.
    scroll: usize,
    /// The transcript's height at the last draw, for paging.
    page: usize,
    /// The first form row shown when the form is taller than its space.
    composer_top: usize,
    graph: Graph,
    /// Whether the decisions pane has the keys instead of the form.
    graph_focused: bool,
    selection: mouse::Selection,
    status: Option<Status>,
    /// The provider and model that answered the last turn.
    model: Option<String>,
    /// Resolved selection for the next request, available before any answer.
    selected_model: ModelSelection,
    /// Ctrl-D warned that quitting discards what is queued.
    warned: bool,
    /// The sessions `/sessions` offers, while that list is open.
    sessions: Option<Vec<sessions::Entry>>,
    /// The chosen row of the open list.
    choice: usize,
    /// Esc hid the command list until the input row changes.
    hidden: bool,
    transcript_width: usize,
    images: Vec<nouli_core::media::Image>,
    drop_start: Option<(Row, usize, Instant)>,
    literal_input: bool,
    failed_drop: Option<(Row, String)>,
    previous_question: Option<PreviousQuestion>,
    /// The latest saved question reachable from the selected session.
    latest_question: Option<PreviousQuestion>,
    /// The previous Enter added/finished a list item; another Enter sends.
    option_enter: bool,
    references: references::References,
}

impl<'a> App<'a> {
    fn new(repl: Repl<'a, Vec<u8>>) -> Self {
        let mut app = Self {
            repl,
            pending_setup: None,
            form: Form::default(),
            transcript: Vec::new(),
            scroll: 0,
            page: 1,
            composer_top: 0,
            graph: Graph::default(),
            graph_focused: false,
            selection: mouse::Selection::default(),
            status: None,
            model: None,
            selected_model: ModelSelection::default(),
            warned: false,
            sessions: None,
            choice: 0,
            hidden: false,
            transcript_width: 80,
            images: Vec::new(),
            drop_start: None,
            literal_input: false,
            failed_drop: None,
            previous_question: None,
            latest_question: None,
            option_enter: false,
            references: references::References::in_directory(std::env::current_dir().ok()),
        };
        app.drain();
        app.refresh_model();
        app.refresh_previous_question();
        app.refresh_graph();
        app
    }

    /// Runs a setup command; returns the status line to show on success.
    fn setup_command(&mut self, command: &str) -> Result<&'static str, crate::connect::SetupError> {
        let (verb, argument) = command.split_once(' ').unwrap_or((command, ""));
        let mut note = "connection settings updated";
        match verb {
            "/connect" => {
                let selection = crate::connect::configure(
                    &self.repl.settings.config_path,
                    &crate::connect::Connect {
                        service: if argument.is_empty() {
                            None
                        } else {
                            Some(argument.into())
                        },
                        ..Default::default()
                    },
                )?;
                self.repl.model = selection;
            }
            "/model" => {
                let selection = crate::connect::pick_model(&self.repl.settings, argument)?;
                self.repl.model = selection;
            }
            "/login" | "/logout" => {
                let provider = if argument.is_empty() {
                    self.repl
                        .model
                        .provider
                        .clone()
                        .or(self.repl.settings.config.default_provider.clone())
                        .ok_or(crate::connect::SetupError::ConnectionRequired)?
                } else {
                    argument.into()
                };
                let auth = crate::settings::auth::path_for(&self.repl.settings.config_path);
                let report = crate::connect::SetupError::Report;
                if verb == "/logout" {
                    let outcome = crate::auth::log_out(&auth, &provider).map_err(report)?;
                    note = if outcome.removed {
                        "credential removed"
                    } else {
                        "no stored credential to remove"
                    };
                } else {
                    let disabled =
                        crate::auth::check_login(&self.repl.settings, &provider).map_err(report)?;
                    let key = crate::connect::read_key("API key: ")?;
                    let source = nouli_core::config::auth::KeySource::Key(key);
                    crate::auth::store_key(&auth, &provider, &source, disabled).map_err(report)?;
                    note = if disabled {
                        "credential stored; authentication is disabled for the default model"
                    } else {
                        "credential stored"
                    };
                }
            }
            _ => return Err(crate::connect::SetupError::UnknownCommand),
        }
        let mut settings = self.repl.settings.reload()?;
        settings
            .load_environment()
            .map_err(crate::connect::SetupError::Report)?;
        self.repl.settings = std::borrow::Cow::Owned(settings);
        Ok(note)
    }

    fn run(&mut self, terminal: &mut DefaultTerminal, runtime: &Runtime) -> io::Result<()> {
        // The timer needs the runtime; the turn runs only while the loop waits.
        let _context = runtime.enter();
        let mut events = EventStream::new();
        let mut tick = tokio::time::interval(Duration::from_millis(100));
        tick.set_missed_tick_behavior(MissedTickBehavior::Skip);
        let mut running: Option<Running<'a>> = None;
        let mut mouse_capture = false;
        loop {
            if let Some(command) = self.pending_setup.take() {
                drop(events);
                execute!(io::stdout(), DisableBracketedPaste, DisableMouseCapture)?;
                ratatui::try_restore()?;
                let result = self.setup_command(&command);
                *terminal = ratatui::try_init()?;
                execute!(io::stdout(), EnableBracketedPaste, DisableMouseCapture)?;
                mouse_capture = false;
                events = EventStream::new();
                self.status = Some(match result {
                    Ok(note) => Status::info(note),
                    Err(e) => Status::error(e.to_string()),
                });
                self.refresh_model();
            }
            if self.selection.should_draw() {
                terminal.draw(|frame| draw::screen(frame, self, running.as_ref()))?;
                self.selection.did_draw();
            }
            let capture = self.captures_mouse();
            if capture != mouse_capture {
                if capture {
                    execute!(io::stdout(), EnableMouseCapture)?;
                } else {
                    execute!(io::stdout(), DisableMouseCapture)?;
                }
                mouse_capture = capture;
            }
            let busy = running.is_some();
            let wake = runtime.block_on(async {
                tokio::select! {
                    event = events.next() => Wake::Input(event),
                    done = finished(&mut running) => Wake::Done(Box::new(done)),
                    // Redraws the spinner while a turn runs.
                    _ = tick.tick(), if !self.selection.active && (busy || self.drop_start.is_some()) => Wake::Tick,
                }
            });
            match wake {
                Wake::Input(None) => return Ok(()),
                Wake::Input(Some(event)) => {
                    if let Flow::Quit = self.event(event?, &mut running)? {
                        return Ok(());
                    }
                }
                Wake::Done(done) => {
                    let (envelope, draft) = *done;
                    if let Some(turn) = running.take() {
                        self.finish(
                            envelope,
                            &draft.request.updates,
                            turn.ephemeral,
                            turn.from_form,
                        )?;
                    }
                }
                Wake::Tick => {
                    if !busy
                        && self.drop_start.is_some_and(|(_, _, time)| {
                            time.elapsed() >= Duration::from_millis(150)
                        })
                    {
                        self.consume_drop();
                    }
                }
            }
        }
    }

    fn event(&mut self, event: Event, running: &mut Option<Running<'a>>) -> io::Result<Flow> {
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

    fn key(&mut self, key: KeyEvent, running: &mut Option<Running<'a>>) -> io::Result<Flow> {
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
    fn selection_key(&mut self, key: &KeyEvent, running: Option<&Running>) -> Flow {
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
    fn settle_before_key(&mut self, key: &KeyEvent, running: bool) {
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
    fn global_key(&mut self, key: &KeyEvent, running: Option<&Running>) -> Option<Flow> {
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
    fn form_key(&mut self, key: &KeyEvent, running: &mut Option<Running<'a>>) -> io::Result<Flow> {
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
    fn type_key(&mut self, key: &KeyEvent) {
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
    fn list_key(
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

    /// The commands the list shows under the input row, if any.
    fn commands_shown(&self) -> Vec<(&'static str, &'static str)> {
        if self.hidden || self.graph_focused || self.literal_input || self.form.row != Row::Input {
            return vec![];
        }
        matching(&self.form.input)
    }

    fn open_sessions(&mut self) {
        match sessions::entries(&self.repl.settings.store, self.repl.session.id) {
            Ok(entries) if entries.is_empty() => {
                self.status = Some(Status::info("no other session has turns yet"));
            }
            Ok(entries) => {
                self.sessions = Some(entries);
                self.choice = 0;
            }
            Err(error) => self.status = Some(Status::error(render::clean(&error.message))),
        }
    }

    /// Switches to the chosen session, as `/use` would.
    fn resume(&mut self) -> io::Result<()> {
        let choice = self.choice;
        let Some(entry) = self
            .sessions
            .take()
            .and_then(|entries| entries.into_iter().nth(choice))
        else {
            return Ok(());
        };
        self.repl.handle(&format!("/use {}", entry.id))?;
        self.drain();
        self.refresh_model();
        self.refresh_previous_question();
        self.refresh_graph();
        Ok(())
    }

    /// Keys that move the decisions pane while it has them; Esc and Tab give
    /// them back to the form. False for keys the pane leaves to the form.
    fn graph_key(&mut self, key: &KeyEvent) -> bool {
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

    /// Ctrl-C: cancels the running turn. Otherwise, as at the line prompt,
    /// it clears the row, then everything queued or typed, and then quits.
    fn interrupt(&mut self, running: Option<&Running>) -> Flow {
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
    fn end_of_input(&mut self, running: bool) -> Flow {
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

    fn enter(&mut self, running: &mut Option<Running<'a>>) -> io::Result<Flow> {
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
    fn submit(&mut self, running: &mut Option<Running<'a>>) -> io::Result<Flow> {
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

    fn paste_text(&mut self, text: &str) {
        self.option_enter = false;
        if let Row::Option(mut index) = self.form.row {
            let mut lines = text.lines().filter(|line| !line.trim().is_empty());
            if let Some(first) = lines.next() {
                self.form.paste(first, &mut self.repl.queue.facts);
                for line in lines {
                    self.form.enter_option(index);
                    index += 1;
                    self.form.paste(line, &mut self.repl.queue.facts);
                }
            }
        } else {
            self.form.paste(text, &mut self.repl.queue.facts);
        }
        self.status = None;
    }

    /// Shared attachment spans for editing and @-anchor detection.
    fn attachment_ranges(&self, text: &str) -> Vec<std::ops::Range<usize>> {
        let mut ranges: Vec<_> = MarkerKind::Image
            .ranges(text)
            .into_iter()
            .map(|tag| {
                let start = self
                    .images
                    .iter()
                    .find(|image| image.reference == text[tag.clone()])
                    .and_then(|image| {
                        text[..tag.start]
                            .strip_suffix(' ')
                            .and_then(|prefix| prefix.strip_suffix(&image.name))
                    })
                    .map_or(tag.start, str::len);
                start..tag.end
            })
            .collect();
        ranges.extend(MarkerKind::Decision.ranges(text).into_iter().map(|tag| {
            let start = self
                .references
                .bindings
                .iter()
                .find(|b| b.reference() == &text[tag.clone()])
                .and_then(|b| {
                    text[..tag.start]
                        .strip_suffix(' ')
                        .and_then(|prefix| prefix.strip_suffix(&b.label))
                })
                .map_or(tag.start, str::len);
            start..tag.end
        }));
        ranges
    }

    /// Imported files and saved references are edited as complete attachments.
    fn edit_form(&mut self, key: &KeyEvent) -> bool {
        let text = self.form.text(&self.repl.queue.facts);
        let ranges = self.attachment_ranges(text);
        let cursor = self.form.cursor;
        let control = key.modifiers.contains(KeyModifiers::CONTROL)
            && !key.modifiers.contains(KeyModifiers::ALT);
        let backwards = key.code == KeyCode::Backspace
            || control && matches!(key.code, KeyCode::Char('h' | 'w'));
        let word_backwards = key.code == KeyCode::Backspace
            && key
                .modifiers
                .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
            || control && key.code == KeyCode::Char('w');
        if word_backwards {
            let start = form::word_start(text, cursor);
            let touched: Vec<_> = ranges
                .iter()
                .filter(|range| start < range.end && cursor > range.start)
                .collect();
            if !touched.is_empty() {
                let start = touched
                    .iter()
                    .fold(start, |start, range| start.min(range.start));
                let end = touched.iter().fold(cursor, |end, range| end.max(range.end));
                self.form
                    .remove_range(start..end, &mut self.repl.queue.facts);
                return true;
            }
        }
        if let Some(range) = ranges.iter().find(|range| {
            backwards && cursor == range.end || key.code == KeyCode::Delete && cursor == range.start
        }) {
            self.form
                .remove_range(range.clone(), &mut self.repl.queue.facts);
            return true;
        }
        if key.code == KeyCode::Left
            && let Some(range) = ranges
                .iter()
                .find(|range| range.start < cursor && cursor <= range.end)
        {
            self.form.cursor = range.start;
            return true;
        }
        if key.code == KeyCode::Right
            && let Some(range) = ranges
                .iter()
                .find(|range| range.start <= cursor && cursor < range.end)
        {
            self.form.cursor = range.end;
            return true;
        }
        let edited = self.form.edit(key, &mut self.repl.queue.facts);
        if matches!(key.code, KeyCode::Left | KeyCode::Right)
            && let Some(range) = ranges
                .iter()
                .find(|range| range.start < self.form.cursor && self.form.cursor < range.end)
        {
            self.form.cursor = if key.code == KeyCode::Left {
                range.start
            } else {
                range.end
            };
        }
        edited
    }

    fn attach(&mut self, paths: &[std::path::PathBuf]) {
        let loaded = paths
            .iter()
            .map(|path| attachments::load(path))
            .collect::<Result<Vec<_>, _>>();
        match loaded {
            Ok(loaded) => self.insert_files(loaded),
            Err(error) => self.status = Some(Status::error(error.to_string())),
        }
    }

    fn insert_files(&mut self, loaded: Vec<attachments::Loaded>) {
        let count = loaded.len();
        if self.form.row == Row::Input {
            self.literal_input = true;
        }
        for (index, file) in loaded.into_iter().enumerate() {
            if index > 0 {
                if matches!(self.form.row, Row::Option(_)) {
                    self.form.add_option(&mut self.repl.queue.facts);
                } else {
                    self.form.paste(" ", &mut self.repl.queue.facts);
                }
            }
            self.option_enter = false;
            self.form.paste(&file.text, &mut self.repl.queue.facts);
            if let Some(image) = file.image {
                self.images.push(image);
            }
        }
        self.status = Some(Status::info(format!(
            "Inserted {count} file(s) here · review, then Send"
        )));
    }

    fn prepare_input(&mut self) -> bool {
        if !self.consume_drop() {
            return false;
        }
        if let Some((row, text)) = &self.failed_drop {
            if self.form.row_text(*row, &self.repl.queue.facts) == text
                && !(matches!(row, Row::Option(_)) && self.form.kind == Kind::Noul)
            {
                self.status = Some(Status::error(
                    "Edit or remove the file path that could not be imported.",
                ));
                return false;
            }
            self.failed_drop = None;
        }
        true
    }

    fn consume_drop(&mut self) -> bool {
        let Some((row, start, _)) = self.drop_start.take() else {
            return true;
        };
        if row != self.form.row {
            return true;
        }
        let end = self.form.cursor;
        let text = self.form.text(&self.repl.queue.facts);
        let Some(candidate) = text.get(start..end) else {
            return true;
        };
        let Some(paths) = attachments::paths(candidate) else {
            return true;
        };
        match paths
            .iter()
            .map(|path| attachments::load(path))
            .collect::<Result<Vec<_>, _>>()
        {
            Ok(loaded) => {
                self.form
                    .remove_range(start..end, &mut self.repl.queue.facts);
                self.insert_files(loaded);
            }
            Err(error) => {
                self.status = Some(Status::error(error.to_string()));
                self.failed_drop = Some((
                    self.form.row,
                    self.form.text(&self.repl.queue.facts).to_owned(),
                ));
                return false;
            }
        }
        true
    }

    /// Shows a finished turn as the line loop would. A saved turn has taken
    /// the queue, and its question when it came from the form.
    fn finish(
        &mut self,
        envelope: ResultEnvelope,
        updates: &[ContextUpdate],
        ephemeral: bool,
        from_form: bool,
    ) -> io::Result<()> {
        let saved = !ephemeral && matches!(envelope.result, Report::Turn(_));
        let next_latest = match &envelope.result {
            Report::Turn(report) if report.session == Some(self.repl.session.id) => report
                .turn
                .zip(report.questions.last())
                .map(|(turn, question)| PreviousQuestion {
                    turn,
                    question: question.clone(),
                }),
            _ => None,
        };
        let next_previous = match &envelope.result {
            Report::Turn(report) if report.session == Some(self.repl.session.id) => {
                match (report.turn, report.questions.as_slice()) {
                    (Some(turn), [question])
                        if !matches!(question.answer_space, AnswerSpace::Noul) =>
                    {
                        Some(PreviousQuestion {
                            turn,
                            question: question.clone(),
                        })
                    }
                    _ => None,
                }
            }
            _ => None,
        };
        if let Report::Turn(report) = &envelope.result {
            let provider = &report.outcome.provider;
            self.model = Some(format!(
                "{}/{}",
                render::clean(&provider.provider),
                render::clean(&provider.model)
            ));
        }
        if let Report::Error(error) = &envelope.result {
            self.status = Some(if error.code == "evaluation_canceled" {
                Status::info("Cancelled · your input is kept")
            } else {
                Status::error(format!(
                    "{} · edit input or Ctrl-S to retry",
                    render::clean(&error.message)
                ))
            });
        }
        self.repl.finish_display(
            envelope.result,
            updates,
            ephemeral,
            Some(self.transcript_width),
        )?;
        self.drain();
        if saved {
            self.previous_question = next_previous;
            self.latest_question = next_latest;
            self.form.sent(from_form);
            self.images.clear();
            self.restore_references();
            self.refresh_graph();
        }
        Ok(())
    }

    fn refresh_graph(&mut self) {
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

    fn prior_for_kind(&self) -> Option<&PreviousQuestion> {
        self.previous_question
            .as_ref()
            .filter(|previous| previous.kind() == self.form.kind && self.form.kind != Kind::Noul)
    }

    fn inherited_answers(&self) -> Option<&PreviousQuestion> {
        if self.form.labels().next().is_some() {
            None
        } else {
            self.prior_for_kind()
        }
    }

    fn refresh_previous_question(&mut self) {
        self.restore_references();
        self.previous_question = None;
        self.latest_question = None;
        let Some(export) = self
            .repl
            .ledger()
            .ok()
            .and_then(|ledger| ledger.export_session(&self.repl.selector()).ok())
        else {
            return;
        };
        self.latest_question = export.turns.iter().rev().find_map(|turn| {
            turn.request
                .questions
                .last()
                .map(|question| PreviousQuestion {
                    turn: turn.id,
                    question: question.clone(),
                })
        });
        let [head] = export.session.frontier.as_slice() else {
            return;
        };
        let Some(turn) = export
            .turns
            .iter()
            .find(|turn| turn.id == *head && turn.session == self.repl.session.id)
        else {
            return;
        };
        let [question] = turn.request.questions.as_slice() else {
            return;
        };
        let previous = PreviousQuestion {
            turn: turn.id,
            question: question.clone(),
        };
        if self.form.is_empty() {
            self.form.kind = previous.kind();
        }
        if previous.kind() != Kind::Noul {
            self.previous_question = Some(previous);
        }
    }

    fn refresh_model(&mut self) {
        let session = self
            .repl
            .ledger()
            .and_then(|ledger| {
                ledger
                    .session(&self.repl.selector())
                    .map_err(|error| error.report())
            })
            .map(|session| session.defaults)
            .unwrap_or_default();
        self.selected_model = crate::selection::model(
            Some(&self.repl.settings.config),
            &[&self.repl.model, &session],
        );
        self.model = None;
    }

    /// Moves what the loop printed into the transcript, which then shows its end.
    fn drain(&mut self) {
        let printed: Vec<_> = String::from_utf8_lossy(&self.repl.out)
            .lines()
            .map(|line| ansi::line(&render::image_labels(line)))
            .collect();
        self.repl.out.clear();
        if !printed.is_empty() {
            self.transcript.extend(printed);
            self.scroll = 0;
        }
    }

    /// A line as the line prompt shows it once typed.
    fn echo(&mut self, text: &str) {
        let prompt = format!("{} › ", self.repl.session.label);
        let prompt = self.repl.paint.apply(TEAL, &prompt);
        self.transcript.push(ansi::line(&format!(
            "{prompt}{}",
            render::clean(&render::image_labels(text))
        )));
        self.scroll = 0;
    }

    /// The lines that would send this turn at the line prompt: each queued
    /// fact, then the question. Queued replacements already showed as commands.
    fn echo_turn(&mut self, question: &str, _ephemeral: bool) {
        let asked = question.to_owned();
        let lines: Vec<String> = self
            .repl
            .queue
            .facts
            .iter()
            .cloned()
            .chain([asked])
            .collect();
        for line in lines {
            self.echo(&line);
        }
    }

    fn help(&mut self) {
        for (name, meaning) in KEYS
            .into_iter()
            .chain(HELP.into_iter().filter(|(name, _)| name.starts_with('/')))
        {
            let line = self
                .repl
                .paint
                .apply(DIM, &format!("  {name:<22}{meaning}"));
            self.transcript.push(ansi::line(&line));
        }
        self.scroll = 0;
    }
}

/// Waits for the running turn, or forever when there is none.
async fn finished(running: &mut Option<Running<'_>>) -> (ResultEnvelope, Draft) {
    match running {
        Some(turn) => turn.turn.as_mut().await,
        None => std::future::pending().await,
    }
}

/// The session's decision graph as the text `nouli view` draws, without its
/// header, and the latest card's first row and columns.
fn graph_text(
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{repl::tests::repl, settings::Settings};
    use nouli_core::protocol::ErrorReport;
    use std::fmt::Write as _;

    pub(super) fn app(settings: &Settings) -> App<'_> {
        App::new(repl(settings, Queue::default()))
    }

    fn press(app: &mut App, code: KeyCode, modifiers: KeyModifiers) -> Flow {
        let mut running = None;
        let flow = app
            .key(KeyEvent::new(code, modifiers), &mut running)
            .unwrap();
        assert!(running.is_none(), "no turn starts in these tests");
        flow
    }

    fn typed(app: &mut App, text: &str) {
        for c in text.chars() {
            press(app, KeyCode::Char(c), KeyModifiers::NONE);
        }
    }

    fn transcript(app: &App) -> String {
        let mut text = String::new();
        for line in &app.transcript {
            writeln!(text, "{line}").unwrap();
        }
        text
    }

    pub(super) fn settings() -> Settings {
        Settings::for_test(std::env::temp_dir().join(format!("nouli-tui-{}", Id::generate())))
    }

    pub(super) fn failed() -> ResultEnvelope {
        ResultEnvelope::new(Report::Error(ErrorReport {
            category: nouli_core::protocol::ErrorCategory::Transport,
            code: "provider_transport".into(),
            message: "The provider did not answer".into(),
            retryable: true,
        }))
    }

    pub(super) fn draft_from(turn: Running<'_>) -> Draft {
        turn.cancel.cancel();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(turn.turn).1
    }

    pub(super) fn previous(kind: Kind) -> PreviousQuestion {
        use nouli_core::decision::{ChoiceOption, Criterion};
        PreviousQuestion {
            turn: Id::generate(),
            question: Question {
                id: "original_question".into(),
                prompt: "Which destination suits this trip?".into(),
                criteria: vec![Criterion::Detailed {
                    label: "Sunlight".into(),
                    description: "Prefer winter sunshine".into(),
                }],
                answer_space: match kind {
                    Kind::Noul => AnswerSpace::Noul,
                    Kind::Choice => AnswerSpace::Choice {
                        options: vec![
                            ChoiceOption {
                                id: "original_a".into(),
                                description: "Palm Springs".into(),
                            },
                            ChoiceOption {
                                id: "original_b".into(),
                                description: "San Diego".into(),
                            },
                        ],
                    },
                    Kind::Score => AnswerSpace::Score {
                        levels: vec![
                            Criterion::Detailed {
                                label: "Low".into(),
                                description: "Cold and cloudy".into(),
                            },
                            Criterion::Detailed {
                                label: "High".into(),
                                description: "Warm and sunny".into(),
                            },
                        ],
                    },
                },
            },
        }
    }

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
    fn help_lists_exactly_the_setup_commands_the_tui_dispatches() {
        let listed: Vec<&str> = KEYS
            .iter()
            .filter_map(|(usage, _)| usage.split_whitespace().next())
            .filter(|name| name.starts_with('/') && *name != "/sessions")
            .collect();
        assert_eq!(listed, SETUP_COMMANDS);
    }

    #[test]
    fn logout_removes_a_stored_credential_for_an_unconfigured_provider_like_auth_logout() {
        use std::os::unix::fs::PermissionsExt;
        let root = std::env::temp_dir().join(format!("nouli-tui-logout-{}", Id::generate()));
        std::fs::create_dir_all(&root).unwrap();
        let config = root.join("config.toml");
        std::fs::write(
            &config,
            "default_provider = \"local\"\ndefault_model = \"offline\"\n[providers.local]\nprotocol = \"system-one\"\nbase_url = \"http://127.0.0.1:9\"\n[providers.local.models.offline]\ncontext_budget_tokens = 100000\nreserved_output_tokens = 128\n",
        )
        .unwrap();
        let auth = crate::settings::auth::path_for(&config);
        std::fs::write(
            &auth,
            "[providers.ghost]\ntype = \"api_key\"\nkey = \"secret\"\n",
        )
        .unwrap();
        std::fs::set_permissions(&auth, std::fs::Permissions::from_mode(0o600)).unwrap();
        let mut settings = Settings::for_test(root.join("store"));
        settings.config_path.clone_from(&config);
        let mut app = app(&settings);
        assert_eq!(
            app.setup_command("/logout ghost").unwrap(),
            "credential removed"
        );
        assert_eq!(
            app.setup_command("/logout ghost").unwrap(),
            "no stored credential to remove"
        );
        let remaining = crate::settings::auth::update(&auth, |store| {
            let present = store.remove("ghost");
            (present, false)
        })
        .unwrap();
        assert!(!remaining, "ghost's credential was not removed");
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn image_labels_edit_as_attachments_without_exposing_reference_characters() {
        let settings = settings();
        let mut app = app(&settings);
        let name = "photo \"中文\".png";
        let reference = MarkerKind::Image.marker(Id::generate());
        let text = format!("{name} {reference}");
        app.insert_files(vec![attachments::Loaded {
            text: text.clone(),
            image: Some(nouli_core::media::Image {
                reference: reference.clone(),
                name: name.into(),
                format: "png".into(),
                data_base64: String::new(),
            }),
        }]);
        assert_eq!(app.form.input, text);
        for character in ['h', 'w'] {
            app.form.input = text.clone();
            app.form.cursor = text.len();
            press(
                &mut app,
                KeyCode::Char(character),
                KeyModifiers::CONTROL | KeyModifiers::ALT,
            );
            assert_eq!(
                app.form.input,
                format!("{text}{character}"),
                "AltGr characters must preserve the attachment"
            );
        }
        app.form.input = text.clone();
        app.form.cursor = text.len();
        press(&mut app, KeyCode::Left, KeyModifiers::NONE);
        assert_eq!(app.form.cursor, 0);
        press(&mut app, KeyCode::Right, KeyModifiers::NONE);
        assert_eq!(app.form.cursor, text.len());
        press(&mut app, KeyCode::Backspace, KeyModifiers::NONE);
        assert_eq!(app.form.input, "");
        app.form.input = text.clone();
        app.form.cursor = 0;
        press(&mut app, KeyCode::Char('d'), KeyModifiers::CONTROL);
        assert!(
            app.form.input.is_empty(),
            "Ctrl-D also removes the full attachment"
        );
        for (key, modifiers) in [
            (KeyCode::Char('w'), KeyModifiers::CONTROL),
            (KeyCode::Backspace, KeyModifiers::CONTROL),
            (KeyCode::Backspace, KeyModifiers::ALT),
        ] {
            app.form.input = format!("prefix {text} ");
            app.form.cursor = app.form.input.len();
            press(&mut app, key, modifiers);
            assert_eq!(
                app.form.input, "prefix ",
                "word deletion must include the attachment, not just its hidden reference"
            );
        }
        app.form.input.clear();
        app.repl
            .out
            .extend_from_slice(format!("photo \"中文\".png {reference}\n").as_bytes());
        app.drain();
        assert!(
            !transcript(&app).contains(MarkerKind::Image.prefix()),
            "all human transcript output hides image metadata"
        );
        app.form.question = "A text-only question?".into();
        let draft = draft_from(app.send_form(false).unwrap().unwrap());
        assert!(
            draft.request.updates.is_empty(),
            "deleted attachments are not sent"
        );
    }

    #[test]
    fn only_saved_answers_replace_the_previous_question() {
        let settings = settings();
        let mut app = app(&settings);
        let previous = previous(Kind::Score);
        app.previous_question = Some(previous.clone());
        app.form.kind = Kind::Score;
        app.form.question = "A new judgment?".into();
        app.finish(failed(), &[], false, true).unwrap();
        assert_eq!(app.previous_question.as_ref().unwrap().turn, previous.turn);
        assert_eq!(app.form.question, "A new judgment?");
        let Report::Turn(mut report) = crate::repl::tests::saved_turn() else {
            unreachable!()
        };
        report.session = Some(app.repl.session.id);
        report.questions = vec![previous.question.clone()];
        app.finish(
            ResultEnvelope::new(Report::Turn(report.clone())),
            &[],
            true,
            true,
        )
        .unwrap();
        assert_eq!(app.previous_question.as_ref().unwrap().turn, previous.turn);
        assert_eq!(app.form.question, "A new judgment?");
        app.finish(
            ResultEnvelope::new(Report::Turn(report.clone())),
            &[],
            false,
            true,
        )
        .unwrap();
        assert_eq!(
            app.previous_question.as_ref().unwrap().turn,
            report.turn.unwrap()
        );
        assert_eq!(app.form.kind, Kind::Score);
        assert!(app.form.is_empty());
        report.questions[0].answer_space = AnswerSpace::Noul;
        app.finish(ResultEnvelope::new(Report::Turn(report)), &[], false, true)
            .unwrap();
        assert!(app.previous_question.is_none());
        app.previous_question = Some(previous);
        // Missing/empty sessions cannot retain another session's definition.
        app.repl.session.id = Id::generate();
        app.refresh_previous_question();
        assert!(app.previous_question.is_none());
    }

    #[test]
    fn imported_commands_stay_literal_and_clearing_restores_the_command_palette() {
        let settings = settings();
        let mut app = app(&settings);
        app.insert_files(vec![attachments::Loaded {
            text: "/cl".into(),
            image: None,
        }]);
        assert_eq!(app.commands_shown(), [] as [(&str, &str); 0]);
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE);
        assert_eq!(app.repl.queue.facts, ["/cl"]);
        app.insert_files(vec![attachments::Loaded {
            text: "/quit".into(),
            image: None,
        }]);
        press(&mut app, KeyCode::Char('c'), KeyModifiers::CONTROL);
        typed(&mut app, "/cl");
        assert_ne!(app.commands_shown(), [] as [(&str, &str); 0]);
    }

    #[test]
    fn raw_drop_is_resolved_before_immediate_send_and_only_submitted_images_are_required() {
        let settings = settings();
        let mut app = app(&settings);
        let dir = std::env::temp_dir().join(format!("nouli-drop-send-{}", Id::generate()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("photo \"test\" 中文.png");
        image::RgbImage::new(2, 2).save(&path).unwrap();
        app.form.kind = Kind::Choice;
        app.form.question = "Which image: [this] or <that>?".into();
        app.form.options = vec![String::new(), "Other".into()];
        app.form.row = Row::Option(0);
        let quoted = shlex::try_quote(path.to_str().unwrap()).unwrap();
        typed(&mut app, &quoted);
        let mut running = None;
        app.key(
            KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL),
            &mut running,
        )
        .unwrap();
        let draft = draft_from(running.unwrap());
        assert_eq!(draft.request.turn.questions[0].prompt, app.form.question);
        let record = draft.request.updates.last().unwrap().new_record().unwrap();
        assert_eq!(draft.request.required_records, [record.id]);
        let image = record.as_image().unwrap().unwrap();
        assert!(image.name.contains("\"test\""));
        assert!(app.form.options[0].contains(MarkerKind::Image.prefix()));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn options_paste_as_separate_rows_and_shift_tab_cycles_answer_types() {
        let settings = settings();
        let mut app = app(&settings);
        app.form.kind = Kind::Choice;
        app.form.focus(Section::Options, &mut app.repl.queue.facts);
        app.event(
            Event::Paste("Palm Springs\nDenver\nLos Angeles\nSan Diego\nSan Francisco".into()),
            &mut None,
        )
        .unwrap();
        assert_eq!(app.form.options.len(), 5);
        assert_eq!(app.form.options[3], "San Diego");
        press(&mut app, KeyCode::BackTab, KeyModifiers::NONE);
        assert_eq!((app.form.kind, app.form.row), (Kind::Score, Row::Option(4)));
        press(&mut app, KeyCode::Tab, KeyModifiers::SHIFT);
        assert_eq!((app.form.kind, app.form.row), (Kind::Noul, Row::Question));
        press(&mut app, KeyCode::BackTab, KeyModifiers::SHIFT);
        assert_eq!((app.form.kind, app.form.row), (Kind::Choice, Row::Question));
        press(&mut app, KeyCode::Tab, KeyModifiers::NONE);
        assert_eq!(app.form.row, Row::Option(0));
        assert_eq!(app.form.kind, Kind::Choice);
        press(&mut app, KeyCode::Tab, KeyModifiers::NONE);
        assert_eq!(app.form.row, Row::Option(1));
        for _ in 0..4 {
            press(&mut app, KeyCode::Tab, KeyModifiers::NONE);
        }
        assert_eq!(app.form.row, Row::Option(5));
        press(&mut app, KeyCode::Tab, KeyModifiers::NONE);
        assert_eq!(app.form.row, Row::Input);
    }

    #[test]
    fn an_idle_drop_failure_blocks_both_enter_and_send_until_corrected() {
        let settings = settings();
        let mut app = app(&settings);
        let path = std::env::temp_dir().join(format!("nouli-binary-{}", Id::generate()));
        std::fs::write(&path, [0_u8, 255, 0]).unwrap();
        typed(&mut app, path.to_str().unwrap());
        assert!(!app.consume_drop()); // the idle-timer path
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE);
        assert_eq!(app.repl.queue.facts, [] as [std::string::String; 0]);
        app.form.question = "Is this relevant?".into();
        assert!(app.send_form(false).unwrap().is_none());
        app.form.clear_row(&mut app.repl.queue.facts);
        assert!(app.send_form(false).unwrap().is_some());
        std::fs::remove_file(path).unwrap();
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
    fn a_failed_file_paste_cancels_the_pending_second_enter() {
        let settings = settings();
        let mut app = app(&settings);
        app.form.kind = Kind::Choice;
        app.form.question = "Which fits?".into();
        app.form.options = vec!["First".into(), "Second".into()];
        app.form.row = Row::Option(1);
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE);
        assert!(app.option_enter);
        let path = std::env::temp_dir().join(format!("nouli-list-drop-{}", Id::generate()));
        std::fs::write(&path, [0, 255]).unwrap();
        app.event(Event::Paste(path.to_string_lossy().into_owned()), &mut None)
            .unwrap();
        assert!(app.status.as_ref().unwrap().error);
        assert!(!app.option_enter);
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE);
        assert!(
            app.option_enter,
            "the next Enter arms again instead of sending"
        );
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn changing_fields_immediately_after_a_raw_drop_keeps_the_original_destination() {
        let settings = settings();
        let mut app = app(&settings);
        let path = std::env::temp_dir().join(format!("nouli-tab-drop-{}", Id::generate()));
        std::fs::write(&path, "Imported context.").unwrap();
        typed(&mut app, path.to_str().unwrap());
        press(&mut app, KeyCode::Tab, KeyModifiers::NONE);
        assert_eq!(app.form.row, Row::Question);
        assert_eq!(app.form.input, "Imported context.");
        std::fs::remove_file(path).unwrap();
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

    #[test]
    fn a_saved_turn_empties_the_form_and_a_failed_one_keeps_it() {
        let settings = settings();
        let mut app = app(&settings);
        let fill = |app: &mut App| {
            app.repl.queue.facts.push("a fact".into());
            app.form.question = "q: Is it?".into();
            app.form.row = Row::Question;
        };
        fill(&mut app);
        app.finish(failed(), &[], false, true).unwrap();
        assert_eq!(app.form.question, "q: Is it?");
        assert_eq!(app.repl.queue.facts.len(), 1);
        assert!(transcript(&app).contains("provider_transport"));
        let saved = ResultEnvelope::new(crate::repl::tests::saved_turn());
        app.finish(saved, &[], false, true).unwrap();
        assert_eq!(app.form.question, "");
        assert!(app.repl.queue.is_empty());
        assert_eq!(app.form.row, Row::Input);
        assert!(app.model.is_some());
    }

    #[test]
    fn the_sessions_list_keeps_the_keys_until_it_closes() {
        let settings = settings();
        let mut app = app(&settings);
        let entry = |label: &str| sessions::Entry {
            id: Id::generate(),
            label: label.into(),
            first: "a fact".into(),
            turns: 1,
        };
        let open = |app: &mut App| app.sessions = Some(vec![entry("a"), entry("b")]);
        open(&mut app);
        // Tab and typing do nothing while it is open; Down moves within it.
        press(&mut app, KeyCode::Tab, KeyModifiers::NONE);
        typed(&mut app, "x");
        press(&mut app, KeyCode::Down, KeyModifiers::NONE);
        press(&mut app, KeyCode::Down, KeyModifiers::NONE);
        assert_eq!((app.choice, app.form.input.as_str()), (1, ""));
        press(&mut app, KeyCode::Esc, KeyModifiers::NONE);
        assert!(app.sessions.is_none());
        open(&mut app);
        press(&mut app, KeyCode::Char('g'), KeyModifiers::CONTROL);
        assert!(app.sessions.is_none());
        assert!(app.graph_focused);
        press(&mut app, KeyCode::Esc, KeyModifiers::NONE);
        assert!(!app.graph_focused);
        open(&mut app);
        press(&mut app, KeyCode::Char('c'), KeyModifiers::CONTROL);
        assert!(
            app.sessions.is_none(),
            "Ctrl-C closes it before anything else"
        );
        // Enter resumes through /use, which reports a session it cannot find.
        open(&mut app);
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE);
        assert!(app.sessions.is_none());
        assert!(
            transcript(&app).contains("no session"),
            "{}",
            transcript(&app)
        );
    }

    #[test]
    fn typing_a_slash_lists_the_commands_and_enter_or_tab_takes_one() {
        let settings = settings();
        let mut app = app(&settings);
        app.repl.queue.facts.push("queued".into());
        typed(&mut app, "/");
        assert!(app.commands_shown().len() > 10, "every command at first");
        typed(&mut app, "cl");
        assert_eq!(app.commands_shown(), [("/clear", "empty the queue")]);
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE);
        assert!(app.repl.queue.is_empty(), "Enter ran /clear");
        // Tab fills in a command that needs a name, and the list closes.
        typed(&mut app, "/us");
        press(&mut app, KeyCode::Tab, KeyModifiers::NONE);
        assert_eq!(app.form.input, "/use ");
        assert_eq!(app.commands_shown(), [] as [(&str, &str); 0]);
        // Esc hides the list until the row changes.
        app.form.clear_row(&mut app.repl.queue.facts);
        typed(&mut app, "/v");
        press(&mut app, KeyCode::Esc, KeyModifiers::NONE);
        assert_eq!(app.commands_shown(), [] as [(&str, &str); 0]);
        typed(&mut app, "i");
        assert_eq!(app.commands_shown().len(), 1);
        // With no other session, /sessions says so.
        app.form.clear_row(&mut app.repl.queue.facts);
        typed(&mut app, "/sessions");
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE);
        assert!(app.sessions.is_none());
        assert!(
            app.status
                .as_ref()
                .unwrap()
                .text
                .contains("no other session")
        );
    }

    #[test]
    fn a_question_sent_from_the_form_replaces_one_a_failed_turn_left() {
        let settings = settings();
        let mut app = app(&settings);
        app.repl.queue.question = Some("a: Is the first one still waiting?".into());
        app.form.question = "b: Is this the one to send again?".into();
        app.form.row = Row::Question;
        assert!(app.send_form(false).unwrap().is_some());
        assert_eq!(app.repl.queue.question, None);
        app.finish(failed(), &[], false, true).unwrap();
        assert_eq!(app.form.question, "b: Is this the one to send again?");
        // Enter from Context resends the current form, including after failure.
        press(&mut app, KeyCode::Up, KeyModifiers::NONE);
        let mut running = None;
        app.key(
            KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
            &mut running,
        )
        .unwrap();
        let draft = draft_from(running.unwrap());
        assert_eq!(draft.request.turn.questions[0].prompt, app.form.question);
    }

    #[test]
    fn ctrl_c_ends_a_turn_whose_provider_never_answers() {
        // A provider that takes the connection and never answers.
        let provider = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let mut settings = settings();
        settings.config = nouli_core::config::Config::parse(&format!(
            "default_provider = 'local'\ndefault_model = 'offline'\n\
             [providers.local]\nprotocol = 'system-one'\nbase_url = 'http://{}'\n\
             [providers.local.models.offline]\ncontext_budget_tokens = 100000\n\
             context_window_tokens = 100000\nreserved_output_tokens = 128\n",
            provider.local_addr().unwrap()
        ))
        .unwrap();
        let creation = crate::turns::Creation {
            name: "s".to_owned().try_into().unwrap(),
            sources: vec![],
            defaults: ModelSelection::default(),
        };
        let session = crate::evaluate::create(&settings, &creation).unwrap().id;
        let mut app = app(&settings);
        app.repl.session.id = session;
        app.form.question = "late: Is the train late?".into();
        app.form.row = Row::Question;
        let mut running = None;
        let key = |code, modifiers| KeyEvent::new(code, modifiers);
        app.key(key(KeyCode::Enter, KeyModifiers::NONE), &mut running)
            .unwrap();
        app.key(key(KeyCode::Char('c'), KeyModifiers::CONTROL), &mut running)
            .unwrap();
        let turn = running.expect("Enter on the question sends it").turn;
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let ended =
            runtime.block_on(async { tokio::time::timeout(Duration::from_secs(30), turn).await });
        let (envelope, _) = ended.expect("Ctrl-C ends the turn before the provider answers");
        let Report::Error(error) = envelope.result else {
            panic!("a cancelled turn saves nothing");
        };
        assert_eq!(error.code, "evaluation_canceled");
        let _ = std::fs::remove_dir_all(&settings.store);
    }
}
