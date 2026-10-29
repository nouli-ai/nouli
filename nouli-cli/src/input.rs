//! Strict decision boundaries, independent of provider/configuration discovery.
use crate::selection::{SelectionArgs, SelectionError};
use clap::{Args, ValueEnum};
use nouli_core::{
    codec::{self, InputFormat},
    data::Input,
    decision::{CONTRACT_VERSION, Question, TurnRequest},
    history::{EvaluationRequest, ModelSelection, SessionSelection},
};
use std::{io::Read, path::PathBuf};
use thiserror::Error;
const MAX_INPUT_BYTES: u64 = 16 * 1024 * 1024;
#[derive(Debug, Clone, Copy, ValueEnum)]
pub(crate) enum Format {
    Json,
    Toon,
}
impl From<Format> for InputFormat {
    fn from(value: Format) -> Self {
        match value {
            Format::Json => Self::Json,
            Format::Toon => Self::Toon,
        }
    }
}
#[derive(Debug, Default, Args)]
pub(crate) struct DecisionArgs {
    /// Text evidence (repeatable); structured state belongs in request files.
    #[arg(short = 's', long, value_name = "TEXT", conflicts_with = "input")]
    pub state: Vec<String>,
    /// JSON question list, or TOON with --input-format toon.
    #[arg(short = 'q', long, value_name = "QUESTIONS", conflicts_with = "input")]
    pub questions: Option<String>,
    /// Complete JSON/TOON decision request, or - for stdin.
    #[arg(long, value_name = "FILE")]
    pub input: Option<PathBuf>,
    /// Required for stdin or files without .json/.toon; controls inline questions too.
    #[arg(long, value_enum)]
    pub input_format: Option<Format>,
    #[command(flatten)]
    pub selection: SelectionArgs,
    /// Override the selected named provider.
    #[arg(long)]
    pub provider: Option<String>,
    /// Override the requested decision model.
    #[arg(long)]
    pub model: Option<String>,
    /// Smaller model context budget, or an explicit budget for an unknown model.
    #[arg(long,value_name="TOKENS",value_parser=clap::value_parser!(u64).range(1..))]
    pub context_budget: Option<u64>,
    /// Store-wide identity for normalized intent and completed replay.
    #[arg(long)]
    pub request_id: Option<String>,
    /// Explicitly retry an interrupted request using its saved settings/sources.
    #[arg(long)]
    pub retry_interrupted: bool,
}
#[derive(Debug, Error)]
pub(crate) enum InputError {
    #[error("supply --questions or --input; see --help")]
    Missing,
    #[error("input format is ambiguous; specify --input-format json or toon")]
    Format,
    #[error("cannot read decision input")]
    Read,
    #[error("decision input exceeds 16 MiB")]
    Size,
    #[error("input does not match the Nouli request contract")]
    Shape,
    #[error(transparent)]
    Codec(#[from] codec::CodecError),
    #[error(transparent)]
    Selection(#[from] SelectionError),
    #[error("--retry-interrupted requires a request ID and persisted evaluation")]
    Retry,
    #[error(transparent)]
    History(#[from] nouli_core::history::HistoryError),
}
fn bounded(reader: &mut impl Read) -> Result<Vec<u8>, InputError> {
    let mut bytes = Vec::new();
    reader
        .take(MAX_INPUT_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| InputError::Read)?;
    if bytes.len() as u64 > MAX_INPUT_BYTES {
        return Err(InputError::Size);
    }
    Ok(bytes)
}
pub(crate) fn file(
    path: &std::path::Path,
    format: Option<Format>,
    stdin: &mut impl Read,
) -> Result<(Vec<u8>, InputFormat), InputError> {
    let format = format
        .map(InputFormat::from)
        .or_else(|| InputFormat::for_path(path).ok())
        .ok_or(InputError::Format)?;
    let bytes = if path.as_os_str() == "-" {
        bounded(stdin)?
    } else {
        bounded(&mut std::fs::File::open(path).map_err(|_| InputError::Read)?)?
    };
    Ok((bytes, format))
}
fn new_session() -> SessionSelection {
    SessionSelection::New { name: None }
}
impl DecisionArgs {
    pub(crate) fn has_options(&self) -> bool {
        self.has_input()
            || self.input_format.is_some()
            || self.selection.session.is_some()
            || !self.selection.from.is_empty()
            || self.selection.name.is_some()
            || self.selection.no_session
            || self.provider.is_some()
            || self.model.is_some()
            || self.context_budget.is_some()
            || self.request_id.is_some()
            || self.retry_interrupted
    }
    pub(crate) fn has_input(&self) -> bool {
        self.input.is_some() || self.questions.is_some() || !self.state.is_empty()
    }
    pub(crate) fn request(&self, stdin: &mut impl Read) -> Result<EvaluationRequest, InputError> {
        let (mut request, explicit) = if let Some(path) = &self.input {
            let (bytes, format) = file(path, self.input_format, stdin)?;
            let mut value = codec::decode_value(&bytes, format)?;
            if value.get("turn").is_some() {
                let object = value.as_object_mut().ok_or(InputError::Shape)?;
                let explicit = object.contains_key("session");
                object
                    .entry("session")
                    .or_insert(serde_json::json!({"mode":"new","name":null}));
                let turn = object
                    .get_mut("turn")
                    .and_then(serde_json::Value::as_object_mut)
                    .ok_or(InputError::Shape)?;
                turn.entry("version")
                    .or_insert(serde_json::json!(CONTRACT_VERSION));
                (
                    serde_json::from_value::<EvaluationRequest>(value)
                        .map_err(|_| InputError::Shape)?,
                    explicit,
                )
            } else {
                let codec::Request::Turn(turn) = codec::decode_request(&bytes, format)? else {
                    return Err(InputError::Shape);
                };
                (
                    EvaluationRequest {
                        turn,
                        session: new_session(),
                        updates: vec![],
                        required_records: vec![],
                        model: ModelSelection::default(),
                        request_id: None,
                    },
                    false,
                )
            }
        } else {
            let questions = self.questions.as_ref().ok_or(InputError::Missing)?;
            let format = self
                .input_format
                .map(InputFormat::from)
                .unwrap_or(InputFormat::Json);
            let mut value = codec::decode_value(questions.as_bytes(), format)?;
            if let Some(object) = value.as_object_mut() {
                if object.len() != 1 {
                    return Err(InputError::Shape);
                }
                value = object.remove("questions").ok_or(InputError::Shape)?;
            }
            let questions: Vec<Question> =
                serde_json::from_value(value).map_err(|_| InputError::Shape)?;
            (
                EvaluationRequest {
                    turn: TurnRequest {
                        version: CONTRACT_VERSION,
                        state: self
                            .state
                            .iter()
                            .cloned()
                            .map(|text| Input::Text { text })
                            .collect(),
                        questions,
                    },
                    session: new_session(),
                    updates: vec![],
                    required_records: vec![],
                    model: ModelSelection::default(),
                    request_id: None,
                },
                false,
            )
        };
        if let Some(key) = &self.request_id {
            request.request_id = Some(key.clone());
        }
        request.session =
            self.selection
                .apply(request.session, explicit, request.request_id.is_some())?;
        if self.retry_interrupted && (self.selection.no_session || request.request_id.is_none()) {
            return Err(InputError::Retry);
        }
        request.validate()?;
        Ok(request)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;
    fn questions() -> String {
        let mut request: TurnRequest = serde_json::from_str(include_str!(
            "../../fixtures/contracts/decision-request.json"
        ))
        .unwrap();
        request.questions.truncate(3);
        serde_json::to_string(&request.questions).unwrap()
    }
    fn args(arguments: Vec<String>) -> DecisionArgs {
        super::super::Args::try_parse_from(arguments)
            .unwrap()
            .decision
    }
    #[test]
    fn inline_modes_validate_source_grammar_and_conflicting_options_before_execution() {
        let head = nouli_core::identity::Id::generate();
        let parsed = args(vec![
            "nouli".into(),
            "-s".into(),
            "caller evidence".into(),
            "-q".into(),
            questions(),
            "--from".into(),
            format!("Source@{head}"),
            "--from".into(),
            "Other".into(),
            "--name".into(),
            "Blend".into(),
        ]);
        let request = parsed.request(&mut std::io::empty()).unwrap();
        let SessionSelection::From { sources, name } = &request.session else {
            panic!("blend")
        };
        assert_eq!(sources.len(), 2);
        assert_eq!(sources[0].frontier, Some(vec![head]));
        assert_eq!(name.as_ref().unwrap().as_str(), "Blend");
        assert!(matches!(&request.turn.state[0],Input::Text {text} if text=="caller evidence"));
        for invalid in [
            vec!["nouli", "--session", "Existing", "--from", "Other"],
            vec!["nouli", "--session", "Existing", "--no-session"],
            vec!["nouli", "--no-session", "--name", "New"],
            vec!["nouli", "--from", "Source@bad"],
            vec!["nouli", "--session", "bad/name"],
        ] {
            assert!(super::super::Args::try_parse_from(invalid).is_err());
        }
    }
    #[test]
    fn full_requests_preserve_structured_state_updates_and_explicit_file_selection() {
        let mut turn: TurnRequest = serde_json::from_str(include_str!(
            "../../fixtures/contracts/decision-request.json"
        ))
        .unwrap();
        turn.questions.truncate(3);
        let request = EvaluationRequest {
            turn,
            session: SessionSelection::New { name: None },
            updates: vec![nouli_core::context::ContextUpdate::Append {
                record: nouli_core::context::ContextRecord {
                    id: nouli_core::identity::Id::generate(),
                    key: "constraint".into(),
                    value: nouli_core::data::Data::new(serde_json::json!(18446744073709551615u64))
                        .unwrap(),
                    kind: nouli_core::context::ContextKind::Commitment,
                },
            }],
            required_records: vec![],
            model: ModelSelection::default(),
            request_id: Some("file-key".into()),
        };
        let bytes = serde_json::to_vec_pretty(&request).unwrap();
        let parsed = args(vec![
            "nouli".into(),
            "--input".into(),
            "-".into(),
            "--input-format".into(),
            "json".into(),
        ]);
        assert_eq!(parsed.request(&mut bytes.as_slice()).unwrap(), request);
        let competing = args(vec![
            "nouli".into(),
            "--input".into(),
            "-".into(),
            "--input-format".into(),
            "json".into(),
            "--from".into(),
            "Other".into(),
        ]);
        assert!(matches!(
            competing.request(&mut bytes.as_slice()),
            Err(InputError::Selection(SelectionError::Competing))
        ));
        let ephemeral = args(vec![
            "nouli".into(),
            "--input".into(),
            "-".into(),
            "--input-format".into(),
            "json".into(),
            "--no-session".into(),
        ]);
        assert!(matches!(
            ephemeral.request(&mut bytes.as_slice()),
            Err(InputError::Selection(SelectionError::Ephemeral))
        ));
    }
    #[test]
    fn toon_questions_and_explicit_stdin_formats_remain_strict_and_lossless() {
        let values: serde_json::Value = serde_json::from_str(&questions()).unwrap();
        let toon = codec::encode_toon(&serde_json::json!({"questions":values})).unwrap();
        let json = args(vec!["nouli".into(), "-q".into(), questions()]);
        let toon = args(vec![
            "nouli".into(),
            "-q".into(),
            toon,
            "--input-format".into(),
            "toon".into(),
        ]);
        assert_eq!(
            json.request(&mut std::io::empty()).unwrap(),
            toon.request(&mut std::io::empty()).unwrap()
        );
        let stdin = args(vec!["nouli".into(), "--input".into(), "-".into()]);
        assert!(matches!(
            stdin.request(&mut &b"{}"[..]),
            Err(InputError::Format)
        ));
        let retry = args(vec![
            "nouli".into(),
            "-q".into(),
            questions(),
            "--retry-interrupted".into(),
        ]);
        assert!(matches!(
            retry.request(&mut std::io::empty()),
            Err(InputError::Retry)
        ));
        let duplicate = args(vec![
            "nouli".into(),
            "--input".into(),
            "-".into(),
            "--input-format".into(),
            "json".into(),
        ]);
        assert!(matches!(
            duplicate.request(&mut &b"{\"questions\":[],\"questions\":[]}"[..]),
            Err(InputError::Codec(codec::CodecError::DuplicateKey))
        ));
    }
}
