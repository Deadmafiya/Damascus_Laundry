//! Property tests for `fill_constant_product` (AC-1).
//!
//! These cover the four invariants the plan asserts:
//! 1. No panic; returns `Ok` or `Err` always.
//! 2. `result.is_ok() ⇒ result.unwrap() <= reserve_out` (cannot extract
//!    more than the pool holds).
//! 3. Monotone in `amount_in`: more in ⇒ more out, non-strict.
//! 4. Zero-fee reduces to `floor(y * dx / (x + dx))`.

use dl_sim::error::SimError;
use dl_sim::fill::fill_constant_product;
use proptest::prelude::*;

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    /// `fill_constant_product` never panics; on `Ok`, the output is in
    /// `[0, reserve_out]`.
    #[test]
    fn result_is_bounded(
        x in 1u128..=1_000_000_000_000_000_000_000_000u128,
        y in 1u128..=1_000_000_000_000_000_000_000_000u128,
        f_bps in 0u16..10_000u16,
        dx in 0u128..=1_000_000_000_000_000_000u128,
    ) {
        let result = fill_constant_product(x, y, f_bps, dx);
        if let Ok(dy) = result {
            prop_assert!(dy <= y, "dy ({dy}) must be <= reserve_out ({y})");
        }
        // `Err` is fine — the function never panics.
    }

    /// `fill_constant_product` is monotone non-strict in `amount_in`:
    /// for fixed `(x, y, f_bps)`, `fill(dx1) <= fill(dx2)` when
    /// `dx1 <= dx2`.
    #[test]
    fn monotone_in_amount_in(
        x in 1u128..=1_000_000_000_000_000_000_000_000u128,
        y in 1u128..=1_000_000_000_000_000_000_000_000u128,
        f_bps in 0u16..10_000u16,
        dx1 in 0u128..=500_000_000_000_000_000u128,
        extra in 0u128..=500_000_000_000_000_000u128,
    ) {
        let dx2 = dx1.saturating_add(extra);
        let r1 = fill_constant_product(x, y, f_bps, dx1);
        let r2 = fill_constant_product(x, y, f_bps, dx2);
        match (r1, r2) {
            (Ok(dy1), Ok(dy2)) => {
                prop_assert!(dy1 <= dy2, "monotone: dy1={dy1} > dy2={dy2}");
            }
            _ => {
                // Either or both errored — not a monotonicity violation.
            }
        }
    }

    /// Zero-fee reduces to the textbook constant-product formula:
    /// `dy = floor(y * dx / (x + dx))` via `mul_div_floor`.
    #[test]
    fn zero_fee_reduces_to_textbook(
        x in 1u128..=1_000_000_000_000_000_000_000_000u128,
        y in 1u128..=1_000_000_000_000_000_000_000_000u128,
        dx in 0u128..=1_000_000_000_000_000_000u128,
    ) {
        let actual = fill_constant_product(x, y, 0, dx);
        let expected: Result<u128, dl_sim::error::SimError> =
            dl_core::fixed::mul_div_floor(y, dx, x + dx).map_err(Into::into);
        prop_assert!(actual == expected);
    }
}

/// `amount_in == 0` always returns `Ok(0)` regardless of reserves/fee.
#[test]
fn zero_input_is_zero_output() {
    let dy = fill_constant_product(1_000_000, 1_000_000, 25, 0).unwrap();
    assert_eq!(dy, 0);
    let dy = fill_constant_product(1_000_000, 1_000_000, 0, 0).unwrap();
    assert_eq!(dy, 0);
}

/// `reserve_in == 0` and `reserve_out == 0` return `SimError::ZeroReserve`.
#[test]
fn zero_reserves_return_zero_reserve_error() {
    let err = fill_constant_product(0, 1_000_000, 25, 100).unwrap_err();
    assert!(matches!(err, SimError::ZeroReserve));
    let err = fill_constant_product(1_000_000, 0, 25, 100).unwrap_err();
    assert!(matches!(err, SimError::ZeroReserve));
}

/// `fee_bps == 10_000` returns `SimError::FeeTooHigh(10_000)`. (Any
/// `fee_bps >= 10_000` is rejected; the boundary is checked.)
#[test]
fn fee_bps_at_or_above_10_000_errors() {
    let err = fill_constant_product(1_000_000, 1_000_000, 10_000, 100).unwrap_err();
    assert!(matches!(err, SimError::FeeTooHigh(10_000)));
}
