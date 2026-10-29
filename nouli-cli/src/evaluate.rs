//! CLI boundary for the shared decision engine and owned signal cancellation.
use crate::{
    Args,
    input::InputError,
    present::{self, PresentError},
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
    history::{EvaluationRequest, ModelSelection, SessionSelection},
    limits::count::ByteCounter,
    protocol::{ErrorCategory, ErrorReport, Report, ResultEnvelope, SessionReport, TurnReport},
    storage::{Ledger, OpenMode, worker::LedgerWorker},
};
use std::io::{self, IsTerminal};
use std::process::ExitCode;
use tokio_util::sync::CancellationToken;
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
    let result = execute(draft, settings).await;
    settings.credentials.forget();
    ResultEnvelope::new(match result {
        Ok(report) => Report::Turn(Box::new(report)),
        Err(error) => Report::Error(error.report(settings)),
    })
}
/// Runs the command behind the turn's provider credential, if it has one,
/// before the engine asks for it, so the wait is off the async runtime and ends
/// on Ctrl-C. When this cannot tell which provider the turn will use, the
/// engine's own lookup runs the command instead.
async fn fetch_credential(
    settings: &Settings,
    overrides: &RuntimeOverrides,
    request: &EvaluationRequest,
    worker: Option<&LedgerWorker>,
    cancel: &CancellationToken,
) {
    let defaults = match (&request.session, worker) {
        (SessionSelection::Continue { session }, Some(worker)) => {
            let selector = session.clone();
            match worker.call(move |ledger| ledger.session(&selector)).await {
                Ok(Ok(session)) => session.defaults,
                _ => return,
            }
        }
        _ => ModelSelection::default(),
    };
    let Ok(snapshot) =
        settings
            .config
            .resolve_snapshot(overrides, &request.model, &defaults, &settings.home)
    else {
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
async fn execute(draft: &Draft, settings: &Settings) -> Result<TurnReport, EvaluationError> {
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
        fetch_credential(settings, &overrides, request, worker.as_ref(), &cancel).await;
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
        signal=tokio::signal::ctrl_c()=> {
            cancel.cancel();let result=evaluation.await;
            if let Err(error)=signal {return Err(EvaluationError::Signal(error));}
            result.map_err(EvaluationError::from)
        }
    }
}
pub(crate) fn input_error(error: &InputError) -> ErrorReport {
    ErrorReport {
        category: ErrorCategory::Input,
        code: "invalid_decision_input".into(),
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

pub(crate) fn create(
    settings: &Settings,
    creation: &Creation,
) -> Result<SessionReport, ErrorReport> {
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
    let format = match render::resolve(
        &args.output,
        settings.config.output,
        io::stdout().is_terminal(),
    ) {
        Ok(format) => format,
        Err(_) => return ExitCode::from(2),
    };
    if let Some(creation) = &draft.create {
        let result = match create(&settings, creation) {
            Ok(report) => ResultEnvelope::new(Report::Session(report)),
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
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(_) => return ExitCode::from(6),
    };
    let result = runtime.block_on(run(&draft, &settings));
    if format == Format::Text
        && let Report::Turn(report) = &result.result
    {
        let color = render::color_enabled(io::stdout().is_terminal());
        return match present::turn_text(
            &mut io::stdout().lock(),
            report,
            &draft.request.updates,
            &settings.store,
            color,
            true,
        ) {
            Ok(()) => ExitCode::from(render::exit_code(&result)),
            Err(PresentError::Render(_)) => ExitCode::from(6),
            Err(PresentError::History(error)) => render_error(args, settings.config.output, error),
        };
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
