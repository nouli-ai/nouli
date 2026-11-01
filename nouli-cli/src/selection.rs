//! Shared CLI source grammar; source heads are resolved only by the ledger.
use clap::Args;
use nouli_core::{
    history::{SessionSelection, SourceSelection},
    identity::{Id, IdentityError, SessionName, SessionSelector},
};
use thiserror::Error;
#[derive(Debug, Default, Args)]
pub(crate) struct SelectionArgs {
    /// Continue an existing session by ID or unique name.
    #[arg(long,value_name="SELECTOR",value_parser=selector,conflicts_with_all=["from","name","no_session"])]
    pub session: Option<SessionSelector>,
    /// Branch/blend from SELECTOR or a fixed SELECTOR@TURN_ID (repeatable).
    #[arg(long,value_name="SOURCE",value_parser=source)]
    pub from: Vec<SourceSelection>,
    /// Assign a unique name to a new persisted session.
    #[arg(long,value_parser=name,conflicts_with="no_session")]
    pub name: Option<SessionName>,
    /// Evaluate without writing history; saved sources are opened read-only.
    #[arg(long)]
    pub no_session: bool,
}
#[derive(Debug, Error)]
pub(crate) enum SelectionError {
    #[error("CLI session selection competes with explicit request-file selection")]
    Competing,
    #[error("ephemeral evaluation cannot continue, name, or replay a session")]
    Ephemeral,
}
pub(crate) fn selector(value: &str) -> Result<SessionSelector, IdentityError> {
    match Id::try_from(value.to_owned()) {
        Ok(id) => Ok(SessionSelector::Id(id)),
        Err(_) => SessionName::try_from(value.to_owned()).map(SessionSelector::Name),
    }
}
pub(crate) fn name(value: &str) -> Result<SessionName, IdentityError> {
    SessionName::try_from(value.to_owned())
}
pub(crate) fn source(value: &str) -> Result<SourceSelection, IdentityError> {
    let (session, frontier) = if let Some((session, turn)) = value.split_once('@') {
        (
            selector(session)?,
            Some(vec![Id::try_from(turn.to_owned())?]),
        )
    } else {
        (selector(value)?, None)
    };
    Ok(SourceSelection { session, frontier })
}
impl SelectionArgs {
    pub(crate) fn apply(
        &self,
        file: SessionSelection,
        explicit: bool,
        request_id: bool,
    ) -> Result<SessionSelection, SelectionError> {
        let cli = self.session.is_some() || !self.from.is_empty() || self.name.is_some();
        if explicit && cli {
            return Err(SelectionError::Competing);
        }
        let selected = if !cli {
            file
        } else if let Some(session) = &self.session {
            SessionSelection::Continue {
                session: session.clone(),
            }
        } else if self.from.is_empty() {
            SessionSelection::New {
                name: self.name.clone(),
            }
        } else {
            SessionSelection::From {
                sources: self.from.clone(),
                name: self.name.clone(),
            }
        };
        validate_mode(&selected, self.no_session, request_id)?;
        Ok(selected)
    }
}

pub(crate) fn validate_mode(
    selected: &SessionSelection,
    ephemeral: bool,
    request_id: bool,
) -> Result<(), SelectionError> {
    if ephemeral
        && (request_id
            || matches!(
                selected,
                SessionSelection::Continue { .. }
                    | SessionSelection::New { name: Some(_) }
                    | SessionSelection::From { name: Some(_), .. }
            ))
    {
        return Err(SelectionError::Ephemeral);
    }
    Ok(())
}

/// Resolve explicit layers first, then optional configuration defaults.
pub(crate) fn model(
    config: Option<&nouli_core::config::Config>,
    layers: &[&nouli_core::history::ModelSelection],
) -> nouli_core::history::ModelSelection {
    let defaults = nouli_core::history::ModelSelection {
        provider: config.and_then(|config| config.default_provider.clone()),
        model: config.and_then(|config| config.default_model.clone()),
    };
    let mut selected = nouli_core::config::resolve::select_model(layers);
    if selected.provider.is_none() {
        selected.provider = defaults.provider;
    }
    if selected.model.is_none() {
        selected.model = defaults.model;
    }
    selected
}

pub(crate) fn id(value: &str) -> Result<Id, IdentityError> {
    Id::try_from(value.to_owned())
}
