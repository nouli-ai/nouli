//! Checked-in primary-source capability snapshot, never a live discovery call.

use super::Modality;
use crate::config::{Protocol, ProviderProfile};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Capabilities {
    pub protocol: Protocol,
    pub host: String,
    pub models: Vec<String>,
    pub total_context: u64,
    pub per_question_context: Option<u64>,
    pub max_questions: Option<usize>,
    pub max_choices: usize,
    pub max_score_levels: usize,
    pub modalities: Vec<Modality>,
    pub supports_refusal: Option<bool>,
    pub supports_confidence: bool,
    pub full_distributions: bool,
    pub sources: Vec<String>,
    pub verified_on: String,
    pub notes: String,
}

pub fn lookup(profile: &ProviderProfile) -> Option<Capabilities> {
    let host = url::Url::parse(&profile.base_url)
        .ok()?
        .host_str()?
        .to_owned();
    let registry: Vec<Capabilities> =
        serde_json::from_str(include_str!("../../../fixtures/capabilities/models.json"))
            .expect("validated checked-in registry");
    registry.into_iter().find(|record| {
        record.protocol == profile.protocol
            && record.host == host
            && record.models.contains(&profile.model)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn registry_records_have_sources_dates_and_portable_caps() {
        let records: Vec<Capabilities> =
            serde_json::from_str(include_str!("../../../fixtures/capabilities/models.json"))
                .unwrap();
        assert!(!records.is_empty());
        for record in records {
            assert!(
                record.total_context > 0 && record.max_choices >= 2 && record.max_score_levels >= 2
            );
            assert!(!record.sources.is_empty());
            assert_eq!(record.verified_on, "2026-10-30");
            assert_eq!(record.modalities, vec![Modality::Text, Modality::Image]);
        }
    }
}
