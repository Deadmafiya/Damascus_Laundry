//! Latency histogram (08-02).
//!
//! Atomic-bucket latency tracker. No floats. The 12 buckets are
//! log-scale: <1ms, <2ms, <4ms, <8ms, <16ms, <32ms, <64ms, <128ms,
//! <256ms, <512ms, <1024ms, <∞. Snapshot is the bucketed counts
//! plus a precise p50/p95/p99 derived from a linear scan over
//! the cumulative histogram.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

const BUCKETS: &[u64] = &[
    1, 2, 4, 8, 16, 32, 64, 128, 256, 512, 1024, u64::MAX,
];
const N_BUCKETS: usize = BUCKETS.len();

/// A point-in-time snapshot of the latency histogram.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct LatencySnapshot {
    pub p50_ms: u64,
    pub p95_ms: u64,
    pub p99_ms: u64,
    pub count: u64,
    pub sum_ms: u64,
    pub mean_ms: u64,
    /// Per-bucket counts (12 buckets). Useful for the report.
    pub buckets: [u64; N_BUCKETS],
}

impl LatencySnapshot {
    /// True if p99 is under the 80ms project budget.
    pub fn meets_budget(&self) -> bool {
        self.count == 0 || self.p99_ms < 80
    }
}

/// Atomic-bucket latency histogram.
pub struct LatencyHistogram {
    buckets: [AtomicU64; N_BUCKETS],
    count: AtomicU64,
    sum_ms: AtomicU64,
}

impl LatencyHistogram {
    pub fn new() -> Self {
        Self {
            buckets: std::array::from_fn(|_| AtomicU64::new(0)),
            count: AtomicU64::new(0),
            sum_ms: AtomicU64::new(0),
        }
    }

    /// Record a single elapsed duration. The duration is bucketed
    /// by the largest bucket whose upper bound is >= the value.
    pub fn record(&self, elapsed: Duration) {
        let ms = elapsed.as_millis().min(u64::MAX as u128) as u64;
        self.sum_ms.fetch_add(ms, Ordering::Relaxed);
        let idx = BUCKETS
            .iter()
            .position(|&b| ms <= b)
            .unwrap_or(N_BUCKETS - 1);
        self.buckets[idx].fetch_add(1, Ordering::Relaxed);
        self.count.fetch_add(1, Ordering::Relaxed);
    }

    /// Snapshot the current state.
    pub fn snapshot(&self) -> LatencySnapshot {
        let buckets: [u64; N_BUCKETS] = std::array::from_fn(|i| {
            self.buckets[i].load(Ordering::Relaxed)
        });
        let count = self.count.load(Ordering::Relaxed);
        let sum_ms = self.sum_ms.load(Ordering::Relaxed);
        let mean_ms = if count > 0 { sum_ms / count } else { 0 };

        // p50 / p95 / p99: linear scan over cumulative counts.
        let (p50, p95, p99) = if count == 0 {
            (0, 0, 0)
        } else {
            let p50_target = count / 2;
            let p95_target = (count * 95) / 100;
            let p99_target = (count * 99) / 100;
            let mut cumulative = 0u64;
            let mut p50 = 0u64;
            let mut p95 = 0u64;
            let mut p99 = 0u64;
            for (i, &count_in_bucket) in buckets.iter().enumerate() {
                cumulative = cumulative.saturating_add(count_in_bucket);
                if p50 == 0 && cumulative >= p50_target {
                    p50 = BUCKETS[i];
                }
                if p95 == 0 && cumulative >= p95_target {
                    p95 = BUCKETS[i];
                }
                if p99 == 0 && cumulative >= p99_target {
                    p99 = BUCKETS[i];
                }
            }
            (p50, p95, p99)
        };

        LatencySnapshot {
            p50_ms: p50,
            p95_ms: p95,
            p99_ms: p99,
            count,
            sum_ms,
            mean_ms,
            buckets,
        }
    }
}

impl Default for LatencyHistogram {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_histogram_is_zero() {
        let h = LatencyHistogram::new();
        let s = h.snapshot();
        assert_eq!(s.count, 0);
        assert_eq!(s.p50_ms, 0);
        assert!(s.meets_budget(), "empty histogram trivially meets budget");
    }

    #[test]
    fn records_bucket_correctly() {
        let h = LatencyHistogram::new();
        h.record(Duration::from_millis(5));
        h.record(Duration::from_millis(15));
        h.record(Duration::from_millis(200));
        let s = h.snapshot();
        assert_eq!(s.count, 3);
        // ms=5 falls in bucket 3 (5<=8), ms=15 in bucket 4 (15<=16),
        // ms=200 in bucket 8 (200<=256).
        assert_eq!(s.buckets[3], 1, "5ms should be in bucket 3 (<=8)");
        assert_eq!(s.buckets[4], 1, "15ms should be in bucket 4 (<=16)");
        assert_eq!(s.buckets[8], 1, "200ms should be in bucket 8 (<=256)");
    }

    #[test]
    fn p99_under_budget_for_fast_path() {
        let h = LatencyHistogram::new();
        for _ in 0..1000 {
            h.record(Duration::from_millis(5));
        }
        let s = h.snapshot();
        assert!(s.p99_ms < 80, "p99 should be < 80ms, got {}", s.p99_ms);
        assert!(s.meets_budget());
    }

    #[test]
    fn p99_above_budget_when_slow() {
        let h = LatencyHistogram::new();
        // 95 fast + 5 slow. p99 = 100ms (the 96th..100th entries
        // are all slow). The cumulative count reaches 99 in the
        // 100ms bucket.
        for _ in 0..95 {
            h.record(Duration::from_millis(5));
        }
        for _ in 0..5 {
            h.record(Duration::from_millis(100));
        }
        let s = h.snapshot();
        assert!(!s.meets_budget(), "p99 should be > 80ms here, got {}", s.p99_ms);
    }

    #[test]
    fn mean_is_correct() {
        let h = LatencyHistogram::new();
        h.record(Duration::from_millis(10));
        h.record(Duration::from_millis(20));
        h.record(Duration::from_millis(30));
        let s = h.snapshot();
        assert_eq!(s.mean_ms, 20);
    }
}
