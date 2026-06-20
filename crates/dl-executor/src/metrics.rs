//! Live-mode metrics (08-01 + Phase 1d extension).
//!
//! In-memory counters updated by the live pipeline. The
//! `dl-app` binary's existing Prometheus emitter (in
//! `crates/dl-app/src/metrics_prom.rs`) reads a `LiveMetricsSnapshot`
//! from a shared `LiveMetrics` to render the `/metrics` endpoint.
//!
//! Phase 1d extension: the `last_submission_latency_ms` field is
//! now backed by a ring buffer of the last N=512 samples, and we
//! expose p50/p95/count/sum via [`LandingLatencySnapshot`].

use std::sync::atomic::{AtomicU64, Ordering};

/// Atomic counters updated by the live pipeline.
#[derive(Debug)]
pub struct LiveMetrics {
    pub bundles_submitted: AtomicU64,
    pub bundles_landed: AtomicU64,
    pub bundles_failed: AtomicU64,
    pub sol_spent_lamports: AtomicU64,
    pub sol_received_lamports: AtomicU64,
    /// Most recent submission-to-landed latency (milliseconds).
    /// Updated via [`LiveMetrics::record_landing_latency_ms`].
    pub last_submission_latency_ms: AtomicU64,
    /// Ring buffer of recent landing latencies (milliseconds).
    /// Sub-millisecond histogram of submit→landed durations. The
    /// last 512 observations are kept in a ring buffer (M9 H1 of
    /// Phase 1d).
    landing_latencies: std::sync::Mutex<LandingLatencyRing>,
    /// Wrong-leg-count counters. Indexed by leg count (so a
    /// 2-leg and a 4-leg cycle show up separately in
    /// Prometheus). Maps `leg_count -> count`.
    wrong_leg_counts: std::sync::Mutex<std::collections::HashMap<usize, u64>>,
    /// Monotonic cycle sequence counter (Phase 2 C2). Used by
    /// `submit_opportunity` to assign a stable `Cycle::seq` to
    /// every detected cycle so calibration captures and
    /// reconciliation rows can join on it.
    pub cycles_evaluated: AtomicU64,
}

/// Number of recent landing samples kept for percentile
/// computation. 512 is enough for ~1 hour at 1 probe/sec, or
/// ~8 minutes at 1 probe/sec; round numbers chosen for the
/// Prometheus quantile lines.
pub const LANDING_LATENCY_CAPACITY: usize = 512;

/// Ring buffer of recent landing latencies (milliseconds).
#[derive(Debug)]
struct LandingLatencyRing {
    /// Storage. `samples[i]` is meaningful when `i < count` (newest
    /// samples are at the end).
    samples: Vec<u64>,
    /// Total samples pushed since the ring started (monotonic,
    /// can exceed capacity).
    total: u64,
}

impl Default for LandingLatencyRing {
    fn default() -> Self {
        Self {
            samples: Vec::with_capacity(LANDING_LATENCY_CAPACITY),
            total: 0,
        }
    }
}

impl Default for LiveMetrics {
    fn default() -> Self {
        Self {
            bundles_submitted: AtomicU64::new(0),
            bundles_landed: AtomicU64::new(0),
            bundles_failed: AtomicU64::new(0),
            sol_spent_lamports: AtomicU64::new(0),
            sol_received_lamports: AtomicU64::new(0),
            last_submission_latency_ms: AtomicU64::new(0),
            landing_latencies: std::sync::Mutex::new(LandingLatencyRing::default()),
            wrong_leg_counts: std::sync::Mutex::new(std::collections::HashMap::new()),
            cycles_evaluated: AtomicU64::new(0),
        }
    }
}

impl LiveMetrics {
    pub fn new() -> Self {
        Self::default()
    }

    /// Record one submission-to-landed latency sample (milliseconds).
    /// Saturates at `u64::MAX` for absurd values; `0` is allowed
    /// (instant landing — unusual but legal).
    pub fn record_landing_latency_ms(&self, ms: u64) {
        self.last_submission_latency_ms
            .store(ms, Ordering::Relaxed);
        let mut ring = self
            .landing_latencies
            .lock()
            .expect("landing-latency mutex poisoned");
        ring.total = ring.total.saturating_add(1);
        if ring.samples.len() < LANDING_LATENCY_CAPACITY {
            ring.samples.push(ms);
        } else {
            // Overwrite oldest (front of vec). shift_remove(0) is O(n)
            // but n=512 is tiny.
            ring.samples.remove(0);
            ring.samples.push(ms);
        }
    }

