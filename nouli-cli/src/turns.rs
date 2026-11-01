//! The `new`, `add`, `ask`, and `whatif` verbs: their arguments, and the
//! evaluation request each one builds before any provider or store setup.
use crate::{
    input::{self, Format, InputError},
    selection::{self, selector, source},
};
use clap::{ArgAction, Args};
use nouli_core::{
    codec,
    context::{ContextUpdate, facts},
    decision::{CONTRACT_VERSION, ContractError, TurnRequest, grammar},
    history::{
        EvaluationRequest, HistoryError, ModelSelection, SessionSelection, SourceSelection,
        StandingChanges,
    },
    identity::{Id, SessionName, SessionSelector},
};
use std::{io::Read, path::PathBuf};

/// Inputs and model options shared by every turn verb.
#[derive(Debug, Args)]
pub(crate) struct Common {
    /// A complete JSON or TOON request without `session`, or - for stdin.
    #[arg(long, value_name = "FILE")]
    pub input: Option<PathBuf>,
    /// Required for stdin or a file without a .json or .toon extension.
    #[arg(long, value_enum)]
    pub input_format: Option<Format>,
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

/// Request identity and recovery, for verbs that save a turn.
#[derive(Debug, Args)]
pub(crate) struct Replay {
    /// Store-wide identity for normalized intent and completed replay.
    #[arg(long)]
    pub request_id: Option<String>,
    /// Explicitly retry an interrupted request using its saved settings and sources.
    #[arg(long)]
    pub retry_interrupted: bool,
}

/// Create a session, and run its first turn when facts or a question are given.
#[derive(Debug, Args)]
pub(crate) struct New {
    /// Unique, case-sensitive name for the new session.
    #[arg(value_name = "NAME", value_parser = selection::name)]
    pub name: SessionName,
    /// Branch from SESSION, or `SESSION@TURN_ID` for a fixed point; repeat to blend.
    #[arg(long, value_name = "SOURCE", value_parser = source)]
    pub from: Vec<SourceSelection>,
    /// Facts to record on the first turn.
    #[arg(value_name = "FACT")]
    pub facts: Vec<String>,
    /// The question to ask, like "id: prompt [a, b]".
    #[arg(short = 'q', long = "question", value_name = "QUESTION")]
    pub question: Option<String>,
    #[command(flatten)]
    pub common: Common,
    #[command(flatten)]
    pub replay: Replay,
}

/// Run a turn on an existing session.
#[derive(Debug, Args)]
pub(crate) struct Add {
    /// The session, by ID or unique name.
    #[arg(value_name = "SESSION", value_parser = selector)]
    pub session: SessionSelector,
    /// Facts to record on this turn.
    #[arg(value_name = "FACT")]
    pub facts: Vec<String>,
    /// The question to ask, like "id: prompt [a, b]".
    #[arg(short = 'q', long = "question", value_name = "QUESTION")]
    pub question: Option<String>,
    /// Replace fact N with new text (repeatable).
    #[arg(long, num_args = 2, value_names = ["N", "TEXT"], action = ArgAction::Append)]
    pub replace: Vec<String>,
    #[command(flatten)]
    pub common: Common,
    #[command(flatten)]
    pub replay: Replay,
}

/// Ask a question without new facts.
#[derive(Debug, Args)]
pub(crate) struct Ask {
    /// The session, by ID or unique name.
    #[arg(value_name = "SESSION", value_parser = selector)]
    pub session: SessionSelector,
    /// The question to ask, like "id: prompt [a, b]".
    #[arg(short = 'q', long = "question", value_name = "QUESTION")]
    pub question: Option<String>,
    #[command(flatten)]
    pub common: Common,
    #[command(flatten)]
    pub replay: Replay,
}

/// Run one turn from the session's head and save nothing.
#[derive(Debug, Args)]
pub(crate) struct Whatif {
    /// The session, by ID or unique name.
    #[arg(value_name = "SESSION", value_parser = selector)]
    pub session: SessionSelector,
    /// Facts to add for this turn only.
    #[arg(value_name = "FACT")]
    pub facts: Vec<String>,
    /// The question to ask, like "id: prompt [a, b]".
    #[arg(short = 'q', long = "question", value_name = "QUESTION")]
    pub question: Option<String>,
    /// Replace fact N with new text for this turn only (repeatable).
    #[arg(long, num_args = 2, value_names = ["N", "TEXT"], action = ArgAction::Append)]
    pub replace: Vec<String>,
    #[command(flatten)]
    pub common: Common,
}

#[derive(Debug)]
pub(crate) enum Verb<'a> {
    New(&'a New),
    Add(&'a Add),
    Ask(&'a Ask),
    Whatif(&'a Whatif),
}

/// What a verb asks for, before it is checked against the store.
pub(crate) struct Draft {
    pub request: EvaluationRequest,
    pub ephemeral: bool,
    pub retry_interrupted: bool,
    pub common_model: ModelSelection,
    pub context_budget: Option<u64>,
    /// Fact replacements `(number, text)`, resolved against the session's facts.
    replaces: Vec<(usize, String)>,
    /// The session whose fact numbers `--replace` refers to.
    numbering: Option<SessionSelector>,
    /// The request ID that fixes the replacement records' IDs, when given.
    request_id: Option<String>,
    /// `new` with nothing to ask creates the session and runs no turn.
    pub create: Option<Creation>,
}

pub(crate) struct Creation {
    pub name: SessionName,
    pub sources: Vec<SourceSelection>,
    pub defaults: ModelSelection,
}

/// The pieces of a turn, however they were entered: command-line arguments or
/// the lines queued in the REPL.
pub(crate) struct Parts<'a> {
    pub selection: SessionSelection,
    pub numbering: Option<SessionSelector>,
    pub facts: &'a [String],
    pub question: Option<&'a str>,
    /// `--replace` values in pairs: a fact number, then its new text.
    pub replace: &'a [String],
    pub common: &'a Common,
    pub replay: Option<&'a Replay>,
    pub ephemeral: bool,
    pub create: Option<Creation>,
}

impl Verb<'_> {
    /// True when the verb only creates a session, so it needs no provider setup.
    pub(crate) fn creates_only(&self) -> bool {
        matches!(self, Verb::New(args)
            if args.facts.is_empty() && args.question.is_none() && args.common.input.is_none())
    }

    pub(crate) fn draft(&self, stdin: &mut impl Read) -> Result<Draft, InputError> {
        let parts = match self {
            Verb::New(args) => {
                let name = Some(args.name.clone());
                let create =
                    args.facts.is_empty() && args.question.is_none() && args.common.input.is_none();
                if create && (args.replay.request_id.is_some() || args.replay.retry_interrupted) {
                    return Err(InputError::CreateOnly);
                }
                Parts {
                    selection: if args.from.is_empty() {
                        SessionSelection::New { name }
                    } else {
                        SessionSelection::From {
                            sources: args.from.clone(),
                            name,
                        }
                    },
                    numbering: None,
                    facts: &args.facts,
                    question: args.question.as_deref(),
                    replace: &[],
                    common: &args.common,
                    replay: Some(&args.replay),
                    ephemeral: false,
                    create: create.then(|| Creation {
                        name: args.name.clone(),
                        sources: args.from.clone(),
                        defaults: ModelSelection {
                            provider: args.common.provider.clone(),
                            model: args.common.model.clone(),
                        },
                    }),
                }
            }
            Verb::Add(args) => Parts {
                selection: SessionSelection::Continue {
                    session: args.session.clone(),
                },
                numbering: Some(args.session.clone()),
                facts: &args.facts,
                question: args.question.as_deref(),
                replace: &args.replace,
                common: &args.common,
                replay: Some(&args.replay),
                ephemeral: false,
                create: None,
            },
            Verb::Ask(args) => Parts {
                selection: SessionSelection::Continue {
                    session: args.session.clone(),
                },
                numbering: None,
                facts: &[],
                question: args.question.as_deref(),
                replace: &[],
                common: &args.common,
                replay: Some(&args.replay),
                ephemeral: false,
                create: None,
            },
            Verb::Whatif(args) => Parts {
                selection: SessionSelection::From {
                    sources: vec![SourceSelection {
                        session: args.session.clone(),
                        frontier: None,
                    }],
                    name: None,
                },
                numbering: Some(args.session.clone()),
                facts: &args.facts,
                question: args.question.as_deref(),
                replace: &args.replace,
                common: &args.common,
                replay: None,
                ephemeral: true,
                create: None,
            },
        };
        parts.draft(stdin)
    }
}

/// A request ID makes a repeated command repeat its record IDs, so the saved
/// request replays instead of conflicting with a fresh one.
fn record_id(replay: Option<&Replay>, kind: &str, index: usize) -> Id {
    match replay.and_then(|replay| replay.request_id.as_deref()) {
        Some(key) => Id::derived(&format!("{key}/{kind}/{index}")),
        None => Id::generate(),
    }
}

fn pairs(replace: &[String]) -> Result<Vec<(usize, String)>, InputError> {
    let mut result: Vec<(usize, String)> = Vec::new();
    for pair in replace.chunks(2) {
        let [number, text] = pair else {
            return Err(InputError::ReplaceNumber);
        };
        let number = number
            .parse::<usize>()
            .ok()
            .filter(|number| *number > 0)
            .ok_or(InputError::ReplaceNumber)?;
        if result.iter().any(|(known, _)| *known == number) {
            return Err(InputError::DuplicateReplace(number));
        }
        result.push((number, text.clone()));
    }
    Ok(result)
}

impl Parts<'_> {
    pub(crate) fn draft(self, stdin: &mut impl Read) -> Result<Draft, InputError> {
        self.draft_with_question(stdin, None)
    }

    pub(crate) fn draft_with_question(
        self,
        stdin: &mut impl Read,
        question: Option<nouli_core::decision::Question>,
    ) -> Result<Draft, InputError> {
        let replaces = pairs(self.replace)?;
        let mut request = if let Some(path) = &self.common.input {
            if !(self.facts.is_empty() && self.question.is_none() && self.replace.is_empty()) {
                return Err(InputError::InputConflict);
            }
            let (bytes, format) = input::file(path, self.common.input_format, stdin)?;
            let mut value = codec::decode_value(&bytes, format)?;
            if value.get("turn").is_some() {
                let object = value.as_object_mut().ok_or(InputError::Shape)?;
                if object.contains_key("session") {
                    return Err(InputError::SessionInRequest);
                }
                object.insert(
                    "session".into(),
                    serde_json::json!({"mode":"new","name":null}),
                );
                codec::default_turn_version(&mut value)?;
                serde_json::from_value::<EvaluationRequest>(value).map_err(|_| InputError::Shape)?
            } else {
                let codec::Request::Turn(turn) = codec::request_from_value(value)? else {
                    return Err(InputError::Shape);
                };
                EvaluationRequest {
                    turn,
                    standing: StandingChanges::default(),
                    session: SessionSelection::New { name: None },
                    updates: vec![],
                    required_records: vec![],
                    model: ModelSelection::default(),
                    request_id: None,
                }
            }
        } else {
            let questions = question
                .or(self
                    .question
                    .map(grammar::parse)
                    .transpose()
                    .map_err(InputError::Question)?)
                .into_iter()
                .collect();
            let mut updates = Vec::with_capacity(self.facts.len());
            for (index, text) in self.facts.iter().enumerate() {
                updates.push(facts::append_with(
                    record_id(self.replay, "fact", index),
                    text,
                )?);
            }
            EvaluationRequest {
                turn: TurnRequest {
                    version: CONTRACT_VERSION,
                    state: vec![],
                    questions,
                },
                standing: StandingChanges::default(),
                session: SessionSelection::New { name: None },
                updates,
                required_records: vec![],
                model: ModelSelection::default(),
                request_id: None,
            }
        };
        for (_, text) in &replaces {
            if text.trim().is_empty() {
                return Err(InputError::Fact(facts::FactError::Empty));
            }
        }
        request.session = self.selection;
        let mut retry_interrupted = false;
        if let Some(replay) = self.replay {
            if let Some(key) = &replay.request_id {
                request.request_id = Some(key.clone());
            }
            if replay.retry_interrupted && request.request_id.is_none() {
                return Err(InputError::Retry);
            }
            retry_interrupted = replay.retry_interrupted;
        }
        // `new` with nothing to ask only creates the session.
        if self.create.is_none() {
            request.validate().map_err(|error| match error {
                HistoryError::Decision(ContractError::Empty("questions")) => InputError::NoQuestion,
                error => error.into(),
            })?;
        }
        Ok(Draft {
            request,
            ephemeral: self.ephemeral,
            retry_interrupted,
            common_model: ModelSelection {
                provider: self.common.provider.clone(),
                model: self.common.model.clone(),
            },
            context_budget: self.common.context_budget,
            replaces,
            numbering: self.numbering,
            request_id: self.replay.and_then(|replay| replay.request_id.clone()),
            create: self.create,
        })
    }
}

impl Draft {
    /// The request ID the command carries, when it has one.
    pub(crate) fn request_id(&self) -> Option<&str> {
        self.request_id.as_deref()
    }

