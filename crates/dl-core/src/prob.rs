//! Integer-only probability primitives for the value path.
//!
//! Phase 5 models competition / landing / detection probabilities as
//! `u128` fixed-point numbers in `[0, PROB_SCALE_1E18]`, where
//! `PROB_SCALE_1E18 == 1.0`. This is the integer-only analog of a `[0,1]`
//! float probability: a probability `p` is represented by the integer
//! `floor(p * 1e18)`.
//!
//! Why 1e18: it matches the existing graph-weight scale used elsewhere in
//! the engine, gives ~18 significant decimal digits (more than enough for
//! probability work), and lets us reuse [`crate::fixed::mul_div_floor`] for
//! all multiplications without overflow at realistic magnitudes.
//!
//! No `f32`/`f64` appears in this module.

use crate::fixed::mul_div_floor;
use crate::rng::Rng;

/// One (1.0) in the probability fixed-point scale: `10^18`.
pub const PROB_SCALE_1E18: u128 = 1_000_000_000_000_000_000;

/// Basis points per unit (1.0): `10_000`. Convenience for bps <-> prob.
pub const BPS_PER_UNIT: u128 = 10_000;

/// Extension trait giving [`Rng`] a probability-scale draw.
///
/// A "probability" here is a `u128` in `[0, PROB_SCALE_1E18]`. Drawing one
/// uniformly and comparing against a threshold is the integer-only analog of
/// `rng.gen::<f64>() < threshold`.
pub trait RngExt: Rng {
    /// Draw a probability-scale value in `[0, PROB_SCALE_1E18)`.
    ///
    /// Uses [`Rng::next_below`] with `PROB_SCALE_1E18` as the bound. The
    /// bound fits in `u64`? No — `1e18 > u64::MAX`? No: `u64::MAX ≈ 1.8e19`,
    /// so `1e18` fits in `u64` and `next_below` is fine.
    #[inline]
    fn next_prob(&mut self) -> u128 {
        // PROB_SCALE_1E18 = 1e18 < u64::MAX (~1.8e19), so this cast is exact.
        self.next_below(PROB_SCALE_1E18 as u64) as u128
    }

    /// Bernoulli trial: returns `true` with probability `p_success`
    /// (in `[0, PROB_SCALE_1E18]`). `p_success >= 1.0` always succeeds;
    /// `p_success == 0` always fails.
    #[inline]
    fn trial(&mut self, p_success: u128) -> bool {
        if p_success >= PROB_SCALE_1E18 {
            return true;
        }
        if p_success == 0 {
            return false;
        }
        self.next_prob() < p_success
    }
}

/// Implement the extension for every [`Rng`] automatically.
impl<T: Rng + ?Sized> RngExt for T {}

/// Multiply two probabilities: `floor(a * b / PROB_SCALE_1E18)`.
///
/// `(a / 1e18) * (b / 1e18) = (a*b) / 1e36`, rescaled back to 1e18 by dividing
/// by `1e18` once more. Uses the overflow-checked 256-bit primitive.
#[inline]
pub fn mul_prob(a: u128, b: u128) -> u128 {
    // a, b <= 1e18, so a*b <= 1e36 which overflows u128 (u128::MAX ≈ 3.4e38,
    // so 1e36 actually fits — but we use mul_div_floor for safety + the
    // single-value wide path is essentially free).
    mul_div_floor(a, b, PROB_SCALE_1E18).unwrap_or(0)
}

/// Convert basis points `[0, 10_000]` to probability scale `[0, PROB_SCALE_1E18]`.
///
/// `bps_to_prob(10_000) == PROB_SCALE_1E18` (1.0); `bps_to_prob(250)` ≈ 0.025.
#[inline]
pub fn bps_to_prob(bps: u32) -> u128 {
    // bps / 10_000 = p; p * 1e18 = bps * 1e18 / 10_000 = bps * 1e14.
    // bps is u32 (max ~4.3e9), * 1e14 ≈ 4.3e23 — overflows u64 but fits u128.
    (bps as u128) * (PROB_SCALE_1E18 / BPS_PER_UNIT)
}

/// `p >= threshold` on probability scale. Trivial compare, but named for the
/// value-path convention (every probability op has a named helper, no bare
/// `<` against a magic 1e18 literal scattered through call sites).
#[inline]
pub fn prob_ge(p: u128, threshold: u128) -> bool {
    p >= threshold
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rng::SeededRng;

    #[test]
    fn scale_is_one_e18() {
        assert_eq!(PROB_SCALE_1E18, 1_000_000_000_000_000_000);
    }

    #[test]
    fn next_prob_in_range() {
        let mut r = SeededRng::new(11);
        for _ in 0..10_000 {
            let p = r.next_prob();
            assert!(p < PROB_SCALE_1E18, "{} >= 1e18", p);
        }
    }

    #[test]
    fn trial_extremes() {
        let mut r = SeededRng::new(99);
        // p = 1.0 always succeeds.
        for _ in 0..100 {
            assert!(r.trial(PROB_SCALE_1E18));
        }
        // p = 0 always fails.
        for _ in 0..100 {
            assert!(!r.trial(0));
        }
    }

    #[test]
    fn trial_p_one_half_roughly_half() {
        // p = 0.5 -> over many trials, success rate near 0.5.
        let mut r = SeededRng::new(7);
        let half = PROB_SCALE_1E18 / 2;
        let mut wins = 0u32;
        let n = 20_000u32;
        for _ in 0..n {
            if r.trial(half) {
                wins += 1;
            }
        }
        let rate = wins as f64 / n as f64;
        // Loose band; SplitMix64 is uniform enough that 20k draws sit well inside.
        assert!(
            (0.48..=0.52).contains(&rate),
            "win rate {} out of band for p=0.5",
            rate
        );
    }

    #[test]
    fn mul_prob_identity() {
        assert_eq!(mul_prob(PROB_SCALE_1E18, PROB_SCALE_1E18), PROB_SCALE_1E18);
        assert_eq!(mul_prob(0, PROB_SCALE_1E18), 0);
        // 0.5 * 0.5 = 0.25
        let half = PROB_SCALE_1E18 / 2;
        let quarter = mul_prob(half, half);
        assert_eq!(quarter, PROB_SCALE_1E18 / 4);
    }

    #[test]
    fn bps_to_prob_known() {
        assert_eq!(bps_to_prob(10_000), PROB_SCALE_1E18);
        assert_eq!(bps_to_prob(0), 0);
        // 250 bps = 0.025 = 2.5e16
        assert_eq!(bps_to_prob(250), 25_000_000_000_000_000);
    }

    #[test]
    fn rng_ext_is_implemented_for_seeded() {
        // Compile-time check that the blanket impl covers the concrete type.
        fn _accepts<R: RngExt>(_r: &R) {}
        _accepts(&SeededRng::new(0));
    }
}
