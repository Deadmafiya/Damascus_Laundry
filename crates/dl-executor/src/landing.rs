//! Landing poll loop (sub-plan 1a-3).
//!
//! Polls the Jito Block Engine until a bundle's status is
//! `Landed { slot }` or `Lost`, or the timeout elapses. Uses
//! exponential backoff to avoid hammering the engine.
//!
//! The poll function is **blocking + synchronous**. It calls a
//! user-provided closure that returns the current landing status.
//! This keeps it trivially testable (no tokio runtime needed) and
//! lets the caller plug in either a `MockJitoClient` (returns
//! instantly) or a real `HttpJitoClient` (added in 1a-5).

use std::thread::sleep;
use std::time::{Duration, Instant};

use crate::error::ExecutorError;
use crate::jito::{JitoClient, LandingResult, MockJitoClient};

/// Configuration for the landing poll loop.
#[derive(Debug, Clone)]
pub struct LandingPollConfig {
    /// Total time to wait before giving up. Default: 30s.
    pub timeout: Duration,
    /// Initial poll interval. Default: 100ms.
    pub initial_poll_interval: Duration,
    /// Maximum poll interval (cap on backoff). Default: 2s.
    pub max_poll_interval: Duration,
    /// Backoff multiplier per retry. Default: 1.5.
    pub backoff_multiplier: f64,
}

impl Default for LandingPollConfig {
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(30),
            initial_poll_interval: Duration::from_millis(100),
            max_poll_interval: Duration::from_secs(2),
            backoff_multiplier: 1.5,
        }
    }
}

/// Poll the Jito client until the bundle lands or the timeout
/// elapses. Returns the final landing status.
///
/// `poll_fn` should call `JitoClient::poll_landing` (or a real
/// HTTP equivalent) and return the current status. The closure is
/// called with exponential backoff between attempts.
///
/// If `poll_fn` returns `Err`, the error is propagated immediately
/// (fail-closed). If the timeout elapses with `Pending` status
/// the function returns `Ok(LandingResult::Pending)` and the caller
/// should treat that as a circuit-breaker loss.
pub fn poll_bundle_landing<F>(
    bundle_id: &str,
    cfg: &LandingPollConfig,
    mut poll_fn: F,
) -> Result<LandingResult, ExecutorError>
where
    F: FnMut(&str) -> Result<LandingResult, ExecutorError>,
{
    let start = Instant::now();
    let mut interval = cfg.initial_poll_interval;

    loop {
        let status = poll_fn(bundle_id)?;
        match status {
            LandingResult::Landed { .. } | LandingResult::Lost => return Ok(status),
            LandingResult::Pending => {
                if start.elapsed() >= cfg.timeout {
                    return Ok(LandingResult::Pending);
                }
                sleep(interval);
                interval = next_interval(interval, cfg.max_poll_interval, cfg.backoff_multiplier);
            }
        }
    }
}

/// Convenience: poll using a MockJitoClient. Useful in tests that
/// don't want to wire a closure. Production code should use
/// `poll_bundle_landing` with a real client.
pub fn poll_with_mock(
    jito: &MockJitoClient,
    bundle_id: &str,
    cfg: &LandingPollConfig,
) -> Result<LandingResult, ExecutorError> {
    poll_bundle_landing(bundle_id, cfg, |id| jito.poll_landing(id))
}

fn next_interval(current: Duration, max: Duration, multiplier: f64) -> Duration {
    let next_ms = (current.as_millis() as f64 * multiplier) as u64;
    let next = Duration::from_millis(next_ms);
    if next > max {
        max
    } else {
        next
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    #[test]
    fn poll_returns_landed_when_first_call_succeeds() {
        let cfg = LandingPollConfig::default();
        let result = poll_bundle_landing("mock-bundle-1", &cfg, |_| {
            Ok(LandingResult::Landed { slot: 12345 })
        })
        .unwrap();
        assert!(matches!(result, LandingResult::Landed { slot: 12345 }));
    }

    #[test]
    fn poll_returns_lost_when_first_call_returns_lost() {
        let cfg = LandingPollConfig::default();
        let result = poll_bundle_landing("mock-bundle-2", &cfg, |_| {
            Ok(LandingResult::Lost)
        })
        .unwrap();
        assert_eq!(result, LandingResult::Lost);
    }

    #[test]
    fn poll_returns_pending_after_timeout() {
        let cfg = LandingPollConfig {
            timeout: Duration::from_millis(150),
            initial_poll_interval: Duration::from_millis(20),
            max_poll_interval: Duration::from_millis(50),
            backoff_multiplier: 1.5,
        };
        let result = poll_bundle_landing("mock-bundle-3", &cfg, |_| {
            Ok(LandingResult::Pending)
        })
        .unwrap();
        assert_eq!(result, LandingResult::Pending);
    }

    #[test]
    fn poll_retries_until_landed() {
        let cfg = LandingPollConfig {
            timeout: Duration::from_secs(2),
            initial_poll_interval: Duration::from_millis(10),
            max_poll_interval: Duration::from_millis(50),
            backoff_multiplier: 1.5,
        };
        let calls = AtomicU32::new(0);
        let result = poll_bundle_landing("mock-bundle-4", &cfg, |_| {
            let n = calls.fetch_add(1, Ordering::SeqCst);
            if n < 3 {
                Ok(LandingResult::Pending)
            } else {
                Ok(LandingResult::Landed { slot: 999 })
            }
        })
        .unwrap();
        assert!(matches!(result, LandingResult::Landed { slot: 999 }));
        assert!(calls.load(Ordering::SeqCst) >= 4);
    }

    #[test]
    fn poll_propagates_error_from_poll_fn() {
        let cfg = LandingPollConfig::default();
        let result = poll_bundle_landing("mock-bundle-5", &cfg, |_| {
            Err(ExecutorError::LandingPoll("rpc down".into()))
        });
        assert!(matches!(result, Err(ExecutorError::LandingPoll(_))));
    }

    #[test]
    fn poll_with_mock_returns_landed_for_mock_id() {
        let mock = MockJitoClient::new();
        let cfg = LandingPollConfig::default();
        let result = poll_with_mock(&mock, "anything", &cfg).unwrap();
        // Mock always returns Landed { slot: 0 } immediately.
        assert!(matches!(result, LandingResult::Landed { slot: 0 }));
    }

    #[test]
    fn next_interval_caps_at_max() {
        let max = Duration::from_millis(100);
        let next = next_interval(Duration::from_millis(80), max, 1.5);
        assert_eq!(next, Duration::from_millis(120).min(max));
        assert!(next <= max);
    }

    #[test]
    fn next_interval_doubles_progression() {
        let max = Duration::from_secs(60);
        let next = next_interval(Duration::from_millis(100), max, 2.0);
        assert_eq!(next, Duration::from_millis(200));
    }
}