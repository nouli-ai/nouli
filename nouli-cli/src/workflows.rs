//! Workflow CLI commands over the shared automatic engine and frozen recovery API.
mod events;
mod input;
use crate::{
    input as boundary,
    render::{self, Format},
    settings::Settings,
};
use clap::Subcommand;
use input::RunArgs;
use nouli_core::{
    codec,
    engine::{
        Engine,
        execution::{NativeProvider, TokioClock},
        workflow::{WorkflowEngineError, WorkflowInput},
    },
    history::SessionSelection,
    identity::Id,
    limits::count::ByteCounter,
    protocol::{ErrorCategory, ErrorReport, InspectionReference, Report, ResultEnvelope},
    storage::{
        Ledger, OpenMode,
        worker::{LedgerWorker, WorkerError},
    },
    workflow::RunMode,
};
use std::{io::Write, sync::Arc};
use thiserror::Error;
use tokio_util::sync::CancellationToken;
#[derive(Debug, Subcommand)]
pub(crate) enum Command {
    Run(RunArgs),
    Show {
        #[arg(value_parser=crate::selection::id)]
        run: Id,
    },
    Resume {
        #[arg(value_parser=crate::selection::id)]
        run: Id,
    },
}
#[derive(Debug, Error)]
pub(crate) enum WorkflowCliError {
    #[error(transparent)]
    Input(#[from] boundary::InputError),
    #[error("configuration setup failed")]
    Setup(ErrorReport),
    #[error(transparent)]
    Engine(#[from] WorkflowEngineError),
    #[error(transparent)]
    Worker(#[from] WorkerError),
    #[error(transparent)]
    Stream(#[from] nouli_core::protocol::stream::StreamError),
    #[error("cannot install cancellation handler")]
    Signal(#[source] std::io::Error),
    #[error("cannot start asynchronous workflow runtime")]
    Runtime(#[source] std::io::Error),
}
impl WorkflowCliError {
    fn envelope(&self) -> ResultEnvelope {
        let report = match self {
            Self::Input(error) => crate::evaluate::input_error(error),
            Self::Setup(error) => error.clone(),
            Self::Engine(error) => error.report(),
            Self::Worker(WorkerError::Storage(error)) => error.report(),
            Self::Worker(_) => ErrorReport {
                category: ErrorCategory::Storage,
                code: "ledger_worker_stopped".into(),
                message: "Ledger worker stopped".into(),
                retryable: false,
            },
            Self::Stream(nouli_core::protocol::stream::StreamError::Sink { error, .. })
                if error.category == ErrorCategory::Interrupted =>
            {
                error.clone()
            }
            Self::Stream(_) => ErrorReport {
                category: ErrorCategory::Storage,
                code: "event_stream_failed".into(),
                message: "Cannot deliver workflow events".into(),
                retryable: false,
            },
            Self::Signal(_) | Self::Runtime(_) => ErrorReport {
                category: ErrorCategory::Internal,
                code: "workflow_runtime_failed".into(),
                message: "Cannot initialize workflow runtime or cancellation".into(),
                retryable: false,
            },
        };
        let mut envelope = ResultEnvelope::new(Report::Error(report));
        if let Self::Stream(nouli_core::protocol::stream::StreamError::Sink {
            run: Some(run),
            ..
        }) = self
        {
            envelope.recovery = Some(InspectionReference {
                run: Some(*run),
                session: None,
                node: None,
                turn: None,
                attempts: vec![],
            });
        }
        envelope
    }
}
pub(crate) fn needs_config(command: &Command) -> bool {
    matches!(command, Command::Run(_))
}
/// Runs the commands behind the credentials the run's nodes will use, before
/// the engine asks for them; see `evaluate::fetch_credential`. The engine plans
/// the node settings, so a continued session's model defaults and a replayed
/// run's saved settings choose the providers here exactly as they do in the run.
async fn fetch_credentials(
    settings: &Settings,
    engine: &Engine<NativeProvider, ByteCounter, TokioClock>,
    worker: Option<&LedgerWorker>,
    input: &WorkflowInput<'_>,
    cancel: &CancellationToken,
) {
    let Ok(snapshots) = engine.planned_workflow_settings(worker, input).await else {
        return;
    };
    for snapshot in snapshots {
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
}
async fn execute(
    command: &Command,
    settings: &Settings,
    format: Format,
    request: Option<nouli_core::workflow::WorkflowRequest>,
) -> Result<ResultEnvelope, WorkflowCliError> {
    if let Command::Resume { run } = command {
        let reader = LedgerWorker::open(settings.store.clone(), OpenMode::ReadOnlyExisting).await?;
        let run = *run;
        reader
            .call(move |ledger| ledger.workflow_run(run))
            .await?
            .map_err(WorkflowEngineError::from)?;
    }
    let mode = request.as_ref().map(|request| request.workflow.mode);
    let fresh = matches!(
        request.as_ref().map(|request| &request.session),
        Some(SessionSelection::New { .. })
    );
    let worker =
        crate::evaluate::ledger_worker(settings, mode == Some(RunMode::Ephemeral), fresh).await?;
    let engine = Arc::new(crate::evaluate::engine());
    let cancel = CancellationToken::new();
    let sink = Arc::new(
        events::Output::new(format == Format::Jsonl, cancel.clone())
            .map_err(|error| nouli_core::protocol::stream::StreamError::Sink { run: None, error })?
            .with_config(settings.config_path.clone()),
    );
    let credentials = settings.credentials.clone();
    let lookup: nouli_core::engine::workflow::CredentialLookup =
        Arc::new(move |source| credentials.lookup(source));
    let evaluation = async {
        match command {
            Command::Run(args) => {
                let request = request.as_ref().ok_or(boundary::InputError::Shape)?;
                let overrides = args.overrides(settings);
                let input = WorkflowInput {
                    request,
                    config: &settings.config,
                    overrides: &overrides,
                    home: &settings.home,
                };
                fetch_credentials(settings, &engine, worker.as_ref(), &input, &cancel).await;
                Ok::<ResultEnvelope, WorkflowCliError>(
                    engine
                        .evaluate_workflow_report(
                            worker.as_ref(),
                            input,
                            lookup,
                            cancel.clone(),
                            sink,
                        )
                        .await?,
                )
            }
            Command::Resume { run } => Ok(engine
                .resume_workflow_report(
                    worker.as_ref().ok_or(boundary::InputError::Shape)?,
                    *run,
                    lookup,
                    cancel.clone(),
                    sink,
                )
                .await?),
            Command::Show { .. } => Err(boundary::InputError::Shape.into()),
        }
    };
    crate::evaluate::cancel_on_interrupt(evaluation, tokio::signal::ctrl_c(), &cancel)
        .await
        .map_err(WorkflowCliError::Signal)?
}
/// Runs a workflow command. `format` is never JSONL for `Command::Show`: the
/// caller resolves `show` with `render::resolve_report_format`.
pub(crate) fn run(command: &Command, settings: &Settings, format: Format) -> u8 {
    if let Command::Show { run } = command {
        let result = Ledger::open(&settings.store, OpenMode::ReadOnlyExisting)
            .map_err(crate::inspect::InspectError::from)
            .and_then(|ledger| {
                ledger
                    .inspect_workflow_full(*run)
                    .map_err(|error| crate::inspect::InspectError::Inspection(error.into()))
            })
            .and_then(|inspection| {
                let value =
                    codec::to_value(&inspection).map_err(crate::inspect::InspectError::from)?;
                crate::inspect::write(
                    "Saved workflow, recovery receipts, and ordered history",
                    &value,
                    &mut std::io::stdout().lock(),
                    format,
                )
            });
        return match result {
            Ok(()) => 0,
            Err(error) if error.output_failed() => 6,
            Err(error) => output_error(error.report(), format),
        };
    }
    let mut settings = settings.clone();
    let prepared = (|| {
        let request = if let Command::Run(args) = command {
            let request = args.request(&mut std::io::stdin().lock())?;
            // Compile before first-run filesystem setup or opening a ledger.
            nouli_core::workflow::compile::compile(&request.workflow)
                .map_err(WorkflowEngineError::from)?;
            Some(request)
        } else {
            None
        };
        settings
            .load_environment()
            .map_err(WorkflowCliError::Setup)?;
        if let (Command::Run(args), Some(request)) = (command, request.as_ref()) {
            settings.initialize().map_err(WorkflowCliError::Setup)?;
            crate::settings::preflight::workflow(&settings, &args.overrides(&settings), request)
                .map_err(|error| WorkflowCliError::Setup(error.report(&settings)))?;
        }
        Ok(request)
    })();
    let result = prepared.and_then(|request| {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(WorkflowCliError::Runtime)
            .and_then(|runtime| runtime.block_on(execute(command, &settings, format, request)))
    });
    match result {
        Ok(envelope) if format == Format::Jsonl => render::exit_code(&envelope),
        Ok(mut envelope) => {
            crate::settings::annotate_envelope(&mut envelope, &settings.config_path);
            render::result(
                &mut std::io::stdout().lock(),
                &mut std::io::stderr().lock(),
                format,
                &envelope,
            )
            .unwrap_or(6)
        }
        Err(error) => {
            let mut envelope = error.envelope();
            crate::settings::annotate_envelope(&mut envelope, &settings.config_path);
            if format == Format::Jsonl && matches!(error, WorkflowCliError::Stream(_)) {
                if let Report::Error(report) = &envelope.result
                    && render::diagnostic(&mut std::io::stderr().lock(), report).is_err()
                {
                    return 6;
                }
                if let Some(reference) = &envelope.recovery
                    && let Some(run) = reference.run
                    && writeln!(std::io::stderr().lock(), "Saved run: {run}").is_err()
                {
                    return 6;
                }

                render::exit_code(&envelope)
            } else {
                render::result(
                    &mut std::io::stdout().lock(),
                    &mut std::io::stderr().lock(),
                    format,
                    &envelope,
                )
                .unwrap_or(6)
            }
        }
    }
}
fn output_error(error: ErrorReport, format: Format) -> u8 {
    render::result(
        &mut std::io::stdout().lock(),
        &mut std::io::stderr().lock(),
        format,
        &ResultEnvelope::new(Report::Error(error)),
    )
    .unwrap_or(6)
}
