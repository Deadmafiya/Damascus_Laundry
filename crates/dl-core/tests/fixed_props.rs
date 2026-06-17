//! Property tests for the fixed-point value-path math (AC-2).

use dl_core::amount::Amount;
use dl_core::fixed::{mul_div_floor, MathError};
use proptest::prelude::*;

proptest! {
    // (a) mul_div_floor never panics on random u128 inputs: it returns Ok or a typed Err.
    #[test]
    fn mul_div_never_panics(value in any::<u128>(), num in any::<u128>(), den in any::<u128>()) {
        let r = mul_div_floor(value, num, den);
        match r {
            Ok(_) => {}
            Err(MathError::DivByZero) => prop_assert_eq!(den, 0),
            Err(MathError::Overflow) => prop_assert!(den != 0),
            Err(MathError::ScaleMismatch) => prop_assert!(false, "mul_div never returns ScaleMismatch"),
        }
    }

    // mul_div_floor agrees with the exact 256-bit math: q*den <= value*num < (q+1)*den,
    // checked via the wide remainder. We verify the fast-path cases against native u128.
    #[test]
    fn mul_div_matches_native_when_product_fits(
        value in 0u128..=u64::MAX as u128,
        num in 0u128..=u64::MAX as u128,
        den in 1u128..=u64::MAX as u128,
    ) {
        // value*num fits in u128 because both are <= 2^64-1.
        let expected = value.checked_mul(num).unwrap() / den;
        prop_assert_eq!(mul_div_floor(value, num, den).unwrap(), expected);
    }

    // (b) decimals round-trip is the identity for representable amounts.
    // Scale up to a higher decimal count then back down — must recover the original.
    #[test]
    fn decimals_roundtrip_is_identity(
        raw in 0u128..=u64::MAX as u128,
        decimals in 0u8..=12,
        bump in 0u8..=6,
    ) {
        let target = decimals + bump;
        let a = Amount::from_base_units(raw, decimals);
        // Scaling up is always exact.
        let up = a.to_scale(target).unwrap();
        // Scaling back down must be lossless and recover the original raw.
        let back = Amount::from_scale(up, target, decimals).unwrap();
        prop_assert_eq!(back.raw(), raw);
        prop_assert_eq!(back.decimals(), decimals);
    }

    // (c) monotonicity: increasing the numerator of mul_div never decreases the result.
    #[test]
    fn mul_div_monotonic_in_numerator(
        value in 0u128..=u64::MAX as u128,
        num in 0u128..=u64::MAX as u128,
        delta in 0u128..=u64::MAX as u128,
        den in 1u128..=u64::MAX as u128,
    ) {
        let lo = mul_div_floor(value, num, den).unwrap();
        let hi = mul_div_floor(value, num.saturating_add(delta), den).unwrap();
        prop_assert!(hi >= lo);
    }

    // (d) add/sub inverse on non-overflowing pairs (same token).
    #[test]
    fn add_sub_inverse(
        a in 0u128..=u64::MAX as u128,
        b in 0u128..=u64::MAX as u128,
        decimals in 0u8..=12,
    ) {
        let x = Amount::from_base_units(a, decimals);
        let y = Amount::from_base_units(b, decimals);
        let sum = x.checked_add(y).unwrap();
        let back = sum.checked_sub(y).unwrap();
        prop_assert_eq!(back, x);
    }

    // Scaling down that discards nonzero low digits must fail loudly, not silently round.
    #[test]
    fn lossy_downscale_errs(
        units in 1u128..=999u128,
        decimals in 3u8..=9,
    ) {
        // `units` (< 1000) at `decimals` has nonzero low digits; dropping 3 decimals loses them.
        prop_assume!(!units.is_multiple_of(1000));
        let a = Amount::from_base_units(units, decimals);
        prop_assert_eq!(a.to_scale(decimals - 3), Err(MathError::ScaleMismatch));
    }
}
