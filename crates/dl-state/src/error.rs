//! Error types for `dl-state`.
//!
//! Decoders are byte-level: they translate raw account data into the
//! normalized [`Pool`] struct. Errors below describe *why* a decode failed.
//! They are not used to report runtime state-mutation problems (those would
//! be a different enum, e.g. `RegistryError`).

use thiserror::Error;

use dl_core::MathError;

#[derive(Debug, Error)]
pub enum DecodeError {
    /// The buffer is shorter than the layout the decoder requires.
    #[error("buffer too short: need {need} bytes, got {got}")]
    TooShort { need: usize, got: usize },

    /// The first few bytes don't match the expected program/discriminator
    /// for the claimed AMM kind. Caller probably mis-routed an account.
    #[error("invalid discriminator: expected {expected:02x?}, got {got:02x?}")]
    BadDiscriminator { expected: Vec<u8>, got: Vec<u8> },

    /// An `AmmKind` tag we don't know how to decode was supplied.
    #[error("unknown amm kind tag: 0x{0:02x}")]
    UnknownKind(u8),

    /// A checked-math operation failed during decoding. Should be rare
    /// — reserves are `u64` and don't usually overflow — but possible
    /// when normalizing decimals.
    #[error("math: {0}")]
    Math(#[from] MathError),
}
