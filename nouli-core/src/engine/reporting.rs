//! Safe category mapping for public reports, separate from process exit policy.
use super::{EngineError, workflow::WorkflowEngineError};
use crate::{
    config::ConfigError,
    context::{builder::BuildError, projection::ProjectionError},
    protocol::{ErrorCategory as Category, ErrorReport, OUTCOME_UNKNOWN},
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
        StorageError::NewerSchema { found, supported } => ErrorReport {
            category: Category::Storage,
            code: "newer_ledger_schema".into(),
            message: format!("Ledger schema {found} is newer than supported schema {supported}"),
            retryable: false,
        },
        StorageError::MigrationRequired { found } => ErrorReport {
            category: Category::Storage,
            code: "ledger_migration_required".into(),
            message: format!("Read-only ledger requires migration from schema {found}"),
            retryable: false,
        },
        StorageError::WrongStore => message(Category::Storage, "not_a_nouli_ledger"),
        StorageError::ReadOnly => message(Category::Storage, "read_only_ledger"),
        StorageError::Payload => message(Category::Storage, "invalid_saved_payload"),
        StorageError::Io { source, .. } if source.kind() == std::io::ErrorKind::NotFound => {
            message(Category::Storage, "missing_ledger")
        }
        StorageError::FrozenSettings => ErrorReport {
            category: Category::Storage,
            code: "unsupported_frozen_settings".into(),
            message: error.to_string(),
            retryable: false,
        },
        _ => message(Category::Storage, "ledger_unavailable"),
    }
}
impl StorageError {
    #[must_use]
    pub fn report(&self) -> ErrorReport {
        storage(self)
    }
}
impl SessionError {
    #[must_use]
    pub fn report(&self) -> ErrorReport {
        session(self)
    }
}
impl crate::storage::inspection::InspectionError {
    #[must_use]
    pub fn report(&self) -> ErrorReport {
        use crate::storage::inspection::InspectionError;
        match self {
            InspectionError::Storage(error) => storage(error),
            InspectionError::Session(error) => session(error),
            InspectionError::Turn(error) => turn(error),
            InspectionError::Workflow(error) => error.report(),
            InspectionError::Head | InspectionError::ForeignTurn => ErrorReport {
                category: Category::Input,
                code: "invalid_inspection_selection".into(),
                message: self.to_string(),
                retryable: false,
            },
        }
    }
}

