//! Prometheus text-format metrics adapter (Phase 7 / plan 02, AC-5).
//!
//! Hand-rolled Prometheus exposition-format renderer. No
//! `prometheus` crate dependency — the workspace is
//! integer-only and the `prometheus` crate uses `f64` for
//! histogram quantile estimation, which would clash with
//! the float-free invariant.
//!
//! ## Wire format
//!
//! Per the Prometheus exposition-format spec
//! (<https://prometheus.io/docs/instrumenting/exposition_formats/>),
//! each metric emits:
//!
//! ```text
//! # HELP <name> <docstring>
//! # TYPE <name> <counter|gauge|histogram>
//! <name> <value>
//! ```
//!
//! Histograms emit one `<name>_bucket{le="<bound>"} <count>` line
//! per bucket plus `<name>_sum <sum>` and `<name>_count <count>`.
//!
//! ## Usage
//!
//! ```text
//! use dl_app::metrics::{MetricsRegistry, RegistryCounter, ...};
//! use dl_app::metrics_prom::MetricsPrometheus;
//! use std::sync::Arc;
//!
//! let registry = Arc::new(MetricsRegistry::new());
//! let sink = Arc::new(MetricsPrometheus::new(registry.clone()));
//! registry.add_sink(sink.clone());
//!
//! // ... engine updates metrics via RegistryCounter::inc() etc ...
//!
//! // Render:
//! let body = sink.render(); // String, Prometheus text format
//! ```
//!
//! In `dl-app`, the `metrics prom` subcommand wraps this in a
//! small HTTP server that serves `/metrics` on demand.

use std::fmt::Write;
use std::sync::Arc;

use dl_executor::metrics::{LandingLatencySnapshot, LiveMetrics};

use crate::metrics::{MetricsRegistry, MetricsSnapshot};

/// Prometheus exposition-format content type.
pub const CONTENT_TYPE: &str = "text/plain; version=0.0.4; charset=utf-8";

/// Render a `LiveMetrics` snapshot (Phase 1d extension) as
/// Prometheus summary lines for the landing-latency histogram.
/// Returns the substring to append to a `/metrics` body.
///
/// This is intentionally separate from `render_snapshot` because
/// `LiveMetrics` (in `dl-executor`) and `MetricsRegistry` (in
/// `dl-app`) are two independent metric systems; merging them is
/// future work (see plan §"Open questions").
pub fn render_live_metrics_prom(live: &LiveMetrics) -> String {
    let snap = live.landing_latency_snapshot();
    render_landing_latency_summary("dl_submission_to_landing_ms", &snap)
}

/// Render a [`LandingLatencySnapshot`] as Prometheus summary
/// quantile lines. The name is the metric name (e.g.
/// `dl_submission_to_landing_ms`).
pub fn render_landing_latency_summary(name: &str, snap: &LandingLatencySnapshot) -> String {
    let mut out = String::new();
    let _ = writeln!(
        out,
        "# HELP {name} Time from Jito submit call to LandingResult::Landed observation (milliseconds)"
    );
    let _ = writeln!(out, "# TYPE {name} summary");
    if snap.count == 0 {
        // Empty histogram: emit zero-valued quantile lines.
        let _ = writeln!(out, "{name}{{quantile=\"0.5\"}} 0");
        let _ = writeln!(out, "{name}{{quantile=\"0.95\"}} 0");
        let _ = writeln!(out, "{name}_count 0");
        let _ = writeln!(out, "{name}_sum 0");
        return out;
    }
    let _ = writeln!(out, "{name}{{quantile=\"0.5\"}} {}", snap.p50_ms);
    let _ = writeln!(out, "{name}{{quantile=\"0.95\"}} {}", snap.p95_ms);
    let _ = writeln!(out, "{name}_count {}", snap.count);
    let _ = writeln!(out, "{name}_sum {}", snap.sum_ms);
    out
}

/// A `MetricsSink` impl that renders the registry's snapshot
/// to Prometheus text format on demand.
///
/// Construction is `Arc<MetricsPrometheus>::new(registry)`.
/// The registry updates trigger per-update notifications via
/// the `MetricsSink` trait; the Prometheus output is built
/// lazily by [`MetricsPrometheus::render`].
pub struct MetricsPrometheus {
    registry: Arc<MetricsRegistry>,
}

