//! Daily + per-bundle SOL cap.
//!
//! The cap is the *primary* security control in the hot-wallet model
//! (per `docs/v1.1.md` §5.1). It limits the worst-case loss to one day's
//! cap (default 5 SOL) even if the host is compromised.
//!
//! ## Mainnet tiny floors (DAM-61 / Phase 1d)
//!
//! When `DL_LIVE_MODE=mainnet`, the cap is *floored* at 0.5 SOL/day
//! and 0.05 SOL/bundle regardless of any env-var override. These
//! floors are pinned by `tests::mainnet_tiny_floors` — the assertion
//! is the spec. Raising these floors requires changing the source
//! and re-building; that is deliberate. A hot-wallet security model
//! that allows the cap to be raised via env var is a misconfiguration
//! waiting to happen.
//!
//! ## Crash recovery (Phase 3 / DAM-67)
//!
//! The cap state is persisted to a JSON snapshot file via
//! `CapState::persist`. On startup the process loads the snapshot
//! via `CapState::load` (or `load_or_init` for first-ever boots).
//! This prevents a crash mid-day from resetting the cap to
//! "full daily budget" — the durable file is the source of
//! truth, not process memory. The persistence is atomic (write
//! to temp file, then `rename`) so a crash mid-write leaves the
//! previous snapshot intact.

use chrono::{Datelike, NaiveDate, Utc};
use serde::{Deserialize, Serialize};

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

/// Errors that can occur when loading or persisting a cap snapshot.
#[derive(Debug)]
pub enum CapSnapshotError {
    /// Underlying I/O error. The string is the `std::io::Error` display.
    Io(String),
    /// The snapshot file is malformed: bad JSON, unknown schema
    /// version, or a deserialised value that violates an invariant.
    BadFormat(String),
}

impl std::fmt::Display for CapSnapshotError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CapSnapshotError::Io(s) => write!(f, "cap snapshot I/O error: {s}"),
            CapSnapshotError::BadFormat(s) => write!(f, "cap snapshot bad format: {s}"),
        }
    }
}

impl std::error::Error for CapSnapshotError {}

impl From<std::io::Error> for CapSnapshotError {
    fn from(e: std::io::Error) -> Self {
        CapSnapshotError::Io(e.to_string())
    }
}

impl From<serde_json::Error> for CapSnapshotError {
    fn from(e: serde_json::Error) -> Self {
        CapSnapshotError::BadFormat(e.to_string())
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
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
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CapState {
    spent_today: u64,
    last_reset: NaiveDate,
    cfg: CapConfig,
}

/// What we tell the operator when a cap blocks a signature.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CapError {
    DailyExceeded { attempted: u64, remaining: u64 },
    PerBundleExceeded { attempted: u64, limit: u64 },
}

/// On-disk snapshot of a `CapState`. Mirrors the persistable
/// fields (`spent_today`, `last_reset`, `cfg`) and is what we
/// round-trip through JSON.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
struct CapSnapshot {
    /// Schema version. Bump when the on-disk shape changes in an
    /// incompatible way so we can fail-closed on a stale file.
    version: u32,
    spent_today: u64,
    last_reset: NaiveDate,
    cfg: CapConfig,
}

const CAP_SNAPSHOT_VERSION: u32 = 1;

impl From<CapState> for CapSnapshot {
    fn from(s: CapState) -> Self {
        Self {
            version: CAP_SNAPSHOT_VERSION,
            spent_today: s.spent_today,
            last_reset: s.last_reset,
            cfg: s.cfg,
        }
    }
}

impl From<CapSnapshot> for CapState {
    fn from(s: CapSnapshot) -> Self {
        Self {
            spent_today: s.spent_today,
            last_reset: s.last_reset,
            cfg: s.cfg,
        }
    }
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

    /// Persist the current cap state to `path` atomically. Writes
    /// to a sibling temp file first, then `rename`s over the
    /// target — a crash mid-write leaves the previous snapshot
    /// intact.
    pub fn persist(&self, path: &std::path::Path) -> Result<(), CapSnapshotError> {
        use std::io::Write;
        let snap = CapSnapshot::from(*self);
        let json = serde_json::to_vec_pretty(&snap)?;
        let parent = path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or_else(|| std::path::Path::new("."));
        // Create a unique temp file in the same directory so the
        // final `rename` is on the same filesystem (atomic on
        // POSIX).
        let tmp = parent.join(format!(
            ".{}.{}.tmp",
            path.file_name()
                .and_then(|f| f.to_str())
                .unwrap_or("cap_snapshot"),
            std::process::id()
        ));
        {
            let mut f = std::fs::File::create(&tmp)?;
            f.write_all(&json)?;
            f.sync_all()?;
        }
        // POSIX `rename` is atomic within a filesystem. If the
        // target already exists, it is replaced.
        std::fs::rename(&tmp, path)?;
        Ok(())
    }