fn session(error: &SessionError) -> ErrorReport {
    match error {
        SessionError::Storage(error) => storage(error),
        SessionError::NameCollision => message(Category::Conflict, "session_name_exists"),
        SessionError::NamesExhausted => {
            let mut report = message(Category::Conflict, "session_names_exhausted");
            report.message = error.to_string();
            report
        }
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
/// Request mistakes the caller can fix by changing what the turn asks.
fn history(error: &crate::history::HistoryError) -> Option<ErrorReport> {
    use crate::{decision::ContractError, history::HistoryError};
    match error {
        HistoryError::Standing => Some(ErrorReport {
            category: Category::Input,
            code: "invalid_standing_question".into(),
            message: error.to_string(),
            retryable: false,
        }),
        HistoryError::Decision(ContractError::Empty("questions")) => Some(ErrorReport {
            category: Category::Input,
            code: "questions_required".into(),
            message: "No questions to ask: add one with -q, or in the request's turn.questions"
                .into(),
            retryable: false,
        }),
        _ => None,
    }
}
fn turn(error: &TurnError) -> ErrorReport {
    match error {
        TurnError::History(error) => {
            history(error).unwrap_or_else(|| message(Category::Input, "invalid_turn"))
        }
        TurnError::Storage(error) => storage(error),
        TurnError::Session(error) => session(error),
        TurnError::Projection(error) => projection(error),
        TurnError::Conflict => message(Category::Conflict, "head_conflict"),
        TurnError::WorkflowAttempt => message(Category::Internal, "workflow_attempt_misrouted"),
        _ => message(Category::Input, "invalid_turn"),
    }
}
fn config(error: &ConfigError) -> ErrorReport {
    match error {
        ConfigError::CredentialSource {
            variable, provider, ..
        } => ErrorReport {
            category: Category::Authentication,
            code: "credential_unavailable".into(),
            message: format!(
                "Set a valid {variable} environment variable for the selected provider, or run `nouli auth login {provider}`"
            ),
            retryable: false,
        },
        ConfigError::StoredCredential { provider, .. } => ErrorReport {
            category: Category::Authentication,
            code: "credential_unavailable".into(),
            message: format!(
                "The stored credential for {provider} is unavailable or invalid. Run `nouli auth login {provider}` to replace it"
            ),
            retryable: false,
        },
        ConfigError::HeaderCredential { .. } => ErrorReport { category: Category::Configuration, code: "header_credential_unavailable".into(), message: error.to_string(), retryable: false },
        ConfigError::MissingCredential | ConfigError::InvalidCredential => {
            message(Category::Authentication, "credential_unavailable")
        }
        ConfigError::NoProviders => ErrorReport {
            category: Category::Configuration,
            code: "providers_required".into(),
            message: "No providers defined. Define a [providers.NAME] section in the configuration, then set default_provider or pass --provider NAME".into(),
            retryable: false,
        },
        ConfigError::MissingProvider => ErrorReport {
            category: Category::Configuration,
            code: "provider_required".into(),
            message: "No provider selected. Set default_provider in the configuration or pass --provider NAME".into(),
            retryable: false,
        },
        ConfigError::MissingModel => ErrorReport {
            category: Category::Configuration,
            code: "model_required".into(),
            message: "No model selected. Set default_model in the configuration, pass --model MODEL, or supply a model in the request".into(),
            retryable: false,
        },
        ConfigError::UnknownProvider => ErrorReport {
            category: Category::Configuration,
            code: "unknown_provider".into(),
            message: "Selected provider is not defined. Add a matching [providers.NAME] section to the configuration".into(),
            retryable: false,
        },
        ConfigError::Read { source, .. } if source.kind() == std::io::ErrorKind::NotFound => message(Category::Configuration, "configuration_not_found"),
        ConfigError::Read { .. } => message(Category::Configuration, "configuration_unreadable"),
        ConfigError::Syntax => message(Category::Configuration, "invalid_configuration_syntax"),
        ConfigError::Invalid(_) => message(Category::Configuration, "invalid_configuration"),
    }
}
fn limit(error: &crate::limits::LimitError) -> ErrorReport {
    use crate::limits::LimitError;
    match error {
        LimitError::Codec(crate::codec::CodecError::ImageReferenceConflict) => ErrorReport {
            category: Category::Input,
            code: "image_reference_conflict".into(),
            message: crate::codec::CodecError::ImageReferenceConflict.to_string(),
            retryable: false,
        },
        LimitError::TotalCapacity | LimitError::QuestionCapacity | LimitError::RequestBytes => {
            message(Category::Capacity, "context_capacity")
        }
        LimitError::InvalidBudget | LimitError::Profile => {
            message(Category::Configuration, "invalid_model_budget")
        }
        _ => message(Category::Capability, "unsupported_model_input"),
    }
}
impl WorkflowError {
    #[must_use]
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
            Self::NotDurable => message(Category::Input, "workflow_not_durable"),
            Self::UnknownRun => message(Category::Input, "unknown_workflow"),
            Self::Compile(_) | Self::History(_) => message(Category::Input, "invalid_workflow"),
        }
    }
}
/// The one report for a provider failure, shared by saved attempts and final reports.
pub(super) fn provider(error: &ProviderError) -> ErrorReport {
    match error {
        ProviderError::Limit(error) => limit(error),
        ProviderError::Profile => message(Category::Configuration, "invalid_provider_profile"),
        ProviderError::Transport(error) => match error {
            TransportError::Authentication { .. } => {
                message(Category::Authentication, "provider_authentication_failed")
            }
            TransportError::Canceled => message(Category::Interrupted, OUTCOME_UNKNOWN),
            TransportError::Timeout => message(Category::Transport, OUTCOME_UNKNOWN),
            TransportError::Capacity => message(Category::Capacity, "provider_capacity"),
            TransportError::Capability => message(Category::Capability, "provider_capability"),
            TransportError::Contract => message(Category::Contract, "provider_contract"),
            _ => message(Category::Transport, "provider_transport_failed"),
        },
        _ => message(Category::Contract, "provider_contract"),
    }
}
impl EngineError {
    #[must_use]
    pub fn report(&self) -> ErrorReport {
        match self {
            Self::Workflow(error) => error.report(),
            Self::Config(error) => config(error),
            Self::Storage(error)
            | Self::Worker(crate::storage::worker::WorkerError::Storage(error)) => storage(error),
            Self::Session(error) => session(error),
            Self::Source(error) => source(error),
            Self::Turn(error) => turn(error),
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
                ReplayError::UnknownRequest => message(Category::Input, "unknown_request"),
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
            Self::Provider(error) => provider(error),
            Self::History(error) => {
                history(error).unwrap_or_else(|| message(Category::Input, "invalid_evaluation"))
            }
            Self::EphemeralOptions | Self::EphemeralStore => {
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
    #[must_use]
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

#[cfg(test)]
mod tests {
    #[test]
    fn exhausted_automatic_names_explain_how_to_create_another_session() {
        let report = super::session(&crate::storage::sessions::SessionError::NamesExhausted);
        assert_eq!(report.code, "session_names_exhausted");
        assert!(report.message.contains("supply an explicit name"));
        assert!(!report.retryable);
    }
    use super::*;
    #[test]
    fn specific_errors_keep_their_codes_ahead_of_catch_all_arms() {
        for (report, code) in [
            (
                EngineError::Replay(ReplayError::UnknownRequest).report(),
                "unknown_request",
            ),
            (
                EngineError::Provider(ProviderError::Profile).report(),
                "invalid_provider_profile",
            ),
            (WorkflowError::NotDurable.report(), "workflow_not_durable"),
            (
                turn(&TurnError::WorkflowAttempt),
                "workflow_attempt_misrouted",
            ),
        ] {
            assert_eq!(report.code, code);
        }
    }

    #[test]
    fn configuration_reports_identify_setup_errors_without_echoing_private_details() {
        for (error, code) in [
            (ConfigError::NoProviders, "providers_required"),
            (ConfigError::MissingProvider, "provider_required"),
            (ConfigError::UnknownProvider, "unknown_provider"),
            (ConfigError::Syntax, "invalid_configuration_syntax"),
            (
                ConfigError::Invalid("PRIVATE_SENTINEL"),
                "invalid_configuration",
            ),
            (
                ConfigError::Read {
                    path: "PRIVATE_SENTINEL".into(),
                    source: std::io::Error::new(
                        std::io::ErrorKind::PermissionDenied,
                        "PRIVATE_SENTINEL",
                    ),
                },
                "configuration_unreadable",
            ),
            (
                ConfigError::Read {
                    path: "PRIVATE_SENTINEL".into(),
                    source: std::io::Error::from(std::io::ErrorKind::NotFound),
                },
                "configuration_not_found",
            ),
        ] {
            let report = EngineError::Config(error).report();
            assert_eq!(report.category, Category::Configuration);
            assert_eq!(report.code, code);
            assert!(!report.message.contains("PRIVATE_SENTINEL"));
            assert!(!report.retryable);
        }
        let missing = EngineError::Config(ConfigError::MissingProvider).report();
        assert!(
            missing.message.contains("default_provider") && missing.message.contains("--provider")
        );
    }
}
