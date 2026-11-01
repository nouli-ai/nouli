//! Existing-store command routing and human/serialized inspection output.
use crate::{
    render::{self, Format},
    selection::selector,
};
use clap::Subcommand;
use nouli_core::{
    codec,
    identity::{Id, SessionSelector},
    protocol::{ErrorCategory, ErrorReport},
    storage::{Ledger, OpenMode, inspection::InspectionError},
};
use std::{io::Write, path::Path};
use thiserror::Error;
#[derive(Debug, Subcommand)]
pub(crate) enum Command {
    /// Print a session's decision graph, including its captured source history.
    View(crate::view::Args),
    /// Inspect global saved sessions without changing the store.
    Sessions {
        #[command(subcommand)]
        command: SessionCommand,
    },
    /// List a session's numbered facts, marking those inherited from a source session.
    Facts {
        #[arg(value_parser=selector)]
        session: SessionSelector,
    },
    /// Inspect a finalized turn's actual context, attempts, and retention state.
    Context {
        #[arg(value_parser=selector)]
        session: SessionSelector,
        /// Defaults to the sole finalized head; required when there are several heads.
        #[arg(long,value_parser=id)]
        turn: Option<Id>,
    },
}
#[derive(Debug, Subcommand)]
pub(crate) enum SessionCommand {
    List,
    Show {
        #[arg(value_parser=selector)]
        session: SessionSelector,
    },
    Export {
        #[arg(value_parser=selector)]
        session: SessionSelector,
    },
}
fn id(value: &str) -> Result<Id, nouli_core::identity::IdentityError> {
    Id::try_from(value.to_owned())
}
#[derive(Debug, Error)]
pub(crate) enum InspectError {
    #[error(transparent)]
    Storage(#[from] nouli_core::storage::StorageError),
    #[error(transparent)]
    Session(#[from] nouli_core::storage::sessions::SessionError),
    #[error(transparent)]
    Inspection(#[from] InspectionError),
    #[error(transparent)]
    Codec(#[from] codec::CodecError),
    #[error(transparent)]
    Render(#[from] render::RenderError),
    #[error("cannot write inspection output")]
    Write(#[from] std::io::Error),
}
pub(crate) fn input_error(message: &str) -> ErrorReport {
    ErrorReport {
        category: ErrorCategory::Input,
        code: "invalid_inspection".into(),
        message: message.into(),
        retryable: false,
    }
}
impl InspectError {
    pub(crate) fn output_failed(&self) -> bool {
        matches!(
            self,
            Self::Write(_) | Self::Render(render::RenderError::Write(_))
        ) || matches!(self,Self::Render(render::RenderError::Json(error)) if error.is_io())
    }
    pub(crate) fn report(&self) -> ErrorReport {
        match self {
            Self::Storage(error) => error.report(),
            Self::Session(error) => error.report(),
            Self::Inspection(error) => error.report(),
            Self::Codec(_) | Self::Render(_) | Self::Write(_) => ErrorReport {
                category: ErrorCategory::Storage,
                code: "inspection_output_failed".into(),
                message: "Cannot encode or write inspection output".into(),
                retryable: false,
            },
        }
    }
}
pub(crate) fn run(
    command: &Command,
    store: &Path,
    sink: &mut impl Write,
    format: Format,
    color: bool,
) -> Result<(), InspectError> {
    let ledger = Ledger::open(store, OpenMode::ReadOnlyExisting)?;
    let (label, value) = match command {
        Command::View(args) => {
            let exports = crate::view::include_references(
                &ledger,
                vec![ledger.export_session(&args.session)?],
            )?;
            let export = &exports[0];
            return if args.mermaid {
                crate::view::write_mermaid(export, sink)
            } else {
                crate::view::write(export, sink, color)
            }
            .map_err(Into::into);
        }
        Command::Sessions {
            command: SessionCommand::List,
        } => {
            let sessions = ledger.list_sessions()?;
            if format == Format::Text {
                for session in sessions {
                    writeln!(
                        sink,
                        "{}  {}  heads={}",
                        session.id,
                        session
                            .name
                            .as_ref()
                            .map_or("(unnamed)", nouli_core::identity::SessionName::as_str),
                        session.frontier.len()
                    )?;
                }
                sink.flush()?;
                return Ok(());
            }
            (
                "Sessions",
                serde_json::json!({"version":1,"sessions":codec::to_value(&sessions)?}),
            )
        }
        Command::Sessions {
            command: SessionCommand::Show { session } | SessionCommand::Export { session },
        } => (
            "Complete saved session (turn outcomes are model judgments; caller commitments are context records; actual model context is recorded in manifests)",
            codec::to_value(&ledger.export_session(session)?)?,
        ),
        Command::Facts { session } => {
            let facts = ledger
                .facts(session)
                .map_err(nouli_core::storage::inspection::InspectionError::from)?;
            if format == Format::Text {
                crate::present::facts_text(&ledger, &facts, sink)?;
                return Ok(());
            }
            (
                "Facts",
                serde_json::json!({"version":1,"facts":codec::to_value(&facts)?}),
            )
        }
        Command::Context { session, turn } => (
            "Finalized turn context and attempts",
            codec::to_value(&ledger.inspect_session_turn(session, *turn)?)?,
        ),
    };
    write(label, &value, sink, format)
}
pub(crate) fn write(
    label: &str,
    value: &serde_json::Value,
    sink: &mut impl Write,
    format: Format,
) -> Result<(), InspectError> {
    match format {
        Format::Json => {
            serde_json::to_writer(&mut *sink, &value).map_err(render::RenderError::from)?;
            sink.write_all(b"\n")?;
        }
        Format::Toon => {
            sink.write_all(codec::encode_toon(&value)?.as_bytes())?;
            sink.write_all(b"\n")?;
        }
        Format::Text => {
            writeln!(sink, "{label}")?;
            sink.write_all(codec::encode_toon(&value)?.as_bytes())?;
            sink.write_all(b"\n")?;
        }
        Format::Auto | Format::Jsonl => return Err(render::RenderError::Modes.into()),
    }
    sink.flush()?;
    Ok(())
}