    /// Load a cap state from `path`. Returns `BadFormat` if the
    /// file is unparseable, the schema version is unknown, or
    /// the deserialised values violate an invariant. Use
    /// [`CapState::load_or_init`] in the normal startup path —
    /// it handles the "no snapshot yet" case by initialising a
    /// fresh state.
    pub fn load(path: &std::path::Path) -> Result<Self, CapSnapshotError> {
        let bytes = std::fs::read(path)?;
        let snap: CapSnapshot = serde_json::from_slice(&bytes)?;
        if snap.version != CAP_SNAPSHOT_VERSION {
            return Err(CapSnapshotError::BadFormat(format!(
                "unknown snapshot version {} (expected {})",
                snap.version, CAP_SNAPSHOT_VERSION
            )));
        }
        // Defensive bound: a corrupted file must not be coerced
        // into a 10^18-lamport cap.
        if snap.cfg.daily_lamports > 1_000_000_000_000_000 {
            return Err(CapSnapshotError::BadFormat(format!(
                "daily cap {} exceeds sanity bound",
                snap.cfg.daily_lamports
            )));
        }
        if snap.cfg.per_bundle_lamports > snap.cfg.daily_lamports {
            return Err(CapSnapshotError::BadFormat(format!(
                "per-bundle cap {} exceeds daily cap {}",
                snap.cfg.per_bundle_lamports, snap.cfg.daily_lamports
            )));
        }
        Ok(snap.into())
    }

