//! NetProfit: per-cycle output, the boundary object Phase 5 reads.
//!
//! Given an [`OptimalInput`] (from [`crate::sizing::find_optimal_input`]) and
//! the corresponding gross output, build a [`NetProfit`] that the Phase 5
//! pessimistic simulation core can read directly: input amount, gross output,
//! full cost breakdown, signed net profit, signed bps profit, and a
//! convenience `profitable` flag.
//!
//! ## Why this struct is the Phase 4/5 boundary
//!
//! Phase 4 is the *fill* + *cost* layer — the per-cycle deterministic math.
//! Phase 5 is the *probability* layer — it takes this struct and multiplies
//! by `p_detect × p_win × p_land` (and subtracts `E[failed_costs]`) to produce
//! a paper-trade decision. By keeping [`NetProfit`] self-contained (no
//! re-running the sizer, no re-resolving pools), Phase 5 can iterate the
//! probability multipliers without recomputing fills.

use crate::cost::{CostBreakdown, CostModel};
use crate::error::SimError;
use crate::sizing::OptimalInput;

/// Per-cycle net-profit result.
///
/// All fields are computed by [`NetProfit::from_optimal`]. The struct is
/// self-contained — Phase 5 reads `net_profit_bps` and `profitable` without
/// needing to re-run the sizer or re-resolve pool reserves.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NetProfit {
    /// Input amount, in input-token base units.
    pub input_amount: u128,
    /// Gross cycle output, in input-token base units, **before** costs.
    pub gross_output: u128,
    /// The full cost stack (base sig fee + priority fee + Jito tip + 5% Jito fee).
    pub total_costs: CostBreakdown,
    /// Signed net: `gross_output - input_amount - total_costs.total_lamports`.
    /// Positive = profit, negative = loss, zero = break-even.
    pub net_profit: i128,
    /// Net profit in basis points: `net_profit * 10_000 / input_amount` (in
    /// fixed-point `i128`, then saturated to `i32`). Positive = profit,
    /// negative = loss, zero = break-even.
    pub net_profit_bps: i32,
    /// Convenience flag: `net_profit > 0` (strictly — break-even is not
    /// "profitable").
    pub profitable: bool,
}

impl NetProfit {
    /// Build a `NetProfit` from the sizer's output and the gross cycle
    /// output.
    ///
    /// The `OptimalInput` enum's `net_profit` field is ignored (it's recomputed
    /// from the explicit `input` + `gross_output` + `cost` for consistency).
    /// The `NoTrade` variant is accepted without special handling: the
    /// `profitable` flag is computed from `net_profit` regardless, and the
    /// caller (Phase 5) is responsible for filtering out `NoTrade` cycles
    /// before they reach the paper ledger.
    pub fn from_optimal(
        _optimal: OptimalInput,
        input: u128,
        gross_output: u128,
        cost: &CostModel,
    ) -> Result<Self, SimError> {
        let breakdown = cost.total_cost()?;
        let net: i128 =
            (gross_output as i128) - (input as i128) - (breakdown.total_lamports as i128);
        let bps: i32 = if input == 0 {
            0
        } else {
            // Fixed-point bps: net * 10_000 / input, in i128, then saturate to i32.
            let bps_i128 = net.saturating_mul(10_000) / (input as i128);
            if bps_i128 > i32::MAX as i128 {
                i32::MAX
            } else if bps_i128 < i32::MIN as i128 {
                i32::MIN
            } else {
                bps_i128 as i32
            }
        };
        Ok(NetProfit {
            input_amount: input,
            gross_output,
            total_costs: breakdown,
            net_profit: net,
            net_profit_bps: bps,
            profitable: net > 0,
        })
    }
}