    /// Snapshot of the landing-latency histogram. Cheap to compute
    /// (sort at most 512 u64s).
    pub fn landing_latency_snapshot(&self) -> LandingLatencySnapshot {
        let ring = self
            .landing_latencies
            .lock()
            .expect("landing-latency mutex poisoned");
        let count = ring.samples.len() as u64;
        if count == 0 {
            return LandingLatencySnapshot::default();
        }
        let sum_ms: u64 = ring.samples.iter().sum();
        let min_ms = *ring.samples.iter().min().unwrap();
        let max_ms = *ring.samples.iter().max().unwrap();
        let mut sorted = ring.samples.clone();
        sorted.sort_unstable();
        LandingLatencySnapshot {
            count,
            sum_ms,
            min_ms,
            max_ms,
            p50_ms: percentile(&sorted, 0.50),
            p95_ms: percentile(&sorted, 0.95),
            total_samples: ring.total,
        }
    }

    /// Allocate and return the next cycle sequence number.
    /// Monotonic; never reused. Thread-safe via `AtomicU64::fetch_add`.
    pub fn next_cycle_seq(&self) -> u64 {
        self.cycles_evaluated.fetch_add(1, Ordering::Relaxed) + 1
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

    /// Increment the wrong-leg-count counter for `leg_count`.
    /// Called by `opportunity::resolve_cycle_legs` when a Cycle
    /// has a length other than 3 (which v2.0 atomicity requires).
    /// The dashboard reads this via Prometheus to show a
    /// histogram of cycle lengths the detector is emitting.
    pub fn inc_wrong_leg_count(&self, leg_count: usize) {
        let mut map = self
            .wrong_leg_counts
            .lock()
            .expect("wrong_leg_counts mutex poisoned");
        *map.entry(leg_count).or_insert(0) += 1;
    }

    /// Read the wrong-leg-count counter for `leg_count`. Returns
    /// 0 if no cycles of that length have been seen.
    pub fn wrong_leg_count_total(&self, leg_count: usize) -> u64 {
        let map = self
            .wrong_leg_counts
            .lock()
            .expect("wrong_leg_counts mutex poisoned");
        map.get(&leg_count).copied().unwrap_or(0)
    }

    /// Snapshot of all wrong-leg-count entries. Used by the
    /// Prometheus emitter.
    pub fn wrong_leg_count_snapshot(&self) -> std::collections::HashMap<usize, u64> {
        self.wrong_leg_counts
            .lock()
            .expect("wrong_leg_counts mutex poisoned")
            .clone()
    }
}

/// Linear-interpolation percentile. `q` in [0.0, 1.0].
/// Rounds up to keep the invariant: result is one of the samples
/// for the median, but interpolated for other quantiles.
fn percentile(sorted: &[u64], q: f64) -> u64 {
    debug_assert!(!sorted.is_empty());
    debug_assert!((0.0..=1.0).contains(&q));
    if sorted.len() == 1 {
        return sorted[0];
    }
    // Position in the sorted array (0-indexed, can be fractional).
    let pos = q * (sorted.len() as f64 - 1.0);
    let lo = pos.floor() as usize;
    let hi = pos.ceil() as usize;
    if lo == hi {
        return sorted[lo];
    }
    let frac = pos - lo as f64;
    let a = sorted[lo];
    let b = sorted[hi];
    (a as f64 + (b as f64 - a as f64) * frac) as u64
}

/// Histogram snapshot suitable for Prometheus text format.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct LandingLatencySnapshot {
    /// Number of samples in the ring buffer (`<= LANDING_LATENCY_CAPACITY`).
    pub count: u64,
    /// Sum of all samples in the ring buffer.
    pub sum_ms: u64,
    /// Minimum sample in the ring buffer.
    pub min_ms: u64,
    /// Maximum sample in the ring buffer.
    pub max_ms: u64,
    /// 50th percentile (linear interpolation between two samples).
    pub p50_ms: u64,
    /// 95th percentile.
    pub p95_ms: u64,
    /// Total samples ever recorded (monotonic; can exceed `count`
    /// once the ring has wrapped).
    pub total_samples: u64,
}

