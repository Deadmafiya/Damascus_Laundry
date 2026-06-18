//! Cycle / Leg / Direction — re-exported from `dl-state` for
//! backward-compatible imports, plus the per-cycle forward-fill
//! helper that depends on `dl-sim`.
//!
//! ## Why this is a re-export
//!
//! Phase 4's `simulate_through_pools` needs to call into `dl-sim`.
//! `dl-sim` consumes `Cycle`s. The natural place for `Cycle` is
//! `dl-state` (the shared data-model crate), but that would force the
//! historical import path `dl_detect::cycle::Cycle` to break. Re-exporting
//! from `dl-state::cycle` keeps the public API stable while making the
//! types available to `dl-sim` without a cyclic dep.
//!
//! ## Why the simulate helper is a free function, not a `Cycle` method
//!
//! `Cycle` lives in `dl-state`; `simulate_through_pools` lives in
//! `dl-detect` (it depends on `dl-sim`, which can't be a dep of
//! `dl-state`). Rust's "orphan rule" forbids inherent `impl Cycle { ... }`
//! blocks in a different crate from the type. A free function is the
//! idiomatic workaround: the function takes `&Cycle` and
//! `&PoolRegistry` as arguments, no method receiver.
//!
//! See `dl-state::cycle` for the type definitions and unit tests.

pub use dl_state::cycle::{compute_profit_bps, Cycle, Direction, Leg};

use dl_state::PoolRegistry;

use crate::error::DetectError;

/// Forward-fill simulation: thin wrapper over `dl_sim::simulate::simulate_cycle`.
///
/// `cycle` is the candidate arbitrage cycle (from Phase 3 detection).
/// `registry` is the live `PoolRegistry` (from Phase 2 ingestion).
/// `input` is the amount of input-token the caller intends to put
/// into the cycle. Returns the gross output of the final leg (i.e. the
/// input-token units the cycle returns, **before** costs are netted —
/// see `dl_sim::net_profit::NetProfit` for the cost-netting boundary
/// object).
///
/// `SimError` is mapped to `DetectError` at the crate boundary so the
/// detector's API stays self-consistent. The mapping is:
/// - `Math` → `InvalidMath` (same `MathError` type)
/// - `PoolNotFound(pk)` → `PoolNotFound(pk)`
/// - `ZeroReserve` / `FeeTooHigh` / `CycleTooLong` (defensive) →
///   `SimulationMismatch(0)` — these would never come from a
///   detector-constructed cycle, but a defensive mapping keeps the
///   API total.
pub fn simulate_through_pools(
    cycle: &Cycle,
    registry: &PoolRegistry,
    input: u128,
) -> Result<u128, DetectError> {
    let fill = dl_sim::simulate::simulate_cycle(cycle, registry, input)?;
    Ok(fill.final_output)
}
