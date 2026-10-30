//! The full-screen loop: the transcript, the question form under it, and the
//! session's decisions beside them. Like the line loop, it is a thin layer
//! over `Repl`, whose printed output fills the transcript. It runs each turn
//! while it keeps drawing, and Ctrl-C, which arrives as a key, cancels it.
mod ansi;
mod draw;
mod form;
mod sessions;

use super::{HELP, Queue, Repl, Step};
use crate::{
    evaluate,
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
        DisableBracketedPaste, EnableBracketedPaste, Event, EventStream, KeyCode, KeyEvent,
        KeyEventKind, KeyModifiers,
    },
    execute,
};
use form::{Form, Kind, Row, Section};
use futures_util::StreamExt;
use nouli_core::{
    context::ContextUpdate,
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

/// The keys, for `/help` before the commands the line loop lists.
const KEYS: [(&str, &str); 10] = [
    (
        "Tab",
        "the next section: State, Question, then Options or Levels",
    ),
    (
        "Shift-Tab",
        "the answer type: noul (yes/no), choice, or score",
    ),
    ("Up, Down", "the row above or below, across the sections"),
    (
        "Enter",
        "in State, queue the fact or run the line; in Question, send",
    ),
    ("", "in Options, add the next one; on an empty one, send"),
    ("PgUp, PgDn", "scroll the transcript"),
    (
        "Ctrl-G",
        "the decisions pane: arrows scroll it, Esc comes back",
    ),
    (
        "Ctrl-C",
        "cancel the turn, or clear the row, then the queue, then quit",
    ),
    ("/whatif", "ask the form's question and save nothing"),
    ("/sessions", "resume an earlier session"),
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
        .is_some_and(|(name, _)| !name.ends_with(','))
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
        execute!(io::stdout(), EnableBracketedPaste)?;
        Ok(screen)
    }

    fn leave(&mut self) -> io::Result<()> {
        if !std::mem::replace(&mut self.entered, false) {
            return Ok(());
        }
        let paste = execute!(io::stdout(), DisableBracketedPaste);
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
}

impl Graph {
    fn show(&mut self, text: &str, latest: Option<Placement>) {
        self.lines = text.lines().map(ansi::line).collect();
        self.latest = latest;
        self.follow = true;
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

struct App<'a> {
    repl: Repl<'a, Vec<u8>>,
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
    status: Option<Status>,
    /// The provider and model that answered the last turn.
    model: Option<String>,
    /// Ctrl-D warned that quitting discards what is queued.
    warned: bool,
    /// The sessions `/sessions` offers, while that list is open.
    sessions: Option<Vec<sessions::Entry>>,
    /// The chosen row of the open list.
    choice: usize,
    /// Esc hid the command list until the input row changes.
    hidden: bool,
}

impl<'a> App<'a> {
    fn new(repl: Repl<'a, Vec<u8>>) -> Self {
        let mut app = Self {
            repl,
            form: Form::default(),
            transcript: Vec::new(),
            scroll: 0,
            page: 1,
            composer_top: 0,
            graph: Graph::default(),
            graph_focused: false,
            status: None,
            model: None,
            warned: false,
            sessions: None,
            choice: 0,
            hidden: false,
        };
        app.drain();
        app.refresh_graph();
        app
    }

    fn run(&mut self, terminal: &mut DefaultTerminal, runtime: &Runtime) -> io::Result<()> {
        // The timer needs the runtime; the turn runs only while the loop waits.
        let _context = runtime.enter();
        let mut events = EventStream::new();
        let mut tick = tokio::time::interval(Duration::from_millis(100));
        tick.set_missed_tick_behavior(MissedTickBehavior::Skip);
        let mut running: Option<Running<'a>> = None;
        loop {
            terminal.draw(|frame| draw::screen(frame, self, running.as_ref()))?;
            let busy = running.is_some();
            let wake = runtime.block_on(async {
                tokio::select! {
                    event = events.next() => Wake::Input(event),
                    done = finished(&mut running) => Wake::Done(Box::new(done)),
                    // Redraws the spinner while a turn runs.
                    _ = tick.tick(), if busy => Wake::Tick,
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
                Wake::Tick => {}
            }
        }
    }

    fn event(&mut self, event: Event, running: &mut Option<Running<'a>>) -> io::Result<Flow> {
        match event {
            Event::Key(key) if key.kind != KeyEventKind::Release => self.key(key, running),
            Event::Paste(text) => {
                if running.is_some() {
                    self.status = Some(Status::info("a turn is running · Ctrl-C cancels it"));
                } else {
                    self.graph_focused = false;
                    self.form.paste(&text, &mut self.repl.queue.facts);
                }
                Ok(Flow::Continue)
            }
            // A resize, like any event, draws the view again.
            _ => Ok(Flow::Continue),
        }
    }

    fn key(&mut self, key: KeyEvent, running: &mut Option<Running<'a>>) -> io::Result<Flow> {
        let control = key.modifiers.contains(KeyModifiers::CONTROL);
        if !(control && key.code == KeyCode::Char('d')) {
            self.warned = false;
        }
        match key.code {
            KeyCode::Char('c') if control => return Ok(self.interrupt(running.as_ref())),
            KeyCode::Char('d') if control => return Ok(self.end_of_input(running.is_some())),
            KeyCode::Char('g') if control => {
                self.graph_focused = !self.graph_focused;
                return Ok(Flow::Continue);
            }
            _ => {}
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
        match key.code {
            KeyCode::PageUp => self.scroll = self.scroll.saturating_add(self.page),
            KeyCode::PageDown => self.scroll = self.scroll.saturating_sub(self.page),
            _ if running.is_some() => {
                self.status = Some(Status::info("a turn is running · Ctrl-C cancels it"));
            }
            KeyCode::Enter => return self.enter(running),
            KeyCode::Esc => self.status = None,
            KeyCode::BackTab => self.form.next_kind(&mut self.repl.queue.facts),
            KeyCode::Tab if key.modifiers.contains(KeyModifiers::SHIFT) => {
                self.form.next_kind(&mut self.repl.queue.facts);
            }
            KeyCode::Tab => self.form.next_section(&mut self.repl.queue.facts),
            KeyCode::Up => self.form.up(&mut self.repl.queue.facts),
            KeyCode::Down => self.form.down(&mut self.repl.queue.facts),
            KeyCode::Backspace if self.form.cursor == 0 => {
                self.form.remove_empty(&mut self.repl.queue.facts);
            }
            _ => {
                if self.form.edit(&key, &mut self.repl.queue.facts) {
                    self.status = None;
                    self.choice = 0;
                    self.hidden = false;
                }
            }
        }
        Ok(Flow::Continue)
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
                self.form.input = if takes_argument(usage) {
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
        if self.hidden || self.graph_focused || self.form.row != Row::Input {
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
        self.refresh_graph();
        Ok(())
    }

    /// Keys that move the decisions pane while it has them; Esc and Tab give
    /// them back to the form. False for keys the pane leaves to the form.
    fn graph_key(&mut self, key: &KeyEvent) -> bool {
        let graph = &mut self.graph;
        match key.code {
            KeyCode::Esc | KeyCode::Tab => {
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
            return Flow::Continue;
        }
        if self.repl.queue.is_empty() && self.form.is_empty() {
            return Flow::Quit;
        }
        self.repl.queue = Queue::default();
        self.form.clear();
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
        let facts = &mut self.repl.queue.facts;
        if !self.form.text(facts).is_empty() {
            let delete = KeyEvent::new(KeyCode::Delete, KeyModifiers::NONE);
            self.form.edit(&delete, facts);
            return Flow::Continue;
        }
        if !(self.repl.queue.is_empty() && self.form.is_empty()) && !self.warned {
            self.warned = true;
            self.status = Some(Status::info(
                "unsent input; press Ctrl-D again to quit and discard it",
            ));
            return Flow::Continue;
        }
        Flow::Quit
    }

    fn enter(&mut self, running: &mut Option<Running<'a>>) -> io::Result<Flow> {
        let facts = &mut self.repl.queue.facts;
        match self.form.row {
            Row::Fact(_) => self.form.down(facts),
            Row::Input => return self.submit(running),
            // A choice or a score needs its options before it can be sent.
            Row::Question if self.form.kind != Kind::Noul && self.form.labels().count() < 2 => {
                self.form.focus(Section::Options, facts);
            }
            Row::Question => *running = self.send_form(false)?,
            Row::Option(index) => {
                if self.form.enter_option(index) {
                    *running = self.send_form(false)?;
                }
            }
        }
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
        match line.as_str() {
            "/help" => {
                self.echo(&line);
                self.help();
                return Ok(Flow::Continue);
            }
            "/whatif" => {
                *running = self.send_form(true)?;
                return Ok(Flow::Continue);
            }
            "/sessions" => {
                self.echo(&line);
                self.open_sessions();
                return Ok(Flow::Continue);
            }
            _ => {}
        }
        let fact = !line.is_empty() && !line.starts_with(['/', '?']);
        let session = self.repl.session.id;
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
        }
        self.drain();
        if self.repl.session.id != session {
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

    /// Sends the form's question with the queue, or says what to fix and
    /// moves there.
    fn send_form(&mut self, ephemeral: bool) -> io::Result<Option<Running<'a>>> {
        match self.form.line() {
            Ok(question) => {
                if !ephemeral {
                    // The form keeps this question if the turn fails, so one
                    // that an earlier failed turn left gives way to it.
                    self.repl.queue.question = None;
                }
                self.echo_turn(&question, ephemeral);
                self.launch(&question, ephemeral, true)
            }
            Err(problem) => {
                self.status = Some(Status::error(problem.message));
                self.form.focus(problem.section, &mut self.repl.queue.facts);
                Ok(None)
            }
        }
    }

    /// Starts the turn the queue and `question` make; the loop polls it while
    /// it draws. `None` when there is no turn, after the loop said why.
    fn launch(
        &mut self,
        question: &str,
        ephemeral: bool,
        from_form: bool,
    ) -> io::Result<Option<Running<'a>>> {
        let draft = self.repl.draft(question, ephemeral)?;
        self.drain();
        let Some(draft) = draft else {
            return Ok(None);
        };
        let cancel = CancellationToken::new();
        let stop = cancel.clone();
        let settings = self.repl.settings;
        self.status = None;
        Ok(Some(Running {
            turn: Box::pin(async move {
                let interrupt = async move {
                    stop.cancelled().await;
                    Ok::<(), io::Error>(())
                };
                let envelope = evaluate::run_until(&draft, settings, interrupt).await;
                (envelope, draft)
            }),
            cancel,
            ephemeral,
            from_form,
            started: Instant::now(),
        }))
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
        if let Report::Turn(report) = &envelope.result {
            let provider = &report.outcome.provider;
            self.model = Some(format!(
                "{}/{}",
                render::clean(&provider.provider),
                render::clean(&provider.model)
            ));
        }
        self.repl.finish(envelope.result, updates, ephemeral)?;
        self.drain();
        if saved {
            self.form.sent(from_form);
            self.refresh_graph();
        }
        Ok(())
    }

    fn refresh_graph(&mut self) {
        let store = &self.repl.settings.store;
        match graph_text(store, self.repl.session.id, self.repl.paint.0) {
            Ok((text, latest)) => self.graph.show(&text, latest),
            Err(error) => {
                self.graph.lines.clear();
                self.graph.problem = Some(render::clean(&error.report().message));
            }
        }
    }

    /// Moves what the loop printed into the transcript, which then shows its end.
    fn drain(&mut self) {
        let printed: Vec<_> = String::from_utf8_lossy(&self.repl.out)
            .lines()
            .map(ansi::line)
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
        self.transcript
            .push(ansi::line(&format!("{prompt}{}", render::clean(text))));
        self.scroll = 0;
    }

    /// The lines that would send this turn at the line prompt: each queued
    /// fact, then the question. Queued replacements already showed as commands.
    fn echo_turn(&mut self, question: &str, ephemeral: bool) {
        let asked = if ephemeral {
            format!("/whatif ?{question}")
        } else {
            format!("?{question}")
        };
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
        for (name, meaning) in KEYS.into_iter().chain(HELP) {
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
    session: Id,
    color: bool,
) -> Result<(String, Option<Placement>), InspectError> {
    let ledger = Ledger::open(store, OpenMode::ReadOnlyExisting)?;
    let export = ledger.export_session(&SessionSelector::Id(session))?;
    let mut text = Vec::new();
    let latest = crate::view::cards(&export, &mut text, color)?;
    Ok((String::from_utf8_lossy(&text).into_owned(), latest))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{repl::tests::repl, settings::Settings};
    use nouli_core::protocol::ErrorReport;

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
        app.transcript
            .iter()
            .map(|line| format!("{line}\n"))
            .collect()
    }

    fn settings() -> Settings {
        Settings::for_test(std::env::temp_dir().join(format!("nouli-tui-{}", Id::generate())))
    }

    fn failed() -> ResultEnvelope {
        ResultEnvelope::new(Report::Error(ErrorReport {
            category: nouli_core::protocol::ErrorCategory::Transport,
            code: "provider_transport".into(),
            message: "The provider did not answer".into(),
            retryable: true,
        }))
    }

    #[test]
    fn shift_tab_picks_the_answer_type_and_tab_the_section() {
        let settings = settings();
        let mut app = app(&settings);
        let none = KeyModifiers::NONE;
        press(&mut app, KeyCode::BackTab, KeyModifiers::SHIFT);
        assert_eq!(app.form.kind, Kind::Choice);
        press(&mut app, KeyCode::Tab, KeyModifiers::SHIFT);
        assert_eq!(app.form.kind, Kind::Score);
        press(&mut app, KeyCode::Tab, none);
        assert_eq!(app.form.row, Row::Question);
        press(&mut app, KeyCode::Tab, none);
        assert_eq!(app.form.row, Row::Option(0));
        press(&mut app, KeyCode::Up, none);
        press(&mut app, KeyCode::Up, none);
        assert_eq!(app.form.row, Row::Input);
        // The graph pane takes the arrows until Esc gives them back.
        press(&mut app, KeyCode::Char('g'), KeyModifiers::CONTROL);
        press(&mut app, KeyCode::Down, none);
        assert_eq!((app.form.row, app.graph.follow), (Row::Input, false));
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
        assert!(app.form.input.is_empty());
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
        assert!(shown.contains("s › ?broken\n  grammar error"), "{shown}");
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
        typed(&mut app, "Will it rain?");
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE);
        let status = app.status.as_ref().unwrap();
        assert!(status.error && status.text.contains("ID and a colon"));
        assert_eq!(app.form.row, Row::Question);
        // A choice goes to its options first, and the list ends on an empty one.
        app.form.question = "rain: Will it rain?".into();
        press(&mut app, KeyCode::BackTab, KeyModifiers::SHIFT);
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE);
        assert_eq!(app.form.row, Row::Option(0));
        typed(&mut app, "Yes");
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE);
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE);
        let status = app.status.as_ref().unwrap();
        assert!(status.text.contains("2 to 255"), "{}", status.text);
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
        assert!(app.form.input.is_empty());
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
        assert!(app.form.question.is_empty());
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
        assert!(app.commands_shown().is_empty());
        // Esc hides the list until the row changes.
        app.form.clear_row(&mut app.repl.queue.facts);
        typed(&mut app, "/v");
        press(&mut app, KeyCode::Esc, KeyModifiers::NONE);
        assert!(app.commands_shown().is_empty());
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
        // With nothing left to send again, an empty line moves to the form's
        // question instead of sending the older one.
        press(&mut app, KeyCode::Up, KeyModifiers::NONE);
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE);
        assert_eq!(app.form.row, Row::Question);
    }

    #[test]
    fn ctrl_c_ends_a_turn_whose_provider_never_answers() {
        // A provider that takes the connection and never answers.
        let provider = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let mut settings = settings();
        settings.config = nouli_core::config::Config::parse(&format!(
            "default_provider = 'local'\ndefault_model = 'offline'\n\
             [providers.local]\nprotocol = 'typesafe'\nbase_url = 'http://{}'\n\
             [providers.local.models.offline]\ncontext_budget_tokens = 100000\n\
             context_window_tokens = 100000\nreserved_output_tokens = 128\n",
            provider.local_addr().unwrap()
        ))
        .unwrap();
        let creation = crate::turns::Creation {
            name: "s".to_owned().try_into().unwrap(),
            sources: vec![],
            defaults: Default::default(),
        };
        let session = crate::evaluate::create(&settings, &creation)
            .unwrap()
            .session;
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
