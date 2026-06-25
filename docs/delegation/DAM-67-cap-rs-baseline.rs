//! Daily + per-bundle SOL cap.
//!
//! The cap is the *primary* security control in the hot-wallet model
//! (per `docs/v1.1.md` §5.1). It limits the worst-case loss to one day's
//! cap (default 5 SOL) even if the host is compromised.

use chrono::{Datelike, NaiveDate, Utc};

use crate::error::SignerError;

/// Pin for the mainnet daily cap floor (DAM-61 / Phase 1d):
/// 0.5 SOL = 500_000_000 lamports. The `mainnet_tiny_floors` test
/// asserts this value. Raising the floor requires changing the
/// source and re-building; that is deliberate.
pub const MAINNET_DAILY_CAP_FLOOR_LAMPORTS: u64 = 500_000_000;

/// Pin for the mainnet per-bundle cap floor: 0.05 SOL =
/// 50_000_000 lamports. The `mainnet_tiny_floors` test asserts
/// this value.
pub const MAINNET_PER_BUNDLE_CAP_FLOOR_LAMPORTS: u64 = 50_000_000;

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

impl CapConfig {
    /// The mainnet tiny floors (DAM-61 / Phase 1d): 0.5 SOL/day,
    /// 0.05 SOL/bundle. These are the production safety floors
    /// for the live `DL_LIVE_MODE=mainnet` mode. They cannot be
    /// raised via env var; only lowered.
    pub const fn mainnet_floors() -> Self {
        Self {
            daily_lamports: MAINNET_DAILY_CAP_FLOOR_LAMPORTS,
            per_bundle_lamports: MAINNET_PER_BUNDLE_CAP_FLOOR_LAMPORTS,
        }
    }

    /// Apply a candidate override (e.g. from `DL_DAILY_CAP_LAMPORTS`)
    /// but clamp both the daily cap and the per-bundle cap at the
    /// mainnet floors (DAM-61). The override can *lower* the cap,
    /// but cannot *raise* it above 0.5 SOL/day or 0.05 SOL/bundle.
    pub fn with_mainnet_floor(
        daily_override: Option<u64>,
        per_bundle_override: Option<u64>,
    ) -> Self {
        let floors = Self::mainnet_floors();
        Self {
            daily_lamports: daily_override
                .map(|v| v.min(floors.daily_lamports))
                .unwrap_or(floors.daily_lamports),
            per_bundle_lamports: per_bundle_override
                .map(|v| v.min(floors.per_bundle_lamports))
                .unwrap_or(floors.per_bundle_lamports),
        }
    }
}

/// Cap state, updated on every successful signature. Resets at UTC
/// midnight.
pub struct CapState {
    spent_today: u64,
    last_reset: NaiveDate,
    cfg: CapConfig,
    clock: Box<dyn Clock>,
}

/// A clock the cap can use to decide when "today" rolls over. The
/// default implementation reads `Utc::now()`; tests inject a fake
/// clock to exercise the daily-reset path without waiting until
/// midnight.
pub trait Clock: Send + Sync {
    fn now_date(&self) -> NaiveDate;
}

/// Real wall clock. Default for production.
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now_date(&self) -> NaiveDate {
        Utc::now().date_naive()
    }
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
        Self::with_clock(cfg, Box::new(SystemClock))
    }

    /// Create a CapState with an injected clock. Used by tests to
    /// fake the passage of midnight without sleeping. Production
    /// code should use `new`; the clock parameter is for tests.
    pub fn with_clock(cfg: CapConfig, clock: Box<dyn Clock>) -> Self {
        let last_reset = clock.now_date();
        Self {
            spent_today: 0,
            last_reset,
            cfg,
            clock,
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
        let today = self.clock.now_date();
        if today != self.last_reset {
            self.spent_today = 0;
            self.last_reset = today;
        }
    }

    /// Refund a previously charged amount. Used when a bundle is
    /// rejected by a downstream safety gate (kill switch, rate
    /// limit, simulate gate, sign failure) AFTER the cap was
    /// charged — the bundle never actually paid a tip, so the
    /// cap should not count it. Saturates at 0 to prevent
    /// underflow (refunding more than was spent is a no-op).
    pub fn refund(&mut self, lamports: u64) {
        self.spent_today = self.spent_today.saturating_sub(lamports);
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

    /// Pinned mainnet cap floors (DAM-61 / Phase 1d). The exact
    /// values here are the spec: 0.5 SOL/day and 0.05 SOL/bundle.
    /// Changing these constants is an explicit, review-gated
    /// decision; the assertion below will fail loudly if a
    /// regression moves them.
    #[test]
    fn mainnet_tiny_floors() {
        let c = CapConfig::mainnet_floors();
        assert_eq!(
            c.daily_lamports, 500_000_000,
            "mainnet daily cap must be exactly 0.5 SOL"
        );
        assert_eq!(
            c.per_bundle_lamports, 50_000_000,
            "mainnet per-bundle cap must be exactly 0.05 SOL"
        );
        // Also assert the public pins, in case the constructor
        // expression above is later refactored to compose other
        // constants.
        assert_eq!(MAINNET_DAILY_CAP_FLOOR_LAMPORTS, 500_000_000);
        assert_eq!(MAINNET_PER_BUNDLE_CAP_FLOOR_LAMPORTS, 50_000_000);
    }

    #[test]
    fn mainnet_floor_caps_env_override_above_floor() {
        // The whole point of the floor: even if the operator (or
        // a misconfigured deploy) sets the env var to a value ABOVE
        // 0.5 SOL, the cap must clamp down to the floor.
        let c = CapConfig::with_mainnet_floor(Some(5_000_000_000), Some(500_000_000));
        assert_eq!(c.daily_lamports, 500_000_000);
        assert_eq!(c.per_bundle_lamports, 50_000_000);
    }

    #[test]
    fn mainnet_floor_honors_override_below_floor() {
        // Operators may tighten the cap below the floor (e.g. for
        // an even smaller pilot). The override is honored.
        let c = CapConfig::with_mainnet_floor(Some(100_000_000), Some(10_000_000));
        assert_eq!(c.daily_lamports, 100_000_000);
        assert_eq!(c.per_bundle_lamports, 10_000_000);
    }

    #[test]
    fn mainnet_floor_none_uses_floor_value() {
        let c = CapConfig::with_mainnet_floor(None, None);
        assert_eq!(c, CapConfig::mainnet_floors());
    }
}
