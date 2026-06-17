//! Token amounts in smallest base units, backed by `u128`.
//!
//! An [`Amount`] carries its token's `decimals` so it can be normalized to a common
//! internal scale and back losslessly. All arithmetic is overflow-checked via [`crate::fixed`].

use crate::fixed::{self, MathError};
use core::fmt;

/// A token quantity expressed in the token's smallest unit (base units), with the token's
/// `decimals`. For example, 1.5 USDC (6 decimals) is `Amount { raw: 1_500_000, decimals: 6 }`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Amount {
    raw: u128,
    decimals: u8,
}

impl Amount {
    /// Construct from raw base units and the token's decimals.
    #[inline]
    pub const fn from_base_units(raw: u128, decimals: u8) -> Self {
        Self { raw, decimals }
    }

    /// Zero at the given decimals.
    #[inline]
    pub const fn zero(decimals: u8) -> Self {
        Self { raw: 0, decimals }
    }

    /// Raw value in base units.
    #[inline]
    pub const fn raw(&self) -> u128 {
        self.raw
    }

    /// The token's decimals.
    #[inline]
    pub const fn decimals(&self) -> u8 {
        self.decimals
    }

    /// Checked add of two amounts of the *same* token (same decimals).
    pub fn checked_add(self, other: Amount) -> Result<Amount, MathError> {
        if self.decimals != other.decimals {
            return Err(MathError::ScaleMismatch);
        }
        Ok(Amount {
            raw: fixed::checked_add(self.raw, other.raw)?,
            decimals: self.decimals,
        })
    }

    /// Checked sub of two amounts of the *same* token (same decimals).
    pub fn checked_sub(self, other: Amount) -> Result<Amount, MathError> {
        if self.decimals != other.decimals {
            return Err(MathError::ScaleMismatch);
        }
        Ok(Amount {
            raw: fixed::checked_sub(self.raw, other.raw)?,
            decimals: self.decimals,
        })
    }

    /// Apply a ratio `numerator/denominator` (floored), preserving decimals. The single
    /// primitive for fee fractions and price ratios on an amount.
    pub fn mul_div(self, numerator: u128, denominator: u128) -> Result<Amount, MathError> {
        Ok(Amount {
            raw: fixed::mul_div_floor(self.raw, numerator, denominator)?,
            decimals: self.decimals,
        })
    }

    /// Normalize to a common internal scale of `target_decimals` base units.
    ///
    /// Scaling *up* (more decimals) is always exact. Scaling *down* (fewer decimals)
    /// returns `Err(ScaleMismatch)` if it would discard nonzero low-order digits, so the
    /// conversion is lossless or it fails loudly.
    pub fn to_scale(self, target_decimals: u8) -> Result<u128, MathError> {
        match target_decimals.cmp(&self.decimals) {
            core::cmp::Ordering::Equal => Ok(self.raw),
            core::cmp::Ordering::Greater => {
                let factor = fixed::pow10((target_decimals - self.decimals) as u32)?;
                self.raw.checked_mul(factor).ok_or(MathError::Overflow)
            }
            core::cmp::Ordering::Less => {
                let factor = fixed::pow10((self.decimals - target_decimals) as u32)?;
                if !self.raw.is_multiple_of(factor) {
                    return Err(MathError::ScaleMismatch);
                }
                Ok(self.raw / factor)
            }
        }
    }

    /// Build an `Amount` (at `decimals`) from a value expressed at `source_decimals`.
    /// Inverse of [`Amount::to_scale`]; lossless or `Err(ScaleMismatch)`.
    pub fn from_scale(value: u128, source_decimals: u8, decimals: u8) -> Result<Amount, MathError> {
        let probe = Amount {
            raw: value,
            decimals: source_decimals,
        };
        Ok(Amount {
            raw: probe.to_scale(decimals)?,
            decimals,
        })
    }
}

impl fmt::Display for Amount {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Render exactly from integer digits — no float involved.
        let d = self.decimals as usize;
        if d == 0 {
            return write!(f, "{}", self.raw);
        }
        let s = self.raw.to_string();
        if s.len() <= d {
            let zeros = d - s.len();
            write!(f, "0.{}{}", "0".repeat(zeros), s)
        } else {
            let (int_part, frac_part) = s.split_at(s.len() - d);
            write!(f, "{}.{}", int_part, frac_part)
        }
    }
}
