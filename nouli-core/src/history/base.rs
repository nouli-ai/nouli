//! A shared caller context event is not a synthetic model judgment.
use super::{HistoryError, SourcePoint};
use crate::{context::ContextUpdate, data::Input, identity::Id};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContextBase {
    pub id: Id,
    pub session: Id,
    pub sources: Vec<SourcePoint>,
    pub state: Vec<Input>,
    pub updates: Vec<ContextUpdate>,
    #[serde(default)]
    pub runtime_only: bool,
}
impl ContextBase {
    pub fn validate(&self) -> Result<(), HistoryError> {
        let mut records = HashSet::new();
        for source in &self.sources {
            if source.frontier.is_empty()
                || source.frontier.iter().collect::<HashSet<_>>().len() != source.frontier.len()
            {
                return Err(HistoryError::InvalidFrontier);
            }
        }
        for update in &self.updates {
            update.validate()?;
            if let Some(record) = update.new_record()
                && !records.insert(record.id)
            {
                return Err(crate::context::ContextError::DuplicateRecord.into());
            }
        }
        Ok(())
    }
}
