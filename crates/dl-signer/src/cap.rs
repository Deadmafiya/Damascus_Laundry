//! Daily + per-bundle SOL cap.
//!
//! The cap is the *primary* security control in the hot-wallet model
//! (per `docs/v1.1.md` §5.1). It limits the worst-case loss to one day's
//! cap (default 5 SOL) even if the host is compromised.

use chrono::{Datelike, NaiveDate, Utc};

use crate::error::SignerError;

/// Configuration for the cap.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CapConfig {
    /// Max lamports per day (default 5_000_000_000 = 5 SOL).
    pub daily_lamports: u64,
    /// Max lamports per single bundle (default 500_000_000 = 0.5 SOL).
    pub per_bundle_lamports: u64,
}

impl Default for CapConfig {
    fn default() -> Self {
        Self {
            daily_lamports: 5_000_000_000,
            per_bundle_lamports: 500_000_000,
        }
    }
}

/// Cap state, updated on every successful signature. Resets at UTC
/// midnight.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CapState {
    spent_today: u64,
    last_reset: NaiveDate,
    cfg: CapConfig,
}

/// What we tell the operator when a cap blocks a signature.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CapError {
    DailyExceeded { attempted: u64, remaining: u64 },
    PerBundleExceeded { attempted: u64, limit: u64 },
}

impl CapState {
    /// Create a new CapState with zero spent today, current UTC date.
    pub fn new(cfg: CapConfig) -> Self {
        Self {
            spent_today: 0,
            last_reset: Utc::now().date_naive(),
            cfg,
        }
    }

    /// Try to charge `lamports` against the cap. On success, returns
    /// the new state (with `spent_today` updated). On failure,
    /// returns the *original* state (unchanged) plus a [`CapError`].
    pub fn try_charge(&mut self, lamports: u64) -> Result<(), CapError> {
        // Reset at UTC midnight.
        self.reset_if_new_day();

        if lamports > self.cfg.per_bundle_lamports {
            return Err(CapError::PerBundleExceeded {
                attempted: lamports,
                limit: self.cfg.per_bundle_lamports,
            });
        }
        let remaining = self.cfg.daily_lamports.saturating_sub(self.spent_today);
        if lamports > remaining {
            return Err(CapError::DailyExceeded {
                attempted: lamports,
                remaining,
            });
        }
        self.spent_today = self.spent_today.saturating_add(lamports);
        Ok(())
    }

    /// Reset the cap at UTC midnight (called automatically on every
    /// `try_charge`).
    pub fn reset_if_new_day(&mut self) {
        let today = Utc::now().date_naive();
        if today != self.last_reset {
            self.spent_today = 0;
            self.last_reset = today;
        }
    }

    /// Lamports remaining in today's cap.
    pub fn remaining(&self) -> u64 {
        self.cfg.daily_lamports.saturating_sub(self.spent_today)
    }

    /// Lamports spent today (after the last reset).
    pub fn spent_today(&self) -> u64 {
        self.spent_today
    }

    /// The current cap configuration.
    pub fn config(&self) -> CapConfig {
        self.cfg
    }
}

impl std::fmt::Display for CapError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CapError::DailyExceeded {
                attempted,
                remaining,
            } => write!(
                f,
                "daily cap exceeded: would charge {attempted} lamports, only {remaining} remaining"
            ),
            CapError::PerBundleExceeded { attempted, limit } => {
                write!(f, "per-bundle cap exceeded: {attempted} > {limit} lamports")
            }
        }
    }
}

impl From<CapError> for SignerError {
    fn from(e: CapError) -> Self {
        match e {
            CapError::DailyExceeded {
                attempted,
                remaining,
            } => SignerError::DailyCapExceeded {
                attempted,
                remaining,
            },
            CapError::PerBundleExceeded { attempted, limit } => {
                SignerError::PerBundleCapExceeded { attempted, limit }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(daily: u64, per_bundle: u64) -> CapConfig {
        CapConfig {
            daily_lamports: daily,
            per_bundle_lamports: per_bundle,
        }
    }

    #[test]
    fn first_charge_within_caps_succeeds() {
        let mut s = CapState::new(cfg(5_000_000_000, 500_000_000));
        s.try_charge(100_000_000).unwrap();
        assert_eq!(s.spent_today(), 100_000_000);
        assert_eq!(s.remaining(), 4_900_000_000);
    }

    #[test]
    fn per_bundle_cap_enforced() {
        let mut s = CapState::new(cfg(5_000_000_000, 500_000_000));
        let err = s.try_charge(600_000_000).unwrap_err();
        assert!(matches!(err, CapError::PerBundleExceeded { .. }));
        // State unchanged.
        assert_eq!(s.spent_today(), 0);
    }

    #[test]
    fn daily_cap_enforced() {
        let mut s = CapState::new(cfg(1_000_000_000, 500_000_000));
        s.try_charge(500_000_000).unwrap();
        s.try_charge(400_000_000).unwrap();
        // Third bundle of 400M would exceed (500M+400M+400M = 1.3B > 1B).
        let err = s.try_charge(400_000_000).unwrap_err();
        assert!(matches!(err, CapError::DailyExceeded { .. }));
        // Only the first two counted.
        assert_eq!(s.spent_today(), 900_000_000);
    }

    #[test]
    fn exactly_at_cap_succeeds() {
        let mut s = CapState::new(cfg(500_000_000, 500_000_000));
        s.try_charge(500_000_000).unwrap();
        assert_eq!(s.remaining(), 0);
        // Next bundle (any size) is over.
        let err = s.try_charge(1).unwrap_err();
        assert!(matches!(err, CapError::DailyExceeded { .. }));
    }

    #[test]
    fn reset_rolls_over() {
        let mut s = CapState::new(cfg(1_000_000_000, 500_000_000));
        s.try_charge(500_000_000).unwrap();
        // Simulate tomorrow by manually setting last_reset to yesterday.
        s.last_reset = s.last_reset.pred_opt().unwrap();
        // First call triggers reset_if_new_day; next charge should fit.
        s.try_charge(500_000_000).unwrap();
        assert_eq!(s.spent_today(), 500_000_000);
    }
}
