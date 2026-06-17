//! Display-only conversions. **This is the only module permitted to use floating point.**
//!
//! Nothing here is used in the value/balance/PnL path — these helpers exist purely to
//! render human-readable approximations (logs, dashboards). The value path uses
//! [`crate::amount::Amount`]'s exact integer [`core::fmt::Display`] instead.

use crate::amount::Amount;

/// Lossy `f64` approximation of an amount, for display/metrics only. Never use in the
/// value path — float rounding here is acceptable because the result is never fed back
/// into balance or PnL arithmetic.
pub fn amount_to_f64(amount: &Amount) -> f64 {
    let divisor = 10f64.powi(amount.decimals() as i32);
    amount.raw() as f64 / divisor
}
