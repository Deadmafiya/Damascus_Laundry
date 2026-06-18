//! Optimal input sizing via golden-section search.
//!
//! Given a [`Cycle`] (Phase 3), a [`PoolRegistry`] (Phase 2), and a [`CostModel`]
//! (Phase 4), find the input size in `[0, max_input]` that **maximizes net profit**
//! (gross cycle output − input − costs). The output is [`OptimalInput::Profitable`]
//! with the optimal `amount` and its `net_profit`, or [`OptimalInput::NoTrade`]
//! if the cycle is unprofitable at every size in the bracket.
//!
//! ## Why golden-section
//!
//! The constant-product output function for a *single* leg has a closed-form
//! inverse — solve for `dx` given `dy`. But for a 3+ leg cycle, output is a
//! composition of three `dy = f(dx)` functions with fee subtraction at each leg,
//! and finding the input that maximizes `output − costs` requires solving
//! `d(net)/d(input) = 0` over that composition. The derivative exists but the
//! algebra gets ugly fast and is not robust to fee changes or per-leg reserve
//! differences.
//!
//! Golden-section is the standard 1-D unimodal-search algorithm: works on any
//! concave-down function (slippage is monotone → output is concave in input →
//! net is concave-down with a constant offset → unimodal), converges in
//! `O(log(1/ε))` iters, no gradients needed. 64 iters × ~1 µs per fill = ~64 µs
//! per cycle, negligible. The determinism is bit-exact (no FP, no system entropy).
//!
//! ## Unimodality proof
//!
//! - `gross_output(input)` is monotone non-decreasing in `input` and concave
//!   (constant-product slippage is monotone diminishing-returns; `d²y/dx² < 0`).
//! - `cost` is constant in `input` (does not depend on trade size).
//! - `input` itself is linear in `input`.
//! - `net(input) = gross(input) − input − cost` is therefore concave-down in
//!   `input` (concave + linear + constant = concave), and so is unimodal on
//!   any closed interval.
//!
//! ## Determinism
//!
//! Golden-section is bit-exact; [`simulate_cycle`] is deterministic; arithmetic
//! is `u128` + `i128`. Two calls on identical inputs return byte-identical
//! `OptimalInput` values.

use crate::cost::CostModel;
use crate::error::SimError;
use crate::simulate::simulate_cycle;

use dl_state::cycle::Cycle;
use dl_state::PoolRegistry;

/// The result of optimal-input sizing.
///
/// - [`OptimalInput::Profitable`]: a positive-net input was found.
/// - [`OptimalInput::NoTrade`]: the cycle is unprofitable at every input in
///   `[0, max_input]`. `best_negative_net` is the *least negative* net seen
///   (useful for logging — the cycle is closest to breaking even at that size).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OptimalInput {
    /// An input size was found that yields positive net profit.
    Profitable {
        /// Optimal input amount, in input-token base units.
        amount: u128,
        /// Net profit at `amount`, signed: positive = profit, in input-token
        /// base units.
        net_profit: i128,
    },
    /// The cycle is unprofitable at every input in `[0, max_input]`.
    NoTrade {
        /// The best (least-negative) net profit seen across the bracket.
        /// Useful for logging — the cycle is closest to breaking even at
        /// that size, even though it doesn't cross zero.
        best_negative_net: i128,
    },
}

/// Inverse golden ratio as a `u128` fraction: `1/φ ≈ 0.618033988`.
///
/// The standard golden-section interior points are
/// `c = a + (1/φ)(b - a)` and `d = a + (φ - 1)(b - a) = a + (1/φ)² × ...`
/// — equivalently, both are at `offset = (b - a) * INV_PHI_NUM / INV_PHI_DEN`
/// from the nearer boundary. We use the inverse so that the offset is always
/// < span (the standard error if you use φ directly: `offset = span * 1.618`
/// exceeds the bracket and `lo + offset` or `hi - offset` overflows on
/// `u128`).
const INV_PHI_NUM: u128 = 618_033_988;
const INV_PHI_DEN: u128 = 1_000_000_000;

/// Maximum golden-section iterations. Each iter shrinks the bracket by
/// `~38%` (1 - 1/φ); 64 iters → bracket shrinks by `0.38^64 ≈ 1e-27`, far
/// below the 1 bp tolerance for any realistic `max_input`. The cap is a
/// defensive bound, not a correctness one.
const MAX_ITERS: usize = 64;

