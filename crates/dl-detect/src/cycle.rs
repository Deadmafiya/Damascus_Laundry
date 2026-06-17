//! Detected arbitrage cycle.
//!
//! A `Cycle` is a closed walk in the price graph whose edges sum to a
//! negative total weight (i.e. net positive return). It is the
//! detector's output and the input to the per-cycle simulator.
//!
//! In this scaffold, the cycle carries a `Vec<Leg>` and a
//! `weight_sum: i64` (1e-18 scale). `expected_profit_bps` and the
//! forward-simulation validator are added in 03-04.

use dl_state::{Pool, PoolRegistry, Pubkey};

use crate::error::DetectError;

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
/// filled in by 03-04.
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

    /// Simulate the cycle forward through the constant-product fill
    /// math against the live `PoolRegistry` and return the net output
    /// in input-token base units.
    ///
    /// Implemented in 03-04. This scaffold returns `Err(SimulationMismatch(0))`
    /// to keep the signature stable.
    pub fn simulate_through_pools(&self, _registry: &PoolRegistry) -> Result<u128, DetectError> {
        // Implementation deferred to 03-04.
        let _ = std::any::type_name::<Pool>();
        Err(DetectError::SimulationMismatch(0))
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
}
