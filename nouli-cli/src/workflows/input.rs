//! Workflow file/selection validation, independent of runtime and persistence.
use crate::{
    input,
    selection::{SelectionArgs, validate_mode},
};
use clap::Args;
use nouli_core::{
    codec,
    config::resolve::RuntimeOverrides,
    history::{ModelSelection, SessionSelection},
    workflow::{RunMode, WorkflowRequest},
};
use std::{io::Read, path::PathBuf};
#[derive(Debug, Args)]
pub(crate) struct RunArgs {
    /// JSON/TOON workflow spec or complete WorkflowRequest, or - for stdin.
    #[arg(value_name = "FILE")]
    file: PathBuf,
    #[arg(long, value_enum)]
    input_format: Option<input::Format>,
    #[command(flatten)]
    selection: SelectionArgs,
    #[arg(long)]
    provider: Option<String>,
    #[arg(long)]
    model: Option<String>,
    /// Bound ready-node concurrency in addition to the graph's own limit.
    #[arg(long,value_parser=positive)]
    parallel: Option<usize>,
    #[arg(long,value_parser=clap::value_parser!(u64).range(1..))]
    context_budget: Option<u64>,
    #[arg(long)]
    request_id: Option<String>,
}
fn positive(value: &str) -> Result<usize, &'static str> {
    value
        .parse::<usize>()
        .ok()
        .filter(|value| *value > 0)
        .ok_or("parallelism must be positive")
}
impl RunArgs {
    pub(super) fn request(
        &self,
        stdin: &mut impl Read,
    ) -> Result<WorkflowRequest, input::InputError> {
        let (bytes, format) = input::file(&self.file, self.input_format, stdin)?;
        let mut value = codec::decode_value(&bytes, format)?;
        let (mut request, explicit) = if value.get("workflow").is_some() {
            let object = value.as_object_mut().ok_or(input::InputError::Shape)?;
            let explicit = object.contains_key("session");
            let workflow = object.remove("workflow").ok_or(input::InputError::Shape)?;
            let codec::Request::Workflow(workflow) = codec::request_from_value(workflow)? else {
                return Err(input::InputError::Shape);
            };
            object.insert("workflow".into(), codec::to_value(&workflow)?);
            object
                .entry("session")
                .or_insert(serde_json::json!({"mode":"new","name":null}));
            (
                serde_json::from_value::<WorkflowRequest>(value)
                    .map_err(|_| input::InputError::Shape)?,
                explicit,
            )
        } else {
            let codec::Request::Workflow(workflow) = codec::request_from_value(value)? else {
                return Err(input::InputError::Shape);
            };
            (
                WorkflowRequest {
                    workflow,
                    session: SessionSelection::New { name: None },
                    updates: vec![],
                    required_records: vec![],
                    request_id: None,
                },
                false,
            )
        };
        if self.selection.no_session {
            request.workflow.mode = RunMode::Ephemeral;
        }
        if let Some(key) = &self.request_id {
            request.request_id = Some(key.clone());
        }
        request.session =
            self.selection
                .apply(request.session, explicit, request.request_id.is_some())?;
        validate_mode(
            &request.session,
            request.workflow.mode == RunMode::Ephemeral,
            request.request_id.is_some(),
        )?;
        nouli_core::storage::workflows::fingerprint(&request)
            .map_err(|_| input::InputError::Shape)?;
        Ok(request)
    }
    pub(super) fn overrides(&self, settings: &crate::settings::Settings) -> RuntimeOverrides {
        RuntimeOverrides {
            selection: ModelSelection {
                provider: self.provider.clone(),
                model: self.model.clone(),
            },
            max_parallel: self.parallel,
            context_budget_tokens: self.context_budget,
            stored_credentials: settings.credentials.stored(),
            store: Some(settings.store.clone()),
            ..Default::default()
        }
    }
}