    /// The session whose facts `--replace` numbers refer to, when any is replaced.
    pub(crate) fn replaced_session(&self) -> Option<&SessionSelector> {
        if self.replaces.is_empty() {
            None
        } else {
            self.numbering.as_ref()
        }
    }

    /// Rebuilds a replayed command's request from the updates its request ID
    /// saved, when they are the ones this command makes: after the first run
    /// the numbered facts are the replacements, so they cannot be resolved
    /// again. `None` when the command differs from the saved one.
    pub(crate) fn replay(mut self, saved: &[ContextUpdate]) -> Option<Self> {
        let key = self.request_id.as_deref()?;
        let (replaced, appended) = saved.split_at_checked(self.replaces.len())?;
        for (index, ((_, text), update)) in self.replaces.iter().zip(replaced).enumerate() {
            let ContextUpdate::Supersede { replacement, .. } = update else {
                return None;
            };
            if replacement.id != Id::derived(&format!("{key}/replace/{index}"))
                || replacement.value.value().as_str() != Some(text.trim())
            {
                return None;
            }
        }
        if appended != self.request.updates {
            return None;
        }
        self.request.updates = saved.to_vec();
        self.replaces.clear();
        Some(self)
    }

    /// Turns `--replace N TEXT` into supersessions of the session's current
    /// facts. They come before the new facts in the turn's updates.
    pub(crate) fn resolve(mut self, current: &[facts::Fact]) -> Result<Self, InputError> {
        let mut updates = Vec::with_capacity(self.replaces.len() + self.request.updates.len());
        for (index, (number, text)) in self.replaces.iter().enumerate() {
            let fact = current
                .get(number - 1)
                .ok_or(InputError::UnknownFact(*number))?;
            let id = match &self.request_id {
                Some(key) => Id::derived(&format!("{key}/replace/{index}")),
                None => Id::generate(),
            };
            updates.push(facts::replace_with(id, fact, text)?);
        }
        updates.append(&mut self.request.updates);
        self.request.updates = updates;
        self.replaces.clear();
        self.request.validate()?;
        Ok(self)
    }
}

/// Fact counts for a turn's header, from the updates it carried.
pub(crate) fn fact_counts(updates: &[ContextUpdate]) -> (usize, usize) {
    let mut added = 0;
    let mut replaced = 0;
    for update in updates {
        match update {
            ContextUpdate::Append { record } if record.key.starts_with(facts::KEY_PREFIX) => {
                added += 1;
            }
            ContextUpdate::Supersede { replacement, .. }
                if replacement.key.starts_with(facts::KEY_PREFIX) =>
            {
                added += 1;
                replaced += 1;
            }
            _ => {}
        }
    }
    (added, replaced)
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[derive(Parser)]
    struct Cli {
        #[command(subcommand)]
        command: crate::commands::Command,
    }

    fn draft(arguments: &[&str]) -> Result<Draft, InputError> {
        let cli = Cli::try_parse_from([&["nouli"][..], arguments].concat()).unwrap();
        let verb = match &cli.command {
            crate::commands::Command::New(args) => Verb::New(args),
            crate::commands::Command::Add(args) => Verb::Add(args),
            crate::commands::Command::Ask(args) => Verb::Ask(args),
            crate::commands::Command::Whatif(args) => Verb::Whatif(args),
            _ => panic!("a turn verb"),
        };
        verb.draft(&mut std::io::empty())
    }

    #[test]
    fn new_builds_a_first_turn_or_only_a_session() {
        let only = draft(&["new", "Trip"]).unwrap();
        assert!(only.create.is_some());
        // Creating a session saves no turn, so a request ID has nothing to identify.
        for flag in [&["--request-id", "key"][..], &["--retry-interrupted"]] {
            assert!(matches!(
                draft(&[&["new", "Trip"][..], flag].concat()),
                Err(InputError::CreateOnly)
            ));
        }
        assert!(
            draft(&[
                "new",
                "Trip",
                "A fact",
                "-q",
                "q: Is it?",
                "--request-id",
                "key"
            ])
            .is_ok()
        );
        let turn = draft(&[
            "new",
            "Trip",
            "A fact",
            "-q",
            "transport: How? [train, taxi]",
            "--from",
            "Other",
        ])
        .unwrap();
        assert!(turn.create.is_none() && !turn.ephemeral);
        assert_eq!(turn.request.updates.len(), 1);
        assert_eq!(turn.request.turn.questions[0].id, "transport");
        assert!(turn.request.standing.is_empty());
        assert!(matches!(
            turn.request.session,
            SessionSelection::From { ref sources, .. } if sources.len() == 1
        ));
        // Facts alone are a turn with nothing to ask.
        assert!(matches!(
            draft(&["new", "Trip", "A fact"]),
            Err(InputError::NoQuestion)
        ));
    }

    #[test]
    fn a_turn_asks_exactly_the_one_question_it_is_given() {
        let asked = draft(&["add", "Trip", "-q", "a: A?"]).unwrap();
        assert_eq!(asked.request.turn.questions.len(), 1);
        assert!(asked.request.standing.is_empty());
        // Each question is its own call.
        for verb in [
            &["new", "Trip"][..],
            &["add", "Trip"],
            &["ask", "Trip"],
            &["whatif", "Trip"],
        ] {
            let twice = [verb, &["-q", "a: A?", "-q", "b: B?"]].concat();
            let Err(error) = Cli::try_parse_from([&["nouli"][..], &twice].concat()) else {
                panic!("a second -q is rejected")
            };
            assert_eq!(error.kind(), clap::error::ErrorKind::ArgumentConflict);
        }
        for verb in ["add", "ask", "whatif"] {
            assert!(matches!(
                draft(&[verb, "Trip"]),
                Err(InputError::NoQuestion)
            ));
        }
        for removed in [&["add", "Trip", "--drop", "a"][..], &["questions", "Trip"]] {
            assert!(Cli::try_parse_from([&["nouli"][..], removed].concat()).is_err());
        }
    }

    #[test]
    fn verbs_choose_their_session_selection_and_durability() {
        let add = draft(&["add", "Trip", "-q", "q: Is it?"]).unwrap();
        assert!(matches!(
            add.request.session,
            SessionSelection::Continue { .. }
        ));
        let what = draft(&["whatif", "Trip", "A fact", "-q", "q: Is it?"]).unwrap();
        assert!(what.ephemeral && what.request.request_id.is_none());
        assert!(matches!(
            what.request.session,
            SessionSelection::From { .. }
        ));
        let ask = draft(&["ask", "Trip", "-q", "q: Is it?", "--request-id", "key"]).unwrap();
        assert_eq!(
            ask.request.updates,
            [] as [nouli_core::context::ContextUpdate; 0]
        );
        assert_eq!(ask.request.request_id.as_deref(), Some("key"));
        assert!(matches!(
            draft(&["ask", "Trip", "--retry-interrupted"]),
            Err(InputError::Retry)
        ));
    }

    #[test]
    fn a_replayed_replace_command_reuses_its_saved_updates_and_others_do_not() {
        let command = |text: &str, extra: &str| {
            draft(&[
                "add",
                "Trip",
                "--replace",
                "1",
                text,
                extra,
                "-q",
                "q: Is it?",
                "--request-id",
                "key",
            ])
            .unwrap()
        };
        let fact = |text: &str| {
            let ContextUpdate::Append { record } = facts::append(text).unwrap() else {
                panic!("append")
            };
            facts::Fact {
                number: 1,
                records: vec![record.id],
                key: record.key,
                text: text.into(),
                alternatives: Vec::new(),
                turn: Id::generate(),
                source_session: None,
                previous: None,
            }
        };
        let first = command("NEW", "Added")
            .resolve(&[fact("old")])
            .unwrap()
            .request
            .updates;
        // The replacement is now fact 1; the command still repeats.
        let replayed = command("NEW", "Added").replay(&first).unwrap();
        assert_eq!(replayed.request.updates, first);
        assert_eq!(replayed.replaces, [] as [(usize, std::string::String); 0]);
        assert!(command("OTHER", "Added").replay(&first).is_none());
        assert!(command("NEW", "Another").replay(&first).is_none());
    }

    #[test]
    fn an_input_file_with_a_turn_wrapper_may_omit_the_turn_version() {
        let dir = std::env::temp_dir().join(format!("nouli-turns-{}", Id::generate()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("request.json");
        let question =
            nouli_core::decision::grammar::parse("transport: How? [train, taxi]").unwrap();
        let value = serde_json::json!({
            "turn": {"state": [{"type": "text", "text": "A fact"}], "questions": [question]}
        });
        std::fs::write(&path, value.to_string()).unwrap();
        let drafted = draft(&["add", "Trip", "--input", path.to_str().unwrap()]);
        std::fs::remove_dir_all(&dir).unwrap();
        let request = drafted.unwrap().request;
        assert_eq!(request.turn.version, nouli_core::decision::CONTRACT_VERSION);
        assert_eq!(request.turn.questions[0].id, "transport");
    }

    #[test]
    fn input_mistakes_are_named_without_echoing_the_text() {
        let wrong = draft(&["add", "Trip", "-q", "SECRET with no id"]);
        let Err(error @ InputError::Question(_)) = wrong else {
            panic!("a grammar error")
        };
        assert!(!error.to_string().contains("SECRET"));
        assert!(matches!(
            draft(&["add", "Trip", "--input", "x.json", "A fact"]),
            Err(InputError::InputConflict)
        ));
        assert!(matches!(
            draft(&["add", "Trip", "  "]),
            Err(InputError::Fact(facts::FactError::Empty))
        ));
        assert!(matches!(
            draft(&["add", "Trip", "--replace", "0", "text"]),
            Err(InputError::ReplaceNumber)
        ));
        assert!(matches!(
            draft(&["add", "Trip", "--replace", "1", "a", "--replace", "1", "b"]),
            Err(InputError::DuplicateReplace(1))
        ));
    }

    #[test]
    fn replacements_resolve_against_numbered_facts_and_a_request_id_fixes_record_ids() {
        let current: Vec<_> = ["one", "two"]
            .iter()
            .map(|text| {
                let ContextUpdate::Append { record } = facts::append(text).unwrap() else {
                    panic!("append")
                };
                facts::Fact {
                    number: 0,
                    records: vec![record.id],
                    key: record.key,
                    text: (*text).into(),
                    alternatives: Vec::new(),
                    turn: Id::generate(),
                    source_session: None,
                    previous: None,
                }
            })
            .collect();
        let resolved = draft(&[
            "add",
            "Trip",
            "New",
            "--replace",
            "2",
            "TWO",
            "-q",
            "q: Is it?",
        ])
        .unwrap()
        .resolve(&current)
        .unwrap();
        assert!(matches!(
            &resolved.request.updates[0],
            ContextUpdate::Supersede { targets, .. } if targets == &current[1].records
        ));
        assert!(matches!(
            &resolved.request.updates[1],
            ContextUpdate::Append { .. }
        ));
        assert!(matches!(
            draft(&["add", "Trip", "--replace", "3", "TWO", "-q", "q: Is it?"])
                .unwrap()
                .resolve(&current),
            Err(InputError::UnknownFact(3))
        ));
        // The same command with the same request ID repeats its record IDs.
        let again = || {
            draft(&[
                "add",
                "Trip",
                "Same",
                "-q",
                "q: Is it?",
                "--request-id",
                "key",
            ])
            .unwrap()
            .request
            .updates
        };
        assert_eq!(again(), again());
        let anonymous = || {
            draft(&["add", "Trip", "Same", "-q", "q: Is it?"])
                .unwrap()
                .request
                .updates
        };
        assert_ne!(anonymous(), anonymous());
        assert_eq!(fact_counts(&resolved.request.updates), (2, 1));
    }
}
