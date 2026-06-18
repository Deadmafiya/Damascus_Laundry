//! Cycle / Leg / Direction — shared data types for the detector and
//! the per-cycle simulator.
//!
//! ## Why these types live in `dl-state`
//!
//! `dl-detect` produces `Cycle`s; `dl-sim` consumes them. Neither owns
//! the type exclusively, and putting the type in either crate forces a
//! cyclic dep (the other crate's `cargo build` would fail). `dl-state`
//! is the shared data-model crate that both already depend on, so the
//! types live here. `dl-detect::cycle` re-exports them for
//! backward-compatible imports (`use dl_detect::cycle::Cycle;`).
//!
//! ## What lives here vs. in the detection crate
//!
//! - **Here (data only)**: `Leg`, `Direction`, `Cycle` struct fields,
//!   [`compute_profit_bps`] helper, unit tests for the helper.
//! - **In `dl-detect` (behavior)**: [`dl_detect::cycle::simulate_through_pools`]
//!   — the per-cycle forward-fill helper (free function, not an inherent
//!   method on `Cycle`, because of the orphan rule; depends on `dl-sim`,
//!   which can't be a dep of `dl-state` without creating a worse cycle).

use crate::Pubkey;

/// One directed hop in a cycle.
///
/// A `Leg` is fully determined by:
/// - the `pool` it's drawn from (drives the forward fill math), and
/// - the `direction` (base -> quote or quote -> base; some pools have
///   asymmetric fee schedules per side in v1.1+, not in v1.0).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Leg {
    /// Pool this leg fills through.
    pub pool: Pubkey,
    /// Whether we are going base->quote or quote->base on this pool.
    pub direction: Direction,
    /// Underlying edge weight from the price graph, in 1e-18 scale.
    /// Kept here so the cycle can compute its own profit without
    /// re-resolving edges.
    pub weight: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    BaseToQuote,
    QuoteToBase,
}

/// A candidate arbitrage cycle, recovered from the price graph.
///
/// `legs` is ordered (a, b, c, ..., a). `weight_sum` is the sum of
/// leg weights; in v1.0 we only keep cycles with `weight_sum < 0`
/// (i.e. log-rate < 0 → gross profit > 1). `expected_profit_bps` is
/// the net expected return in basis points (positive == profit),
/// computed by [`compute_profit_bps`] from `weight_sum`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cycle {
    pub legs: Vec<Leg>,
    /// Sum of `legs[*].weight`, 1e-18 scale. Negative == profitable.
    pub weight_sum: i64,
    /// Net expected return in basis points, positive == profit. Set
    /// by [`compute_profit_bps`]; default 0 in the scaffold.
    pub expected_profit_bps: i32,
}

impl Cycle {
    /// Build a `Cycle` from a list of legs. `weight_sum` is computed
    /// from the leg weights; `expected_profit_bps` is left at 0 (use
    /// [`Cycle::compute_expected_profit_bps`] to fill it).
    pub fn new(legs: Vec<Leg>) -> Self {
        let weight_sum = legs.iter().map(|l| l.weight).sum();
        Self {
            legs,
            weight_sum,
            expected_profit_bps: 0,
        }
    }

    /// Number of legs (== number of pool hops). A 2-cycle = direct
    /// arb on a single pair; a 3-cycle = triangle arb.
    pub fn n_legs(&self) -> usize {
        self.legs.len()
    }

    /// Borrow the legs.
    pub fn legs(&self) -> &[Leg] {
        &self.legs
    }

    /// Recompute [`Self::expected_profit_bps`] from the current
    /// `weight_sum` using [`compute_profit_bps`].
    pub fn compute_expected_profit_bps(&mut self) {
        self.expected_profit_bps = compute_profit_bps(self.weight_sum);
    }
}

/// Compute `expected_profit_bps` from a `weight_sum` (1e-18 scale).
///
/// `weight_sum < 0` means the cycle has gross return > 1 (per the
/// log-rate formulation). The bps profit is
/// `-weight_sum * 10000 / 1e18`, saturated to `i32` range.
pub fn compute_profit_bps(weight_sum: i64) -> i32 {
    // Use i128 to avoid `i64::MIN.checked_neg()` panicking; saturate
    // to i64 range first.
    let neg: i128 = if weight_sum == i64::MIN {
        i128::from(i64::MAX)
    } else {
        i128::from(-weight_sum)
    };
    let bps = neg.saturating_mul(10_000) / 1_000_000_000_000_000_000i128;
    if bps > i32::MAX as i128 {
        i32::MAX
    } else if bps < i32::MIN as i128 {
        i32::MIN
    } else {
        bps as i32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profit_bps_positive_on_negative_weight() {
        // weight_sum = -1e18 means gross = 2.0 → +10_000 bps = +100%.
        // bps = -(-1e18) * 10_000 / 1e18 = 10_000.
        assert_eq!(compute_profit_bps(-1_000_000_000_000_000_000), 10_000);
    }

    #[test]
    fn profit_bps_zero_at_break_even() {
        // weight_sum = 0 → gross = 1.0 → 0 bps.
        assert_eq!(compute_profit_bps(0), 0);
    }

    #[test]
    fn profit_bps_negative_on_positive_weight() {
        // weight_sum = +1e18 means gross = 0.5 → -10_000 bps.
        assert_eq!(compute_profit_bps(1_000_000_000_000_000_000), -10_000);
    }

    #[test]
    fn cycle_new_computes_weight_sum() {
        let cycle = Cycle::new(vec![
            Leg {
                pool: Pubkey([1u8; 32]),
                direction: Direction::BaseToQuote,
                weight: -500,
            },
            Leg {
                pool: Pubkey([2u8; 32]),
                direction: Direction::BaseToQuote,
                weight: -300,
            },
            Leg {
                pool: Pubkey([3u8; 32]),
                direction: Direction::BaseToQuote,
                weight: 200,
            },
        ]);
        assert_eq!(cycle.weight_sum, -600);
        assert_eq!(cycle.n_legs(), 3);
        assert_eq!(cycle.expected_profit_bps, 0); // not auto-computed
    }
}
