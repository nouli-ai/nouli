//! Read-only source evaluation without durable identity or audit writes.
use super::{
    Engine, EngineError, EvaluationInput,
    execution::{DecisionProvider, RetryClock},
    run::{Executed, Recorder, RunInput},
};
use crate::{
    decision::TurnRequest,
    history::{EvaluationManifest, ModelSelection, SessionSelection},
    identity::Id,
    limits::count::TokenCounter,
    outcome::TurnOutcome,
    storage::{OpenMode, worker::LedgerWorker},
};
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EphemeralResult {
    pub request: TurnRequest,
    pub outcome: TurnOutcome,
    pub manifest: EvaluationManifest,
}
impl<P: DecisionProvider, C: TokenCounter + Sync, T: RetryClock> Engine<P, C, T> {
    /// Evaluate one turn without writing to the ledger.
    ///
    /// # Errors
    ///
    /// - [`EngineError::Canceled`] if `cancel` fires before or during evaluation.
    /// - [`EngineError::History`] if the request fails validation.
    /// - [`EngineError::EphemeralOptions`] if the request sets a request ID, interrupted
    ///   retry, session name, or continuation.
    /// - [`EngineError::EphemeralStore`] if sources are selected without a read-only
    ///   ledger worker; [`EngineError::Worker`], [`EngineError::Source`], or
    ///   [`EngineError::History`] if those sources cannot be resolved.
    /// - [`EngineError::Config`] if settings cannot be resolved.
    /// - [`EngineError::Limit`], [`EngineError::Build`], or [`EngineError::Config`] if the
    ///   resolved profile, context plan, credentials, or attempt timeout are invalid.
    /// - [`EngineError::Provider`] if every provider attempt fails or a capacity failure
    ///   cannot be retried with a smaller context.
    pub async fn evaluate_ephemeral(
        &self,
        ledger: Option<&LedgerWorker>,
        input: EvaluationInput<'_>,
        lookup: impl Fn(&crate::config::auth::CredentialSource) -> Option<String>,
        cancel: &CancellationToken,
    ) -> Result<EphemeralResult, EngineError> {
        if cancel.is_cancelled() {
            return Err(EngineError::Canceled);
        }
        let request = input.request;
        request.validate()?;
        if request.request_id.is_some() || input.retry_interrupted {
            return Err(EngineError::EphemeralOptions);
        }
        let selections = match &request.session {
            SessionSelection::New { name: None } => None,
            SessionSelection::From {
                sources,
                name: None,
            } => Some(sources.clone()),
            _ => return Err(EngineError::EphemeralOptions),
        };
        let (sources, history) = if let Some(selections) = selections {
            ledger
                .ok_or(EngineError::EphemeralStore)?
                .call(move |ledger| {
                    if ledger.mode() != OpenMode::ReadOnlyExisting {
                        return Err(EngineError::EphemeralStore);
                    }
                    let sources = ledger.resolve_sources(&selections)?;
                    let history = ledger.ancestry(&sources)?;
                    Ok::<_, EngineError>((sources, history))
                })
                .await??
        } else {
            (vec![], vec![])
        };
        let settings = input.config.resolve_snapshot(
            input.overrides,
            &request.model,
            &ModelSelection::default(),
            input.home,
        )?;
        match self
            .execute(
                RunInput {
                    context_base: None,
                    runtime_only: true,
                    request,
                    sources: &sources,
                    history: &history,
                    settings,
                    session: Id::generate(),
                },
                Recorder::ephemeral(),
                lookup,
                cancel,
            )
            .await?
        {
            Executed::Ephemeral { outcome, manifest } => Ok(EphemeralResult {
                request: request.turn.clone(),
                outcome: *outcome,
                manifest: *manifest,
            }),
            Executed::Saved(_) => Err(crate::storage::StorageError::Payload.into()),
        }
    }
}

