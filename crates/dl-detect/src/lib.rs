//! `dl-detect` — atomic-arbitrage opportunity detection.
//!
//! Phase 3 builds a price graph (tokens = nodes, pools = edges weighted
//! by `-log(effective rate)`) and runs Bellman-Ford negative-cycle
//! detection. Each detected cycle is the engine's atomic-arb opportunity
//! and is handed off to the per-cycle simulator (Phase 4/5).
//!
//! ## Submodules
//!
//! - [`graph`] — token-interned directed graph + [`build_from_pools`]
//! - [`cycle`] — the [`Cycle`] / [`Leg`] / [`Direction`] detector output
//!   and the [`compute_profit_bps`] helper
//! - [`bellman_ford`] — the [`find_negative_cycles`] search (v1.0 stub;
//!   real impl lands in task 03-03)
//! - [`error`] — [`DetectError`] failures from graph build and forward sim

pub mod bellman_ford;
pub mod cycle;
pub mod error;
pub mod graph;

pub use bellman_ford::find_negative_cycles;
pub use cycle::{compute_profit_bps, Cycle, Direction, Leg};
pub use error::DetectError;
pub use graph::{build_from_pools, Edge, Graph, TokenId};
