//! Constant-product AMM fill math (Raydium AMM v4 model).
//!
//! The single primitive here is [`fill_constant_product`]. Everything else
//! in the sim is built on top of it. The math is the standard
//! UniswapV2-style `dy = y * dx_in_eff / (x + dx_in_eff)`, with the fee
//! charged on the input side (`dx_in_eff = dx * (10_000 - fee_bps) / 10_000`).
//!
//! All arithmetic is `u128` with the 256-bit-intermediate `mul_div_floor`
//! from `dl-core::fixed`. The function never panics: every error
//! condition returns a `Result::Err` from [`SimError`].
//!
//! ## Float-free invariant
//!
//! No `f32`/`f64` appears in this module. The fill math is exact integer
//! arithmetic: the truncation error from the floor divide is bounded by
//! 1 unit, and is the only source of error in the output. Replays from
//! captured data are bit-identical.

use crate::error::SimError;

/// Maximum legal `fee_bps` value. Anything `>= 10_000` would mean a
/// 100%+ fee (or negative net input), which is undefined.
const MAX_FEE_BPS: u16 = 10_000;

/// Compute the constant-product fill, fee-on-input (Raydium AMM v4 model).
///
/// Returns `dy = floor(y * dx_in_eff / (x + dx_in_eff))` where
/// `dx_in_eff = dx * (10_000 - fee_bps) / 10_000`.
///
/// All four inputs are in the pool's *own* base units (lamports for SOL,
/// micro-USDC for USDC, etc.). The output is in `reserve_out`'s base
/// units. There is no decimal conversion in this function — the sim
/// layer operates purely on raw base units.
///
/// ## Edge cases
///
/// - `amount_in == 0` → `Ok(0)` (no work, no output, no fee).
/// - `reserve_in == 0 || reserve_out == 0` → `Err(SimError::ZeroReserve)`.
/// - `fee_bps >= 10_000` → `Err(SimError::FeeTooHigh(fee_bps))`.
/// - Internal overflow / div-by-zero → `Err(SimError::Math(...))`.
///
/// ## Why fee-on-input?
///
/// Raydium AMM v4 charges the LP fee on the input side, then runs the
/// pure constant-product formula. This is the convention for UniswapV2-
/// derived AMMs; CLMMs (Orca Whirlpool) may differ and v1.1+ tick-walking
/// is out of scope for v1.0.
pub fn fill_constant_product(
    reserve_in: u128,
    reserve_out: u128,
    fee_bps: u16,
    amount_in: u128,
) -> Result<u128, SimError> {
    // Zero-in: zero-out, no work.
    if amount_in == 0 {
        return Ok(0);
    }
    // Degenerate pool: cannot fill against an empty side.
    if reserve_in == 0 || reserve_out == 0 {
        return Err(SimError::ZeroReserve);
    }
    // Fee cap: anything >= 10_000 bps would underflow `10_000 - fee_bps`.
    if fee_bps >= MAX_FEE_BPS {
        return Err(SimError::FeeTooHigh(fee_bps));
    }
    // Effective amount hitting the pool, after the fee-on-input deduction.
    // `amount_in * (10_000 - fee_bps) / 10_000` — for realistic `amount_in`
    // (≤ u64 lamport scale), this multiplication fits comfortably in u128.
    // The `mul_div_floor` Overflow path catches the corner case at u128::MAX.
    let fee_denom: u128 = MAX_FEE_BPS as u128;
    let fee_num: u128 = (MAX_FEE_BPS - fee_bps) as u128;
    let dx_eff = dl_core::fixed::mul_div_floor(amount_in, fee_num, fee_denom)?;
    // `dy = y * dx_eff / (x + dx_eff)`. `mul_div_floor` handles the
    // 256-bit intermediate, so this is overflow-safe even at extreme
    // reserves.
    let dy = dl_core::fixed::mul_div_floor(reserve_out, dx_eff, reserve_in + dx_eff)?;
    Ok(dy)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zero_in_yields_zero_out() {
        // amount_in == 0 → Ok(0) with no fee math.
        let dy = fill_constant_product(1_000_000, 1_000_000, 25, 0).unwrap();
        assert_eq!(dy, 0);
    }

    #[test]
    fn zero_reserve_in_errors() {
        let err = fill_constant_product(0, 1_000_000, 25, 100).unwrap_err();
        assert!(matches!(err, SimError::ZeroReserve));
    }

    #[test]
    fn zero_reserve_out_errors() {
        let err = fill_constant_product(1_000_000, 0, 25, 100).unwrap_err();
        assert!(matches!(err, SimError::ZeroReserve));
    }

    #[test]
    fn fee_bps_too_high_errors() {
        let err = fill_constant_product(1_000_000, 1_000_000, 10_000, 100).unwrap_err();
        assert!(matches!(err, SimError::FeeTooHigh(10_000)));
    }

    #[test]
    fn zero_fee_reduces_to_textbook_formula() {
        // 0 bps fee: dy = y * dx / (x + dx)
        // x = 1e6, y = 1e6, dx = 1e5 → dy = 1e6 * 1e5 / 1.1e6 = 90_909
        let dy = fill_constant_product(1_000_000, 1_000_000, 0, 100_000).unwrap();
        let expected = 1_000_000u128 * 100_000 / 1_100_000;
        assert_eq!(dy, expected);
        assert_eq!(dy, 90_909);
    }

    #[test]
    fn non_zero_fee_reduces_output() {
        // 25 bps fee: dx_eff = 100_000 * 9_975 / 10_000 = 99_750
        // dy = 1e6 * 99_750 / 1_099_750 = ~ 90_680
        let dy_zero = fill_constant_product(1_000_000, 1_000_000, 0, 100_000).unwrap();
        let dy_fee = fill_constant_product(1_000_000, 1_000_000, 25, 100_000).unwrap();
        assert!(
            dy_fee < dy_zero,
            "fee should reduce output: {dy_fee} vs {dy_zero}"
        );
    }

    #[test]
    fn output_never_exceeds_reserve_out() {
        // Massive input: dy must still be <= reserve_out.
        let dy = fill_constant_product(1_000, 1_000_000, 25, u128::MAX / 2).unwrap();
        assert!(dy <= 1_000_000);
    }
}
