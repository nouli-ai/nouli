//! Safe category mapping for public reports, separate from process exit policy.
use super::{EngineError, workflow::WorkflowEngineError};
use crate::{
    config::ConfigError,
    context::{builder::BuildError, projection::ProjectionError},
    protocol::{ErrorCategory as Category, ErrorReport},
    provider::ProviderError,
    storage::{
        StorageError, guards::GuardError, replay::ReplayError, sessions::SessionError,
        sources::SourceError, turns::TurnError, workflows::WorkflowError,
    },
    transport::TransportError,
};
pub(crate) fn message(category: Category, code: &str) -> ErrorReport {
    ErrorReport {
        category,
        code: code.into(),
        message: code.replace('_', " "),
        retryable: false,
    }
}
fn storage(error: &StorageError) -> ErrorReport {
    match error {
        StorageError::Busy | StorageError::Conflict => {
            message(Category::Conflict, "ledger_conflict")
        }
        _ => message(Category::Storage, "ledger_unavailable"),
    }
}
fn session(error: &SessionError) -> ErrorReport {
    match error {
        SessionError::Storage(error) => storage(error),
        SessionError::NameCollision => message(Category::Conflict, "session_name_exists"),
        _ => message(Category::Input, "invalid_session"),
    }
}
fn guard(error: &GuardError) -> ErrorReport {
    match error {
        GuardError::Storage(error) => storage(error),
        GuardError::Session(error) => session(error),
        GuardError::WriterActive => message(Category::Conflict, "writer_active"),
    }
}
fn source(error: &SourceError) -> ErrorReport {
    match error {
        SourceError::Storage(error) => storage(error),
        SourceError::Session(error) => session(error),
        SourceError::History(_) => message(Category::Input, "invalid_source"),
    }
}
fn projection(error: &ProjectionError) -> ErrorReport {
    if matches!(error, ProjectionError::Conflicts { .. }) {
        message(Category::Conflict, "context_conflict")
    } else {
        message(Category::Input, "invalid_context")
    }
}
fn turn(error: &TurnError) -> ErrorReport {
    match error {
        TurnError::Storage(error) => storage(error),
        TurnError::Session(error) => session(error),
        TurnError::Projection(error) => projection(error),
        TurnError::Conflict => message(Category::Conflict, "head_conflict"),
        _ => message(Category::Input, "invalid_turn"),
    }
}
fn config(error: &ConfigError) -> ErrorReport {
    match error {
        ConfigError::MissingCredential | ConfigError::InvalidCredential => {
            message(Category::Authentication, "credential_unavailable")
        }
        _ => message(Category::Configuration, "invalid_configuration"),
    }
}
fn limit(error: &crate::limits::LimitError) -> ErrorReport {
    use crate::limits::LimitError;
    match error {
        LimitError::TotalCapacity | LimitError::QuestionCapacity => {
            message(Category::Capacity, "context_capacity")
        }
        LimitError::MissingBudget | LimitError::InvalidBudget | LimitError::Profile => {
            message(Category::Configuration, "invalid_model_budget")
        }
        _ => message(Category::Capability, "unsupported_model_input"),
    }
}
impl WorkflowError {
    pub fn report(&self) -> ErrorReport {
        match self {
            Self::Storage(error) => storage(error),
            Self::Session(error) => session(error),
            Self::Source(error) => source(error),
            Self::Turn(error) => turn(error),
            Self::Guard(error) => guard(error),
            Self::IntentConflict => message(Category::Conflict, "request_conflict"),
            Self::Conflict => message(Category::Conflict, "workflow_conflict"),
            Self::AdvancedHead => message(Category::Conflict, "advanced_session_head"),
            Self::Snapshot => message(Category::Storage, "invalid_saved_workflow"),
            Self::UnknownRun => message(Category::Input, "unknown_workflow"),
            Self::Compile(_) | Self::History(_) => message(Category::Input, "invalid_workflow"),
        }
    }
}
impl EngineError {
    pub fn report(&self) -> ErrorReport {
        match self {
            Self::Workflow(error) => error.report(),
            Self::Config(error) => config(error),
            Self::Storage(error) => storage(error),
            Self::Session(error) => session(error),
            Self::Source(error) => source(error),
            Self::Turn(error) => turn(error),
            Self::Worker(crate::storage::worker::WorkerError::Storage(error)) => storage(error),
            Self::Worker(_) => message(Category::Storage, "ledger_worker_stopped"),
            Self::Replay(error) => match error {
                ReplayError::Storage(error) => storage(error),
                ReplayError::Session(error) => session(error),
                ReplayError::Guard(error) => guard(error),
                ReplayError::Turn(error) => turn(error),
                ReplayError::Source(error) => source(error),
                ReplayError::Conflict | ReplayError::IntentConflict => {
                    message(Category::Conflict, "request_conflict")
                }
                _ => message(Category::Input, "invalid_request"),
            },
            Self::InProgress { .. } => message(Category::Conflict, "request_in_progress"),
            Self::Interrupted { .. } => {
                message(Category::Interrupted, "request_requires_explicit_retry")
            }
            Self::Canceled => message(Category::Interrupted, "evaluation_canceled"),
            Self::Build(error) => match error {
                BuildError::ProtectedCapacity | BuildError::OptionalCapacity => {
                    message(Category::Capacity, "context_capacity")
                }
                BuildError::Projection(error) => projection(error),
                BuildError::Limit(error) => limit(error),
                _ => message(Category::Input, "invalid_context"),
            },
            Self::Limit(error) => limit(error),
            Self::Provider(error) => match error {
                ProviderError::Limit(error) => limit(error),
                ProviderError::Transport(error) => match error {
                    TransportError::Authentication { .. } => {
                        message(Category::Authentication, "provider_authentication_failed")
                    }
                    TransportError::Canceled => {
                        message(Category::Interrupted, "provider_outcome_unknown")
                    }
                    TransportError::Timeout => {
                        message(Category::Transport, "provider_outcome_unknown")
                    }
                    TransportError::Capacity => message(Category::Capacity, "provider_capacity"),
                    TransportError::Capability => {
                        message(Category::Capability, "provider_capability")
                    }
                    TransportError::Contract => message(Category::Contract, "provider_contract"),
                    _ => message(Category::Transport, "provider_transport_failed"),
                },
                _ => message(Category::Contract, "provider_contract"),
            },
            Self::History(_) | Self::EphemeralOptions | Self::EphemeralStore => {
                message(Category::Input, "invalid_evaluation")
            }
        }
    }
}
fn schedule(error: &crate::workflow::schedule::ScheduleError) -> ErrorReport {
    use crate::workflow::schedule::ScheduleError;
    match error {
        ScheduleError::Observer(report) => report.clone(),
        ScheduleError::Parallelism => message(Category::Input, "invalid_parallelism"),
        ScheduleError::State => message(Category::Internal, "workflow_scheduler_failed"),
        ScheduleError::Resolve(_) => message(Category::Contract, "workflow_reference_failed"),
    }
}
impl WorkflowEngineError {
    pub fn report(&self) -> ErrorReport {
        match self {
            Self::Engine(error) => error.report(),
            Self::Repository(error) => error.report(),
            Self::Config(error) => config(error),
            Self::Worker(crate::storage::worker::WorkerError::Storage(error)) => storage(error),
            Self::Worker(_) => message(Category::Storage, "ledger_worker_stopped"),
            Self::Existing { .. } => {
                message(Category::Interrupted, "workflow_requires_explicit_resume")
            }
            Self::InProgress => message(Category::Conflict, "workflow_in_progress"),
            Self::Store => message(Category::Storage, "writable_ledger_required"),
            Self::Compile(_) => message(Category::Input, "invalid_workflow"),
            Self::Execution { error, .. } => schedule(error),
            Self::Schedule(error) => schedule(error),
        }
    }
}
