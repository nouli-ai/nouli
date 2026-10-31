//! The interactive loop. It is a thin layer over the verbs' own operations, and
//! prints only what the ledger holds or the engine returned.
mod input;
mod show;
mod start;
pub(crate) mod tui;

use crate::{
    evaluate::{self, input_error},
    present,
    render::{
        self,
        turn::{Paint, TEAL},
    },
    selection,
    settings::Settings,
    turns::{Common, Creation, Draft, Parts},
};
use clap::Args as ClapArgs;
use input::{Command, Line};
use nouli_core::{
    context::ContextUpdate,
    decision::grammar,
    history::{ModelSelection, SessionSelection, SourcePoint, SourceSelection},
    identity::{Id, SessionName, SessionSelector},
    protocol::{ErrorReport, Report},
    storage::{Ledger, OpenMode, sessions::Session},
};
pub(crate) use start::{Target, run};
use std::io::{self, BufRead, Write};
use tokio::runtime::Runtime;

/// Open the interactive session loop on a session, creating it if it is missing.
#[derive(Debug, ClapArgs)]
pub(crate) struct Open {
    /// The session, by ID or unique name.
    #[arg(value_name = "SESSION", value_parser = selection::selector)]
    pub session: SessionSelector,
    /// Override the selected named provider.
    #[arg(long)]
    pub provider: Option<String>,
    /// Override the requested decision model.
    #[arg(long)]
    pub model: Option<String>,
    /// Smaller model context budget, or an explicit budget for an unknown model.
    #[arg(long,value_name="TOKENS",value_parser=clap::value_parser!(u64).range(1..))]
    pub context_budget: Option<u64>,
}

/// Whether standard input and output are a terminal that can draw the view.
/// One that takes no escape codes is read like piped input.
pub(crate) fn can_draw() -> bool {
    use std::io::IsTerminal;
    io::stdin().is_terminal()
        && io::stdout().is_terminal()
        && !render::plain_terminal(std::env::var_os("TERM").as_deref())
}

/// The next line of `input`, echoed after `prompt` so a script's output reads
/// as a transcript; `None` at the end of the input.
fn read_line(
    input: &mut impl BufRead,
    prompt: &str,
    out: &mut impl Write,
) -> io::Result<Option<String>> {
    let mut line = String::new();
    if input.read_line(&mut line)? == 0 {
        return Ok(None);
    }
    let line = line.trim_end_matches(['\n', '\r']).to_owned();
    let shown = if line.is_empty() { "↵" } else { &line };
    writeln!(out, "{prompt}{shown}")?;
    Ok(Some(line))
}

/// Queued input that the next turn will carry. The question is there only
/// while a failed turn left it to send again.
#[derive(Default)]
struct Queue {
    facts: Vec<String>,
    question: Option<String>,
    replaces: Vec<(usize, String)>,
}

impl Queue {
    fn is_empty(&self) -> bool {
        self.len() == 0
    }
    fn len(&self) -> usize {
        self.facts.len() + usize::from(self.question.is_some()) + self.replaces.len()
    }
}

struct Current {
    id: Id,
    label: String,
}

const HELP: [(&str, &str); 14] = [
    ("plain text", "queue a fact for the next question"),
    (
        "?id: prompt [a, b]",
        "ask it now with the queue (<a, b> for a score; no list for yes/no)",
    ),
    ("(empty line)", "send the queue again after a failed turn"),
    ("/replace N TEXT", "replace fact N (queued)"),
    ("/clear", "empty the queue"),
    ("/facts", "show the session's facts"),
    (
        "/new [NAME]",
        "start a session; name is automatic if omitted",
    ),
    ("/use NAME", "switch session"),
    ("/rename NAME", "rename the current session"),
    ("/branch NAME", "a new session from here"),
    ("/blend A B as NAME", "combine histories into a new session"),
    ("/view", "the sessions and how they connect"),
    ("/context", "what the model saw last turn"),
    ("/quit, /exit", "leave (or Ctrl-C on an empty prompt)"),
];

