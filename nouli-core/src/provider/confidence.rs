//! Native confidence fields stay distinct and carry provider/model provenance.

use super::ProviderError;
use crate::outcome::{Confidence, ProviderIdentity};

pub(super) fn native_confidence(
    value: Option<f64>,
    answer_confidence: Option<f64>,
    identity: &ProviderIdentity,
) -> Result<Option<Confidence>, ProviderError> {
    for n in [value, answer_confidence].into_iter().flatten() {
        if !n.is_finite() || !(0.0..=1.0).contains(&n) {
            return Err(ProviderError::Wire);
        }
    }
    if value.is_none() && answer_confidence.is_none() {
        return Ok(None);
    }
    Ok(Some(Confidence {
        value,
        meaning: format!(
            "native confidence fields from profile {} model {}",
            identity.provider, identity.model
        ),
        answer_confidence,
    }))
}
