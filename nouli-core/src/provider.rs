//! One-attempt protocol adapters. Evaluation owns retry orchestration.

mod confidence;
pub mod openai;
pub mod typesafe;

use crate::{decision::ContractError, limits::LimitError, transport::TransportError};
use thiserror::Error;

#[derive(Debug, Error, PartialEq)]
pub enum ProviderError {
    #[error("provider protocol does not match the selected adapter")]
    Protocol,
    #[error("provider response does not match its documented schema")]
    Wire,
    #[error(transparent)]
    Codec(#[from] crate::codec::CodecError),
    #[error(transparent)]
    Contract(#[from] ContractError),
    #[error(transparent)]
    Limit(#[from] LimitError),
    #[error(transparent)]
    Transport(#[from] TransportError),
}
