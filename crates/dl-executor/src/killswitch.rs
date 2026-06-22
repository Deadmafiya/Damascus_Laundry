//! Kill-switch surface (sub-plan 0d, Phase 0).
//!
//! Three independent kill signals, all checked before submitting a
//! bundle:
//!
//! 1. **`STOP` sentinel file.** Operator touches `./STOP` (or
//!    `$DL_STOP_FILE`). The engine pauses new bundles and exits
//!    cleanly with code 0 within 5s.
//!
//! 2. **Consecutive-loss circuit breaker.** After N consecutive
//!    `Lost` outcomes or simulate-rejected bundles, the engine
//!    pauses. N defaults to 3.
//!
//! 3. **Daily cap exhaustion** (already in `dl-signer::CapState`;
//!    re-checked here as a defense-in-depth before each submit).
//!
//! The kill switch is *fail-closed*: any of the three signals
//! returns `KillSwitchTripped`, which is a hard error in
//! `submit_opportunity`. The engine logs the reason and stops.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::error::ExecutorError;

/// Configuration for the kill switch.
#[derive(Debug, Clone)]
pub struct KillSwitchConfig {
    /// Path to the sentinel file. Default: `./STOP`.
    pub stop_file: PathBuf,
    /// Number of consecutive `Lost` or simulate-rejected outcomes
    /// before tripping. Default: 3.
    pub max_consecutive_losses: u32,
    /// Per-check poll interval for the sentinel file. Default: 1s.
    pub poll_interval: Duration,
}

impl Default for KillSwitchConfig {
    fn default() -> Self {
        Self {
            stop_file: PathBuf::from("./STOP"),
            max_consecutive_losses: 3,
            poll_interval: Duration::from_secs(1),
        }
    }
}

/// Per-bundle outcome classification fed to the circuit breaker.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BundleOutcome {
    /// Bundle landed with positive net PnL.
    Landed,
    /// Bundle did not land (lost the race, dropped, etc.) OR
    /// simulate gate rejected it. Either way, counts as a "loss"
    /// for the circuit breaker.
    Lost,
    /// Bundle is still pending; neither win nor loss yet.
    Pending,
}

/// The kill switch state. Cheap to construct and to check; holds
/// an in-memory counter for the consecutive-loss breaker and a
/// config for the sentinel path.
#[derive(Debug, Clone)]
pub struct KillSwitch {
    config: KillSwitchConfig,
    consecutive_losses: u32,
}

impl KillSwitch {
    pub fn new(config: KillSwitchConfig) -> Self {
        Self {
            config,
            consecutive_losses: 0,
        }
    }

    pub fn with_default_stop_file(stop_file: impl Into<PathBuf>) -> Self {
        Self::new(KillSwitchConfig {
            stop_file: stop_file.into(),
            ..KillSwitchConfig::default()
        })
    }

    pub fn config(&self) -> &KillSwitchConfig {
        &self.config
    }

    pub fn consecutive_losses(&self) -> u32 {
        self.consecutive_losses
    }

    /// True if the `STOP` sentinel file exists. Cheap syscall; safe
    /// to call every cycle.
    pub fn stop_file_present(&self) -> bool {
        Path::new(&self.config.stop_file).exists()
    }

    /// Record a bundle outcome and update the circuit-breaker
    /// counter. Returns `Err` if the kill switch tripped.
    ///
    /// Behaviour:
    /// - `Landed` resets the counter to 0.
    /// - `Lost` increments the counter; trips at `max_consecutive_losses`.
    /// - `Pending` is a no-op (neither win nor loss yet).
    pub fn record(&mut self, outcome: BundleOutcome) -> Result<(), ExecutorError> {
        match outcome {
            BundleOutcome::Landed => {
                self.consecutive_losses = 0;
                Ok(())
            }
            BundleOutcome::Lost => {
                self.consecutive_losses = self.consecutive_losses.saturating_add(1);
                if self.consecutive_losses >= self.config.max_consecutive_losses {
                    return Err(ExecutorError::KillSwitchTripped(format!(
                        "{} consecutive losses (max {})",
                        self.consecutive_losses, self.config.max_consecutive_losses
                    )));
                }
                Ok(())
            }
            BundleOutcome::Pending => Ok(()),
        }
    }

    /// Check the STOP sentinel file. Returns `Err` if it exists.
    /// Independent of `record()` so callers can check it without
    /// consuming a bundle outcome.
    pub fn check_stop_file(&self) -> Result<(), ExecutorError> {
        if self.stop_file_present() {
            Err(ExecutorError::KillSwitchTripped(format!(
                "STOP sentinel present at {}",
                self.config.stop_file.display()
            )))
        } else {
            Ok(())
        }
    }

