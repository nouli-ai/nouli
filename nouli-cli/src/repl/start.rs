//! Starting the loop: finding or creating the session, then running it on the
//! terminal or on piped input.
use super::{Current, Open, Queue, Repl, can_draw, label, tui};
use crate::{
    Args, evaluate,
    render::{self, turn::Paint},
    render_error, settings,
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
    process::ExitCode,
};

/// How the loop finds its session.
pub(crate) enum Target {
    Named(SessionSelector),
    Pick,
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

/// Resolves the target to a session, creating a named one that is missing.
fn resolve(settings: &Settings, selector: SessionSelector) -> Result<(Session, bool), ErrorReport> {
    let existing = match open_existing(settings)? {
        Some(ledger) => match ledger.session(&selector) {
            Ok(session) => Some(session),
            Err(SessionError::UnknownSession) => None,
            Err(error) => return Err(error.report()),
        },
        None => None,
    };
    if let Some(session) = existing {
        return Ok((session, false));
    }
    let SessionSelector::Name(name) = selector else {
        return Err(ErrorReport {
            category: nouli_core::protocol::ErrorCategory::Input,
            code: "invalid_session".into(),
            message: "No session has that ID".into(),
            retryable: false,
        });
    };
    let creation = Creation {
        name,
        sources: vec![],
        defaults: ModelSelection::default(),
    };
    evaluate::create(settings, &creation).and_then(|report| {
        Ledger::open(&settings.store, OpenMode::ReadOnlyExisting)
            .map_err(|error| error.report())
            .and_then(|ledger| {
                ledger
                    .session(&SessionSelector::Id(report.session))
                    .map_err(|error| error.report())
            })
            .map(|session| (session, true))
    })
}

/// A new session with no name.
fn new_session(settings: &Settings) -> Result<Session, ErrorReport> {
    Ledger::open(&settings.store, OpenMode::ReadWriteCreate)
        .map_err(|error| error.report())?
        .create_session(None, ModelSelection::default())
        .map_err(|error| error.report())
}

/// How a loop was started.
struct Start {
    paint: Paint,
    target: Target,
    model: ModelSelection,
    context_budget: Option<u64>,
}

/// Runs the view on a terminal that can draw it, and reads lines from
/// standard input otherwise.
fn session_loop(settings: &Settings, start: Start) -> io::Result<Result<(), ErrorReport>> {
    let Start {
        paint,
        target,
        model,
        context_budget,
    } = start;
    let found = match target {
        Target::Named(selector) => resolve(settings, selector),
        // A bare `nouli` starts a new session, known by its ID.
        Target::Pick => new_session(settings).map(|session| (session, true)),
    };
    let (session, created) = match found {
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
    let current = Current {
        id: session.id,
        label: label(&session),
    };
    if can_draw() {
        // The view's header identifies the app and session.
        let repl = Repl {
            settings,
            out: Vec::new(),
            paint,
            model,
            context_budget,
            session: current,
            queue: Queue::default(),
        };
        return tui::run(repl, &runtime).map(Ok);
    }
    let mut repl = Repl {
        settings,
        out: io::stdout().lock(),
        paint,
        model,
        context_budget,
        session: current,
        queue: Queue::default(),
    };
    repl.banner(created)?;
    let result = repl.run(&mut io::stdin().lock(), &runtime);
    repl.out.flush()?;
    result.map(Ok)
}

/// Runs the loop full screen on a terminal that can draw it, and on lines from
/// standard input otherwise.
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
    let paint = Paint(render::color_enabled(io::stdout().is_terminal()));
    let model = ModelSelection {
        provider: open.and_then(|open| open.provider.clone()),
        model: open.and_then(|open| open.model.clone()),
    };
    let outcome = session_loop(
        &settings,
        Start {
            paint,
            target,
            model,
            context_budget: open.and_then(|open| open.context_budget),
        },
    );
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
    fn a_missing_named_session_is_created_and_an_unreadable_store_is_an_error() {
        let root = std::env::temp_dir().join(format!("nouli-repl-store-{}", Id::generate()));
        let settings = Settings::for_test(root.clone());
        let selector = || SessionSelector::Name(SessionName::try_from("Trip".to_owned()).unwrap());
        assert!(matches!(resolve(&settings, selector()), Ok((_, true))));
        assert!(matches!(resolve(&settings, selector()), Ok((_, false))));
        // A ledger that cannot be read is reported, not taken for an empty one.
        fs::write(root.join("ledger.sqlite3"), b"not a ledger at all").unwrap();
        assert!(resolve(&settings, selector()).is_err());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn a_bare_start_makes_a_new_session_without_a_name() {
        let root = std::env::temp_dir().join(format!("nouli-repl-new-{}", Id::generate()));
        let settings = Settings::for_test(root.clone());
        let session = new_session(&settings).unwrap();
        assert!(session.name.is_none());
        assert_eq!(label(&session).len(), 8, "known by its short ID");
        let _ = fs::remove_dir_all(&root);
    }
}
