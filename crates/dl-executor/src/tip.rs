//! Tip lamports calculation (08-01 / AC-6).
//!
//! ## Algorithm
//!
//! For a profitable opportunity with `net_pnl_lamports` (signed):
//!
//! 1. `gross_tip = max(min_tip, net_pnl_lamports × bps / 10_000)`.
//!    For a losing opportunity (net_pnl <= 0), tip = `min_tip` only.
//! 2. `jito_fee = gross_tip × 5 / 100` (the 5% Jito takes).
//! 3. `total_cost = gross_tip + jito_fee`. This is what the user pays
//!    per landed bundle.
//!
//! The function returns `gross_tip` (the tip transaction amount); the
//! caller is responsible for adding the 5% Jito fee to the user's
//! expected cost elsewhere (it's already in the Phase-4 cost stack).

use crate::error::ExecutorError;

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct TipConfig {
    /// Basis points of net PnL the user bids. Default 50 (= 0.5%).
    pub bps: u16,
    /// Minimum tip in lamports. Default 10_000 (the Jito minimum + margin).
    pub min_lamports: u64,
}

impl Default for TipConfig {
    fn default() -> Self {
        Self {
            bps: 50,
            min_lamports: 10_000,
        }
    }
}

/// Calculate the tip lamports for one opportunity.
///
/// - `net_pnl_lamports`: the net PnL of the cycle in lamports (signed).
///   Positive = profitable; negative or zero = unprofitable.
/// - `cfg`: the tip configuration.
///
/// Returns: the tip to bid in lamports.
pub fn tip_lamports(net_pnl_lamports: i128, cfg: &TipConfig) -> u64 {
    if cfg.min_lamports == 0 {
        return 0;
    }
    if net_pnl_lamports > 0 && cfg.bps > 0 {
        // `net × bps / 10_000`, floored at `min_lamports`. The
        // saturating math is defensive; the realistic upper bound
        // is on the order of 1e15 lamports (1e9 SOL), well below
        // i128::MAX.
        let raw = (net_pnl_lamports as u128).saturating_mul(cfg.bps as u128) / 10_000;
        let raw_u64 = raw.min(u64::MAX as u128) as u64;
        raw_u64.max(cfg.min_lamports)
    } else {
        cfg.min_lamports
    }
}

/// Validate that a tip config is internally consistent. Used at
/// boot time to fail fast on bad config.
pub fn validate_config(cfg: &TipConfig) -> Result<(), ExecutorError> {
    if cfg.bps > 10_000 {
        return Err(ExecutorError::TipConfig(format!(
            "bps must be <= 10_000, got {}",
            cfg.bps
        )));
    }
    if cfg.min_lamports > 0 && cfg.min_lamports < 1_000 {
        return Err(ExecutorError::TipConfig(format!(
            "min_lamports must be >= 1000 (Jito minimum), got {}",
            cfg.min_lamports
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profitable_opportunity_uses_bps() {
        let cfg = TipConfig {
            bps: 50,
            min_lamports: 10_000,
        };
        // net = 0.1 SOL = 100_000_000 lamports
        // 100_000_000 × 50 / 10_000 = 500_000 lamports
        // max(500_000, 10_000) = 500_000
        assert_eq!(tip_lamports(100_000_000, &cfg), 500_000);
    }

    #[test]
    fn small_profit_uses_min_lamports() {
        let cfg = TipConfig {
            bps: 50,
            min_lamports: 10_000,
        };
        // net = 0.0001 SOL = 100_000 lamports
        // 100_000 × 50 / 10_000 = 500 lamports
        // max(500, 10_000) = 10_000 (the minimum)
        assert_eq!(tip_lamports(100_000, &cfg), 10_000);
    }

    #[test]
    fn losing_opportunity_uses_min_lamports() {
        let cfg = TipConfig {
            bps: 50,
            min_lamports: 10_000,
        };
        // net = -0.01 SOL (a loss). We still tip the minimum to bid
        // for the bundle, but this opportunity should have been
        // rejected by the conservative bound before getting here.
        assert_eq!(tip_lamports(-1_000_000, &cfg), 10_000);
    }

    #[test]
    fn zero_profit_uses_min_lamports() {
        let cfg = TipConfig {
            bps: 50,
            min_lamports: 10_000,
        };
        assert_eq!(tip_lamports(0, &cfg), 10_000);
    }

    #[test]
    fn zero_min_lamports_returns_zero() {
        // Bypass: min_lamports = 0 means "don't tip at all"
        // (paper mode; never used in production).
        let cfg = TipConfig {
            bps: 50,
            min_lamports: 0,
        };
        assert_eq!(tip_lamports(100_000_000, &cfg), 0);
    }

    #[test]
    fn bps_max_100_pct_profitable() {
        let cfg = TipConfig {
            bps: 10_000, // 100%
            min_lamports: 1_000,
        };
        // 1 SOL = 1e9 lamports × 10000 / 10000 = 1e9 lamports (the
        // entire profit). Floor at min_lamports.
        assert_eq!(tip_lamports(1_000_000_000, &cfg), 1_000_000_000);
    }

    #[test]
    fn validate_rejects_invalid_bps() {
        let err = validate_config(&TipConfig {
            bps: 10_001,
            min_lamports: 10_000,
        })
        .unwrap_err();
        assert!(matches!(err, ExecutorError::TipConfig(_)));
    }

    #[test]
    fn validate_rejects_tiny_min_lamports() {
        let err = validate_config(&TipConfig {
            bps: 50,
            min_lamports: 999,
        })
        .unwrap_err();
        assert!(matches!(err, ExecutorError::TipConfig(_)));
    }

    #[test]
    fn validate_accepts_valid_config() {
        validate_config(&TipConfig::default()).unwrap();
    }
}