impl MetricsPrometheus {
    /// New Prometheus sink bound to a registry.
    pub fn new(registry: Arc<MetricsRegistry>) -> Self {
        Self { registry }
    }

    /// Render the current registry snapshot as Prometheus
    /// text-format. Integer-only: all values are `u64`. The
    /// `Display` formatting via `write!` is the only
    /// non-integer arithmetic in this module.
    pub fn render(&self) -> String {
        let snap = self.registry.snapshot();
        render_snapshot(&snap)
    }
}

impl crate::metrics::MetricsSink for MetricsPrometheus {
    fn counter_published(&self, _name: &'static str, _value: u64) {
        // No-op: render() reads the current snapshot on demand.
    }
    fn gauge_published(&self, _name: &'static str, _value: u64) {
        // No-op.
    }
    fn histogram_observed(&self, _name: &'static str, _sum: u64, _count: u64, _buckets: &[u64]) {
        // No-op.
    }
    fn flush(&self) {
        // No-op: render() is on-demand.
    }
}

/// Render a snapshot as Prometheus text format. Pure
/// function: no state. Integer-only. The bucket boundaries
/// are the standard Prometheus power-of-2 sequence:
/// `1, 2, 4, 8, 16, ..., 2^10, +Inf`. The underflow and
/// overflow buckets are reported as `le="-Inf"` (we omit
/// it for the underflow bucket since the SDK convention
/// is to start at `le="1"`) and `le="+Inf"` (which is the
/// count of all observations, including the overflow).
pub fn render_snapshot(snap: &MetricsSnapshot) -> String {
    let mut out = String::new();
    for (name, value) in &snap.counters {
        let _ = writeln!(out, "# HELP {name} counter");
        let _ = writeln!(out, "# TYPE {name} counter");
        let _ = writeln!(out, "{name} {value}");
    }
    for (name, value) in &snap.gauges {
        let _ = writeln!(out, "# HELP {name} gauge");
        let _ = writeln!(out, "# TYPE {name} gauge");
        let _ = writeln!(out, "{name} {value}");
    }
    for (name, buckets) in &snap.histograms {
        let _ = writeln!(out, "# HELP {name} histogram");
        let _ = writeln!(out, "# TYPE {name} histogram");
        // Bucket bounds: 1, 2, 4, 8, ..., 2^(MAX_BUCKETS-2), +Inf.
        // We emit cumulative counts: the +Inf bucket holds the
        // total observation count. The underflow bucket (le=0)
        // is reported as `le="1"` since the SDK's bucket
        // ordering starts at 1.
        let mut cumulative: u64 = 0;
        let num_buckets = buckets.len();
        // Power-of-2 bounds: 2^0, 2^1, ..., 2^(num_buckets-1).
        for (i, count) in buckets.iter().enumerate() {
            cumulative = cumulative.saturating_add(*count);
            // Upper bound is 2^i.
            let bound = 1u128 << i;
            if i + 1 == num_buckets {
                // Last bucket: +Inf.
                let _ = writeln!(out, "{name}_bucket{{le=\"+Inf\"}} {cumulative}");
            } else {
                let _ = writeln!(out, "{name}_bucket{{le=\"{bound}\"}} {cumulative}");
            }
        }
        // Histogram sum and count.
        let total: u64 = buckets.iter().copied().sum();
        let _ = writeln!(out, "{name}_sum {total}");
        let _ = writeln!(out, "{name}_count {total}");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::metrics::{MetricsRegistry, RegistryCounter, RegistryGauge, RegistryHistogram};
    use std::sync::Arc;

    #[test]
    fn empty_registry_renders_empty_body() {
        let r = Arc::new(MetricsRegistry::new());
        let s = Arc::new(MetricsPrometheus::new(r));
        assert_eq!(s.render(), "");
    }

    #[test]
    fn counter_renders_with_help_and_type() {
        let r = Arc::new(MetricsRegistry::new());
        let c = RegistryCounter::new(r.clone(), "opps_per_sec");
        c.inc();
        c.add(2);
        let s = Arc::new(MetricsPrometheus::new(r));
        let body = s.render();
        assert!(body.contains("# HELP opps_per_sec counter"));
        assert!(body.contains("# TYPE opps_per_sec counter"));
        assert!(body.contains("opps_per_sec 3"));
    }

    #[test]
    fn gauge_renders_with_help_and_type() {
        let r = Arc::new(MetricsRegistry::new());
        let g = RegistryGauge::new(r.clone(), "queue_depth");
        g.set(7);
        let s = Arc::new(MetricsPrometheus::new(r));
        let body = s.render();
        assert!(body.contains("# HELP queue_depth gauge"));
        assert!(body.contains("queue_depth 7"));
    }

    #[test]
    fn histogram_renders_buckets_sum_and_count() {
        let r = Arc::new(MetricsRegistry::new());
        let h = RegistryHistogram::new(r.clone(), "detection_latency_us");
        h.observe(100);
        h.observe(100);
        h.observe(1000);
        let s = Arc::new(MetricsPrometheus::new(r));
        let body = s.render();
        assert!(body.contains("# HELP detection_latency_us histogram"));
        assert!(body.contains("# TYPE detection_latency_us histogram"));
        // All three observations are in the underflow /
        // first few buckets; the cumulative line for
        // le="+Inf" should equal 3.
        assert!(body.contains("detection_latency_us_count 3"));
        assert!(body.contains("detection_latency_us_bucket{le=\"+Inf\"} 3"));
    }

    #[test]
    fn multiple_metrics_appear_in_render() {
        let r = Arc::new(MetricsRegistry::new());
        let c1 = RegistryCounter::new(r.clone(), "opps_per_sec");
        c1.inc();
        let c2 = RegistryCounter::new(r.clone(), "would_trade");
        c2.add(5);
        let g = RegistryGauge::new(r.clone(), "active_pools");
        g.set(42);
        let s = Arc::new(MetricsPrometheus::new(r));
        let body = s.render();
        assert!(body.contains("opps_per_sec 1"));
        assert!(body.contains("would_trade 5"));
        assert!(body.contains("active_pools 42"));
    }

    #[test]
    fn content_type_is_prometheus_text() {
        assert_eq!(CONTENT_TYPE, "text/plain; version=0.0.4; charset=utf-8");
    }

    #[test]
    fn render_snapshot_is_pure_function() {
        // Two snapshots with the same data should produce
        // identical output (rendering is deterministic).
        let r = Arc::new(MetricsRegistry::new());
        let c = RegistryCounter::new(r.clone(), "test");
        c.add(42);
        let snap1 = r.snapshot();
        let snap2 = r.snapshot();
        assert_eq!(render_snapshot(&snap1), render_snapshot(&snap2));
    }

    #[test]
    fn sink_is_object_safe_and_implements_metricssink() {
        use crate::metrics::MetricsSink;
        let r = Arc::new(MetricsRegistry::new());
        let s = Arc::new(MetricsPrometheus::new(r));
        // Object-safe: trait dispatch through Arc<dyn MetricsSink>.
        let trait_obj: Arc<dyn MetricsSink> = s.clone();
        trait_obj.counter_published("test", 1);
        trait_obj.flush();
    }

    #[test]
    fn render_live_metrics_prom_handles_empty() {
        let live = LiveMetrics::new();
        let body = render_live_metrics_prom(&live);
        assert!(body.contains("# HELP dl_submission_to_landing_ms"));
        assert!(body.contains("# TYPE dl_submission_to_landing_ms summary"));
        assert!(body.contains("dl_submission_to_landing_ms_count 0"));
        assert!(body.contains("dl_submission_to_landing_ms_sum 0"));
    }

    #[test]
    fn render_live_metrics_prom_handles_populated() {
        let live = LiveMetrics::new();
        for i in 1..=100 {
            live.record_landing_latency_ms(i * 5); // 5, 10, ..., 500 ms
        }
        let body = render_live_metrics_prom(&live);
        assert!(body.contains("dl_submission_to_landing_ms_count 100"));
        assert!(body.contains("dl_submission_to_landing_ms_sum 25250")); // 5+10+...+500
        assert!(body.contains("dl_submission_to_landing_ms{quantile=\"0.5\"}"));
        assert!(body.contains("dl_submission_to_landing_ms{quantile=\"0.95\"}"));
        // p50 of [5..=500 step 5]: pos = 0.5 * 99 = 49.5
        // interpolated between sorted[49]=250 and sorted[50]=255,
        // result = 250 + 0.5 * 5 = 252.5 → 252 (u64 cast).
        assert!(body.contains("dl_submission_to_landing_ms{quantile=\"0.5\"} 252"));
    }
}
