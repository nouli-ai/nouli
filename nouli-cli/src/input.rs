//! Strict decision boundaries, independent of provider/configuration discovery.
use crate::selection::SelectionError;
use clap::ValueEnum;
use nouli_core::{
    codec::{self, InputFormat},
    context::facts::FactError,
    decision::grammar::GrammarError,
};
use std::io::Read;
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
#[derive(Debug, Error)]
pub(crate) enum InputError {
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
    #[error("--input cannot be combined with facts, -q, --drop, or --replace")]
    InputConflict,
    #[error("the request file must not name a session; the command supplies it")]
    SessionInRequest,
    #[error("question {position}: {source}")]
    Question {
        position: usize,
        source: GrammarError,
    },
    #[error(transparent)]
    Fact(#[from] FactError),
    #[error(
        "--request-id and --retry-interrupted need a turn: give `new` a fact or -q question, or leave them out to only create the session"
    )]
    CreateOnly,
    #[error("--replace takes a fact number from `nouli facts` and the new text")]
    ReplaceNumber,
    #[error("fact {0} does not exist; see the numbers in `nouli facts`")]
    UnknownFact(usize),
    #[error("fact {0} is replaced more than once")]
    DuplicateReplace(usize),
    #[error("no question to drop is named")]
    DropEmpty,
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
