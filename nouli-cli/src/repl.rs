//! The interactive loop. It is a thin layer over the verbs' own operations, and
//! prints only what the ledger holds or the engine returned.
mod input;
mod show;
mod start;
mod terminal;

use crate::{
    evaluate::{self, input_error},
    present,
    render::{
        self,
        turn::{Paint, TEAL},
    },
    selection,
    settings::Settings,
    turns::{Common, Creation, Parts},
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

/// Open the interactive session loop on a session, creating it if you agree.
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

/// What the person typed, or Ctrl-C and Ctrl-D.
pub(crate) enum Event {
    Line(String),
    Interrupt,
    Eof,
}

/// A source of lines. The terminal edits and remembers them; a reader of a
/// script or pipe echoes them, so the output reads as a transcript.
pub(crate) trait Lines {
    fn read(&mut self, raw: &str, styled: &str, out: &mut dyn Write) -> io::Result<Event>;
    /// Reads a yes/no answer, where Ctrl-C always interrupts, even after typing.
    fn confirm(&mut self, prompt: &str, out: &mut dyn Write) -> io::Result<Event> {
        self.read(prompt, prompt, out)
    }
    fn finish(&mut self) {}
}

/// Lines from any `BufRead`, echoed after the prompt.
pub(crate) struct Reader<R>(pub R);

impl<R: BufRead> Lines for Reader<R> {
    fn read(&mut self, _raw: &str, styled: &str, out: &mut dyn Write) -> io::Result<Event> {
        let mut line = String::new();
        if self.0.read_line(&mut line)? == 0 {
            return Ok(Event::Eof);
        }
        let line = line.trim_end_matches(['\n', '\r']).to_owned();
        let shown = if line.is_empty() { "↵" } else { &line };
        writeln!(out, "{styled}{shown}")?;
        Ok(Event::Line(line))
    }
}

/// Queued input that the next turn will carry.
#[derive(Default)]
struct Queue {
    facts: Vec<String>,
    questions: Vec<String>,
    drops: Vec<String>,
    replaces: Vec<(usize, String)>,
}

impl Queue {
    fn is_empty(&self) -> bool {
        self.facts.is_empty()
            && self.questions.is_empty()
            && self.drops.is_empty()
            && self.replaces.is_empty()
    }
    fn len(&self) -> usize {
        self.facts.len() + self.questions.len() + self.drops.len() + self.replaces.len()
    }
}

struct Current {
    id: Id,
    label: String,
}

const HELP: [(&str, &str); 16] = [
    ("plain text", "queue a fact for the next turn"),
    (
        "?id: prompt [a, b]",
        "ask now with the queue; the question stays (<a, b> for a score; no list for yes/no)",
    ),
    (
        "(empty line)",
        "send the queue as one turn, or re-ask the standing questions",
    ),
    ("/ask", "send now"),
    ("/whatif FACT", "an unsaved turn with the queue plus FACT"),
    ("/replace N TEXT", "replace fact N (queued)"),
    ("/drop ID", "drop a standing question (queued)"),
    ("/clear", "empty the queue"),
    ("/facts, /questions", "show the session"),
    ("/new NAME", "start a session"),
    ("/use NAME", "switch session"),
    ("/branch NAME", "a new session from here"),
    ("/blend A B as NAME", "combine histories into a new session"),
    ("/view", "the sessions and how they connect"),
    ("/context", "what the model saw last turn"),
    ("/quit, /exit", "leave (or Ctrl-C on an empty prompt)"),
];

struct Repl<'a, L, W> {
    settings: &'a Settings,
    runtime: tokio::runtime::Runtime,
    lines: L,
    out: W,
    paint: Paint,
    model: ModelSelection,
    context_budget: Option<u64>,
    session: Current,
    queue: Queue,
}

