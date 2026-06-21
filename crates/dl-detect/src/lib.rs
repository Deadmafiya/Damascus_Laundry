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
//! - [`staleness`] — graph-level per-edge staleness prune (DAM-44c).
//!   Sits next to [`graph`] (rather than in `dl-state`) to avoid a
//!   `dl-state` ↔ `dl-detect` cyclic dep. See the module's docs for
//!   the distinction from `dl_feed::staleness::StalenessGuard`.

pub mod bellman_ford;
pub mod cycle;
pub mod error;
pub mod graph;
pub mod staleness;

pub use bellman_ford::find_negative_cycles;
pub use cycle::{compute_profit_bps, Cycle, Direction, Leg};
pub use error::DetectError;
pub use graph::{build_from_pools, Edge, Graph, TokenId};
pub use staleness::{
    max_pool_age_slots_from_env, prune_stale_edges, PruneReport, StaleEdgeDrop,
    DEFAULT_MAX_POOL_AGE_SLOTS, MAX_POOL_AGE_SLOTS_ENV, STALE_EDGES_PRUNED_TOTAL,
};
