//! Explicitly authorized, single-attempt observations; never runs in default tests.
use nouli_core::{
    codec::{self, InputFormat},
    config::{Config, resolve::RuntimeOverrides},
    decision::TurnRequest,
    engine::execution::{DecisionProvider, NativeProvider},
    history::{ModelSelection, TokenCount},
    limits::{ResolvedLimits, count::ByteCounter},
    outcome::TurnOutcome,
};
use serde::Serialize;
use std::{io::Write, path::PathBuf};
use thiserror::Error;
use tokio_util::sync::CancellationToken;
const USAGE: &str = "Usage: cargo run --package nouli-core --example compare_context --locked -- --allow-network CONFIG PROFILE REQUEST.json OUTPUT.json\nMakes exactly two single-attempt calls to the explicit profile, one per context format. No model installation or ledger writes.\n";
#[derive(Debug, Error)]
enum EvaluationError {
    #[error("explicit --allow-network CONFIG PROFILE REQUEST.json OUTPUT.json required")]
    Arguments,
    #[error(transparent)]
    Config(#[from] nouli_core::config::ConfigError),
    #[error(transparent)]
    Codec(#[from] codec::CodecError),
    #[error(transparent)]
    Limit(#[from] nouli_core::limits::LimitError),
    #[error("evaluation input/output failed")]
    Io(#[from] std::io::Error),
    #[error("observation serialization failed")]
    Json(#[from] serde_json::Error),
}
#[derive(Serialize)]
struct Observation {
    format: &'static str,
    context_bytes: usize,
    estimated_input: TokenCount,
    outcome: Result<TurnOutcome, String>,
}
#[derive(Serialize)]
struct Comparison {
    version: u32,
    nouli_version: &'static str,
    profile: String,
    requested_model: String,
    request: TurnRequest,
    observations: Vec<Observation>,
    interpretation: &'static str,
}
fn contexts(request: &TurnRequest) -> Result<[(&'static str, String); 2], EvaluationError> {
    // The state values are identical. Live questions stay in the native question
    // envelope for both calls; neither format gains extra evidence or instructions.
    Ok([
        ("json_text", serde_json::to_string(&request.state)?),
        ("toon", codec::encode_toon(&request.state)?),
    ])
}
#[tokio::main]
async fn main() -> Result<(), EvaluationError> {
    let arguments = std::env::args_os().skip(1).collect::<Vec<_>>();
    if arguments.len() == 1 && arguments[0] == "--help" {
        std::io::stdout().lock().write_all(USAGE.as_bytes())?;
        return Ok(());
    }
    if arguments.len() != 5 || arguments[0] != "--allow-network" {
        return Err(EvaluationError::Arguments);
    }
    let config = Config::load(&PathBuf::from(&arguments[1]), false)?;
    let profile = arguments[2]
        .to_str()
        .ok_or(EvaluationError::Arguments)?
        .to_owned();
    let overrides = RuntimeOverrides {
        selection: ModelSelection {
            provider: Some(profile.clone()),
            model: None,
        },
        ..Default::default()
    };
    let settings = config.resolve(
        &overrides,
        &ModelSelection::default(),
        &ModelSelection::default(),
        &std::env::current_dir()?,
        |source| match source {
            nouli_core::config::auth::CredentialSource::Env(name) => std::env::var(name).ok(),
            nouli_core::config::auth::CredentialSource::Auth(_) => None,
        },
    )?;
    let request: TurnRequest = codec::decode(
        &std::fs::read(PathBuf::from(&arguments[3]))?,
        InputFormat::Json,
    )?;
    let limits = ResolvedLimits::for_profile(&settings.snapshot.profile)?;
    let contexts = contexts(&request)?;
    // Preflight both variants and reserve the new output before any dispatch.
    let counts = contexts
        .iter()
        .map(|(_, text)| {
            limits
                .assess(text, &request, &ByteCounter)
                .map(|assessment| assessment.total)
        })
        .collect::<Result<Vec<_>, _>>()?;
    let mut output = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(PathBuf::from(&arguments[4]))?;
    let cancel = CancellationToken::new();
    let mut observations = vec![];
    for ((format, text), estimated_input) in contexts.into_iter().zip(counts) {
        let outcome = NativeProvider
            .attempt(&settings, &request, &text, &ByteCounter, &cancel)
            .await
            .map_err(|error| error.to_string());
        observations.push(Observation {
            format,
            context_bytes: text.len(),
            estimated_input,
            outcome,
        });
    }
    let comparison = Comparison {
        version: 1,
        nouli_version: nouli_core::version(),
        profile,
        requested_model: settings.snapshot.profile.model.clone(),
        request,
        observations,
        interpretation: "Observed calls, not an accuracy benchmark. Native usage and confidence meanings are separate from estimates. Format efficiency and decision quality depend on model and data shape; no universal savings claim.",
    };
    serde_json::to_writer_pretty(&mut output, &comparison)?;
    output.write_all(b"\n")?;
    output.sync_all()?;
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn compared_formats_contain_identical_facts_without_live_calls() {
        let request: TurnRequest = codec::decode(
            include_bytes!("../../fixtures/journeys/food.json"),
            InputFormat::Json,
        )
        .unwrap();
        let [(_, json), (_, toon)] = contexts(&request).unwrap();
        assert_eq!(
            codec::decode_value(json.as_bytes(), InputFormat::Json).unwrap(),
            codec::decode_value(toon.as_bytes(), InputFormat::Toon).unwrap()
        );
    }
}
