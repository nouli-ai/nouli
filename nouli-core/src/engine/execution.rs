//! Injectable single-attempt dispatch and retry time, with no persistence.
use crate::{
    config::{Protocol, resolve::ResolvedSettings},
    decision::TurnRequest,
    limits::count::TokenCounter,
    outcome::TurnOutcome,
    provider::{self, ProviderError},
    transport::HttpTransport,
};
use std::{
    future::Future,
    time::{Duration, SystemTime},
};
use tokio_util::sync::CancellationToken;

/// Implementations must be cancellation-safe when their future is dropped and
/// make exactly one selected-provider attempt, without their own retry loop.
pub trait DecisionProvider: Sync {
    fn attempt<C: TokenCounter + Sync>(
        &self,
        settings: &ResolvedSettings,
        request: &TurnRequest,
        evidence: &str,
        counter: &C,
        cancel: &CancellationToken,
    ) -> impl Future<Output = Result<TurnOutcome, ProviderError>> + Send;
}
#[derive(Debug, Default, Clone, Copy)]
pub struct NativeProvider;
impl DecisionProvider for NativeProvider {
    async fn attempt<C: TokenCounter + Sync>(
        &self,
        settings: &ResolvedSettings,
        request: &TurnRequest,
        evidence: &str,
        counter: &C,
        cancel: &CancellationToken,
    ) -> Result<TurnOutcome, ProviderError> {
        let transport = HttpTransport::new(&settings.snapshot.execution)?;
        match settings.snapshot.profile.protocol {
            Protocol::SystemOne => {
                provider::system_one::evaluate(
                    &transport, settings, request, evidence, counter, cancel,
                )
                .await
            }
            Protocol::OpenaiDecisions => {
                provider::openai::evaluate(&transport, settings, request, evidence, counter, cancel)
                    .await
            }
        }
    }
}

pub trait RetryClock: Sync {
    fn now(&self) -> SystemTime;
    /// Return false if cancellation prevents the next attempt.
    fn wait(
        &self,
        delay: Duration,
        cancel: &CancellationToken,
    ) -> impl Future<Output = bool> + Send;
}
#[derive(Debug, Default, Clone, Copy)]
pub struct TokioClock;
impl RetryClock for TokioClock {
    fn now(&self) -> SystemTime {
        SystemTime::now()
    }
    async fn wait(&self, delay: Duration, cancel: &CancellationToken) -> bool {
        tokio::select! {biased; ()=cancel.cancelled()=>false,()=tokio::time::sleep(delay)=>true}
    }
}