impl Default for LandingLatencySnapshot {
    fn default() -> Self {
        Self {
            count: 0,
            sum_ms: 0,
            min_ms: 0,
            max_ms: 0,
            p50_ms: 0,
            p95_ms: 0,
            total_samples: 0,
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

    #[test]
    fn record_landing_latency_ms_updates_last_field() {
        let m = LiveMetrics::new();
        m.record_landing_latency_ms(420);
        assert_eq!(m.last_submission_latency_ms.load(Ordering::Relaxed), 420);
        m.record_landing_latency_ms(680);
        assert_eq!(m.last_submission_latency_ms.load(Ordering::Relaxed), 680);
    }

    #[test]
    fn landing_latency_snapshot_handles_empty_buffer() {
        let m = LiveMetrics::new();
        let snap = m.landing_latency_snapshot();
        assert_eq!(snap, LandingLatencySnapshot::default());
        assert_eq!(snap.count, 0);
        assert_eq!(snap.total_samples, 0);
    }

    #[test]
    fn landing_latency_snapshot_records_single_sample() {
        let m = LiveMetrics::new();
        m.record_landing_latency_ms(500);
        let snap = m.landing_latency_snapshot();
        assert_eq!(snap.count, 1);
        assert_eq!(snap.sum_ms, 500);
        assert_eq!(snap.min_ms, 500);
        assert_eq!(snap.max_ms, 500);
        assert_eq!(snap.p50_ms, 500);
        assert_eq!(snap.p95_ms, 500);
        assert_eq!(snap.total_samples, 1);
    }

    #[test]
    fn landing_latency_snapshot_computes_p50_and_p95() {
        // Insert 100 samples: 1, 2, ..., 100 ms.
        let m = LiveMetrics::new();
        for i in 1..=100 {
            m.record_landing_latency_ms(i);
        }
        let snap = m.landing_latency_snapshot();
        assert_eq!(snap.count, 100);
        assert_eq!(snap.min_ms, 1);
        assert_eq!(snap.max_ms, 100);
        // p50 of [1..=100] is 50 (median).
        assert_eq!(snap.p50_ms, 50);
        // p95: linear interpolation, pos = 0.95 * 99 = 94.05 → lo=94, hi=95.
        // value = 95 + 0.05 * (96 - 95) = 95.05 → 95.
        assert_eq!(snap.p95_ms, 95);
        assert_eq!(snap.total_samples, 100);
    }

    #[test]
    fn landing_latency_ring_buffer_wraps_at_capacity() {
        let m = LiveMetrics::new();
        // Insert LANDING_LATENCY_CAPACITY + 100 samples: 1, 2, ...
        for i in 1..=(LANDING_LATENCY_CAPACITY + 100) {
            m.record_landing_latency_ms(i as u64);
        }
        let snap = m.landing_latency_snapshot();
        assert_eq!(snap.count, LANDING_LATENCY_CAPACITY as u64);
        // The buffer keeps the LAST 512 samples, so min is
        // (LANDING_LATENCY_CAPACITY+100) - 512 + 1.
        assert_eq!(
            snap.min_ms,
            (LANDING_LATENCY_CAPACITY as u64) - 512 + 1 + 100
        );
        assert_eq!(snap.max_ms, (LANDING_LATENCY_CAPACITY + 100) as u64);
        assert_eq!(snap.total_samples, (LANDING_LATENCY_CAPACITY + 100) as u64);
    }

    #[test]
    fn percentile_handles_edge_quantiles() {
        let sorted = vec![10u64, 20, 30, 40, 50];
        assert_eq!(percentile(&sorted, 0.0), 10);
        assert_eq!(percentile(&sorted, 1.0), 50);
        assert_eq!(percentile(&sorted, 0.5), 30);
        // 25th percentile: pos = 0.25 * 4 = 1.0 → exact index 1 → 20
        assert_eq!(percentile(&sorted, 0.25), 20);
    }

    #[test]
    fn percentile_handles_single_element() {
        assert_eq!(percentile(&[42u64], 0.5), 42);
        assert_eq!(percentile(&[42u64], 0.95), 42);
        assert_eq!(percentile(&[42u64], 0.0), 42);
    }

    #[test]
    fn snapshot_serializes_for_recon() {
        let snap = LandingLatencySnapshot {
            count: 10,
            sum_ms: 5000,
            min_ms: 100,
            max_ms: 900,
            p50_ms: 480,
            p95_ms: 850,
            total_samples: 10,
        };
        let json = serde_json::to_string(&snap).expect("serialize");
        let back: LandingLatencySnapshot = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back, snap);
    }

    #[test]
    fn wrong_leg_count_starts_at_zero() {
        let m = LiveMetrics::new();
        assert_eq!(m.wrong_leg_count_total(2), 0);
        assert_eq!(m.wrong_leg_count_total(4), 0);
    }

    #[test]
    fn wrong_leg_count_tracks_per_leg_count() {
        let m = LiveMetrics::new();
        m.inc_wrong_leg_count(2);
        m.inc_wrong_leg_count(2);
        m.inc_wrong_leg_count(4);
        assert_eq!(m.wrong_leg_count_total(2), 2);
        assert_eq!(m.wrong_leg_count_total(4), 1);
        assert_eq!(m.wrong_leg_count_total(7), 0);
    }

    #[test]
    fn wrong_leg_count_snapshot_returns_full_map() {
        let m = LiveMetrics::new();
        m.inc_wrong_leg_count(2);
        m.inc_wrong_leg_count(4);
        m.inc_wrong_leg_count(4);
        let snap = m.wrong_leg_count_snapshot();
        assert_eq!(snap.get(&2).copied(), Some(1));
        assert_eq!(snap.get(&4).copied(), Some(2));
    }
}