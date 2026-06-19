//! Live-mode metrics (08-01).
//!
//! In-memory counters that the live pipeline updates. The
//! `dl-app` binary's existing Prometheus emitter (in
//! `crates/dl-app/src/metrics_prom.rs`) reads a `LiveMetricsSnapshot`
//! from a shared `LiveMetrics` to render the `/metrics` endpoint.

use std::sync::atomic::{AtomicU64, Ordering};

/// Atomic counters updated by the live pipeline.
#[derive(Debug, Default)]
pub struct LiveMetrics {
    pub bundles_submitted: AtomicU64,
    pub bundles_landed: AtomicU64,
    pub bundles_failed: AtomicU64,
    pub sol_spent_lamports: AtomicU64,
    pub sol_received_lamports: AtomicU64,
    pub last_submission_latency_ms: AtomicU64,
}

impl LiveMetrics {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn snapshot(&self) -> LiveMetricsSnapshot {
        LiveMetricsSnapshot {
            bundles_submitted: self.bundles_submitted.load(Ordering::Relaxed),
            bundles_landed: self.bundles_landed.load(Ordering::Relaxed),
            bundles_failed: self.bundles_failed.load(Ordering::Relaxed),
            sol_spent_lamports: self.sol_spent_lamports.load(Ordering::Relaxed),
            sol_received_lamports: self.sol_received_lamports.load(Ordering::Relaxed),
            last_submission_latency_ms: self.last_submission_latency_ms.load(Ordering::Relaxed),
        }
    }
}

/// A point-in-time copy of the live metrics, suitable for the
/// Prometheus emitter to render.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct LiveMetricsSnapshot {
    pub bundles_submitted: u64,
    pub bundles_landed: u64,
    pub bundles_failed: u64,
    pub sol_spent_lamports: u64,
    pub sol_received_lamports: u64,
    pub last_submission_latency_ms: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_reflects_increments() {
        let m = LiveMetrics::new();
        m.bundles_submitted.fetch_add(3, Ordering::Relaxed);
        m.bundles_landed.fetch_add(2, Ordering::Relaxed);
        let s = m.snapshot();
        assert_eq!(s.bundles_submitted, 3);
        assert_eq!(s.bundles_landed, 2);
        assert_eq!(s.bundles_failed, 0);
        assert_eq!(s.sol_spent_lamports, 0);
    }

    #[test]
    fn snapshot_is_zero_for_fresh_metrics() {
        let s = LiveMetrics::new().snapshot();
        assert_eq!(s.bundles_submitted, 0);
        assert_eq!(s.sol_spent_lamports, 0);
    }
}
