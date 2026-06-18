//! Error types for `dl-sim`.
//!
//! The sim layer's failure modes are:
//! 1. A checked-math primitive failed (`Math`) — overflow, div-by-zero, etc.
//! 2. A cycle leg references a pool that isn't in the registry (`PoolNotFound`).
//! 3. A pool's reserves are degenerate (zero on either side) (`ZeroReserve`).
//! 4. A fee_bps is `>= 10_000` (would underflow `10_000 - fee_bps`) (`FeeTooHigh`).
//! 5. A cycle has more legs than the sim can handle (`CycleTooLong`).
//!
//! Every variant carries enough context to log a useful warning without
//! needing to re-derive the input.

use thiserror::Error;

use dl_core::MathError;
use dl_state::Pubkey;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum SimError {
    /// A checked-math primitive in the sim failed. Most commonly:
    /// `mul_div_floor` overflow while applying the constant-product formula
    /// at extreme reserve sizes.
    #[error("math: {0}")]
    Math(#[from] MathError),

    /// A cycle leg references a pool that's not in the registry.
    ///
    /// Uses `{0:?}` (Debug) because `Pubkey` does not implement `Display`
    /// in `dl-state` — intentionally, so adding `Display` doesn't make
    /// raw byte arrays look like a public key on the formatting surface.
    #[error("pool not found in registry: {0:?}")]
    PoolNotFound(Pubkey),

    /// A pool has a zero reserve on at least one side. Constant-product
    /// math is undefined there (division by zero). The detector should
    /// have filtered these out, but the sim checks defensively.
    #[error("pool has zero reserves")]
    ZeroReserve,

    /// `fee_bps >= 10_000` would underflow `10_000 - fee_bps` in the
    /// fee-on-input formula. Carries the offending value.
    #[error("fee_bps too high: {0} (must be < 10_000)")]
    FeeTooHigh(u16),

    /// A cycle has more legs than the sim can process in a single call.
    /// The bound is a defensive cap, not a correctness one.
    #[error("cycle too long: {0} legs (max supported)")]
    CycleTooLong(usize),

    /// A probability was constructed outside the valid parts-per-million
    /// range `0..=1_000_000`. Carries the offending value.
    #[error("probability out of range: {0} ppm (must be <= 1_000_000)")]
    ProbOutOfRange(u32),
}
