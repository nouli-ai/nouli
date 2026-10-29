//! The interactive loop. It is a thin layer over the verbs' own operations, and
//! prints only what the ledger holds or the engine returned.
mod input;
mod terminal;

use crate::{
    Args,
    evaluate::{self, input_error},
    present,
    render::{
        self,
        turn::{DIM, Paint, TEAL},
    },
    render_error, selection, settings,
    settings::Settings,
    turns::{Common, Creation, Parts},
};
use clap::Args as ClapArgs;
use input::{Command, Line};
use nouli_core::{
    decision::grammar,
    history::{ModelSelection, SessionSelection, SourcePoint, SourceSelection},
    identity::{Id, SessionName, SessionSelector},
    protocol::{ErrorReport, Report},
    storage::{Ledger, OpenMode, sessions::Session},
};
use std::{
    io::{self, BufRead, IsTerminal, Write},
    path::Path,
    process::ExitCode,
};

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
    ("plain text", "queue a fact"),
    (
        "?id: prompt [a, b]",
        "queue a standing choice question (<a, b> for a score; no list for yes/no)",
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
    ("/quit", "leave"),
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
        writeln!(self.out, "{text}")
    }

    fn dim(&mut self, text: &str) -> io::Result<()> {
        let text = self.paint.apply(DIM, text);
        writeln!(self.out, "{text}")
    }

    fn fail(&mut self, message: &str) -> io::Result<()> {
        writeln!(self.out, "  error: {}", render::clean(message))
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
                Event::Interrupt => {
                    warned = false;
                    if self.queue.is_empty() {
                        self.say("")?;
                    } else {
                        self.queue = Queue::default();
                        self.dim("^C  queue cleared")?;
                    }
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
            Line::Question(text) => self.queue_question(&text)?,
            Line::Command(command) => return self.command(command),
        }
        Ok(true)
    }

    fn queue_question(&mut self, text: &str) -> io::Result<()> {
        match grammar::parse(text) {
            Ok(question) => {
                use nouli_core::decision::AnswerSpace;
                let kind = match &question.answer_space {
                    AnswerSpace::Noul => "yes/no".to_owned(),
                    AnswerSpace::Choice { .. } => "choice".to_owned(),
                    AnswerSpace::Score { .. } => "score".to_owned(),
                };
                let options = match &question.answer_space {
                    AnswerSpace::Choice { options } => format!(
                        " · options {}",
                        options
                            .iter()
                            .map(|option| option.id.as_str())
                            .collect::<Vec<_>>()
                            .join(", ")
                    ),
                    _ => String::new(),
                };
                self.queue.questions.push(text.to_owned());
                self.dim(&format!(
                    "  queued: {kind} question \"{}\"{options}",
                    render::clean(&question.id)
                ))
            }
            Err(error) => writeln!(self.out, "  grammar error: {error}"),
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
        match result.result {
            Report::Turn(report) => {
                if let Err(error) = present::turn_text(
                    &mut self.out,
                    &report,
                    &draft.request.updates,
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
                // A what-if saves nothing, so what was queued is still pending.
                if !ephemeral {
                    self.queue = Queue::default();
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
        let listed = self.ledger().and_then(|ledger| {
            let facts = ledger
                .facts(&self.selector())
                .map_err(|error| nouli_core::engine::EngineError::Turn(error).report())?;
            let mut text = Vec::new();
            present::facts_text(&ledger, &facts, &mut text).map_err(|_| ErrorReport {
                category: nouli_core::protocol::ErrorCategory::Internal,
                code: "facts_unavailable".into(),
                message: "Cannot list the facts".into(),
                retryable: false,
            })?;
            Ok(String::from_utf8_lossy(&text).into_owned())
        });
        match listed {
            Ok(text) => {
                let queued = !self.queue.facts.is_empty() || !self.queue.replaces.is_empty();
                for line in text.lines() {
                    if line.contains("(from ")
                        || line.contains("(replaced;")
                        || line.contains("no facts yet")
                    {
                        self.dim(line)?;
                    } else {
                        self.say(line)?;
                    }
                }
                let pending: Vec<String> = self
                    .queue
                    .facts
                    .iter()
                    .map(|fact| format!("   +  {}   (queued)", render::clean(fact)))
                    .chain(self.queue.replaces.iter().map(|(number, text)| {
                        format!("   ~  {number}: {}   (queued)", render::clean(text))
                    }))
                    .collect();
                if queued {
                    for line in pending {
                        self.dim(&line)?;
                    }
                }
                Ok(())
            }
            Err(error) => self.fail(&error.message),
        }
    }

    fn questions(&mut self) -> io::Result<()> {
        let standing = self.ledger().and_then(|ledger| {
            ledger
                .standing_questions(&self.selector())
                .map_err(|error| nouli_core::engine::EngineError::Turn(error).report())
        });
        let standing = match standing {
            Ok(standing) => standing,
            Err(error) => return self.fail(&error.message),
        };
        if standing.is_empty() && self.queue.questions.is_empty() {
            return self.dim("  no standing questions");
        }
        for question in &standing {
            self.say(&format!("  {}", render::clean(&grammar::format(question))))?;
        }
        for line in self.queue.questions.clone() {
            self.dim(&format!("  {}   (queued)", render::clean(&line)))?;
        }
        Ok(())
    }

    fn view(&mut self) -> io::Result<()> {
        let rows = self.ledger().and_then(|ledger| {
            let mut rows = Vec::new();
            for session in ledger.list_sessions().map_err(|e| e.report())? {
                let parents: Vec<String> = ledger
                    .session_sources(session.id)
                    .map_err(|e| e.report())?
                    .iter()
                    .filter_map(|source| {
                        ledger
                            .session(&SessionSelector::Id(source.session))
                            .ok()
                            .map(|origin| label(&origin))
                    })
                    .collect();
                rows.push((
                    session.id,
                    label(&session),
                    own_turns(&ledger, &session)?,
                    parents,
                ));
            }
            Ok(rows)
        });
        let rows = match rows {
            Ok(rows) => rows,
            Err(error) => return self.fail(&error.message),
        };
        for (id, name, turns, parents) in rows {
            let current = id == self.session.id;
            let mut text = format!(
                "  {} {}  {}",
                if current { "●" } else { "○" },
                render::clean(&name),
                plural(turns, "turn")
            );
            if !parents.is_empty() {
                text.push_str(&format!("  ← {}", render::clean(&parents.join(" + "))));
            }
            let text = self.paint.apply(if current { TEAL } else { DIM }, &text);
            self.say(&text)?;
        }
        Ok(())
    }

    fn context(&mut self) -> io::Result<()> {
        let lines = self.ledger().and_then(|ledger| {
            let session = ledger.session(&self.selector()).map_err(|e| e.report())?;
            let Some(head) = session.frontier.first().copied() else {
                return Ok(vec![
                    "  nothing sent yet: the model has seen no turn".to_owned(),
                ]);
            };
            let turn = ledger
                .turn(head)
                .map_err(|e| nouli_core::engine::EngineError::Turn(e).report())?;
            let history = ledger
                .ancestry(&[SourcePoint {
                    session: session.id,
                    frontier: session.frontier.clone(),
                }])
                .map_err(|e| nouli_core::engine::EngineError::Turn(e).report())?;
            let mut records = std::collections::HashMap::new();
            for earlier in &history {
                for update in &earlier.updates {
                    if let Some(record) = update.new_record() {
                        records.insert(record.id, record.clone());
                    }
                }
            }
            let mut lines = vec![format!(
                "  {} · last turn {} saw {} records",
                render::clean(&label(&session)),
                short(head),
                turn.manifest.records.len()
            )];
            for projection in &turn.manifest.records {
                let what = if let Some(record) = records.get(&projection.record) {
                    let text = record
                        .value
                        .value()
                        .as_str()
                        .map_or_else(|| record.key.clone(), str::to_owned);
                    let kind = if record
                        .key
                        .starts_with(nouli_core::context::facts::KEY_PREFIX)
                    {
                        "fact"
                    } else {
                        "record"
                    };
                    format!("{kind:<10}{}", render::clean(&text))
                } else {
                    format!("{:<10}turn {}", "decision", short(projection.record))
                };
                let how = format!("{:?}", projection.representation).to_lowercase();
                lines.push(format!("  {what}   ({how})"));
            }
            for outcome in &turn.outcome.outcomes {
                lines.push(format!(
                    "  {:<10}{}",
                    "answer",
                    render::clean(outcome.question_id())
                ));
            }
            Ok(lines)
        });
        match lines {
            Ok(lines) => {
                for line in lines {
                    self.dim(&line)?;
                }
                Ok(())
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

/// How the loop finds its session.
pub(crate) enum Target {
    Named(SessionSelector),
    Pick,
}

fn history_path(store: &Path) -> std::path::PathBuf {
    store.join("repl_history")
}

/// Lists saved sessions and asks for a number or a new name.
fn pick<L: Lines, W: Write>(
    settings: &Settings,
    paint: Paint,
    lines: &mut L,
    out: &mut W,
) -> io::Result<Option<SessionSelector>> {
    let sessions = Ledger::open(&settings.store, OpenMode::ReadOnlyExisting)
        .ok()
        .and_then(|ledger| {
            let listed = ledger.list_sessions().ok()?;
            let counted = listed
                .into_iter()
                .map(|session| {
                    let turns = own_turns(&ledger, &session).unwrap_or(0);
                    (session, turns)
                })
                .collect::<Vec<_>>();
            Some(counted)
        })
        .unwrap_or_default();
    if sessions.is_empty() {
        writeln!(
            out,
            "{}",
            paint.apply(DIM, "  no sessions yet · name your first one")
        )?;
    }
    for (index, (session, turns)) in sessions.iter().enumerate() {
        writeln!(
            out,
            "{}",
            paint.apply(
                DIM,
                &format!(
                    "  {:>2}  {}  {}",
                    index + 1,
                    render::clean(&label(session)),
                    plural(*turns, "turn")
                )
            )
        )?;
    }
    loop {
        let raw = "session › ";
        match lines.read(raw, &paint.apply(TEAL, raw), out)? {
            Event::Line(answer) => {
                let answer = answer.trim();
                if answer.is_empty() {
                    continue;
                }
                if let Ok(number) = answer.parse::<usize>() {
                    match sessions.get(number.wrapping_sub(1)) {
                        Some((session, _)) => {
                            return Ok(Some(SessionSelector::Id(session.id)));
                        }
                        None => {
                            writeln!(out, "  no session number {number}")?;
                            continue;
                        }
                    }
                }
                match selection::selector(answer) {
                    Ok(selector) => return Ok(Some(selector)),
                    Err(_) => writeln!(
                        out,
                        "  a session name starts with a letter and uses letters, digits, `.`, `_` or `-`"
                    )?,
                }
            }
            Event::Interrupt => continue,
            Event::Eof => return Ok(None),
        }
    }
}

/// Resolves the target to a session, offering to create a named one that is
/// missing. A script cannot answer, so it creates the session.
fn resolve<L: Lines, W: Write>(
    settings: &Settings,
    selector: SessionSelector,
    interactive: bool,
    lines: &mut L,
    out: &mut W,
) -> io::Result<Result<(Session, bool), ErrorReport>> {
    let existing = Ledger::open(&settings.store, OpenMode::ReadOnlyExisting)
        .ok()
        .and_then(|ledger| ledger.session(&selector).ok());
    if let Some(session) = existing {
        return Ok(Ok((session, false)));
    }
    let SessionSelector::Name(name) = selector else {
        return Ok(Err(ErrorReport {
            category: nouli_core::protocol::ErrorCategory::Input,
            code: "invalid_session".into(),
            message: "No session has that ID".into(),
            retryable: false,
        }));
    };
    if interactive {
        let raw = format!("Create session \"{}\"? [Y/n] ", name.as_str());
        match lines.read(&raw, &raw, out)? {
            Event::Line(answer)
                if matches!(answer.trim().to_lowercase().as_str(), "" | "y" | "yes") => {}
            _ => {
                return Ok(Err(ErrorReport {
                    category: nouli_core::protocol::ErrorCategory::Input,
                    code: "session_not_created".into(),
                    message: "No session was created".into(),
                    retryable: false,
                }));
            }
        }
    }
    let creation = Creation {
        name,
        sources: vec![],
        defaults: ModelSelection::default(),
    };
    Ok(evaluate::create(settings, &creation).and_then(|report| {
        Ledger::open(&settings.store, OpenMode::ReadOnlyExisting)
            .map_err(|error| error.report())
            .and_then(|ledger| {
                ledger
                    .session(&SessionSelector::Id(report.session))
                    .map_err(|error| error.report())
            })
            .map(|session| (session, true))
    }))
}

/// How a loop was started, apart from its input and output.
struct Start {
    interactive: bool,
    paint: Paint,
    target: Target,
    model: ModelSelection,
    context_budget: Option<u64>,
}

fn session_loop<L: Lines, W: Write>(
    settings: &Settings,
    mut lines: L,
    mut out: W,
    start: Start,
) -> io::Result<Result<(), ErrorReport>> {
    let Start {
        interactive,
        paint,
        target,
        model,
        context_budget,
    } = start;
    let selector = match target {
        Target::Named(selector) => selector,
        Target::Pick => match pick(settings, paint, &mut lines, &mut out)? {
            Some(selector) => selector,
            None => return Ok(Ok(())),
        },
    };
    let (session, created) = match resolve(settings, selector, interactive, &mut lines, &mut out)? {
        Ok(found) => found,
        Err(error) => return Ok(Err(error)),
    };
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => return Err(io::Error::other(error)),
    };
    let mut repl = Repl {
        settings,
        runtime,
        lines,
        out,
        paint,
        model,
        context_budget,
        session: Current {
            id: session.id,
            label: label(&session),
        },
        queue: Queue::default(),
    };
    repl.banner(created)?;
    let result = repl.run();
    repl.lines.finish();
    repl.out.flush()?;
    result.map(Ok)
}

/// Runs the loop on the terminal when there is one, and on piped input otherwise.
pub(crate) fn run(args: &Args, target: Target, open: Option<&Open>) -> ExitCode {
    let mut settings = match settings::load(args) {
        Ok(settings) => settings,
        Err(error) => {
            return render_error(
                args,
                nouli_core::config::OutputFormat::Auto,
                settings::error(args, error),
            );
        }
    };
    if let Err(error) = settings
        .load_environment()
        .and_then(|()| settings.initialize())
    {
        return render_error(args, settings.config.output, error);
    }
    let interactive = io::stdin().is_terminal() && io::stdout().is_terminal();
    let paint = Paint(render::color_enabled(io::stdout().is_terminal()));
    let model = ModelSelection {
        provider: open.and_then(|open| open.provider.clone()),
        model: open.and_then(|open| open.model.clone()),
    };
    let budget = open.and_then(|open| open.context_budget);
    let outcome = if interactive {
        match terminal::Terminal::new(history_path(&settings.store), paint.0) {
            Ok(lines) => session_loop(
                &settings,
                lines,
                io::stdout().lock(),
                Start {
                    interactive: true,
                    paint,
                    target,
                    model,
                    context_budget: budget,
                },
            ),
            Err(error) => Err(error),
        }
    } else {
        session_loop(
            &settings,
            Reader(io::stdin().lock()),
            io::stdout().lock(),
            Start {
                interactive: false,
                paint,
                target,
                model,
                context_budget: budget,
            },
        )
    };
    match outcome {
        Ok(Ok(())) => ExitCode::SUCCESS,
        Ok(Err(error)) => render_error(args, settings.config.output, error),
        Err(_) => ExitCode::from(6),
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