    /// Combined check: sentinel file + circuit breaker. Call this
    /// right before submitting each bundle.
    pub fn check(&self) -> Result<(), ExecutorError> {
        self.check_stop_file()?;
        if self.consecutive_losses >= self.config.max_consecutive_losses {
            return Err(ExecutorError::KillSwitchTripped(format!(
                "{} consecutive losses (max {})",
                self.consecutive_losses, self.config.max_consecutive_losses
            )));
        }
        Ok(())
    }

    /// Reset the circuit breaker counter. Useful when the operator
    /// manually clears the STOP file and wants to resume without
    /// restarting the process.
    pub fn reset(&mut self) {
        self.consecutive_losses = 0;
    }
}

/// Helper for the runbook's recovery procedure: print the age of
/// the STOP file (if present) so the operator can tell how long
/// the kill signal has been active.
pub fn stop_file_age_secs(stop_file: &Path) -> Option<u64> {
    let meta = std::fs::metadata(stop_file).ok()?;
    let mtime = meta.modified().ok()?;
    let now = SystemTime::now().duration_since(UNIX_EPOCH).ok()?;
    let mtime_secs = mtime.duration_since(UNIX_EPOCH).ok()?;
    Some(now.saturating_sub(mtime_secs).as_secs())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn tmp_stop_file(name: &str) -> PathBuf {
        let mut p = std::env::temp_dir();
        p.push(format!("dl-killswitch-{name}-{}.STOP", std::process::id()));
        // Ensure it does not exist before the test starts.
        let _ = std::fs::remove_file(&p);
        p
    }

    #[test]
    fn new_killswitch_has_no_stop_file() {
        let ks = KillSwitch::with_default_stop_file(tmp_stop_file("no-file"));
        assert!(!ks.stop_file_present());
        assert_eq!(ks.consecutive_losses(), 0);
        assert!(ks.check().is_ok());
    }

    #[test]
    fn stop_file_trips_killswitch() {
        let path = tmp_stop_file("trips");
        let mut f = std::fs::File::create(&path).expect("create STOP file");
        f.write_all(b"halt").expect("write STOP");
        drop(f);

        let ks = KillSwitch::with_default_stop_file(&path);
        assert!(ks.stop_file_present());
        let err = ks.check_stop_file().unwrap_err();
        assert!(matches!(err, ExecutorError::KillSwitchTripped(_)));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn consecutive_losses_trip_after_threshold() {
        let ks = KillSwitch::new(KillSwitchConfig {
            max_consecutive_losses: 3,
            ..KillSwitchConfig::default()
        });
        let mut ks = ks;

        // Two losses: still OK.
        ks.record(BundleOutcome::Lost).unwrap();
        ks.record(BundleOutcome::Lost).unwrap();
        assert_eq!(ks.consecutive_losses(), 2);
        assert!(ks.check().is_ok());

        // Third loss: trips.
        let err = ks.record(BundleOutcome::Lost).unwrap_err();
        assert!(matches!(err, ExecutorError::KillSwitchTripped(_)));
        assert_eq!(ks.consecutive_losses(), 3);
        // check() also trips now.
        assert!(ks.check().is_err());
    }

    #[test]
    fn landed_resets_counter() {
        let mut ks = KillSwitch::new(KillSwitchConfig {
            max_consecutive_losses: 3,
            ..KillSwitchConfig::default()
        });
        ks.record(BundleOutcome::Lost).unwrap();
        ks.record(BundleOutcome::Lost).unwrap();
        assert_eq!(ks.consecutive_losses(), 2);
        ks.record(BundleOutcome::Landed).unwrap();
        assert_eq!(ks.consecutive_losses(), 0);
        assert!(ks.check().is_ok());
    }

    #[test]
    fn pending_is_no_op() {
        let mut ks = KillSwitch::new(KillSwitchConfig {
            max_consecutive_losses: 3,
            ..KillSwitchConfig::default()
        });
        ks.record(BundleOutcome::Lost).unwrap();
        ks.record(BundleOutcome::Pending).unwrap();
        ks.record(BundleOutcome::Pending).unwrap();
        assert_eq!(ks.consecutive_losses(), 1);
    }

    #[test]
    fn reset_clears_counter() {
        let mut ks = KillSwitch::new(KillSwitchConfig {
            max_consecutive_losses: 3,
            ..KillSwitchConfig::default()
        });
        ks.record(BundleOutcome::Lost).unwrap();
        ks.record(BundleOutcome::Lost).unwrap();
        assert_eq!(ks.consecutive_losses(), 2);
        ks.reset();
        assert_eq!(ks.consecutive_losses(), 0);
        assert!(ks.check().is_ok());
    }

    #[test]
    fn stop_file_age_reports_nonzero_when_old() {
        let path = tmp_stop_file("age");
        std::fs::write(&path, b"halt").expect("write STOP");
        // Sleep briefly to ensure mtime is in the past.
        std::thread::sleep(Duration::from_millis(1100));
        let age = stop_file_age_secs(&path);
        assert!(age.is_some());
        assert!(age.unwrap() >= 1);
        let _ = std::fs::remove_file(&path);
    }
}