    /// Startup helper: load from `path` if it exists, otherwise
    /// initialise a fresh state with the given config and
    /// persist it. The first-ever run writes the snapshot;
    /// subsequent restarts read it back. This is the function
    /// `dl-app` calls on boot.
    pub fn load_or_init(
        path: &std::path::Path,
        cfg: CapConfig,
    ) -> Result<Self, CapSnapshotError> {
        if path.exists() {
            Self::load(path)
        } else {
            let s = Self::new(cfg);
            // Best-effort persist on first run.
            if let Err(e) = s.persist(path) {
                tracing::warn!(
                    "cap snapshot: first-run persist to {} failed: {} — running with in-memory state only",
                    path.display(),
                    e
                );
            }
            Ok(s)
        }
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

    /// Fakes the UTC clock past midnight and asserts the cap is
    /// replenished. The production state machine reads `Utc::now()`
    /// directly, so we manipulate the private `last_reset` field —
    /// which is the same "yesterday" signal the wall clock would
    /// have produced. The next `try_charge` triggers
    /// `reset_if_new_day` and the cap is replenished.
    #[test]
    fn daily_reset_fake_clock_past_midnight() {
        let mut s = CapState::new(cfg(500_000_000, 500_000_000));
        s.try_charge(450_000_000).unwrap();
        assert_eq!(s.spent_today(), 450_000_000);
        assert_eq!(s.remaining(), 50_000_000);

        // "Fake the clock past midnight" by rolling last_reset
        // back to yesterday. From the state machine's point of
        // view, this is identical to wall-clock time advancing
        // past UTC midnight.
        s.last_reset = s.last_reset.pred_opt().unwrap();

        // The next charge must hit reset_if_new_day, zero
        // spent_today, and succeed.
        s.try_charge(50_000_000).unwrap();
        assert_eq!(
            s.spent_today(),
            50_000_000,
            "cap must replenish at UTC midnight"
        );
        assert_eq!(s.remaining(), 450_000_000);
    }

    // ---- Phase 3 / DAM-67: crash recovery via durable snapshot ----

    fn snapshot_path(suffix: &str) -> std::path::PathBuf {
        let mut p = std::env::temp_dir();
        p.push(format!(
            "dl-signer-cap-snapshot-{}-{}-{}.json",
            std::process::id(),
            suffix,
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        let _ = std::fs::remove_file(&p);
        p
    }

    /// Required by DAM-67: a state created, charged, and
    /// persisted in one process must reload with the same
    /// `spent_today` value in a fresh process.
    #[test]
    fn crash_recovery_loads_persisted_state() {
        let path = snapshot_path("crash_recovery");
        let cfg = CapConfig {
            daily_lamports: 5_000_000_000,
            per_bundle_lamports: 500_000_000,
        };

        // Pre-crash: charge three bundles, persist, simulate
        // process death.
        let mut pre_crash = CapState::new(cfg);
        pre_crash.try_charge(100_000_000).unwrap();
        pre_crash.try_charge(200_000_000).unwrap();
        pre_crash.try_charge(150_000_000).unwrap();
        let pre_spent = pre_crash.spent_today();
        assert_eq!(pre_spent, 450_000_000);
        pre_crash.persist(&path).expect("persist pre-crash snapshot");
        drop(pre_crash);

        // Post-crash: a brand-new process loads the snapshot.
        let mut post_crash =
            CapState::load(&path).expect("load post-crash snapshot");

        assert_eq!(
            post_crash.spent_today(),
            pre_spent,
            "post-crash cap must reflect pre-crash spend"
        );
        assert_eq!(post_crash.spent_today(), 450_000_000);
        assert_eq!(post_crash.remaining(), 5_000_000_000 - 450_000_000);
        assert_eq!(post_crash.config(), cfg);

        post_crash.try_charge(50_000_000).unwrap();
        assert_eq!(post_crash.spent_today(), 500_000_000);
        post_crash.persist(&path).expect("persist after post-crash charge");
        let post_post =
            CapState::load(&path).expect("reload after second charge");
        assert_eq!(post_post.spent_today(), 500_000_000);

        let _ = std::fs::remove_file(&path);
    }

    /// Chaos test: kills the process mid-day, restarts, asserts
    /// the new cap equals the pre-crash cap minus already-bundled.
    #[test]
    fn crash_recovery_chaos_kill_mid_day_restart() {
        let path = snapshot_path("chaos");
        let cfg = CapConfig {
            daily_lamports: 1_000_000_000,
            per_bundle_lamports: 200_000_000,
        };

        let mut day1 = CapState::new(cfg);
        day1.try_charge(200_000_000).unwrap();
        day1.try_charge(150_000_000).unwrap();
        day1.try_charge(150_000_000).unwrap();
        let day1_spent = day1.spent_today();
        assert_eq!(day1_spent, 500_000_000);
        day1.persist(&path).expect("day1 persist");
        drop(day1);

        let mut day1_p2 =
            CapState::load_or_init(&path, cfg).expect("day1 process 2 boot");
        assert_eq!(
            day1_p2.spent_today(),
            day1_spent,
            "load_or_init must NOT reset spent_today when a snapshot exists"
        );
        assert_eq!(day1_p2.remaining(), 1_000_000_000 - 500_000_000);

        day1_p2.try_charge(100_000_000).unwrap();
        let day1_p2_spent = day1_p2.spent_today();
        assert_eq!(day1_p2_spent, 600_000_000);
        day1_p2.persist(&path).expect("day1 p2 persist");
        drop(day1_p2);

        let day1_p3 =
            CapState::load_or_init(&path, cfg).expect("day1 process 3 boot");
        assert_eq!(day1_p3.spent_today(), 600_000_000);
        assert_eq!(day1_p3.remaining(), 400_000_000);

        let fresh_path = snapshot_path("chaos_fresh");
        let fresh = CapState::load_or_init(&fresh_path, cfg)
            .expect("fresh boot with no snapshot");
        assert_eq!(fresh.spent_today(), 0);
        assert!(
            fresh_path.exists(),
            "load_or_init must persist on first run"
        );
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(&fresh_path);
    }

    /// Corrupted snapshot files must fail-closed, not silently
    /// give the bot a fresh daily cap.
    #[test]
    fn crash_recovery_rejects_corrupted_snapshot() {
        let path = snapshot_path("corrupt");
        std::fs::write(&path, b"{not valid json at all").unwrap();
        let res = CapState::load(&path);
        assert!(
            matches!(res, Err(CapSnapshotError::BadFormat(_))),
            "corrupted snapshot must be rejected: got {:?}",
            res
        );
        let _ = std::fs::remove_file(&path);
    }

    /// Stale-schema snapshot (unknown version) must be rejected.
    #[test]
    fn crash_recovery_rejects_unknown_version() {
        let path = snapshot_path("badversion");
        let bogus = serde_json::json!({
            "version": 999,
            "spent_today": 0,
            "last_reset": "2026-06-21",
            "cfg": { "daily_lamports": 5_000_000_000u64, "per_bundle_lamports": 500_000_000u64 }
        });
        std::fs::write(&path, serde_json::to_vec_pretty(&bogus).unwrap()).unwrap();
        let res = CapState::load(&path);
        assert!(
            matches!(res, Err(CapSnapshotError::BadFormat(_))),
            "unknown schema version must be rejected: got {:?}",
            res
        );
        let _ = std::fs::remove_file(&path);
    }

    /// Per-bundle cap > daily cap is nonsensical and must be
    /// rejected on load.
    #[test]
    fn crash_recovery_rejects_invariant_violation() {
        let path = snapshot_path("invariant");
        let bogus = serde_json::json!({
            "version": 1,
            "spent_today": 0,
            "last_reset": "2026-06-21",
            "cfg": { "daily_lamports": 100_000_000u64, "per_bundle_lamports": 200_000_000u64 }
        });
        std::fs::write(&path, serde_json::to_vec_pretty(&bogus).unwrap()).unwrap();
        let res = CapState::load(&path);
        assert!(
            matches!(res, Err(CapSnapshotError::BadFormat(_))),
            "per_bundle > daily must be rejected: got {:?}",
            res
        );
        let _ = std::fs::remove_file(&path);
    }
}
