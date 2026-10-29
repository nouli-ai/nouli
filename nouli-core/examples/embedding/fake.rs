//! Deterministic embedding dependencies, not a second engine or network service.
use nouli_core::{
    codec::{self, InputFormat},
    config::{Config, resolve::ResolvedSettings},
    decision::TurnRequest,
    engine::execution::DecisionProvider,
    limits::count::TokenCounter,
    outcome::TurnOutcome,
    provider::ProviderError,
};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use tokio_util::sync::CancellationToken;
#[derive(Default)]
pub struct FakeProvider {
    pub calls: AtomicUsize,
    pub fail_high_once: AtomicBool,
}
impl DecisionProvider for FakeProvider {
    async fn attempt<C: TokenCounter + Sync>(
        &self,
        settings: &ResolvedSettings,
        request: &TurnRequest,
        _evidence: &str,
        _counter: &C,
        _cancel: &CancellationToken,
    ) -> Result<TurnOutcome, ProviderError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if request.state.iter().any(
            |input| matches!(input, nouli_core::data::Input::Text { text } if text == "NODE:high"),
        ) && self.fail_high_once.swap(false, Ordering::SeqCst)
        {
            return Err(ProviderError::Transport(
                nouli_core::transport::TransportError::Contract,
            ));
        }
        let mut outcome: TurnOutcome = codec::decode(
            include_bytes!("../../../fixtures/contracts/decision-outcome.json"),
            InputFormat::Json,
        )?;
        outcome.provider.provider = settings.snapshot.profile_name.clone();
        outcome.provider.model = "fake-served-version-1".into();
        if let nouli_core::outcome::QuestionOutcome::Answered {
            confidence: Some(confidence),
            ..
        } = &mut outcome.outcomes[0]
        {
            confidence.meaning = "fixture native certainty (fake-served-version-1)".into();
        }
        outcome.validate(request)?;
        Ok(outcome)
    }
}
pub fn config() -> Result<Config, nouli_core::config::ConfigError> {
    Config::parse(
        "default_provider='fake'\ndefault_model='requested-alias'\n[execution]\nmax_attempts=1\n[providers.fake]\nprotocol='typesafe'\nbase_url='http://127.0.0.1:1'\n[providers.fake.models.'requested-alias']\ncontext_budget_tokens=100000\nreserved_output_tokens=128\n",
    )
}
