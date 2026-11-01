//! Validate first-store provider setup without interfering with frozen replay.
use super::Settings;
use nouli_core::{
    config::{
        ConfigError,
        resolve::{RuntimeOverrides, SettingsSnapshot, select_model},
    },
    engine::EngineError,
    history::{EvaluationRequest, ModelSelection},
    limits::{LimitError, Modality, ResolvedLimits},
    protocol::{ErrorCategory, ErrorReport},
    workflow::WorkflowRequest,
};
use std::fmt::Write as _;
use thiserror::Error;
#[derive(Debug, Error)]
pub(crate) enum PreflightError {
    #[error(transparent)]
    Config(#[from] ConfigError),
    #[error(transparent)]
    Limit(#[from] LimitError),
    #[error("cannot inspect ledger location")]
    Io(#[from] std::io::Error),
}
impl PreflightError {
    pub(crate) fn report(self, settings: &Settings) -> ErrorReport {
        match self {
            Self::Config(source) => settings.config_error(source),
            Self::Limit(error) => {
                let mut report = EngineError::Limit(error).report();
                if report.category == ErrorCategory::Configuration {
                    let _ = write!(
                        report.message,
                        ". Configuration: {}",
                        settings.config_path.display()
                    );
                }
                report
            }
            Self::Io(_) => ErrorReport {
                category: ErrorCategory::Storage,
                code: "ledger_unavailable".into(),
                message: "Cannot inspect ledger location before setup".into(),
                retryable: false,
            },
        }
    }
}
fn config(source: ConfigError) -> PreflightError {
    PreflightError::Config(source)
}
fn snapshot(
    settings: &Settings,
    overrides: &RuntimeOverrides,
    model: &ModelSelection,
) -> Result<SettingsSnapshot, PreflightError> {
    settings
        .config
        .resolve_snapshot(overrides, model, &ModelSelection::default(), &settings.home)
        .map_err(config)
}
fn validate(
    settings: &Settings,
    snapshot: SettingsSnapshot,
    request: &nouli_core::decision::TurnRequest,
) -> Result<(), PreflightError> {
    ResolvedLimits::for_profile(&snapshot.profile)?.validate_request(request, Modality::Text)?;
    snapshot
        .with_credentials(|source| settings.credentials.lookup(source))
        .map_err(config)?;
    Ok(())
}
fn existing(settings: &Settings) -> Result<bool, PreflightError> {
    Ok(settings.store.join("ledger.sqlite3").try_exists()?)
}
pub(crate) fn turn(
    settings: &Settings,
    overrides: &RuntimeOverrides,
    request: &EvaluationRequest,
) -> Result<(), PreflightError> {
    if existing(settings)? {
        return Ok(());
    }
    validate(
        settings,
        snapshot(settings, overrides, &request.model)?,
        &request.turn,
    )
}
pub(crate) fn workflow(
    settings: &Settings,
    overrides: &RuntimeOverrides,
    request: &WorkflowRequest,
) -> Result<(), PreflightError> {
    if existing(settings)? {
        return Ok(());
    }
    let run_model = select_model(&[&overrides.selection, &request.workflow.model]);
    for node in &request.workflow.nodes {
        let mut node_overrides = overrides.clone();
        node_overrides.selection = node.model.clone();
        validate(
            settings,
            snapshot(settings, &node_overrides, &run_model)?,
            &node.turn,
        )?;
    }
    Ok(())
}
