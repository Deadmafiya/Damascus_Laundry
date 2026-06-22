//! Adapter that forwards `dl-feed::FeedStats` counters into the
//! `dl-app` metrics registry so they appear in the Prometheus
//! `/metrics` body alongside `opps_per_sec`, `would_trade`, etc.
//!
//! ## Why a periodic poller (not a push hook)
//!
//! `FeedStats` is updated by the WS background task; a push hook
//! would need an `Arc<MetricsRegistry>` threaded into the
//! `FeedConfig`, which forces `dl-feed` to depend on the registry
//! type. A poller reads the stats every `poll_interval` and writes
//! the latest values into a `RegistryGauge`. The latency cost (one
//! tick of `poll_interval`) is acceptable for human-readable
//! dashboarding. For real alerting, the existing
//! `MetricsSink`-based tracing adapter is the right path.
//!
//! ## Metric names
//!
//! - `feed_reconnect_count` (gauge) — successful reconnects so far
//! - `feed_reconnect_storm_count` (gauge) — storm events
//! - `feed_stale_pool_count` (gauge) — staleness guard trips
//! - `feed_halted` (gauge, 0/1) — feed is halted (1) or not (0)

use std::sync::Arc;
use std::time::Duration;

use dl_feed::ws_feed::FeedStats;

use crate::metrics::{MetricsRegistry, RegistryGauge};

/// Adapter that periodically copies `FeedStats` into a `MetricsRegistry`.
pub struct FeedMetricsAdapter {
    feed_stats: Arc<FeedStats>,
    reconnect_count: RegistryGauge,
    reconnect_storm_count: RegistryGauge,
    stale_pool_count: RegistryGauge,
    halted: RegistryGauge,
}

impl FeedMetricsAdapter {
    /// Bind a new adapter to a `FeedStats` handle and a registry.
    /// The three gauges are created (idempotent) on the registry.
    pub fn new(registry: Arc<MetricsRegistry>, feed_stats: Arc<FeedStats>) -> Self {
        let reconnect_count =
            RegistryGauge::new(registry.clone(), "feed_reconnect_count");
        let reconnect_storm_count =
            RegistryGauge::new(registry.clone(), "feed_reconnect_storm_count");
        let stale_pool_count =
            RegistryGauge::new(registry.clone(), "feed_stale_pool_count");
        let halted = RegistryGauge::new(registry.clone(), "feed_halted");
        Self {
            feed_stats,
            reconnect_count,
            reconnect_storm_count,
            stale_pool_count,
            halted,
        }
    }

    /// Copy current `FeedStats` values into the registry gauges.
    /// Cheap; safe to call from any thread.
    pub fn poll(&self) {
        let snap = self.feed_stats.snapshot();
        self.reconnect_count.set(snap.reconnect_count);
        self.reconnect_storm_count
            .set(snap.reconnect_storm_count);
        self.stale_pool_count.set(snap.stale_pool_count);
        self.halted.set(if snap.halted { 1 } else { 0 });
    }
}

/// Run a polling loop on a tokio runtime. The loop ticks every
/// `interval` and calls `adapter.poll()`. Returns when the
/// `shutdown` receiver fires.
///
/// Used by the `dl-app run` path to keep the metrics fresh while
/// the WS feed is running. The `dl-app metrics prom` path uses a
/// similar mechanism (read on every `/metrics` request via the
/// metrics-prom snapshot).
pub async fn run_poll_loop(
    adapter: Arc<FeedMetricsAdapter>,
    mut shutdown: tokio::sync::watch::Receiver<bool>,
    interval: Duration,
) {
    let mut ticker = tokio::time::interval(interval);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tokio::select! {
            _ = ticker.tick() => {
                adapter.poll();
            }
            _ = shutdown.changed() => {
                if *shutdown.borrow() {
                    adapter.poll();
                    return;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn poll_writes_all_gauges() {
        let registry = Arc::new(MetricsRegistry::new());
        let feed_stats = FeedStats::new();
        // Bump the underlying counters by hand: the WS feed is
        // async; we don't need it for the poller test.
        feed_stats.__set_for_test(3, 1, 2, true);

        let adapter = FeedMetricsAdapter::new(registry.clone(), feed_stats);
        adapter.poll();

        let snap = registry.snapshot();
        let get = |name: &str| -> u64 {
            snap.gauges
                .iter()
                .find(|(n, _)| *n == name)
                .map(|(_, v)| *v)
                .unwrap_or(0)
        };
        assert_eq!(get("feed_reconnect_count"), 3);
        assert_eq!(get("feed_reconnect_storm_count"), 1);
        assert_eq!(get("feed_stale_pool_count"), 2);
        assert_eq!(get("feed_halted"), 1);
    }

    #[test]
    fn poll_handles_zero_state() {
        let registry = Arc::new(MetricsRegistry::new());
        let feed_stats = FeedStats::new();
        let adapter = FeedMetricsAdapter::new(registry.clone(), feed_stats);
        adapter.poll();
        let snap = registry.snapshot();
        for name in [
            "feed_reconnect_count",
            "feed_reconnect_storm_count",
            "feed_stale_pool_count",
            "feed_halted",
        ] {
            let v = snap
                .gauges
                .iter()
                .find(|(n, _)| *n == name)
                .map(|(_, v)| *v)
                .unwrap_or(0);
            assert_eq!(v, 0, "expected {name} = 0");
        }
    }
}
