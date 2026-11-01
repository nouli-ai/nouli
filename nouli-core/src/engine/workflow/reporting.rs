//! Public compact reports and one ordered terminal event per invocation.
mod projection;
use super::{
    Arc, CancellationToken, CredentialLookup, DecisionProvider, Engine, ErrorCategory, Id,
    LedgerWorker, RetryClock, TokenCounter, WorkflowEngineError, WorkflowInput, WorkflowResult,
};
use crate::{
    protocol::{
        InspectionReference, Report, ResultEnvelope,
        stream::{EventSink, EventStream, StreamError},
    },
    workflow::WorkflowSpec,
};
pub use projection::{ReportError, workflow};

impl<
    P: DecisionProvider + Send + 'static,
    C: TokenCounter + Sync + Send + 'static,
    T: RetryClock + Send + 'static,
> Engine<P, C, T>
{
    /// Evaluate a workflow and stream its events and terminal result envelope to `sink`.
    ///
    /// # Errors
    ///
    /// Workflow failures are reported inside the returned envelope, not as `Err`.
    /// Returns [`StreamError::Sink`] if the sink rejected any event, including the terminal
    /// event, or [`StreamError::State`] if the event stream is inconsistent.
    pub async fn evaluate_workflow_report<S: EventSink>(
        self: &Arc<Self>,
        worker: Option<&LedgerWorker>,
        input: WorkflowInput<'_>,
        lookup: CredentialLookup,
        cancel: CancellationToken,
        sink: Arc<S>,
    ) -> Result<ResultEnvelope, StreamError> {
        let definition = &input.request.workflow;
        let stream = Arc::new(EventStream::new(
            sink,
            definition
                .nodes
                .iter()
                .map(|node| node.id.clone())
                .collect(),
        ));
        let result = self
            .evaluate_workflow_observed(worker, input, lookup, cancel, stream.clone())
            .await;
        finish(Some(definition), None, result, &stream).await
    }
    /// Resume a saved workflow run and stream its events and terminal result envelope to `sink`.
    ///
    /// # Errors
    ///
    /// Workflow failures are reported inside the returned envelope, not as `Err`.
    /// Returns [`StreamError::Sink`] if the sink rejected any event, including the terminal
    /// event, or [`StreamError::State`] if the event stream is inconsistent.
    pub async fn resume_workflow_report<S: EventSink>(
        self: &Arc<Self>,
        worker: &LedgerWorker,
        run: Id,
        lookup: CredentialLookup,
        cancel: CancellationToken,
        sink: Arc<S>,
    ) -> Result<ResultEnvelope, StreamError> {
        let stored = worker.call(move |ledger| ledger.workflow_run(run)).await;
        let definition = match stored {
            Ok(Ok(stored)) => stored.frozen.definition.request.workflow,
            result => {
                let error = match result {
                    Err(error) => WorkflowEngineError::Worker(error),
                    Ok(Err(error)) => WorkflowEngineError::Repository(error),
                    Ok(Ok(_)) => unreachable!(),
                };
                let stream = Arc::new(EventStream::new(sink, vec![]));
                return finish(None, None, Err(error), &stream).await;
            }
        };
        let stream = Arc::new(EventStream::new(
            sink,
            definition
                .nodes
                .iter()
                .map(|node| node.id.clone())
                .collect(),
        ));
        let result = self
            .resume_workflow_observed(worker, run, lookup, cancel, stream.clone())
            .await;
        finish(Some(&definition), Some(run), result, &stream).await
    }
}
async fn finish<S: EventSink>(
    definition: Option<&WorkflowSpec>,
    known_run: Option<Id>,
    result: Result<WorkflowResult, WorkflowEngineError>,
    stream: &EventStream<S>,
) -> Result<ResultEnvelope, StreamError> {
    let mut recovery_run = known_run.or(stream.established_run().await);
    let report = match result {
        Ok(result) => {
            recovery_run = result.run;
            let projected = definition
                .ok_or(projection::ReportError::Snapshot)
                .and_then(|definition| projection::workflow(definition, &result, None));
            match projected {
                Ok(report) => Report::Run(report),
                Err(_) => Report::Error(super::super::reporting::message(
                    ErrorCategory::Internal,
                    "workflow_report_mismatch",
                )),
            }
        }
        Err(error) => {
            if let WorkflowEngineError::Execution { run, .. }
            | WorkflowEngineError::Existing { run, .. } = &error
            {
                recovery_run = Some(*run);
            }
            Report::Error(error.report())
        }
    };
    let mut envelope = ResultEnvelope::new(report);
    if matches!(envelope.result, Report::Error(_)) && recovery_run.is_some() {
        envelope.recovery = Some(recovery(recovery_run, None));
    }
    stream.finish(envelope.clone()).await?;
    Ok(envelope)
}
fn recovery(run: Option<Id>, session: Option<Id>) -> InspectionReference {
    InspectionReference {
        run,
        session,
        node: None,
        turn: None,
        attempts: vec![],
    }
}
