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
        Ledger, OpenMode,
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

/// Resolves the target to a session, creating a named one that is missing.
fn resolve(settings: &Settings, selector: SessionSelector) -> Result<(Session, bool), ErrorReport> {
    let existing = match super::open_existing(&settings.store)? {
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
    evaluate::create(settings, &creation).map(|session| (session, true))
}

/// A new session with an automatically assigned name.
pub(super) fn new_session(settings: &Settings) -> Result<Session, ErrorReport> {
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
        // A bare `nouli` starts a new session with a generated name.
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
            settings: std::borrow::Cow::Borrowed(settings),
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
        settings: std::borrow::Cow::Borrowed(settings),
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
    if can_draw() && settings.config.providers.is_empty() {
        match crate::connect::configure(&settings.config_path, &crate::connect::Connect::default())
        {
            Ok(_) => {
                settings = match settings::load(args) {
                    Ok(s) => s,
                    Err(e) => {
                        return render_error(
                            args,
                            nouli_core::config::OutputFormat::Auto,
                            settings::error(args, e),
                        );
                    }
                };
                if let Err(e) = settings.load_environment() {
                    return render_error(args, settings.config.output, e);
                }
            }
            Err(error) => {
                let mut report = error.report();
                settings.annotate(&mut report);
                return render_error(args, settings.config.output, report);
            }
        }
    }
    let paint = Paint(render::color_enabled(io::stdout().is_terminal()));
    let mut model = ModelSelection {
        provider: open.and_then(|open| open.provider.clone()),
        model: open.and_then(|open| open.model.clone()),
    };
    if can_draw()
        && let Target::Named(selector) = &target
    {
        let ledger = match super::open_existing(&settings.store) {
            Ok(ledger) => ledger,
            Err(error) => return render_error(args, settings.config.output, error),
        };
        if let Some(ledger) = ledger {
            match ledger.session(selector) {
                Ok(session) => {
                    model = crate::selection::model(None, &[&model, &session.defaults]);
                }
                Err(SessionError::UnknownSession) => {}
                Err(error) => return render_error(args, settings.config.output, error.report()),
            }
        }
    }
    if can_draw()
        && let Err(error) = crate::connect::startup(&mut settings, &mut model)
    {
        let mut report = error.report();
        settings.annotate(&mut report);
        return render_error(args, settings.config.output, report);
    }
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
    fn a_bare_start_makes_a_new_session_with_a_stable_generated_name() {
        let root = std::env::temp_dir().join(format!("nouli-repl-new-{}", Id::generate()));
        let settings = Settings::for_test(root.clone());
        let session = new_session(&settings).unwrap();
        let name = session.name.as_ref().unwrap();
        assert_eq!(label(&session), name.as_str());
        assert_eq!(name.as_str().split('-').count(), 3);
        let (restored, created) = resolve(&settings, SessionSelector::Name(name.clone())).unwrap();
        assert!(!created);
        assert_eq!(restored, session);
        let _ = fs::remove_dir_all(&root);
    }
}
