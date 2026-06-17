//! Fixed-point, overflow-checked integer math for the value path.
//!
//! Everything here operates on `u128`. The key primitive is [`mul_div_floor`], which
//! computes `floor(value * numerator / denominator)` *without* intermediate overflow even
//! when `value * numerator` exceeds `u128::MAX`, by using a 256-bit intermediate. This is
//! the single primitive used to apply fee fractions and price ratios.
//!
//! No `f32`/`f64` appears in this module.

use core::fmt;

const MASK64: u128 = 0xFFFF_FFFF_FFFF_FFFF;

/// Errors from value-path arithmetic. Operations return these instead of panicking or
/// wrapping silently.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MathError {
    /// A result exceeded the representable range.
    Overflow,
    /// Division (or `mul_div`) by zero.
    DivByZero,
    /// A rescale/normalization would lose value (not exactly divisible).
    ScaleMismatch,
}

impl fmt::Display for MathError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MathError::Overflow => write!(f, "arithmetic overflow"),
            MathError::DivByZero => write!(f, "division by zero"),
            MathError::ScaleMismatch => write!(f, "scale mismatch (lossy rescale)"),
        }
    }
}

impl std::error::Error for MathError {}

/// Checked addition. `Err(Overflow)` instead of wrapping.
#[inline]
pub fn checked_add(a: u128, b: u128) -> Result<u128, MathError> {
    a.checked_add(b).ok_or(MathError::Overflow)
}

/// Checked subtraction. `Err(Overflow)` on underflow.
#[inline]
pub fn checked_sub(a: u128, b: u128) -> Result<u128, MathError> {
    a.checked_sub(b).ok_or(MathError::Overflow)
}

/// `floor(value * numerator / denominator)` with a 256-bit intermediate.
///
/// Never panics. Returns:
/// - `Err(DivByZero)` if `denominator == 0`
/// - `Err(Overflow)` if the mathematically-correct quotient does not fit in `u128`
/// - `Ok(q)` otherwise, where `q` is the exact floored quotient
#[inline]
pub fn mul_div_floor(value: u128, numerator: u128, denominator: u128) -> Result<u128, MathError> {
    if denominator == 0 {
        return Err(MathError::DivByZero);
    }
    // Fast path: product fits in u128.
    if let Some(p) = value.checked_mul(numerator) {
        return Ok(p / denominator);
    }
    // Wide path: 256-bit product, then 256 / 128 division.
    let (hi, lo) = full_mul(value, numerator);
    div_256_by_128(hi, lo, denominator).ok_or(MathError::Overflow)
}

/// 10^exp as `u128`, or `Err(Overflow)` if it does not fit.
pub fn pow10(exp: u32) -> Result<u128, MathError> {
    10u128.checked_pow(exp).ok_or(MathError::Overflow)
}

/// Full 128x128 -> 256 bit multiply, returning `(hi, lo)`.
fn full_mul(a: u128, b: u128) -> (u128, u128) {
    let a_lo = a & MASK64;
    let a_hi = a >> 64;
    let b_lo = b & MASK64;
    let b_hi = b >> 64;

    let ll = a_lo * b_lo;
    let lh = a_lo * b_hi;
    let hl = a_hi * b_lo;
    let hh = a_hi * b_hi;

    // mid accumulates the overlapping 64-bit column; it can be up to ~2^66, fits u128.
    let mid = (ll >> 64) + (lh & MASK64) + (hl & MASK64);

    let lo = (ll & MASK64) | ((mid & MASK64) << 64);
    let hi = hh + (lh >> 64) + (hl >> 64) + (mid >> 64);
    (hi, lo)
}

/// Divide the 256-bit value `(hi, lo)` by `d`, returning the quotient if it fits in `u128`.
///
/// Returns `None` if `d == 0` or if the quotient would overflow `u128` (i.e. `hi >= d`).
/// Bitwise long division; maintains the invariant `rem < d`.
fn div_256_by_128(hi: u128, lo: u128, d: u128) -> Option<u128> {
    if d == 0 {
        return None;
    }
    // hi >= d would make the quotient >= 2^128 — does not fit u128.
    if hi >= d {
        return None;
    }
    let mut rem: u128 = hi;
    let mut quo: u128 = 0;
    let mut i: i32 = 127;
    while i >= 0 {
        let bit = (lo >> i as u32) & 1;
        let carry = rem >> 127; // top bit that would be lost by the shift
        let shifted = (rem << 1) | bit; // low 128 bits of (rem*2 + bit)
                                        // True value is carry*2^128 + shifted; it is >= d iff carry==1 or shifted >= d.
        if carry == 1 || shifted >= d {
            rem = shifted.wrapping_sub(d); // exact for the low 128 bits in both cases
            quo |= 1u128 << i as u32;
        } else {
            rem = shifted;
        }
        i -= 1;
    }
    Some(quo)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fast_path_matches_native() {
        assert_eq!(mul_div_floor(100, 3, 7), Ok(100 * 3 / 7));
        assert_eq!(mul_div_floor(0, 5, 9), Ok(0));
    }

    #[test]
    fn div_by_zero() {
        assert_eq!(mul_div_floor(10, 10, 0), Err(MathError::DivByZero));
    }

    #[test]
    fn wide_path_no_overflow() {
        // value * numerator overflows u128, but /denominator brings it back in range.
        let big = u128::MAX;
        // big * 4 / 2 = big * 2 -> overflows u128 -> Overflow.
        assert_eq!(mul_div_floor(big, 4, 2), Err(MathError::Overflow));
        // big * 4 / 8 = big / 2 -> fits.
        assert_eq!(mul_div_floor(big, 4, 8), Ok(big / 2));
    }

    #[test]
    fn wide_path_exact() {
        // (2^100) * (2^100) / (2^100) = 2^100, exact through the wide path.
        let x = 1u128 << 100;
        assert_eq!(mul_div_floor(x, x, x), Ok(x));
    }
}
