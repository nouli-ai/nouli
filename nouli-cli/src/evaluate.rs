//! CLI boundary for the shared decision engine and owned signal cancellation.
use crate::{
    Args,
    input::InputError,
    present,
    render::{self, Format},
    render_error, settings,
    settings::Settings,
    turns::{Creation, Draft, Verb},
};
use nouli_core::{
    config::resolve::RuntimeOverrides,
    engine::{
        Engine, EngineError, EvaluationInput,
        execution::{NativeProvider, TokioClock},
    },
    history::{ModelSelection, SessionSelection},
    limits::count::ByteCounter,
    protocol::{ErrorCategory, ErrorReport, Report, ResultEnvelope, SessionReport, TurnReport},
    storage::{Ledger, OpenMode, replay::ReplayError, sessions::Session, worker::LedgerWorker},
};
use std::io::{self, IsTerminal, Write};
use std::process::ExitCode;
use tokio_util::sync::CancellationToken;
pub(crate) const LEDGER_FILE: &str = "ledger.sqlite3";

#[derive(Debug, thiserror::Error)]
enum EvaluationError {
    #[error(transparent)]
    Engine(#[from] EngineError),
    #[error("cannot install cancellation handler")]
    Signal(#[source] std::io::Error),
}
impl EvaluationError {
    fn report(&self, settings: &Settings) -> ErrorReport {
        match self {
            Self::Engine(error) => {
                let mut report = error.report();
                settings.annotate(&mut report);
                report
            }
            Self::Signal(_) => ErrorReport {
                category: ErrorCategory::Internal,
                code: "signal_handler_failed".into(),
                message: "Cannot install cancellation handler".into(),
                retryable: false,
            },
        }
    }
}
pub(crate) async fn run(draft: &Draft, settings: &Settings) -> ResultEnvelope {
    run_until(draft, settings, tokio::signal::ctrl_c()).await
}
/// Runs a turn that ends when `interrupt` completes, the way Ctrl-C ends one
/// for [`run`]. The full-screen loop reads Ctrl-C as a key, not a signal.
pub(crate) async fn run_until(
    draft: &Draft,
    settings: &Settings,
    interrupt: impl Future<Output = io::Result<()>>,
) -> ResultEnvelope {
    let result = execute(draft, settings, interrupt).await;
    settings.credentials.forget();
    ResultEnvelope::new(match result {
        Ok(report) => Report::Turn(Box::new(report)),
        Err(error) => Report::Error(error.report(settings)),
    })
}
/// Runs the command behind the turn's provider credential, if it has one,
/// before the engine asks for it, so the wait is off the async runtime and ends
/// on Ctrl-C. The engine plans the settings, so the provider chosen here is the
/// one the turn uses. When the plan fails, the engine's own lookup runs the
/// command instead.
async fn fetch_credential(
    settings: &Settings,
    engine: &Engine<NativeProvider, ByteCounter, TokioClock>,
    input: &EvaluationInput<'_>,
    worker: Option<&LedgerWorker>,
    ephemeral: bool,
    cancel: &CancellationToken,
) {
    let planned = if ephemeral {
        engine.planned_ephemeral_settings(input).ok()
    } else if let Some(worker) = worker {
        engine.planned_settings(worker, input).await.ok()
    } else {
        None
    };
    let Some(snapshot) = planned else {
        return;
    };
    if snapshot.profile.api_key_env.is_some() || snapshot.profile.stored_credential {
        settings
            .credentials
            .fetch(
                &snapshot.profile_name,
                snapshot.profile.api_key_env.as_deref(),
                cancel,
            )
            .await;
    }
}
async fn execute(
    draft: &Draft,
    settings: &Settings,
    interrupt: impl Future<Output = io::Result<()>>,
) -> Result<TurnReport, EvaluationError> {
    let request = &draft.request;
    let engine = Engine {
        provider: NativeProvider,
        counter: ByteCounter,
        clock: TokioClock,
    };
    let overrides = overrides(draft, settings);
    let ephemeral = draft.ephemeral;
    let worker = if ephemeral && matches!(request.session, SessionSelection::New { .. }) {
        None
    } else {
        Some(
            LedgerWorker::open(
                settings.store.clone(),
                if ephemeral {
                    OpenMode::ReadOnlyExisting
                } else {
                    OpenMode::ReadWriteCreate
                },
            )
            .await
            .map_err(EngineError::from)?,
        )
    };
    let input = EvaluationInput {
        request,
        config: &settings.config,
        overrides: &overrides,
        home: &settings.home,
        retry_interrupted: draft.retry_interrupted,
    };
    let cancel = CancellationToken::new();
    let evaluation = async {
        fetch_credential(
            settings,
            &engine,
            &input,
            worker.as_ref(),
            ephemeral,
            &cancel,
        )
        .await;
        if ephemeral {
            let result = engine
                .evaluate_ephemeral(
                    worker.as_ref(),
                    input,
                    |source| settings.credentials.lookup(source),
                    &cancel,
                )
                .await?;
            Ok::<TurnReport, EngineError>(TurnReport {
                questions: result.request.questions,
                session: None,
                turn: None,
                outcome: result.outcome,
                manifest: result.manifest,
                replayed: false,
            })
        } else {
            let result = engine
                .evaluate(
                    worker.as_ref().ok_or(EngineError::EphemeralStore)?,
                    input,
                    |source| settings.credentials.lookup(source),
                    &cancel,
                )
                .await?;
            Ok::<TurnReport, EngineError>(TurnReport {
                questions: result.turn.request.questions,
                session: Some(result.turn.session),
                turn: Some(result.turn.id),
                outcome: result.turn.outcome,
                manifest: result.turn.manifest,
                replayed: result.replayed,
            })
        }
    };
    tokio::pin!(evaluation);
    tokio::select! {
        biased;
        result=&mut evaluation=>result.map_err(EvaluationError::from),
        signal=interrupt=> {
            cancel.cancel();let result=evaluation.await;
            if let Err(error)=signal {return Err(EvaluationError::Signal(error));}
            result.map_err(EvaluationError::from)
        }
    }
}
pub(crate) fn input_error(error: &InputError) -> ErrorReport {
    ErrorReport {
        category: ErrorCategory::Input,
        code: match error {
            InputError::NoQuestion => "questions_required",
            InputError::History(nouli_core::history::HistoryError::Standing) => {
                "invalid_standing_question"
            }
            _ => "invalid_decision_input",
        }
        .into(),
        message: error.to_string(),
        retryable: false,
    }
}

pub(crate) fn overrides(draft: &Draft, settings: &Settings) -> RuntimeOverrides {
    RuntimeOverrides {
        selection: ModelSelection {
            provider: draft.common_model.provider.clone(),
            model: draft.common_model.model.clone(),
        },
        store: Some(settings.store.clone()),
        stored_credentials: settings.credentials.stored(),
        context_budget_tokens: draft.context_budget,
        ..Default::default()
    }
}

pub(crate) fn create(settings: &Settings, creation: &Creation) -> Result<Session, ErrorReport> {
    let mut ledger =
        Ledger::open(&settings.store, OpenMode::ReadWriteCreate).map_err(|error| error.report())?;
    let session = if creation.sources.is_empty() {
        ledger
            .create_session(Some(creation.name.clone()), creation.defaults.clone())
            .map_err(|error| error.report())?
    } else {
        ledger
            .create_session_from(
                Some(creation.name.clone()),
                creation.defaults.clone(),
                &creation.sources,
            )
            .map_err(|error| EngineError::Source(error).report())?
    };
    Ok(session)
}

fn session_report(settings: &Settings, session: Session) -> Result<SessionReport, ErrorReport> {
    let ledger = Ledger::open(&settings.store, OpenMode::ReadOnlyExisting)
        .map_err(|error| error.report())?;
    let sources = ledger
        .session_sources(session.id)
        .map_err(|error| error.report())?;
    Ok(SessionReport {
        session: session.id,
        name: session.name,
        sources,
    })
}

/// Replaces `--replace N` numbers with the session's current facts.
pub(crate) fn resolve(settings: &Settings, draft: Draft) -> Result<Draft, ErrorReport> {
    let Some(session) = draft.replaced_session().cloned() else {
        return Ok(draft);
    };
    let ledger = Ledger::open(&settings.store, OpenMode::ReadOnlyExisting)
        .map_err(|error| error.report())?;
    // A repeated command repeats its saved replacements; after the first run
    // the numbers no longer name the facts they named then.
    if let Some(key) = draft.request_id()
        && let Some(saved) = ledger
            .saved_request_by_id(key)
            .map_err(|error| EngineError::from(error).report())?
    {
        return draft
            .replay(&saved.request.updates)
            .ok_or_else(|| EngineError::from(ReplayError::IntentConflict).report());
    }
    let current = ledger
        .facts(&session)
        .map_err(|error| EngineError::Turn(error).report())?;
    draft.resolve(&current).map_err(|error| input_error(&error))
}

/// Runs one of the turn verbs and writes its result.
pub(crate) fn command(args: &Args, verb: Verb<'_>) -> ExitCode {
    let creates = verb.creates_only();
    let loaded = if creates {
        settings::load_inspection(args)
    } else {
        settings::load(args)
    };
    let mut settings = match loaded {
        Ok(settings) => settings,
        Err(error) => {
            return render_error(
                args,
                nouli_core::config::OutputFormat::Auto,
                settings::error(args, error),
            );
        }
    };
    let draft = match verb.draft(&mut io::stdin().lock()) {
        Ok(draft) => draft,
        Err(error) => {
            return render_error(args, settings.config.output, input_error(&error));
        }
    };
    let format = match render::resolve_stream_format(
        &args.output,
        settings.config.output,
        io::stdout().is_terminal(),
    ) {
        Ok(format) => format,
        Err(error) => return render_error(args, settings.config.output, error),
    };
    if let Some(creation) = &draft.create {
        let result = match create(&settings, creation) {
            Ok(session) => match session_report(&settings, session) {
                Ok(report) => ResultEnvelope::new(Report::Session(report)),
                Err(error) => ResultEnvelope::new(Report::Error(error)),
            },
            Err(error) => ResultEnvelope::new(Report::Error(error)),
        };
        return write_result(&result, format);
    }
    if let Err(error) = settings
        .load_environment()
        .and_then(|()| settings.initialize())
    {
        return render_error(args, settings.config.output, error);
    }
    let draft = match resolve(&settings, draft) {
        Ok(draft) => draft,
        Err(error) => return render_error(args, settings.config.output, error),
    };
    let overrides = overrides(&draft, &settings);
    if let Err(error) = settings::preflight::turn(&settings, &overrides, &draft.request) {
        return render_error(args, settings.config.output, error.report(&settings));
    }
    let Ok(runtime) = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    else {
        return ExitCode::from(6);
    };
    let result = runtime.block_on(run(&draft, &settings));
    if format == Format::Text
        && let Report::Turn(report) = &result.result
    {
        // The turn is saved, so failing to draw it must not look like a failed
        // turn: render into a buffer, and fall back to the plain result.
        let mut text = Vec::new();
        let drawn = present::turn_text(
            &mut text,
            report,
            &draft.request.updates,
            &settings.store,
            render::color_enabled(io::stdout().is_terminal()),
            present::TurnDisplay::Full,
        );
        if drawn.is_ok() {
            let mut stdout = io::stdout().lock();
            return match stdout.write_all(&text).and_then(|()| stdout.flush()) {
                Ok(()) => ExitCode::from(render::exit_code(&result)),
                Err(_) => ExitCode::from(6),
            };
        }
        tracing::warn!("cannot draw the saved turn; writing its plain result");
    }
    write_result(&result, format)
}

fn write_result(result: &ResultEnvelope, format: Format) -> ExitCode {
    match render::result(
        &mut io::stdout().lock(),
        &mut io::stderr().lock(),
        format,
        result,
    ) {
        Ok(code) => ExitCode::from(code),
        Err(_) => ExitCode::from(6),
    }
}
