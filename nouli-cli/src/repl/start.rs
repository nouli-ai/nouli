//! Starting the loop: finding or creating the session, then running it on the
//! terminal or on piped input.
use super::{Current, Event, Lines, Open, Queue, Reader, Repl, label, own_turns, plural, terminal};
use crate::{
    Args, evaluate,
    render::{
        self,
        turn::{DIM, Paint, TEAL},
    },
    render_error, selection, settings,
    settings::Settings,
    turns::Creation,
};
use nouli_core::{
    history::ModelSelection,
    identity::SessionSelector,
    protocol::ErrorReport,
    storage::{
        Ledger, OpenMode, StorageError,
        sessions::{Session, SessionError},
    },
};
use std::{
    io::{self, IsTerminal, Write},
    path::Path,
    process::ExitCode,
};

/// How the loop finds its session.
pub(crate) enum Target {
    Named(SessionSelector),
    Pick,
}

fn history_path(store: &Path) -> std::path::PathBuf {
    store.join("repl_history")
}

/// The ledger, or `None` when the store has no ledger yet. Every other failure
/// to open it is an error, not an empty store.
fn open_existing(settings: &Settings) -> Result<Option<Ledger>, ErrorReport> {
    match Ledger::open(&settings.store, OpenMode::ReadOnlyExisting) {
        Ok(ledger) => Ok(Some(ledger)),
        Err(StorageError::Io { source, .. }) if source.kind() == io::ErrorKind::NotFound => {
            Ok(None)
        }
        Err(error) => Err(error.report()),
    }
}

/// The saved sessions with their own turn counts, oldest first.
fn saved_sessions(settings: &Settings) -> Result<Vec<(Session, usize)>, ErrorReport> {
    let Some(ledger) = open_existing(settings)? else {
        return Ok(vec![]);
    };
    ledger
        .list_sessions()
        .map_err(|error| error.report())?
        .into_iter()
        .map(|session| {
            let turns = own_turns(&ledger, &session)?;
            Ok((session, turns))
        })
        .collect()
}

/// Lists saved sessions and asks for a number or a new name.
fn pick<L: Lines, W: Write>(
    settings: &Settings,
    paint: Paint,
    lines: &mut L,
    out: &mut W,
) -> io::Result<Result<Option<SessionSelector>, ErrorReport>> {
    let sessions = match saved_sessions(settings) {
        Ok(sessions) => sessions,
        Err(error) => return Ok(Err(error)),
    };
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
                            return Ok(Ok(Some(SessionSelector::Id(session.id))));
                        }
                        None => {
                            writeln!(out, "  no session number {number}")?;
                            continue;
                        }
                    }
                }
                match selection::selector(answer) {
                    Ok(selector) => return Ok(Ok(Some(selector))),
                    Err(_) => writeln!(
                        out,
                        "  a session name starts with a letter and uses letters, digits, `.`, `_` or `-`"
                    )?,
                }
            }
            Event::Interrupt | Event::Eof => return Ok(Ok(None)),
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
    let existing = match open_existing(settings) {
        Ok(Some(ledger)) => match ledger.session(&selector) {
            Ok(session) => Some(session),
            Err(SessionError::UnknownSession) => None,
            Err(error) => return Ok(Err(error.report())),
        },
        Ok(None) => None,
        Err(error) => return Ok(Err(error)),
    };
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
        // Anything but a yes, including Ctrl-C, declines.
        match lines.confirm(&raw, out)? {
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
            Ok(Some(selector)) => selector,
            Ok(None) => return Ok(Ok(())),
            Err(error) => return Ok(Err(error)),
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
    use nouli_core::identity::{Id, SessionName};
    use std::fs;

    #[test]
    fn an_unreadable_store_is_an_error_but_a_missing_one_is_empty() {
        let root = std::env::temp_dir().join(format!("nouli-repl-store-{}", Id::generate()));
        let settings = Settings::for_test(root.clone());
        let pick = |settings: &Settings| {
            let mut out = Vec::new();
            let picked = pick(settings, Paint(false), &mut Reader(&b""[..]), &mut out).unwrap();
            (picked, String::from_utf8(out).unwrap())
        };
        // No ledger yet: no sessions, and the name prompt follows.
        let (picked, shown) = pick(&settings);
        assert!(matches!(picked, Ok(None)), "{shown}");
        assert!(shown.contains("no sessions yet"));
        let selector = || SessionSelector::Name(SessionName::try_from("Trip".to_owned()).unwrap());
        let mut out = Vec::new();
        let mut lines = Reader(&b""[..]);
        let found = resolve(&settings, selector(), false, &mut lines, &mut out).unwrap();
        // The script creates the missing session.
        assert!(matches!(found, Ok((_, true))));
        // A ledger that cannot be read is reported, not taken for an empty one.
        fs::write(root.join("ledger.sqlite3"), b"not a ledger at all").unwrap();
        let (picked, shown) = pick(&settings);
        let Err(error) = picked else {
            panic!("an error report, got {shown}")
        };
        assert!(!shown.contains("no sessions yet"));
        assert!(!error.message.is_empty());
        let found = resolve(&settings, selector(), false, &mut lines, &mut out).unwrap();
        assert!(found.is_err());
        let _ = fs::remove_dir_all(&root);
    }

    /// One answer to the creation prompt.
    struct Answer(Option<Event>);

    impl Lines for Answer {
        fn read(&mut self, _: &str, _: &str, _: &mut dyn Write) -> io::Result<Event> {
            Ok(self.0.take().unwrap_or(Event::Eof))
        }
    }

    #[test]
    fn only_a_yes_creates_a_missing_session() {
        let root = std::env::temp_dir().join(format!("nouli-repl-confirm-{}", Id::generate()));
        let settings = Settings::for_test(root.clone());
        let ask = |answer: Event| {
            let selector = SessionSelector::Name(SessionName::try_from("Trip".to_owned()).unwrap());
            let mut out = Vec::new();
            resolve(
                &settings,
                selector,
                true,
                &mut Answer(Some(answer)),
                &mut out,
            )
            .unwrap()
        };
        for declined in [Event::Interrupt, Event::Eof, Event::Line("n".into())] {
            let Err(error) = ask(declined) else {
                panic!("a declined prompt created the session")
            };
            assert_eq!(error.code, "session_not_created");
        }
        assert!(matches!(ask(Event::Line(String::new())), Ok((_, true))));
        let _ = fs::remove_dir_all(&root);
    }
}
