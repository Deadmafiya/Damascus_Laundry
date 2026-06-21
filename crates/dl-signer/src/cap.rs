//! Daily + per-bundle SOL cap.
//!
//! The cap is the *primary* security control in the hot-wallet model
//! (per `docs/v1.1.md` §5.1). It limits the worst-case loss to one day's
//! cap (default 5 SOL) even if the host is compromised.
//!
//! ## Mainnet-paper floor (DAM-58 / Phase 1c)
//!
//! The `mainnet-paper` live mode is the **dry-run path against
//! mainnet infrastructure** — real Jito Block Engine, real
//! Jupiter, real wallet, but capped at 0.001 SOL/day. The whole
//! point of the mode is to prove the wallet / signing / tip
//! flow works without risking a real loss. A misconfiguration
//! that allowed the env var `DL_DAILY_CAP_LAMPORTS` to raise
//! the cap above 0.001 SOL would defeat the mode.
//!
//! The `mainnet_paper_floor` test pins the value at
//! `MAINNET_PAPER_DAILY_CAP_FLOOR_LAMPORTS` and pins the
//! behavior of `with_mainnet_paper_floor` (clamp above, honor
//! below). The end-to-end `ResolvedLiveMode::from_env` test
//! asserts that the env var is ignored in `mainnet-paper` mode.

use chrono::{Datelike, NaiveDate, Utc};

use crate::error::SignerError;

