//! Error types for `dl-detect`.
//!
//! The detector's two main failure modes are:
//! 1. a graph-builder/registry lookup missed something it needed
//!    (`EmptyGraph`, `UnknownToken`, `PoolNotFound`),
//! 2. a forward simulation rejected a candidate cycle (`InvalidMath`,
//!    `SimulationMismatch`).
//!
//! Every variant carries enough context to log a useful warning without
//! needing to re-derive the input.

use thiserror::Error;

use dl_core::MathError;
use dl_state::Pubkey;

#[derive(Debug, Error)]
pub enum DetectError {
    /// `build_from_pools` received an empty slice.
    #[error("graph build: no pools provided")]
    EmptyGraph,

    /// A `TokenId` was passed that doesn't exist in the graph.
    #[error("unknown token id: {0}")]
    UnknownToken(u32),

    /// A cycle references a pool that's not in the registry.
    ///
    /// Uses `{0:?}` (Debug) because `Pubkey` does not implement `Display`
    /// in `dl-state` — intentionally, so adding `Display` doesn't make
    /// raw byte arrays look like a public key on the formatting surface.
    #[error("pool not found in registry: {0:?}")]
    PoolNotFound(Pubkey),

    /// A checked-math primitive in the detector failed.
    /// Most commonly: `mul_div_floor` underflow/overflow while computing
    /// a leg's effective rate.
    #[error("math: {0}")]
    InvalidMath(#[from] MathError),

    /// A candidate cycle was detected, but the forward simulation
    /// (constant-product fill math) did not produce positive net output.
    /// Surfaces "BF said there was a cycle, reality said no" — useful
    /// for filtering out stale-state false positives in production.
    #[error("simulation rejected cycle: net output = {0}")]
    SimulationMismatch(u128),
}
