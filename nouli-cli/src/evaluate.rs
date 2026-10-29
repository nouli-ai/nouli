//! CLI boundary for the shared decision engine and owned signal cancellation.
use crate::{input::DecisionArgs, settings::Settings};
use nouli_core::{
    config::resolve::RuntimeOverrides,
    engine::{
        Engine, EngineError, EvaluationInput,
        execution::{NativeProvider, TokioClock},
    },
    history::{EvaluationRequest, ModelSelection, SessionSelection},
    limits::count::ByteCounter,
    protocol::{ErrorCategory, ErrorReport, Report, ResultEnvelope, TurnReport},
    storage::{OpenMode, worker::LedgerWorker},
};
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
pub(crate) async fn run(
    args: &DecisionArgs,
    request: &EvaluationRequest,
    settings: &Settings,
) -> ResultEnvelope {
    let result = execute(args, request, settings).await;
    ResultEnvelope::new(match result {
        Ok(report) => Report::Turn(Box::new(report)),
        Err(error) => Report::Error(error.report(settings)),
    })
}
async fn execute(
    args: &DecisionArgs,
    request: &EvaluationRequest,
    settings: &Settings,
) -> Result<TurnReport, EvaluationError> {
    let engine = Engine {
        provider: NativeProvider,
        counter: ByteCounter,
        clock: TokioClock,
    };
    let overrides = overrides(args, settings);
    let ephemeral = args.selection.no_session;
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
        retry_interrupted: args.retry_interrupted,
    };
    let cancel = CancellationToken::new();
    let evaluation = async {
        if ephemeral {
            let result = engine
                .evaluate_ephemeral(
                    worker.as_ref(),
                    input,
                    |name| std::env::var(name).ok(),
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
                    |name| std::env::var(name).ok(),
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
pub(crate) fn input_error(error: &crate::input::InputError) -> ErrorReport {
    ErrorReport {
        category: ErrorCategory::Input,
        code: if matches!(error, crate::input::InputError::Missing) {
            "decision_input_required".into()
        } else {
            "invalid_decision_input".into()
        },
        message: error.to_string(),
        retryable: false,
    }
}

pub(crate) fn overrides(args: &DecisionArgs, settings: &Settings) -> RuntimeOverrides {
    RuntimeOverrides {
        selection: ModelSelection {
            provider: args.provider.clone(),
            model: args.model.clone(),
        },
        store: Some(settings.store.clone()),
        context_budget_tokens: args.context_budget,
        ..Default::default()
    }
}
