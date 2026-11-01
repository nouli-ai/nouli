//! Complete credential-free embedding journey. All decisions use `FakeProvider`.
#[path = "embedding/fake.rs"]
mod fake;
use nouli_core::{
    codec::{self, InputFormat},
    config::{Config, resolve::RuntimeOverrides},
    context::{ContextKind, ContextRecord, ContextUpdate, PinTarget},
    data::Data,
    engine::{
        Engine, EngineError, EvaluationInput, EvaluationResult, execution::TokioClock,
        workflow::WorkflowInput,
    },
    history::{
        EvaluationRequest, ModelSelection, SessionSelection, SourceSelection, StandingChanges,
    },
    identity::{Id, SessionSelector},
    limits::count::ByteCounter,
    outcome::{Answer, QuestionOutcome},
    protocol::{Report, stream::CollectEvents, validate_events},
    storage::{OpenMode, worker::LedgerWorker},
    workflow::{
        OutputBinding, RunStatus, WorkflowRequest, WorkflowSpec,
        reference::{Absence, MissingPolicy, ReferenceValue, ResultField, ResultReference},
    },
};
use std::{
    path::{Path, PathBuf},
    sync::{Arc, atomic::Ordering},
};
use thiserror::Error;
use tokio_util::sync::CancellationToken;
#[derive(Debug, Error)]
enum ExampleError {
    #[error(transparent)]
    Config(#[from] nouli_core::config::ConfigError),
    #[error(transparent)]
    Codec(#[from] codec::CodecError),
    #[error(transparent)]
    Engine(#[from] EngineError),
    #[error(transparent)]
    Worker(#[from] nouli_core::storage::worker::WorkerError),
    #[error(transparent)]
    Stream(#[from] nouli_core::protocol::stream::StreamError),
    #[error(transparent)]
    Events(#[from] nouli_core::protocol::EventError),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Data(#[from] nouli_core::data::DataError),
    #[error(transparent)]
    Replay(#[from] nouli_core::storage::replay::ReplayError),
}
fn request(selection: SessionSelection) -> Result<EvaluationRequest, ExampleError> {
    Ok(EvaluationRequest {
        standing: StandingChanges::default(),
        turn: codec::decode(
            include_bytes!("../../fixtures/contracts/decision-request.json"),
            InputFormat::Json,
        )?,
        session: selection,
        updates: vec![],
        required_records: vec![],
        model: ModelSelection::default(),
        request_id: None,
    })
}
fn sources(sessions: &[Id]) -> SessionSelection {
    SessionSelection::From {
        sources: sessions
            .iter()
            .map(|session| SourceSelection {
                session: SessionSelector::Id(*session),
                frontier: None,
            })
            .collect(),
        name: None,
    }
}
type FakeEngine = Engine<fake::FakeProvider, ByteCounter, TokioClock>;
/// What every evaluation in the journey shares: the embedding's home, store,
/// configuration, and engine.
struct Journey<'a> {
    root: &'a Path,
    store: PathBuf,
    config: Config,
    options: RuntimeOverrides,
    cancel: CancellationToken,
    engine: Arc<FakeEngine>,
}
impl Journey<'_> {
    fn input<'a>(&'a self, request: &'a EvaluationRequest) -> EvaluationInput<'a> {
        EvaluationInput {
            request,
            config: &self.config,
            overrides: &self.options,
            home: self.root,
            retry_interrupted: false,
        }
    }
    async fn evaluate(
        &self,
        ledger: &LedgerWorker,
        request: &EvaluationRequest,
    ) -> Result<EvaluationResult, EngineError> {
        self.engine
            .evaluate(ledger, self.input(request), |_| None, &self.cancel)
            .await
    }
    fn provider_calls(&self) -> usize {
        self.engine.provider.calls.load(Ordering::SeqCst)
    }
}
async fn journey(root: &Path) -> Result<(), ExampleError> {
    let store = root.join("ledger");
    let ledger = LedgerWorker::open(store.clone(), OpenMode::ReadWriteCreate).await?;
    let journey = Journey {
        root,
        options: RuntimeOverrides {
            store: Some(store.clone()),
            ..Default::default()
        },
        store,
        config: fake::config()?,
        cancel: CancellationToken::new(),
        engine: Arc::new(Engine {
            provider: fake::FakeProvider::default(),
            counter: ByteCounter,
            clock: TokioClock,
        }),
    };
    let (initial, commitment) = first_turn(&journey, &ledger).await?;
    let blended = branch_and_blend(&journey, &ledger, &initial, &commitment).await?;
    drop(ledger);
    ephemeral_evaluation_leaves_the_store_unchanged(&journey, &initial, &blended).await?;
    canceled_evaluation_never_calls_the_provider(&journey).await?;
    let Journey {
        store,
        config,
        options,
        engine,
        ..
    } = journey;
    workflow_journey(root, store, &engine, &config, &options).await
}
/// Starts a session with a pinned commitment, checks the provider's native
/// outcomes, and replays the same request without calling the provider again.
async fn first_turn(
    journey: &Journey<'_>,
    ledger: &LedgerWorker,
) -> Result<(EvaluationResult, ContextRecord), ExampleError> {
    let commitment = ContextRecord {
        id: Id::generate(),
        key: "selected_route".into(),
        value: Data::new(serde_json::json!({"route":"train", "completed":true, "receipt":null}))?,
        kind: ContextKind::Commitment,
    };
    let mut first = request(SessionSelection::New { name: None })?;
    first.request_id = Some("embedding-first".into());
    first.updates.push(ContextUpdate::Pin {
        target: PinTarget::New {
            record: commitment.clone(),
        },
    });
    let initial = journey.evaluate(ledger, &first).await?;
    assert_native_outcomes(&initial);
    let accepted = first.clone();
    let frozen = ledger
        .call(move |ledger| ledger.saved_request(&accepted))
        .await??
        .expect("accepted request");
    assert_eq!(frozen.settings.store, journey.store);
    let replay = journey.evaluate(ledger, &first).await?;
    assert!(replay.replayed);
    assert_eq!(replay.turn.id, initial.turn.id);
    assert_eq!(journey.provider_calls(), 1);
    Ok((initial, commitment))
}
/// The first turn keeps the provider's own model, confidence, score scale, and
/// each answer kind, including a refusal.
fn assert_native_outcomes(initial: &EvaluationResult) {
    assert_eq!(initial.turn.outcome.provider.provider, "fake");
    assert_eq!(initial.turn.outcome.provider.model, "fake-served-version-1");
    let QuestionOutcome::Answered {
        confidence: Some(confidence),
        ..
    } = &initial.turn.outcome.outcomes[0]
    else {
        panic!("native confidence")
    };
    assert_eq!(confidence.value, Some(0.8));
    assert!(confidence.meaning.contains("fake-served-version-1"));
    let nouli_core::decision::AnswerSpace::Score { levels } =
        &initial.turn.request.questions[2].answer_space
    else {
        panic!("original score scale")
    };
    assert_eq!(
        levels[2],
        nouli_core::decision::Criterion::Text("Excellent".into())
    );
    assert!(matches!(
        initial.turn.outcome.outcomes[0],
        QuestionOutcome::Answered {
            answer: Answer::Choice { .. },
            ..
        }
    ));
    assert!(matches!(
        initial.turn.outcome.outcomes[1],
        QuestionOutcome::Answered {
            answer: Answer::Noul { probability: 1.0 },
            ..
        }
    ));
    assert!(matches!(
        initial.turn.outcome.outcomes[2],
        QuestionOutcome::Answered {
            answer: Answer::Score { value: 1.43, .. },
            ..
        }
    ));
    assert!(matches!(
        initial.turn.outcome.outcomes[3],
        QuestionOutcome::Refused { .. }
    ));
}
/// Continues the first session, branches from it, and blends both. Advancing
/// a source after the blend captured it leaves the blend's saved base alone.
async fn branch_and_blend(
    journey: &Journey<'_>,
    ledger: &LedgerWorker,
    initial: &EvaluationResult,
    commitment: &ContextRecord,
) -> Result<EvaluationResult, ExampleError> {
    let continued = request(SessionSelection::Continue {
        session: SessionSelector::Id(initial.turn.session),
    })?;
    let continuation = journey.evaluate(ledger, &continued).await?;
    let mut branch = request(sources(&[initial.turn.session]))?;
    branch.required_records = vec![commitment.id, initial.turn.id];
    branch.updates.push(ContextUpdate::Unpin {
        target: commitment.id,
    });
    let branched = journey.evaluate(ledger, &branch).await?;
    assert_eq!(
        branched.turn.manifest.sources[0].frontier,
        vec![continuation.turn.id]
    );
    let blend = request(sources(&[initial.turn.session, branched.turn.session]))?;
    let blended = journey.evaluate(ledger, &blend).await?;
    assert_eq!(blended.turn.outcome.outcomes, initial.turn.outcome.outcomes);
    // Advance a source after capture. Continuing the blend retains the saved base.
    let later = journey.evaluate(ledger, &continued).await?;
    let follow = request(SessionSelection::Continue {
        session: SessionSelector::Id(blended.turn.session),
    })?;
    let followed = journey.evaluate(ledger, &follow).await?;
    assert!(
        !followed
            .turn
            .manifest
            .context_toon
            .contains(&later.turn.id.to_string())
    );
    Ok(blended)
}
/// An ephemeral evaluation reads the blend from a read-only store and leaves
/// its bytes unchanged.
async fn ephemeral_evaluation_leaves_the_store_unchanged(
    journey: &Journey<'_>,
    initial: &EvaluationResult,
    blended: &EvaluationResult,
) -> Result<(), ExampleError> {
    let bytes = std::fs::read(journey.store.join("ledger.sqlite3"))?;
    let readonly = LedgerWorker::open(journey.store.clone(), OpenMode::ReadOnlyExisting).await?;
    let ephemeral = request(sources(&[blended.turn.session]))?;
    let temporary = journey
        .engine
        .evaluate_ephemeral(
            Some(&readonly),
            journey.input(&ephemeral),
            |_| None,
            &journey.cancel,
        )
        .await?;
    assert_eq!(temporary.outcome.outcomes, initial.turn.outcome.outcomes);
    drop(readonly);
    assert!(
        bytes == std::fs::read(journey.store.join("ledger.sqlite3"))?,
        "ephemeral evaluation changed source store"
    );
    Ok(())
}
/// An evaluation canceled before it starts fails without calling the provider.
async fn canceled_evaluation_never_calls_the_provider(
    journey: &Journey<'_>,
) -> Result<(), ExampleError> {
    let canceled = CancellationToken::new();
    canceled.cancel();
    let fresh = request(SessionSelection::New { name: None })?;
    let before = journey.provider_calls();
    assert!(matches!(
        journey
            .engine
            .evaluate_ephemeral(None, journey.input(&fresh), |_| None, &canceled)
            .await,
        Err(EngineError::Canceled)
    ));
    assert_eq!(journey.provider_calls(), before);
    Ok(())
}
async fn workflow_journey(
    root: &Path,
    store: PathBuf,
    engine: &Arc<FakeEngine>,
    config: &Config,
    options: &RuntimeOverrides,
) -> Result<(), ExampleError> {
    let ledger = LedgerWorker::open(store, OpenMode::ReadWriteCreate).await?;
    let mut workflow: WorkflowSpec = codec::decode(
        include_bytes!("../../fixtures/contracts/workflow.json"),
        InputFormat::Json,
    )?;
    for node in &mut workflow.nodes {
        node.turn.state.push(nouli_core::data::Input::Text {
            text: format!("NODE:{}", node.id),
        });
    }
    for (name, question, field, pointer) in [
        (
            "explicit_null",
            "on_time",
            ResultField::Outcome,
            Some("/confidence".into()),
        ),
        ("refused_answer", "private", ResultField::Answer, None),
    ] {
        workflow.outputs.push(OutputBinding {
            name: name.into(),
            reference: ResultReference {
                node: "final".into(),
                question: question.into(),
                field,
                pointer,
                on_missing: MissingPolicy::Optional,
            },
        });
    }
    let mut request = WorkflowRequest {
        workflow,
        session: SessionSelection::New { name: None },
        updates: vec![],
        required_records: vec![],
        request_id: Some("embedding-workflow".into()),
    };
    engine.provider.fail_high_once.store(true, Ordering::SeqCst);
    let events = Arc::new(CollectEvents::default());
    let partial = engine
        .evaluate_workflow_report(
            Some(&ledger),
            WorkflowInput {
                request: &request,
                config,
                overrides: options,
                home: root,
            },
            Arc::new(|_| None),
            CancellationToken::new(),
            events.clone(),
        )
        .await?;
    validate_events(&events.snapshot()?)?;
    let Report::Run(partial) = partial.result else {
        panic!("partial workflow")
    };
    assert_eq!(partial.status, RunStatus::Partial);
    let run = partial.run.unwrap();
    let before = engine.provider.calls.load(Ordering::SeqCst);
    // Recovery does not accept a new graph/configuration: it uses the stored run.
    request.workflow.nodes.clear();
    let events = Arc::new(CollectEvents::default());
    let done = engine
        .resume_workflow_report(
            &ledger,
            run,
            Arc::new(|_| None),
            CancellationToken::new(),
            events.clone(),
        )
        .await?;
    validate_events(&events.snapshot()?)?;
    let Report::Run(done) = done.result else {
        panic!("completed workflow")
    };
    assert_eq!(done.status, RunStatus::Completed);
    assert_eq!(done.nodes[0].turn, partial.nodes[0].turn);
    assert_eq!(engine.provider.calls.load(Ordering::SeqCst) - before, 3);
    assert!(
        matches!(&done.outputs[0].value, ReferenceValue::Present { value } if value.value() == &serde_json::json!(1.43))
    );
    assert!(matches!(
        done.outputs[1].value,
        ReferenceValue::Absent {
            reason: Absence::Skipped
        }
    ));
    assert!(
        matches!(&done.outputs[2].value, ReferenceValue::Present { value } if value.value().is_null())
    );
    assert!(matches!(
        done.outputs[3].value,
        ReferenceValue::Absent {
            reason: Absence::Refused
        }
    ));
    Ok(())
}
// Keep the isolated directory owned even if an example assertion panics.
struct ExampleDirectory(std::path::PathBuf);
impl Drop for ExampleDirectory {
    fn drop(&mut self) {
        if let Err(error) = std::fs::remove_dir_all(&self.0)
            && error.kind() != std::io::ErrorKind::NotFound
        {
            tracing::warn!("embedding example temporary directory cleanup failed");
        }
    }
}
#[tokio::main]
async fn main() -> Result<(), ExampleError> {
    let root = std::env::temp_dir().join(format!("nouli-embedding-{}", Id::generate()));
    std::fs::create_dir(&root)?;
    let directory = ExampleDirectory(root);
    let canonical = std::fs::canonicalize(&directory.0)?;
    let result = Box::pin(journey(&canonical)).await;
    let cleanup = std::fs::remove_dir_all(&directory.0);
    result?;
    cleanup?;
    Ok(())
}
#[cfg(test)]
mod tests {
    #[test]
    fn complete_embedding_journey_runs_without_credentials_or_cli() {
        super::main().unwrap();
    }
}