/// What a line leaves the loop to do.
#[derive(Debug, PartialEq, Eq)]
enum Step {
    Continue,
    /// Send the queue with this question; a what-if saves nothing. The loop
    /// runs the turn, so it can wait for it or keep drawing while it runs.
    Send {
        question: String,
        ephemeral: bool,
    },
    Quit,
}

struct Repl<'a, W> {
    settings: &'a Settings,
    out: W,
    paint: Paint,
    model: ModelSelection,
    context_budget: Option<u64>,
    session: Current,
    queue: Queue,
}

fn short(id: Id) -> String {
    let id = id.to_string();
    id[id.len() - 8..].to_owned()
}

fn label(session: &Session) -> String {
    session
        .name
        .as_ref()
        .map_or_else(|| short(session.id), |name| name.as_str().to_owned())
}

fn plural(count: usize, noun: &str) -> String {
    format!("{count} {noun}{}", if count == 1 { "" } else { "s" })
}

/// How many turns a session wrote itself, not counting inherited history.
fn own_turns(ledger: &Ledger, session: &Session) -> Result<usize, ErrorReport> {
    if session.frontier.is_empty() {
        return Ok(0);
    }
    let history = ledger
        .ancestry(&[SourcePoint {
            session: session.id,
            frontier: session.frontier.clone(),
        }])
        .map_err(|error| nouli_core::engine::EngineError::Turn(error).report())?;
    Ok(history
        .iter()
        .filter(|turn| turn.session == session.id)
        .count())
}

