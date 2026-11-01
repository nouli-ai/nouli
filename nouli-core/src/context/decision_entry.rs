//! Full decision entries shared by context assembly and deterministic reduction.
use crate::{decision::TurnRequest, identity::Id, outcome::TurnOutcome};
use serde::{Deserialize, Serialize};
use std::borrow::Cow;

#[derive(Serialize, Deserialize)]
pub(super) struct DecisionEntry<'a> {
    pub session: Option<Id>,
    pub turn: Option<Id>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workflow_node: Option<Cow<'a, str>>,
    pub request: Cow<'a, TurnRequest>,
    pub outcome: Cow<'a, TurnOutcome>,
}
