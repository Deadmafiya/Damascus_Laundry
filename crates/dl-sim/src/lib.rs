//! `dl-sim` — profit/cost estimation + pessimistic simulation core.
//!
//! Phase 4 adds AMM-curve-accurate fills and optimal sizing; Phase 5 adds the
//! multiplicative EV decomposition `E[PnL] = p_detect * p_win * p_land * (gross - costs)
//! - E[failed_costs]`, latency re-check, and the winner's-curse haircut. Placeholder.