/// Pin for the mainnet-paper daily cap floor (DAM-58 / Phase 1c):
/// 0.001 SOL = 1_000_000 lamports. The `mainnet_paper_floor` test
/// asserts this value. The mainnet-paper mode is the dry-run
/// path against mainnet infrastructure — real Jito, real Jupiter,
/// real wallet, but capped at 0.001 SOL/day to bound worst-case
/// loss. A misconfiguration that allowed the cap to be raised
/// above this floor would defeat the mode.
///
/// The same value is mirrored in `livemode.rs` as
/// `LiveMode::MAINNET_PAPER_DAILY_CAP_LAMPORTS` (the canonical
/// mode-level pin). Both definitions must stay in lock-step —
/// `mainnet_paper_floor` below asserts equality.
pub const MAINNET_PAPER_DAILY_CAP_FLOOR_LAMPORTS: u64 = 1_000_000;

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
    /// The mainnet-paper daily floor (DAM-58 / Phase 1c):
    /// 0.001 SOL/day. The per-bundle ceiling inherits the standard
    /// 0.5 SOL default — a single tip in mainnet-paper must still
    /// fit the standard per-bundle cap; the daily cap is the
    /// strict control surface.
    ///
    /// This is a *separate* floor from any mainnet (production)
    /// floor. The mainnet (production) cap and the mainnet-paper
    /// cap are different control surfaces; do not conflate them.
    pub const fn mainnet_paper_floor() -> Self {
        Self {
            daily_lamports: MAINNET_PAPER_DAILY_CAP_FLOOR_LAMPORTS,
            per_bundle_lamports: 500_000_000,
        }
    }

    /// Apply a candidate override (e.g. from `DL_DAILY_CAP_LAMPORTS`)
    /// but clamp the daily cap at the mainnet-paper floor
    /// (DAM-58). The override can *lower* the daily cap, but
    /// cannot *raise* it above 0.001 SOL — that is the spec.
    /// The per-bundle ceiling honors overrides normally (a
    /// smaller per-bundle cap is always safe).
    pub fn with_mainnet_paper_floor(
        daily_override: Option<u64>,
        per_bundle_override: Option<u64>,
    ) -> Self {
        let floor = Self::mainnet_paper_floor();
        Self {
            daily_lamports: daily_override
                .map(|v| v.min(floor.daily_lamports))
                .unwrap_or(floor.daily_lamports),
            per_bundle_lamports: per_bundle_override.unwrap_or(floor.per_bundle_lamports),
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

    /// DAM-58 / Phase 1c. Pinned mainnet-paper cap floor.
    ///
    /// The `mainnet-paper` live mode is the **dry-run path against
    /// mainnet infrastructure** — real Jito Block Engine, real
    /// Jupiter, real wallet, but capped at 0.001 SOL/day. The whole
    /// point of the mode is to prove the wallet / signing / tip
    /// flow works without risking a real loss. A misconfiguration
    /// that allowed the env var `DL_DAILY_CAP_LAMPORTS` to raise
    /// the cap above 0.001 SOL would defeat the mode.
    ///
    /// This test pins:
    /// 1. The constant `MAINNET_PAPER_DAILY_CAP_FLOOR_LAMPORTS`
    ///    is exactly 1_000_000 (0.001 SOL).
    /// 2. The `with_mainnet_paper_floor` constructor rejects any
    ///    daily override ABOVE the floor (clamps to the floor).
    /// 3. A daily override BELOW the floor is honored (the floor
    ///    is the *upper* bound for the mode).
    /// 4. The end-to-end `ResolvedLiveMode::from_env` integration
    ///    ignores the `DL_DAILY_CAP_LAMPORTS` env var entirely
    ///    when `DL_LIVE_MODE=mainnet-paper` — the floor is
    ///    hard-coded, not configured.
    /// 5. The mainnet (production) mode does NOT use the
    ///    mainnet-paper floor; it uses the larger production
    ///    cap. The two floors must not be conflated.
    #[test]
    fn mainnet_paper_floor() {
        // (1) Pin the constant.
        assert_eq!(
            MAINNET_PAPER_DAILY_CAP_FLOOR_LAMPORTS, 1_000_000,
            "mainnet-paper daily cap floor must be exactly 0.001 SOL"
        );
        // (1') Lock-step with the livemode-level constant.
        assert_eq!(
            crate::livemode::LiveMode::MAINNET_PAPER_DAILY_CAP_LAMPORTS,
            MAINNET_PAPER_DAILY_CAP_FLOOR_LAMPORTS,
            "cap.rs and livemode.rs mainnet-paper floor constants must agree"
        );

        // (1'') Pin the constructor.
        let c = CapConfig::mainnet_paper_floor();
        assert_eq!(c.daily_lamports, 1_000_000);
        assert_eq!(c.per_bundle_lamports, 500_000_000);

        // (2) Override ABOVE the floor → clamped to the floor.
        let c = CapConfig::with_mainnet_paper_floor(Some(5_000_000_000), Some(500_000_000));
        assert_eq!(
            c.daily_lamports, 1_000_000,
            "with_mainnet_paper_floor must clamp daily override above the floor"
        );
        // Per-bundle cap is honored (the per-bundle floor is a
        // different control surface — it belongs to mainnet mode).
        assert_eq!(c.per_bundle_lamports, 500_000_000);

        // (3) Override BELOW the floor → honored (tighter is safe).
        let c = CapConfig::with_mainnet_paper_floor(Some(500_000), None);
        assert_eq!(
            c.daily_lamports, 500_000,
            "with_mainnet_paper_floor must honor a tighter daily override"
        );

        // No override → the floor.
        let c = CapConfig::with_mainnet_paper_floor(None, None);
        assert_eq!(c, CapConfig::mainnet_paper_floor());

        // (4) End-to-end: env var is ignored in mainnet-paper mode.
        // Process-global env vars, so save and restore.
        let saved_mode = std::env::var("DL_LIVE_MODE").ok();
        let saved_daily = std::env::var("DL_DAILY_CAP_LAMPORTS").ok();
        let saved_per_bundle = std::env::var("DL_PER_BUNDLE_CAP_LAMPORTS").ok();
        // Helper that clones its argument so the caller doesn't
        // have to clone at every restore() call site.
        let restore = |name: &str, saved: Option<String>| match saved {
            Some(v) => std::env::set_var(name, v),
            None => std::env::remove_var(name),
        };

        std::env::set_var("DL_LIVE_MODE", "mainnet-paper");
        std::env::set_var("DL_DAILY_CAP_LAMPORTS", "5000000000");
        std::env::set_var("DL_PER_BUNDLE_CAP_LAMPORTS", "500000000");
        let r = crate::livemode::ResolvedLiveMode::from_env().expect("valid mainnet-paper env");
        assert_eq!(r.mode, crate::livemode::LiveMode::MainnetPaper);
        assert_eq!(
            r.daily_cap_lamports, 1_000_000,
            "mainnet-paper mode must ignore DL_DAILY_CAP_LAMPORTS env override"
        );
        // The per-bundle cap is operator-configurable in
        // mainnet-paper mode.
        assert_eq!(r.per_bundle_cap_lamports, 500_000_000);

        // Restore.
        restore("DL_LIVE_MODE", saved_mode.clone());
        restore("DL_DAILY_CAP_LAMPORTS", saved_daily);
        restore("DL_PER_BUNDLE_CAP_LAMPORTS", saved_per_bundle);

        // (5) Sanity: the mainnet (production) mode is a distinct
        // floor, not the paper floor. Without the per-mode
        // separation, an operator who typed the wrong env var
        // would be silently capped at 0.001 SOL. The default
        // mainnet cap is 5 SOL (5_000_000_000), not the paper floor.
        std::env::set_var("DL_LIVE_MODE", "mainnet");
        std::env::remove_var("DL_DAILY_CAP_LAMPORTS");
        let r = crate::livemode::ResolvedLiveMode::from_env().expect("valid mainnet env");
        assert_eq!(r.mode, crate::livemode::LiveMode::Mainnet);
        assert!(
            r.daily_cap_lamports > 1_000_000,
            "mainnet (production) default must be the production cap, not the paper floor"
        );
        // And the paper mode without the env-var-ignore (i.e. when
        // DL_LIVE_MODE=mainnet) is *not* the paper floor.
        assert_ne!(
            r.daily_cap_lamports, 1_000_000,
            "mainnet (production) daily cap must not equal the mainnet-paper floor"
        );
        restore("DL_LIVE_MODE", saved_mode.clone());
    }
}