impl<W: Write> Repl<'_, W> {
    fn say(&mut self, text: &str) -> io::Result<()> {
        show::say(&mut self.out, text)
    }

    fn dim(&mut self, text: &str) -> io::Result<()> {
        show::dim(&mut self.out, self.paint, text)
    }

    fn fail(&mut self, message: &str) -> io::Result<()> {
        show::fail(&mut self.out, message)
    }

    fn ledger(&self) -> Result<Ledger, ErrorReport> {
        Ledger::open(&self.settings.store, OpenMode::ReadOnlyExisting)
            .map_err(|error| error.report())
    }

    fn selector(&self) -> SessionSelector {
        SessionSelector::Id(self.session.id)
    }

    fn switch(&mut self, session: &Session) {
        self.session = Current {
            id: session.id,
            label: label(session),
        };
    }

    fn banner(&mut self, created: bool) -> io::Result<()> {
        let label = render::clean(&self.session.label);
        let text = if created {
            format!("new session \"{label}\" · /help for commands")
        } else {
            format!("{label} · /help for commands")
        };
        self.dim(&text)?;
        self.say("")
    }

    fn run(&mut self, input: &mut impl BufRead, runtime: &Runtime) -> io::Result<()> {
        let mut warned = false;
        loop {
            let prompt = self
                .paint
                .apply(TEAL, &format!("{} › ", self.session.label));
            match read_line(input, &prompt, &mut self.out)? {
                None => {
                    if !self.queue.is_empty() && !warned {
                        warned = true;
                        let text = format!(
                            "  {} queued; press Ctrl-D again to quit and discard them",
                            self.queue.len()
                        );
                        self.dim(&text)?;
                        continue;
                    }
                    return self.say("  bye");
                }
                Some(line) => {
                    warned = false;
                    match self.handle(&line)? {
                        Step::Continue => {}
                        Step::Send {
                            question,
                            ephemeral,
                        } => self.send(runtime, &question, ephemeral)?,
                        Step::Quit => return Ok(()),
                    }
                }
            }
            self.out.flush()?;
        }
    }

    fn handle(&mut self, line: &str) -> io::Result<Step> {
        match input::parse(line) {
            Line::Send => match self.queue.question.clone() {
                Some(question) => {
                    return Ok(Step::Send {
                        question,
                        ephemeral: false,
                    });
                }
                None if self.queue.is_empty() => {}
                None => {
                    let text = format!(
                        "  {} queued; ask a question with ?id: prompt to send them",
                        self.queue.len()
                    );
                    self.dim(&text)?;
                }
            },
            Line::Fact(text) => self.queue.facts.push(text),
            Line::Question(text) => {
                if self.valid(&text)? {
                    // A question left by a failed turn gives way to the new one.
                    self.queue.question = Some(text.clone());
                    return Ok(Step::Send {
                        question: text,
                        ephemeral: false,
                    });
                }
            }
            Line::Command(command) => return self.command(command),
        }
        Ok(Step::Continue)
    }

    /// True for a line in the question grammar; otherwise reports why it isn't one.
    fn valid(&mut self, text: &str) -> io::Result<bool> {
        match grammar::parse(text) {
            Ok(_) => Ok(true),
            Err(error) => {
                writeln!(self.out, "  grammar error: {error}")?;
                Ok(false)
            }
        }
    }

    fn command(&mut self, command: Command) -> io::Result<Step> {
        match command {
            Command::Quit => {
                self.say("  bye")?;
                return Ok(Step::Quit);
            }
            Command::Help => {
                for (name, meaning) in HELP {
                    let text = format!("  {name:<22}{meaning}");
                    self.dim(&text)?;
                }
            }
            Command::Clear => {
                self.queue = Queue::default();
                self.dim("  queue cleared")?;
            }
            Command::Replace(number, text) => {
                self.queue.replaces.push((number, text));
                self.dim(&format!("  queued: replace fact {number}"))?;
            }
            Command::Facts => self.facts()?,
            Command::View => self.view()?,
            Command::Context => self.context()?,
            Command::Use(name) => self.use_session(&name)?,
            Command::Rename(name) => self.rename_session(&name)?,
            Command::New(name) => self.new_session(name.as_deref())?,
            Command::Branch(name) => self.branch(&name)?,
            Command::Blend { sources, name } => self.blend(&sources, &name)?,
            Command::Usage(usage) => self.say(&format!("  usage: {usage}"))?,
            Command::Unknown(name) => self.say(&format!(
                "  unknown command /{} · /help lists them",
                render::clean(&name)
            ))?,
        }
        Ok(Step::Continue)
    }

    /// Sends the queued facts with `question` as one turn, or as a what-if
    /// that saves nothing, and waits for it.
    fn send(&mut self, runtime: &Runtime, question: &str, ephemeral: bool) -> io::Result<()> {
        let Some(draft) = self.draft(question, ephemeral)? else {
            return Ok(());
        };
        let result = runtime.block_on(evaluate::run(&draft, self.settings));
        self.finish(result.result, &draft.request.updates, ephemeral)
    }

    /// The turn the queued facts and `question` make, or `None` once the
    /// reason there is none is shown.
    fn draft(&mut self, question: &str, ephemeral: bool) -> io::Result<Option<Draft>> {
        match grammar::parse(question) {
            Ok(question) => self.draft_question(question, ephemeral),
            Err(error) => {
                self.fail(&error.to_string())?;
                Ok(None)
            }
        }
    }

    fn draft_question(
        &mut self,
        question: nouli_core::decision::Question,
        ephemeral: bool,
    ) -> io::Result<Option<Draft>> {
        if ephemeral {
            match self.ledger().and_then(|ledger| {
                ledger
                    .session(&self.selector())
                    .map_err(|error| error.report())
            }) {
                Ok(session) if session.frontier.is_empty() => {
                    self.say("  send a turn first: a what-if starts from the last one")?;
                    return Ok(None);
                }
                Ok(_) => {}
                Err(error) => {
                    self.fail(&error.message)?;
                    return Ok(None);
                }
            }
        }
        let replace: Vec<String> = self
            .queue
            .replaces
            .iter()
            .flat_map(|(number, text)| [number.to_string(), text.clone()])
            .collect();
        let common = Common {
            input: None,
            input_format: None,
            provider: self.model.provider.clone(),
            model: self.model.model.clone(),
            context_budget: self.context_budget,
        };
        let selection = if ephemeral {
            SessionSelection::From {
                sources: vec![SourceSelection {
                    session: self.selector(),
                    frontier: None,
                }],
                name: None,
            }
        } else {
            SessionSelection::Continue {
                session: self.selector(),
            }
        };
        let draft = Parts {
            selection,
            numbering: Some(self.selector()),
            facts: &self.queue.facts,
            question: None,
            replace: &replace,
            common: &common,
            replay: None,
            ephemeral,
            create: None,
        }
        .draft_with_question(&mut io::empty(), Some(question));
        let draft = match draft {
            Ok(draft) => evaluate::resolve(self.settings, draft),
            Err(error) => Err(input_error(&error)),
        };
        match draft {
            Ok(draft) => Ok(Some(draft)),
            Err(error) => {
                self.fail(&error.message)?;
                Ok(None)
            }
        }
    }

    /// Shows what a turn returned. A saved turn has taken the queue, even when
    /// showing it fails: sending the queue again would repeat the turn.
    fn finish(
        &mut self,
        result: Report,
        updates: &[ContextUpdate],
        ephemeral: bool,
    ) -> io::Result<()> {
        self.finish_display(result, updates, ephemeral, None)
    }

    fn finish_display(
        &mut self,
        result: Report,
        updates: &[ContextUpdate],
        ephemeral: bool,
        width: Option<usize>,
    ) -> io::Result<()> {
        match result {
            Report::Turn(report) => {
                // A what-if saves nothing, so what was queued is still pending.
                if !ephemeral {
                    self.queue = Queue::default();
                }
                if let Err(error) = present::turn_text(
                    &mut self.out,
                    &report,
                    updates,
                    &self.settings.store,
                    self.paint.0,
                    width.map_or(
                        present::TurnDisplay::Transcript,
                        present::TurnDisplay::Compact,
                    ),
                ) {
                    return match error {
                        present::PresentError::Render(error) => {
                            writeln!(self.out, "  error: cannot show the result: {error}")
                        }
                        present::PresentError::History(error) => self.fail(&error.message),
                    };
                }
                Ok(())
            }
            Report::Error(error) => self.fail(&format!("{}: {}", error.code, error.message)),
            _ => self.fail("unexpected result"),
        }
    }

    fn facts(&mut self) -> io::Result<()> {
        match self.ledger() {
            Ok(ledger) => {
                let session = self.selector();
                show::facts(&ledger, &session, &self.queue, self.paint, &mut self.out)
            }
            Err(error) => self.fail(&error.message),
        }
    }

    fn view(&mut self) -> io::Result<()> {
        match self.ledger() {
            Ok(ledger) => show::view(&ledger, self.session.id, self.paint, &mut self.out),
            Err(error) => self.fail(&error.message),
        }
    }

    fn context(&mut self) -> io::Result<()> {
        match self.ledger() {
            Ok(ledger) => {
                let session = self.selector();
                show::context(&ledger, &session, self.paint, &mut self.out)
            }
            Err(error) => self.fail(&error.message),
        }
    }

    fn use_session(&mut self, name: &str) -> io::Result<()> {
        let selector = match selection::selector(name) {
            Ok(selector) => selector,
            Err(_) => return self.fail("that is not a valid session name"),
        };
        let found = self
            .ledger()
            .and_then(|ledger| ledger.session(&selector).map_err(|e| e.report()));
        match found {
            Ok(session) => {
                self.switch(&session);
                self.dim(&format!("  {}", render::clean(&label(&session))))
            }
            Err(_) => self.fail(&format!("no session \"{}\"", render::clean(name))),
        }
    }

    fn create(&mut self, name: &str, sources: Vec<SourceSelection>) -> Option<Session> {
        let name = match SessionName::try_from(name.to_owned()) {
            Ok(name) => name,
            Err(_) => {
                let _ = self.fail(
                    "a session name starts with a letter and uses letters, digits, `.`, `_` or `-`",
                );
                return None;
            }
        };
        let creation = Creation {
            name,
            sources,
            defaults: ModelSelection::default(),
        };
        match evaluate::create(self.settings, &creation) {
            Ok(report) => self
                .ledger()
                .and_then(|ledger| {
                    ledger
                        .session(&SessionSelector::Id(report.session))
                        .map_err(|e| e.report())
                })
                .map_err(|error| {
                    let _ = self.fail(&error.message);
                })
                .ok(),
            Err(error) => {
                let _ = self.fail(&error.message);
                None
            }
        }
    }

    fn rename_session(&mut self, name: &str) -> io::Result<()> {
        let name=match SessionName::try_from(name.to_owned()) {
            Ok(name)=>name,
            Err(_)=>return self.fail("Use a session name of up to 64 letters, digits, dots, underscores or hyphens, starting with a letter."),
        };
        let renamed = Ledger::open(&self.settings.store, OpenMode::ReadWriteCreate)
            .map_err(|error| error.report())
            .and_then(|mut ledger| {
                ledger
                    .rename_session(self.session.id, name)
                    .map_err(|error| error.report())
            });
        match renamed {
            Ok(session) => {
                self.switch(&session);
                self.dim(&format!(
                    "  renamed to {}",
                    render::clean(&self.session.label)
                ))
            }
            Err(error) => self.fail(&error.message),
        }
    }

    fn new_session(&mut self, name: Option<&str>) -> io::Result<()> {
        let session = if let Some(name) = name {
            self.create(name, vec![])
        } else {
            match start::new_session(self.settings) {
                Ok(session) => Some(session),
                Err(error) => {
                    self.fail(&error.message)?;
                    None
                }
            }
        };
        if let Some(session) = session {
            self.switch(&session);
            self.dim(&format!(
                "  new session \"{}\"",
                render::clean(&self.session.label)
            ))?;
        }
        Ok(())
    }

    fn branch(&mut self, name: &str) -> io::Result<()> {
        let from = self.ledger().and_then(|ledger| {
            let session = ledger.session(&self.selector()).map_err(|e| e.report())?;
            let turns = own_turns(&ledger, &session)?;
            Ok((session, turns))
        });
        let (source, turns) = match from {
            Ok(found) => found,
            Err(error) => return self.fail(&error.message),
        };
        if source.frontier.is_empty() {
            return self.say("  send a turn first: a branch starts from the last one");
        }
        let sources = vec![SourceSelection {
            session: self.selector(),
            frontier: None,
        }];
        if let Some(session) = self.create(name, sources) {
            self.switch(&session);
            self.dim(&format!(
                "  new session \"{}\" from {} @ turn {turns}",
                render::clean(name),
                render::clean(&label(&source))
            ))?;
        }
        Ok(())
    }

    fn blend(&mut self, names: &[String], name: &str) -> io::Result<()> {
        let mut sources = Vec::new();
        for source in names {
            let selector = match selection::selector(source) {
                Ok(selector) => selector,
                Err(_) => return self.fail("that is not a valid session name"),
            };
            let found = self
                .ledger()
                .and_then(|ledger| ledger.session(&selector).map_err(|e| e.report()));
            match found {
                Ok(session) if session.frontier.is_empty() => {
                    return self.say(&format!(
                        "  send a turn on \"{}\" first: a blend takes each source's last turn",
                        render::clean(source)
                    ));
                }
                Ok(session) => sources.push(SourceSelection {
                    session: SessionSelector::Id(session.id),
                    frontier: None,
                }),
                Err(_) => return self.fail(&format!("no session \"{}\"", render::clean(source))),
            }
        }
        if let Some(session) = self.create(name, sources) {
            self.switch(&session);
            self.dim(&format!(
                "  new session \"{}\" from {}",
                render::clean(name),
                render::clean(&names.join(" + "))
            ))?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nearby_unnamed_sessions_use_randomness_in_their_short_labels() {
        let first = Id::generate();
        let second = Id::generate();
        assert_ne!(short(first), short(second));
        assert!(first.to_string().ends_with(&short(first)));
        assert!(second.to_string().ends_with(&short(second)));
    }

    #[test]
    fn a_reader_echoes_each_line_after_the_prompt_and_ends_at_the_end_of_input() {
        let mut input = &b"A fact\n\n?q: Why?\r\n"[..];
        let mut out = Vec::new();
        let mut seen = Vec::new();
        while let Some(line) = read_line(&mut input, "s › ", &mut out).unwrap() {
            seen.push(line);
        }
        assert_eq!(seen, ["A fact", "", "?q: Why?"]);
        assert_eq!(
            String::from_utf8(out).unwrap(),
            "s › A fact\ns › ↵\ns › ?q: Why?\n"
        );
    }

    pub(super) fn repl(settings: &Settings, queue: Queue) -> Repl<'_, Vec<u8>> {
        Repl {
            settings,
            out: Vec::new(),
            paint: Paint(false),
            model: ModelSelection::default(),
            context_budget: None,
            session: Current {
                id: Id::generate(),
                label: "s".into(),
            },
            queue,
        }
    }

    pub(super) fn saved_turn() -> nouli_core::protocol::Report {
        let data: serde_json::Value =
            serde_json::from_str(include_str!("../../fixtures/contracts/history.json")).unwrap();
        let turn: nouli_core::history::FinalizedTurn =
            serde_json::from_value(data["turns"][0].clone()).unwrap();
        Report::Turn(Box::new(nouli_core::protocol::TurnReport {
            questions: turn.request.questions,
            session: Some(turn.session),
            turn: Some(turn.id),
            outcome: turn.outcome,
            manifest: turn.manifest,
            replayed: false,
        }))
    }

    #[test]
    fn a_saved_turn_empties_the_queue_even_when_it_cannot_be_shown() {
        // The store does not exist, so showing the turn fails reading history.
        let settings = Settings::for_test(
            std::env::temp_dir().join(format!("nouli-repl-missing-{}", Id::generate())),
        );
        let queue = || Queue {
            facts: vec!["a fact".into()],
            question: Some("q: Is it?".into()),
            ..Queue::default()
        };
        let mut sent = repl(&settings, queue());
        sent.finish(saved_turn(), &[], false).unwrap();
        assert!(sent.queue.is_empty());
        let shown = String::from_utf8(sent.out).unwrap();
        assert!(shown.contains("error:"), "{shown}");
        // A what-if saves nothing, so the queue stays.
        let mut whatif = repl(&settings, queue());
        whatif.finish(saved_turn(), &[], true).unwrap();
        assert_eq!(whatif.queue.len(), 2);
    }

    #[test]
    fn a_question_that_breaks_the_grammar_is_reported_and_not_sent() {
        let settings = Settings::for_test(std::env::temp_dir());
        let mut session = repl(&settings, Queue::default());
        assert!(session.valid("q: Is it?").unwrap());
        assert!(!session.valid("broken").unwrap());
        assert_eq!(session.handle("?broken").unwrap(), Step::Continue);
        assert!(session.queue.is_empty());
        let shown = String::from_utf8(session.out).unwrap();
        assert_eq!(shown.matches("grammar error").count(), 2, "{shown}");
    }

    #[test]
    fn lines_that_send_a_turn_leave_running_it_to_the_loop() {
        let settings = Settings::for_test(std::env::temp_dir());
        let mut session = repl(&settings, Queue::default());
        let send = |question: &str, ephemeral| Step::Send {
            question: question.into(),
            ephemeral,
        };
        assert_eq!(session.handle("A fact").unwrap(), Step::Continue);
        assert_eq!(
            session.handle("?q: Is it?").unwrap(),
            send("q: Is it?", false)
        );
        // Nothing ran, so the question waits for an empty line to send it again.
        assert_eq!(session.handle("").unwrap(), send("q: Is it?", false));
        assert_eq!(
            session.handle("/whatif ?w: Would it?").unwrap(),
            Step::Continue
        );
        assert!(String::from_utf8_lossy(&session.out).contains("unknown command /whatif"));
        assert_eq!(session.queue.facts, ["A fact"]);
        assert_eq!(session.handle("/quit").unwrap(), Step::Quit);
    }

    #[test]
    fn the_queue_counts_everything_pending() {
        let mut queue = Queue::default();
        assert!(queue.is_empty());
        queue.facts.push("a".into());
        queue.question = Some("b: Is it?".into());
        queue.replaces.push((1, "c".into()));
        assert_eq!(queue.len(), 3);
        assert!(!queue.is_empty());
    }
}
