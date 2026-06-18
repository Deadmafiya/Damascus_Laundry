//! `dl-sim` — AMM-curve-accurate cycle fills + optimal input sizing + full
//! cost netting. The per-cycle `NetProfit` this crate produces is the input
//! to Phase 5's pessimistic simulation core (multiplicative EV
//! decomposition: `E[PnL] = p_detect * p_win * p_land * (gross - costs)
//! - E[failed_costs]`).
//!
//! ## Module layout
//!
//! - [`error`] — [`SimError`] failures
//! - [`fill`] — [`fill::fill_constant_product`], the single primitive
//! - [`simulate`] — [`simulate::simulate_cycle`], multi-leg forward fill (Task 2)
//! - [`cost`] — [`cost::CostModel`] / [`cost::CostBreakdown`], cost stack (Task 3)
//! - [`sizing`] — [`sizing::find_optimal_input`], golden-section sizer (Task 4)
//! - [`net_profit`] — [`net_profit::NetProfit`], per-cycle output (Task 5)
//!
//! ## Integer-only invariant
//!
//! This crate is value-path. No fractional types anywhere in `src/`.
//! Decimals are confined to `dl-core::display` (Phase 1 boundary). The CI
//! guard `tests/fixed_point_no_fractional.rs` (Task 7) enforces this.

pub mod cost;
pub mod error;
pub mod fill;
pub mod net_profit;
pub mod simulate;
pub mod sizing;