#[cfg(test)]
#[expect(
    clippy::unused_async_trait_impl,
    reason = "test fakes implement async traits without needing to await"
)]
mod tests {
    use super::*;
    use crate::config::ConnectionSettings;
    use crate::history::StandingChanges;
    use crate::{
        config::{
            Config,
            resolve::{ResolvedSettings, RuntimeOverrides},
        },
        context::{ContextKind, ContextRecord, ContextUpdate, PinTarget},
        data::{Data, Input},
        engine::execution::{DecisionProvider, TokioClock},
        history::{EvaluationRequest, Representation, SourceSelection},
        identity::{SessionName, SessionSelector},
        limits::count::ByteCounter,
        provider::ProviderError,
        transport::TransportError,
    };
    use std::{
        collections::BTreeMap,
        path::{Path, PathBuf},
        sync::Mutex,
    };
    struct Store(PathBuf);
    impl Store {
        fn new() -> Self {
            Self(std::env::temp_dir().join(format!("nouli-ephemeral-{}", Id::generate())))
        }
    }
    impl Drop for Store {
        fn drop(&mut self) {
            if self.0.exists() {
                std::fs::remove_dir_all(&self.0).unwrap();
            }
        }
    }
    #[derive(Default)]
    struct Fake {
        calls: Mutex<Vec<(String, TurnRequest)>>,
        fail: bool,
    }
    impl DecisionProvider for Fake {
        async fn attempt<C: TokenCounter + Sync>(
            &self,
            _settings: &ResolvedSettings,
            request: &TurnRequest,
            evidence: &str,
            _counter: &C,
            _cancel: &CancellationToken,
        ) -> Result<TurnOutcome, ProviderError> {
            self.calls
                .lock()
                .unwrap()
                .push((evidence.into(), request.clone()));
            if self.fail {
                return Err(ProviderError::Transport(TransportError::Contract));
            }
            Ok(serde_json::from_str(include_str!(
                "../../../fixtures/contracts/decision-outcome.json"
            ))
            .unwrap())
        }
    }
    fn engine(fail: bool) -> Engine<Fake, ByteCounter, TokioClock> {
        Engine {
            provider: Fake {
                fail,
                ..Fake::default()
            },
            counter: ByteCounter,
            clock: TokioClock,
        }
    }
    fn config() -> Config {
        let mut config =
            Config::parse(include_str!("../../../fixtures/config/providers.toml")).unwrap();
        let mut profile = config.providers["local"].clone();
        profile.models.insert(
            "offline".into(),
            crate::config::ModelSettings {
                connection: ConnectionSettings::default(),
                per_question_context_tokens: None,
                max_questions: None,
                max_choices: None,
                max_score_levels: None,
                max_request_bytes: None,
                input_modalities: None,
                context_budget_tokens: Some(100_000),
                context_window_tokens: Some(100_000),
                reserved_output_tokens: Some(128),
            },
        );
        config.providers = BTreeMap::from([("fixture".into(), profile)]);
        config.default_provider = Some("fixture".into());
        config.default_model = Some("offline".into());
        config.execution.max_attempts = 1;
        config
    }
    fn request() -> EvaluationRequest {
        EvaluationRequest {
            standing: StandingChanges::default(),
            turn: serde_json::from_str(include_str!(
                "../../../fixtures/contracts/decision-request.json"
            ))
            .unwrap(),
            session: SessionSelection::New { name: None },
            updates: vec![],
            required_records: vec![],
            model: ModelSelection::default(),
            request_id: None,
        }
    }
    fn input<'a>(
        request: &'a EvaluationRequest,
        config: &'a Config,
        overrides: &'a RuntimeOverrides,
        home: &'a Path,
    ) -> EvaluationInput<'a> {
        EvaluationInput {
            request,
            config,
            overrides,
            home,
            retry_interrupted: false,
        }
    }
    fn snapshot(root: &Path) -> BTreeMap<PathBuf, Option<Vec<u8>>> {
        let mut result = BTreeMap::new();
        let mut pending = vec![root.to_path_buf()];
        while let Some(path) = pending.pop() {
            if path.is_dir() {
                result.insert(path.strip_prefix(root).unwrap().to_owned(), None);
                pending.extend(
                    std::fs::read_dir(path)
                        .unwrap()
                        .map(|entry| entry.unwrap().path()),
                );
            } else {
                result.insert(
                    path.strip_prefix(root).unwrap().to_owned(),
                    Some(std::fs::read(path).unwrap()),
                );
            }
        }
        result
    }
    #[tokio::test]
    async fn fresh_calls_need_no_home_store_or_persisted_identity_and_reject_durable_options() {
        let store = Store::new();
        let home = store.0.join("missing-home");
        let engine = engine(false);
        let config = config();
        let overrides = RuntimeOverrides::default();
        let mut request = request();
        let cancel = CancellationToken::new();
        let result = engine
            .evaluate_ephemeral(
                None,
                input(&request, &config, &overrides, &home),
                |_| panic!("local model needs no key"),
                &cancel,
            )
            .await
            .unwrap();
        assert!(!store.0.exists());
        assert!(matches!(
            result.outcome.outcomes[3],
            crate::outcome::QuestionOutcome::Refused { .. }
        ));
        let encoded = serde_json::to_value(result).unwrap();
        assert!(encoded.get("session").is_none());
        assert!(encoded.get("turn").is_none());
        assert!(encoded.get("request_id").is_none());
        request.request_id = Some("replay-is-durable".into());
        assert!(matches!(
            engine
                .evaluate_ephemeral(
                    None,
                    input(&request, &config, &overrides, &home),
                    |_| None,
                    &cancel
                )
                .await,
            Err(EngineError::EphemeralOptions)
        ));
        request.request_id = None;
        request.session = SessionSelection::New {
            name: Some(SessionName::try_from("durable".to_owned()).unwrap()),
        };
        assert!(matches!(
            engine
                .evaluate_ephemeral(
                    None,
                    input(&request, &config, &overrides, &home),
                    |_| None,
                    &cancel
                )
                .await,
            Err(EngineError::EphemeralOptions)
        ));
        request.session = SessionSelection::Continue {
            session: SessionSelector::Id(Id::generate()),
        };
        assert!(matches!(
            engine
                .evaluate_ephemeral(
                    None,
                    input(&request, &config, &overrides, &home),
                    |_| None,
                    &cancel
                )
                .await,
            Err(EngineError::EphemeralOptions)
        ));
        request.session = SessionSelection::From {
            sources: vec![SourceSelection {
                session: SessionSelector::Id(Id::generate()),
                frontier: None,
            }],
            name: None,
        };
        assert!(matches!(
            engine
                .evaluate_ephemeral(
                    None,
                    input(&request, &config, &overrides, &home),
                    |_| None,
                    &cancel
                )
                .await,
            Err(EngineError::EphemeralStore)
        ));
        assert_eq!(engine.provider.calls.lock().unwrap().len(), 1);
        assert!(!store.0.exists());
    }
    #[tokio::test]
    async fn readonly_sources_survive_success_refusal_failure_reduction_and_temporary_updates() {
        let store = Store::new();
        let writer = LedgerWorker::open(store.0.clone(), OpenMode::ReadWriteCreate)
            .await
            .unwrap();
        let overrides = RuntimeOverrides::default();
        let config = config();
        let cancel = CancellationToken::new();
        let mut seed = request();
        let original = ContextRecord {
            id: Id::generate(),
            key: "budget".into(),
            value: Data::new(serde_json::json!(8)).unwrap(),
            kind: ContextKind::Commitment,
        };
        seed.updates = vec![ContextUpdate::Pin {
            target: PinTarget::New {
                record: original.clone(),
            },
        }];
        seed.turn.state.push(Input::Text {
            text: "historical state ".repeat(10_000),
        });
        let saved = engine(false)
            .evaluate(
                &writer,
                input(&seed, &config, &overrides, &store.0),
                |_| None,
                &cancel,
            )
            .await
            .unwrap();
        let readonly = LedgerWorker::open(store.0.clone(), OpenMode::ReadOnlyExisting)
            .await
            .unwrap();
        let before = snapshot(&store.0);
        let mut request = request();
        request.session = SessionSelection::From {
            sources: vec![SourceSelection {
                session: SessionSelector::Id(saved.turn.session),
                frontier: None,
            }],
            name: None,
        };
        request.updates = vec![ContextUpdate::Supersede {
            targets: vec![original.id],
            replacement: ContextRecord {
                id: Id::generate(),
                value: Data::new(serde_json::json!(12)).unwrap(),
                ..original.clone()
            },
            pinned: None,
        }];
        let mut small = config.clone();
        small
            .providers
            .get_mut("fixture")
            .unwrap()
            .models
            .get_mut("offline")
            .unwrap()
            .context_budget_tokens = Some(2000);
        let ephemeral = engine(false);
        let result = ephemeral
            .evaluate_ephemeral(
                Some(&readonly),
                input(&request, &small, &overrides, &store.0),
                |_| None,
                &cancel,
            )
            .await
            .unwrap();
        assert!(
            result
                .manifest
                .records
                .iter()
                .any(|record| record.representation == Representation::Compacted)
        );
        assert_eq!(result.manifest.sources[0].frontier, vec![saved.turn.id]);
        assert!(matches!(
            result.outcome.outcomes[3],
            crate::outcome::QuestionOutcome::Refused { .. }
        ));
        assert!(
            engine(true)
                .evaluate_ephemeral(
                    Some(&readonly),
                    input(&request, &small, &overrides, &store.0),
                    |_| None,
                    &cancel
                )
                .await
                .is_err()
        );
        assert!(
            snapshot(&store.0) == before,
            "ephemeral evaluations must leave all store files unchanged"
        );
        let session = saved.turn.session;
        let active = readonly
            .call(move |ledger| {
                ledger.context_at(
                    &ledger.context_sources(session).unwrap(),
                    &[],
                    session,
                    Id::generate(),
                )
            })
            .await
            .unwrap()
            .unwrap();
        assert_eq!(active[0].record, original);
        let mut missing = request.clone();
        missing.session = SessionSelection::From {
            sources: vec![SourceSelection {
                session: SessionSelector::Id(Id::generate()),
                frontier: None,
            }],
            name: None,
        };
        assert!(
            engine(false)
                .evaluate_ephemeral(
                    Some(&readonly),
                    input(&missing, &small, &overrides, &store.0),
                    |_| None,
                    &cancel
                )
                .await
                .is_err()
        );
        assert!(matches!(
            engine(false)
                .evaluate_ephemeral(
                    Some(&writer),
                    input(&request, &small, &overrides, &store.0),
                    |_| None,
                    &cancel
                )
                .await,
            Err(EngineError::EphemeralStore)
        ));
        assert!(
            snapshot(&store.0) == before,
            "invalid source selections must not write"
        );
        let durable = engine(false);
        let saved = durable
            .evaluate(
                &writer,
                input(&request, &small, &overrides, &store.0),
                |_| None,
                &cancel,
            )
            .await
            .unwrap();
        let ephemeral_call = ephemeral.provider.calls.lock().unwrap()[0].clone();
        let durable_call = durable.provider.calls.lock().unwrap()[0].clone();
        assert_eq!(ephemeral_call, durable_call);
        assert_eq!(
            result.manifest.context_toon,
            saved.turn.manifest.context_toon
        );
    }
}
