//! The full-screen loop: the transcript, the question form under it, and the
//! session's decisions beside them. Like the line loop, it is a thin layer
//! over `Repl`, whose printed output fills the transcript. It runs each turn
//! while it keeps drawing, and Ctrl-C, which arrives as a key, cancels it.
mod ansi;
pub(crate) mod attachments;
mod commands;
mod draw;
mod drops;
mod form;
mod graph;
mod greeter;
mod keys;
mod model_picker;
mod mouse;
mod overlay;
mod references;
mod screen;
mod sessions;
mod setup;
mod status_line;
mod turn;

use super::{Repl, Step};
use crate::{
    render::{self, turn::TEAL},
    turns::Draft,
};
use crossterm::{
    event::{DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, Event, EventStream},
    execute,
};
use form::{Form, Kind, Row};
use futures_util::StreamExt;
use graph::Graph;
use nouli_core::{
    history::ModelSelection,
    protocol::{Report, ResultEnvelope},
};
use ratatui::{DefaultTerminal, text::Line};
use screen::Screen;
use std::{
    collections::BTreeMap,
    io,
    pin::Pin,
    time::{Duration, Instant},
};
use tokio::{runtime::Runtime, time::MissedTickBehavior};
use tokio_util::sync::CancellationToken;
use turn::PreviousQuestion;

/// Runs the view on the terminal until the person leaves, then puts the
/// terminal back as it was.
pub(super) fn run(repl: Repl<'_, Vec<u8>>, runtime: &Runtime) -> io::Result<()> {
    let mut app = App::new(repl);
    let mut screen = Screen::enter()?;
    let result = app.run(&mut screen.terminal, runtime);
    let restored = screen.leave();
    result.and(restored)
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
    /// Sending again might work: the form still holds what failed.
    retry: bool,
}

impl Status {
    fn info(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            error: false,
            retry: false,
        }
    }

    fn error(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            error: true,
            retry: false,
        }
    }

    /// A request that failed and left its input to send again.
    fn failed(text: impl Into<String>) -> Self {
        Self {
            retry: true,
            ..Self::error(text)
        }
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
    /// Models the providers listed, by provider, for the model picker.
    catalog: BTreeMap<String, Vec<String>>,
    /// Model lists on their way.
    fetches: Option<model_picker::Fetches>,
    /// The dialog that has the keys, if one is open.
    overlay: Option<overlay::Overlay>,
    /// The fields the status line shows.
    status_line: Vec<status_line::Field>,
    /// How long the last answered request took.
    latency: Option<Duration>,
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
        let status_line = status_line::load(&repl.settings.config_path);
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
            catalog: BTreeMap::new(),
            fetches: None,
            overlay: None,
            status_line,
            latency: None,
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
                screen::capture_mouse(capture)?;
                mouse_capture = capture;
            }
            let busy = running.is_some();
            let wake = runtime.block_on(async {
                tokio::select! {
                    event = events.next() => Wake::Input(event),
                    done = finished(&mut running) => Wake::Done(Box::new(done)),
                    // Redraws the spinner while a turn runs.
                    _ = tick.tick(), if !self.selection.active && (busy || self.drop_start.is_some() || self.fetches.is_some()) => Wake::Tick,
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
                        if matches!(envelope.result, Report::Turn(_)) {
                            self.latency = Some(turn.started.elapsed());
                        }
                        self.finish(
                            envelope,
                            &draft.request.updates,
                            turn.ephemeral,
                            turn.from_form,
                        )?;
                    }
                }
                Wake::Tick => {
                    self.poll_models();
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
}

/// Waits for the running turn, or forever when there is none.
async fn finished(running: &mut Option<Running<'_>>) -> (ResultEnvelope, Draft) {
    match running {
        Some(turn) => turn.turn.as_mut().await,
        None => std::future::pending().await,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repl::tests::repl;
    pub(super) use crate::{repl::Queue, settings::Settings};
    pub(super) use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    pub(super) use nouli_core::{
        decision::{AnswerSpace, Question},
        identity::Id,
        protocol::{ErrorReport, Report},
    };
    use std::fmt::Write as _;

    pub(super) fn app(settings: &Settings) -> App<'_> {
        App::new(repl(settings, Queue::default()))
    }

    pub(super) fn press(app: &mut App, code: KeyCode, modifiers: KeyModifiers) -> Flow {
        let mut running = None;
        let flow = app
            .key(KeyEvent::new(code, modifiers), &mut running)
            .unwrap();
        assert!(running.is_none(), "no turn starts in these tests");
        flow
    }

    pub(super) fn typed(app: &mut App, text: &str) {
        for c in text.chars() {
            press(app, KeyCode::Char(c), KeyModifiers::NONE);
        }
    }

    pub(super) fn transcript(app: &App) -> String {
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
            code: "provider_transport_failed".into(),
            message: "The provider did not answer".into(),
            retryable: false,
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
}