fn short(id: Id) -> String {
    id.to_string().chars().take(8).collect()
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

impl<L: Lines, W: Write> Repl<'_, L, W> {
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

    fn standing_count(&self, id: Id) -> Result<usize, ErrorReport> {
        self.ledger()?
            .standing_questions(&SessionSelector::Id(id))
            .map(|questions| questions.len())
            .map_err(|error| nouli_core::engine::EngineError::Turn(error).report())
    }

    fn switch(&mut self, session: &Session) {
        self.session = Current {
            id: session.id,
            label: label(session),
        };
    }

    fn banner(&mut self, created: bool) -> io::Result<()> {
        let text = if created {
            format!(
                "new session \"{}\" · /help for commands",
                render::clean(&self.session.label)
            )
        } else {
            let standing = self.standing_count(self.session.id).unwrap_or(0);
            format!(
                "{} · {} · /help for commands",
                render::clean(&self.session.label),
                plural(standing, "standing question")
            )
        };
        self.dim(&text)?;
        self.say("")
    }

    fn run(&mut self) -> io::Result<()> {
        let mut warned = false;
        loop {
            let raw = format!("{} › ", self.session.label);
            let styled = self.paint.apply(TEAL, &raw);
            match self.lines.read(&raw, &styled, &mut self.out)? {
                Event::Eof => {
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
                // The editor clears a typed line itself, so this is an empty
                // prompt: clear the queue first, and leave once it is empty.
                Event::Interrupt => {
                    warned = false;
                    if self.queue.is_empty() {
                        return self.say("  bye");
                    }
                    self.queue = Queue::default();
                    self.dim("^C  queue cleared")?;
                }
                Event::Line(line) => {
                    warned = false;
                    if !self.handle(&line)? {
                        return Ok(());
                    }
                }
            }
            self.out.flush()?;
        }
    }

    /// Returns false when the session should end.
    fn handle(&mut self, line: &str) -> io::Result<bool> {
        match input::parse(line) {
            Line::Send => self.send(None, true)?,
            Line::Fact(text) => self.queue.facts.push(text),
            Line::Question(text) => {
                if self.queue_question(&text)? {
                    self.send(None, false)?;
                }
            }
            Line::Command(command) => return self.command(command),
        }
        Ok(true)
    }

    /// Queues a valid question for the turn it starts, or reports why it isn't one.
    /// A question left queued by a failed turn is replaced, not repeated.
    fn queue_question(&mut self, text: &str) -> io::Result<bool> {
        match grammar::parse(text) {
            Ok(question) => {
                self.queue.questions.retain(|queued| {
                    grammar::parse(queued).map_or(true, |old| old.id != question.id)
                });
                self.queue.questions.push(text.to_owned());
                Ok(true)
            }
            Err(error) => {
                writeln!(self.out, "  grammar error: {error}")?;
                Ok(false)
            }
        }
    }

    fn command(&mut self, command: Command) -> io::Result<bool> {
        match command {
            Command::Quit => {
                self.say("  bye")?;
                return Ok(false);
            }
            Command::Help => {
                for (name, meaning) in HELP {
                    let text = format!("  {name:<22}{meaning}");
                    self.dim(&text)?;
                }
            }
            Command::Ask => self.send(None, false)?,
            Command::Whatif(fact) => self.send(Some(fact), false)?,
            Command::Clear => {
                self.queue = Queue::default();
                self.dim("  queue cleared")?;
            }
            Command::Replace(number, text) => {
                self.queue.replaces.push((number, text));
                self.dim(&format!("  queued: replace fact {number}"))?;
            }
            Command::Drop(id) => {
                self.dim(&format!("  queued: drop {}", render::clean(&id)))?;
                self.queue.drops.push(id);
            }
            Command::Facts => self.facts()?,
            Command::Questions => self.questions()?,
            Command::View => self.view()?,
            Command::Context => self.context()?,
            Command::Use(name) => self.use_session(&name)?,
            Command::New(name) => self.new_session(&name)?,
            Command::Branch(name) => self.branch(&name)?,
            Command::Blend { sources, name } => self.blend(&sources, &name)?,
            Command::Usage(usage) => self.say(&format!("  usage: {usage}"))?,
            Command::Unknown(name) => self.say(&format!(
                "  unknown command /{} · /help lists them",
                render::clean(&name)
            ))?,
        }
        Ok(true)
    }

    /// Sends the queue as a turn, or as a what-if when `whatif` carries a fact.
    /// `quiet` makes an empty line with nothing to ask do nothing at all.
    fn send(&mut self, whatif: Option<String>, quiet: bool) -> io::Result<()> {
        let ephemeral = whatif.is_some();
        if quiet && self.queue.is_empty() {
            match self.standing_count(self.session.id) {
                Ok(0) => return Ok(()),
                Ok(_) => {}
                Err(error) => return self.fail(&error.message),
            }
        }
        if ephemeral {
            match self.ledger().and_then(|ledger| {
                ledger
                    .session(&self.selector())
                    .map_err(|error| error.report())
            }) {
                Ok(session) if session.frontier.is_empty() => {
                    return self.say("  send a turn first: a what-if starts from the last one");
                }
                Ok(_) => {}
                Err(error) => return self.fail(&error.message),
            }
        }
        let mut facts = self.queue.facts.clone();
        facts.extend(whatif);
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
            facts: &facts,
            questions: &self.queue.questions,
            drop: &self.queue.drops,
            replace: &replace,
            common: &common,
            replay: None,
            ephemeral,
            create: None,
        }
        .draft(&mut io::empty());
        let draft = match draft {
            Ok(draft) => evaluate::resolve(self.settings, draft),
            Err(error) => Err(input_error(&error)),
        };
        let draft = match draft {
            Ok(draft) => draft,
            Err(error) => return self.fail(&error.message),
        };
        let result = self.runtime.block_on(evaluate::run(&draft, self.settings));
        self.finish(result.result, &draft.request.updates, ephemeral)
    }

    /// Shows what a turn returned. A saved turn has taken the queue, even when
    /// showing it fails: sending the queue again would repeat the turn.
    fn finish(
        &mut self,
        result: Report,
        updates: &[ContextUpdate],
        ephemeral: bool,
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
                    false,
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
            Report::Error(error) if error.code == "questions_required" => {
                self.say("  nothing to ask: define a question with ?id: prompt …")
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

    fn questions(&mut self) -> io::Result<()> {
        match self.ledger() {
            Ok(ledger) => {
                let session = self.selector();
                show::questions(&ledger, &session, &self.queue, self.paint, &mut self.out)
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
                let standing = self.standing_count(session.id).unwrap_or(0);
                self.dim(&format!(
                    "  {} · {}",
                    render::clean(&label(&session)),
                    plural(standing, "standing question")
                ))
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

    fn new_session(&mut self, name: &str) -> io::Result<()> {
        if let Some(session) = self.create(name, vec![]) {
            self.switch(&session);
            self.dim(&format!("  new session \"{}\"", render::clean(name)))?;
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
            let standing = self.standing_count(session.id).unwrap_or(0);
            self.dim(&format!(
                "  new session \"{}\" from {} @ turn {turns} · {}",
                render::clean(name),
                render::clean(&label(&source)),
                plural(standing, "standing question")
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
                "  new session \"{}\" from {} · no standing questions",
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
    fn a_reader_echoes_each_line_after_the_prompt_and_ends_at_the_end_of_input() {
        let mut reader = Reader(&b"A fact\n\n?q: Why?\r\n"[..]);
        let mut out = Vec::new();
        let mut seen = Vec::new();
        while let Event::Line(line) = reader.read("s › ", "s › ", &mut out).unwrap() {
            seen.push(line);
        }
        assert_eq!(seen, ["A fact", "", "?q: Why?"]);
        assert_eq!(
            String::from_utf8(out).unwrap(),
            "s › A fact\ns › ↵\ns › ?q: Why?\n"
        );
    }

    /// Key presses for the loop to read, then the end of input.
    struct Keys(std::collections::VecDeque<Event>);

    impl Lines for Keys {
        fn read(&mut self, _: &str, _: &str, _: &mut dyn Write) -> io::Result<Event> {
            Ok(self.0.pop_front().unwrap_or(Event::Eof))
        }
    }

    fn repl<'a, L: Lines>(settings: &'a Settings, queue: Queue, lines: L) -> Repl<'a, L, Vec<u8>> {
        Repl {
            settings,
            runtime: tokio::runtime::Builder::new_current_thread()
                .build()
                .unwrap(),
            lines,
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

    fn saved_turn() -> nouli_core::protocol::Report {
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
            drops: vec!["old".into()],
            ..Queue::default()
        };
        let mut sent = repl(&settings, queue(), Reader(&b""[..]));
        sent.finish(saved_turn(), &[], false).unwrap();
        assert!(sent.queue.is_empty());
        let shown = String::from_utf8(sent.out).unwrap();
        assert!(shown.contains("error:"), "{shown}");
        // A what-if saves nothing, so the queue stays.
        let mut whatif = repl(&settings, queue(), Reader(&b""[..]));
        whatif.finish(saved_turn(), &[], true).unwrap();
        assert_eq!(whatif.queue.len(), 2);
    }

    #[test]
    fn ctrl_c_clears_the_queue_and_then_leaves_from_an_empty_prompt() {
        let settings = Settings::for_test(
            std::env::temp_dir().join(format!("nouli-repl-keys-{}", Id::generate())),
        );
        let queue = Queue {
            facts: vec!["a fact".into()],
            ..Queue::default()
        };
        let keys = Keys(
            [
                Event::Interrupt,
                Event::Interrupt,
                Event::Line("never read".into()),
            ]
            .into(),
        );
        let mut session = repl(&settings, queue, keys);
        session.run().unwrap();
        assert!(session.queue.is_empty());
        assert_eq!(session.lines.0.len(), 1);
        let shown = String::from_utf8(session.out).unwrap();
        assert!(shown.contains("queue cleared"), "{shown}");
        assert!(shown.ends_with("  bye\n"), "{shown}");
    }

    #[test]
    fn a_question_asked_again_replaces_the_queued_one() {
        let settings = Settings::for_test(std::env::temp_dir());
        let mut session = repl(&settings, Queue::default(), Reader(&b""[..]));
        assert!(session.queue_question("q: Is it?").unwrap());
        assert!(session.queue_question("other: And this?").unwrap());
        assert!(session.queue_question("q: Is it now?").unwrap());
        assert!(!session.queue_question("broken").unwrap());
        assert_eq!(
            session.queue.questions,
            ["other: And this?", "q: Is it now?"]
        );
    }

    #[test]
    fn the_queue_counts_everything_pending() {
        let mut queue = Queue::default();
        assert!(queue.is_empty());
        queue.facts.push("a".into());
        queue.drops.push("b".into());
        queue.replaces.push((1, "c".into()));
        assert_eq!(queue.len(), 3);
        assert!(!queue.is_empty());
    }
}