/// Find the input size in `[0, max_input]` that maximizes net profit
/// (`gross_output − input − total_costs`).
///
/// Algorithm (see module docs for the unimodality proof):
/// 1. Evaluate `net_profit_at(0)`, `net_profit_at(max_input)`,
///    `net_profit_at(max_input / 2)`. If all three are ≤ 0, the cycle is
///    unprofitable at the endpoints *and* the midpoint — return
///    `NoTrade` with the best of the three.
/// 2. Otherwise, run golden-section search over `[0, max_input]` for
///    `MAX_ITERS` iters, or until the bracket is smaller than 1 bp of
///    `max_input`.
/// 3. Return the bracket midpoint as `amount`, the net at that amount as
///    `net_profit`.
///
/// **Edge cases:**
/// - `max_input == 0` → `NoTrade { best_negative_net: 0 }` (zero input → zero
///   output → net = −cost; the caller should treat this as "nothing to do").
/// - `simulate_cycle` errors → propagate the `Err` (the sim hit a pool
///   not in the registry, a degenerate reserve, etc.).
/// - `cost.total_cost()` errors → propagate the `Err` (overflow, which is
///   unreachable for realistic inputs but defensive).
pub fn find_optimal_input(
    cycle: &Cycle,
    registry: &PoolRegistry,
    cost: &CostModel,
    max_input: u128,
) -> Result<OptimalInput, SimError> {
    if max_input == 0 {
        return Ok(OptimalInput::NoTrade {
            best_negative_net: 0,
        });
    }

    // Pre-compute the cost once — it doesn't depend on input.
    let total_cost = cost.total_cost()?.total_lamports as i128;

    // Helper: net profit at a given input.
    let net_profit_at = |input: u128| -> Result<i128, SimError> {
        let gross = simulate_cycle(cycle, registry, input)?.final_output;
        Ok((gross as i128) - (input as i128) - total_cost)
    };

    // Convexity pre-check: if all three samples are non-positive, the cycle
    // is unprofitable everywhere in [0, max_input]. (For a unimodal
    // concave-down function, the max is at an endpoint only if the function
    // is monotone — i.e. the midpoint is not strictly better than the
    // endpoints.)
    let n0 = net_profit_at(0)?;
    let n_max = net_profit_at(max_input)?;
    let n_mid = net_profit_at(max_input / 2)?;
    if n0 <= 0 && n_max <= 0 && n_mid <= 0 {
        let best = n0.max(n_max).max(n_mid);
        return Ok(OptimalInput::NoTrade {
            best_negative_net: best,
        });
    }

    // Golden-section search.
    // The bracket is [lo, hi]; at each iter we maintain two interior
    // points m1 < m2 and shrink the bracket by ~38% per iter.
    let mut lo: u128 = 0;
    let mut hi: u128 = max_input;
    let tol: u128 = max_input / 10_000; // 1 bp of max_input (0 if max_input < 10_000)

    // Initial interior points: m1 = lo + (1/φ)(hi - lo), m2 = hi - (1/φ)(hi - lo).
    // `offset = (hi - lo) * 1/φ` puts m1 at ~38% of the way from lo, and
    // m2 at ~38% of the way from hi. The two interior points are
    // symmetrically placed at the golden ratio within the bracket.
    let span = hi - lo;
    let offset = span * INV_PHI_NUM / INV_PHI_DEN;
    let mut m1: u128 = lo + offset;
    let mut m2: u128 = hi - offset;
    let mut f1: i128 = net_profit_at(m1)?;
    let mut f2: i128 = net_profit_at(m2)?;

    for _ in 0..MAX_ITERS {
        if hi - lo <= tol {
            break;
        }
        if f1 < f2 {
            // The max is in [m1, hi]; new bracket is [m1, hi].
            // m1 becomes the new left boundary; m2 (the old right interior)
            // becomes the new m1 (left interior) — it's already in the
            // bracket at the right position. The new m2 (right interior)
            // is recomputed: lo_new + offset = m1_old + offset.
            lo = m1;
            m1 = m2;
            f1 = f2;
            let new_offset = (hi - lo) * INV_PHI_NUM / INV_PHI_DEN;
            m2 = lo + new_offset;
            f2 = net_profit_at(m2)?;
        } else {
            // The max is in [lo, m2]; new bracket is [lo, m2].
            // m2 becomes the new right boundary; m1 (the old left interior)
            // becomes the new m2 (right interior). The new m1 is recomputed:
            // hi_new - offset = m2_old - offset.
            hi = m2;
            m2 = m1;
            f2 = f1;
            let new_offset = (hi - lo) * INV_PHI_NUM / INV_PHI_DEN;
            m1 = hi - new_offset;
            f1 = net_profit_at(m1)?;
        }
    }

    // After the loop, m1 and m2 are near the max. Return whichever has
    // the higher net profit (they should be within 1 bp of each other;
    // a single sample at the midpoint disambiguates).
    let midpoint: u128 = (lo + hi) / 2;
    let f_mid: i128 = net_profit_at(midpoint)?;
    // Pick the best of (m1, f1), (m2, f2), (midpoint, f_mid).
    let mut best_amount: u128 = midpoint;
    let mut best_f: i128 = f_mid;
    if f1 > best_f {
        best_amount = m1;
        best_f = f1;
    }
    if f2 > best_f {
        best_amount = m2;
        best_f = f2;
    }

    if best_f > 0 {
        Ok(OptimalInput::Profitable {
            amount: best_amount,
            net_profit: best_f,
        })
    } else {
        Ok(OptimalInput::NoTrade {
            best_negative_net: best_f,
        })
    }
}
