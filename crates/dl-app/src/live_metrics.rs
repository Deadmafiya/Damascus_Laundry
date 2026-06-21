//! Adapter that forwards `dl-executor::LiveMetrics` counters
//! and `dl-signer::CapState` into the `dl-app` metrics
//! registry so the four DAM-68 alert series appear in the
//! Prometheus `/metrics` body.
//!
//! ## Why a periodic poller (not a push hook)
//!
//! `LiveMetrics` is updated by the live cycle path; a push
//! hook would need an `Arc<MetricsRegistry>` threaded into
//! `dl-executor` and `dl-signer`, which forces those crates
//! to depend on the registry type. A poller reads the
//! atomics and the cap state every `poll_interval` and
//! writes the latest values into `RegistryGauge` /
//! `RegistryCounter`. The registry counter has no `set` so
//! the poller tracks the last absolute value and emits
//! deltas via `add()`.
//!
//! ## Metric names
//!
//! - `dl_jito_submit_total` (counter) — total Jito bundle
//!   submissions. Mirrors `LiveMetrics::bundles_submitted`.
//! - `dl_jito_landed_total` (counter) — total successful
//!   landings. Mirrors `LiveMetrics::bundles_landed`.
//! - `dl_daily_cap_remaining_lamports` (gauge) — current
//!   daily cap minus `sol_spent_lamports`. Source:
//!   `CapState::remaining()`. Resets at UTC midnight per
//!   `CapState::new()` semantics.
//! - `dl_realized_pnl_sol` (gauge) — signed realized PnL
//!   since process start, in **SOL** (not lamports). The
//!   live cycle path updates `PnLTracker` after every
//!   successful landing; the adapter snapshots the
//!   cumulative value. Negative values are bitcast through
//!   `f64::to_bits` because the registry is integer-only —
//!   this is the standard Prometheus convention for
//!   f64-as-u64 gauges.

use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use dl_executor::metrics::LiveMetrics;
use dl_signer::cap::CapState;

use crate::metrics::{MetricsRegistry, RegistryCounter, RegistryGauge};

/// PnL tracker. The live cycle path calls `add` after every
/// successful landing with `(sol_received_lamports -
/// sol_spent_lamports)` for that cycle. The adapter
/// snapshots the running total via `current_lamports()`
/// and converts it to SOL.
pub struct PnLTracker {
    cumulative_lamports: AtomicI64,
}

impl PnLTracker {
    /// Create a new tracker starting at zero.
    pub fn new() -> Self {
        Self { cumulative_lamports: AtomicI64::new(0) }
    }

    /// Add a per-cycle PnL delta (signed, lamports).
    /// `delta > 0` means profit, `delta < 0` means loss.
    /// Saturates at `i64::MIN` / `i64::MAX`.
    pub fn add(&self, delta_lamports: i64) {
        self.cumulative_lamports
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |cur| {
                Some(cur.saturating_add(delta_lamports))
            })
            .ok();
    }

    /// Current cumulative PnL, in lamports (signed).
    pub fn current_lamports(&self) -> i64 {
        self.cumulative_lamports.load(Ordering::Relaxed)
    }
}

impl Default for PnLTracker {
    fn default() -> Self {
        Self::new()
    }
}

/// Convert signed lamports to SOL (1 SOL = 1_000_000_000
/// lamports). Returns a f64.
fn lamports_to_sol(lamports: i64) -> f64 {
    let as_f64 = lamports as f64;
    as_f64 / 1_000_000_000.0_f64
}

/// Bitcast a f64 to u64 (preserving the bit pattern) for
/// storage in the integer-only `RegistryGauge`.
fn f64_to_u64_bits(v: f64) -> u64 {
    v.to_bits()
}

/// Adapter that periodically copies `LiveMetrics` and
/// `CapState` into the `MetricsRegistry`.
pub struct LiveMetricsAdapter {
    live: Arc<LiveMetrics>,
    cap_state: Arc<Mutex<CapState>>,
    pnl: Arc<PnLTracker>,
    submit_counter: RegistryCounter,
    landed_counter: RegistryCounter,
    cap_remaining_gauge: RegistryGauge,
    pnl_sol_gauge: RegistryGauge,
    /// Last absolute value of `bundles_submitted` we
    /// observed. Used to translate the source atomic
    /// (absolute count) into a delta that the
    /// `RegistryCounter` can apply via `add()`.
    last_submitted: AtomicU64,
    last_landed: AtomicU64,
}

impl LiveMetricsAdapter {
    /// Bind a new adapter to the source atomics, the cap
    /// state, the PnL tracker, and a registry. The four
    /// metrics are created (idempotent) on the registry.
    pub fn new(
        registry: Arc<MetricsRegistry>,
        live: Arc<LiveMetrics>,
        cap_state: Arc<Mutex<CapState>>,
        pnl: Arc<PnLTracker>,
    ) -> Self {
        let submit_counter =
            RegistryCounter::new(registry.clone(), "dl_jito_submit_total");
        let landed_counter =
            RegistryCounter::new(registry.clone(), "dl_jito_landed_total");
        let cap_remaining_gauge =
            RegistryGauge::new(registry.clone(), "dl_daily_cap_remaining_lamports");
        let pnl_sol_gauge =
            RegistryGauge::new(registry.clone(), "dl_realized_pnl_sol");
        Self {
            live,
            cap_state,
            pnl,
            submit_counter,
            landed_counter,
            cap_remaining_gauge,
            pnl_sol_gauge,
            last_submitted: AtomicU64::new(0),
            last_landed: AtomicU64::new(0),
        }
    }

    /// Copy current `LiveMetrics`, `CapState`, and PnL
    /// values into the registry counters / gauges. Cheap
    /// (four atomic reads + one mutex lock); safe to call
    /// from any thread.
    pub fn poll(&self) {
        // Counters: `RegistryCounter` only exposes
        // `inc` / `add` — there is no `set`. The
        // source-of-truth atomics are absolute, so we
        // track the last-observed absolute value and
        // apply the delta to the registry counter.
        let submitted = self.live.bundles_submitted.load(Ordering::Relaxed);
        let landed = self.live.bundles_landed.load(Ordering::Relaxed);

        let prev_submitted = self.last_submitted.load(Ordering::Relaxed);
        if submitted > prev_submitted {
            self.submit_counter.add(submitted - prev_submitted);
        }
        self.last_submitted.store(submitted, Ordering::Relaxed);

        let prev_landed = self.last_landed.load(Ordering::Relaxed);
        if landed > prev_landed {
            self.landed_counter.add(landed - prev_landed);
        }
        self.last_landed.store(landed, Ordering::Relaxed);

        // Cap remaining: read the cap state under a
        // cheap mutex lock.
        let cap_remaining = {
            let state = self.cap_state.lock().expect("cap state mutex");
            state.remaining()
        };
        self.cap_remaining_gauge.set(cap_remaining);

        // PnL: read the cumulative lamports, convert to
        // SOL, bitcast to u64.
        let pnl_lamports = self.pnl.current_lamports();
        let pnl_sol = lamports_to_sol(pnl_lamports);
        self.pnl_sol_gauge.set(f64_to_u64_bits(pnl_sol));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::metrics::MetricsRegistry;
    use dl_signer::cap::CapConfig;

    fn fresh_registry() -> Arc<MetricsRegistry> {
        Arc::new(MetricsRegistry::new())
    }

    fn fresh_live() -> Arc<LiveMetrics> {
        Arc::new(LiveMetrics::new())
    }

    fn fresh_cap() -> Arc<Mutex<CapState>> {
        Arc::new(Mutex::new(CapState::new(CapConfig::default())))
    }

    fn fresh_pnl() -> Arc<PnLTracker> {
        Arc::new(PnLTracker::new())
    }

    /// Build a full adapter with fresh state.
    fn fresh_adapter(
        reg: Arc<MetricsRegistry>,
    ) -> (LiveMetricsAdapter, Arc<LiveMetrics>, Arc<Mutex<CapState>>, Arc<PnLTracker>) {
        let live = fresh_live();
        let cap = fresh_cap();
        let pnl = fresh_pnl();
        let adapter = LiveMetricsAdapter::new(reg, live.clone(), cap.clone(), pnl.clone());
        (adapter, live, cap, pnl)
    }

    /// Look up a counter value in a `MetricsSnapshot`.
    fn snap_counter(snap: &crate::metrics::MetricsSnapshot, name: &str) -> u64 {
        snap.counters
            .iter()
            .find(|(n, _)| *n == name)
            .map(|(_, v)| *v)
            .unwrap_or(0)
    }

    /// Look up a gauge value in a `MetricsSnapshot`.
    fn snap_gauge(snap: &crate::metrics::MetricsSnapshot, name: &str) -> u64 {
        snap.gauges
            .iter()
            .find(|(n, _)| *n == name)
            .map(|(_, v)| *v)
            .unwrap_or(0)
    }

    #[test]
    fn poll_zeros_all_four_metrics() {
        let reg = fresh_registry();
        let (adapter, _live, _cap, _pnl) = fresh_adapter(reg.clone());

        adapter.poll();

        let snap = reg.snapshot();
        assert_eq!(snap_counter(&snap, "dl_jito_submit_total"), 0);
        assert_eq!(snap_counter(&snap, "dl_jito_landed_total"), 0);
        // Default cap: 5 SOL = 5_000_000_000 lamports, 0 spent.
        assert_eq!(
            snap_gauge(&snap, "dl_daily_cap_remaining_lamports"),
            5_000_000_000
        );
        // PnL = 0 SOL -> 0.0_f64 -> 0u64 bits.
        assert_eq!(snap_gauge(&snap, "dl_realized_pnl_sol"), 0);
    }

    #[test]
    fn poll_copies_bundle_counters() {
        let reg = fresh_registry();
        let live = fresh_live();
        let cap = fresh_cap();
        let pnl = fresh_pnl();
        live.bundles_submitted.fetch_add(7, Ordering::Relaxed);
        live.bundles_landed.fetch_add(3, Ordering::Relaxed);
        let adapter =
            LiveMetricsAdapter::new(reg.clone(), live.clone(), cap, pnl);

        adapter.poll();

        let snap = reg.snapshot();
        assert_eq!(snap_counter(&snap, "dl_jito_submit_total"), 7);
        assert_eq!(snap_counter(&snap, "dl_jito_landed_total"), 3);

        // Advance the atomics and re-poll: the counters
        // must apply the delta to the previous value
        // (7 + 5 = 12; 3 + 2 = 5).
        live.bundles_submitted.fetch_add(5, Ordering::Relaxed);
        live.bundles_landed.fetch_add(2, Ordering::Relaxed);
        adapter.poll();
        let snap = reg.snapshot();
        assert_eq!(snap_counter(&snap, "dl_jito_submit_total"), 12);
        assert_eq!(snap_counter(&snap, "dl_jito_landed_total"), 5);
    }

    #[test]
    fn poll_copies_daily_cap_remaining() {
        let reg = fresh_registry();
        // Use a non-default cap to verify the adapter
        // actually reads the cap_state, not a hard-coded
        // default.
        let cap = Arc::new(Mutex::new(CapState::new(CapConfig {
            daily_lamports: 1_000_000_000, // 1 SOL
            per_bundle_lamports: 100_000_000, // 0.1 SOL
        })));
        let pnl = fresh_pnl();
        let live = fresh_live();
        let adapter =
            LiveMetricsAdapter::new(reg.clone(), live, cap.clone(), pnl);

        adapter.poll();

        let snap = reg.snapshot();
        // Cap is 1 SOL = 1_000_000_000, nothing spent yet.
        assert_eq!(
            snap_gauge(&snap, "dl_daily_cap_remaining_lamports"),
            1_000_000_000
        );
    }

    #[test]
    fn poll_pnl_zero_when_balanced() {
        let reg = fresh_registry();
        let (adapter, _live, _cap, pnl) = fresh_adapter(reg.clone());

        // +1 SOL received, -1 SOL spent — net zero.
        pnl.add(1_000_000_000);
        pnl.add(-1_000_000_000);

        adapter.poll();

        let snap = reg.snapshot();
        assert_eq!(snap_gauge(&snap, "dl_realized_pnl_sol"), 0);
    }

    #[test]
    fn poll_pnl_positive_when_received_exceeds_spent() {
        let reg = fresh_registry();
        let (adapter, _live, _cap, pnl) = fresh_adapter(reg.clone());

        // +0.5 SOL net.
        pnl.add(500_000_000);
        adapter.poll();

        let snap = reg.snapshot();
        let bits = snap_gauge(&snap, "dl_realized_pnl_sol");
        let as_f64 = f64::from_bits(bits);
        assert!(
            (as_f64 - 0.5).abs() < 1e-9,
            "expected ~0.5 SOL, got {as_f64}"
        );
    }

    #[test]
    fn poll_pnl_negative_bitcast_when_spent_exceeds_received() {
        let reg = fresh_registry();
        let (adapter, _live, _cap, pnl) = fresh_adapter(reg.clone());

        // -0.25 SOL net. The integer-only `RegistryGauge`
        // bitcasts the f64 to u64, so the gauge carries
        // the IEEE-754 sign bit and the f64 reads back
        // correctly on the prom side.
        pnl.add(-250_000_000);
        adapter.poll();

        let snap = reg.snapshot();
        let bits = snap_gauge(&snap, "dl_realized_pnl_sol");
        let as_f64 = f64::from_bits(bits);
        assert!(
            (as_f64 + 0.25).abs() < 1e-9,
            "expected ~-0.25 SOL, got {as_f64}"
        );
        // Sanity: the top bit (sign) is set for any
        // negative f64, so the bitcast u64 is greater
        // than `i64::MAX as u64`.
        assert!(bits > (i64::MAX as u64));
    }

    #[test]
    fn poll_reports_landed_le_submitted() {
        // The alert `DlLandingRate` invariant:
        // `dl_jito_landed_total <= dl_jito_submit_total`
        // always. The source atomics are monotonic and
        // the live path only increments `bundles_landed`
        // after a prior `bundles_submitted`, so the
        // adapter inherits the invariant.
        let reg = fresh_registry();
        let live = fresh_live();
        let cap = fresh_cap();
        let pnl = fresh_pnl();
        // Landed = 4, Submitted = 4 (equal is allowed:
        // every submitted bundle landed).
        live.bundles_submitted.fetch_add(4, Ordering::Relaxed);
        live.bundles_landed.fetch_add(4, Ordering::Relaxed);
        let adapter =
            LiveMetricsAdapter::new(reg.clone(), live, cap, pnl);

        adapter.poll();

        let snap = reg.snapshot();
        let submitted = snap_counter(&snap, "dl_jito_submit_total");
        let landed = snap_counter(&snap, "dl_jito_landed_total");
        assert!(landed <= submitted);
    }

    #[test]
    fn poll_respects_custom_cap_config() {
        // The adapter reads `CapState::remaining()`, not
        // a hard-coded constant. Use a tiny cap to
        // distinguish.
        let reg = fresh_registry();
        let cap = Arc::new(Mutex::new(CapState::new(CapConfig {
            daily_lamports: 42,
            per_bundle_lamports: 7,
        })));
        let pnl = fresh_pnl();
        let live = fresh_live();
        let adapter =
            LiveMetricsAdapter::new(reg.clone(), live, cap, pnl);

        adapter.poll();

        let snap = reg.snapshot();
        assert_eq!(
            snap_gauge(&snap, "dl_daily_cap_remaining_lamports"),
            42
        );
    }

    #[test]
    fn repeated_poll_is_idempotent() {
        // Idempotency matters: the 1Hz poller must not
        // double-count or drift if the source atomics
        // don't change between polls.
        let reg = fresh_registry();
        let (adapter, _live, _cap, pnl) = fresh_adapter(reg.clone());

        pnl.add(100_000_000); // +0.1 SOL
        adapter.poll();
        let snap1 = reg.snapshot();
        let cap1 = snap_gauge(&snap1, "dl_daily_cap_remaining_lamports");
        let pnl1 = snap_gauge(&snap1, "dl_realized_pnl_sol");
        let sub1 = snap_counter(&snap1, "dl_jito_submit_total");
        let land1 = snap_counter(&snap1, "dl_jito_landed_total");

        // Poll again without changing source state.
        adapter.poll();
        let snap2 = reg.snapshot();
        assert_eq!(cap1, snap_gauge(&snap2, "dl_daily_cap_remaining_lamports"));
        assert_eq!(pnl1, snap_gauge(&snap2, "dl_realized_pnl_sol"));
        assert_eq!(sub1, snap_counter(&snap2, "dl_jito_submit_total"));
        assert_eq!(land1, snap_counter(&snap2, "dl_jito_landed_total"));
    }

    #[test]
    fn four_target_series_appear_in_prom_body() {
        // End-to-end: poll the adapter, then render the
        // registry's snapshot via the prom renderer, and
        // confirm all four series show up in the output
        // text. This is the acceptance test: with the
        // adapter in the run path, `dl-app metrics prom`
        // emits all four `dl_*` lines on `/metrics`.
        use crate::metrics_prom::render_snapshot;

        let reg = fresh_registry();
        let live = fresh_live();
        let cap = fresh_cap();
        let pnl = fresh_pnl();

        live.bundles_submitted.fetch_add(11, Ordering::Relaxed);
        live.bundles_landed.fetch_add(7, Ordering::Relaxed);
        pnl.add(123_456_789); // +0.123456789 SOL

        let adapter =
            LiveMetricsAdapter::new(reg.clone(), live, cap, pnl);
        adapter.poll();

        let snap = reg.snapshot();
        let body = render_snapshot(&snap);

        for name in &[
            "dl_jito_submit_total",
            "dl_jito_landed_total",
            "dl_daily_cap_remaining_lamports",
            "dl_realized_pnl_sol",
        ] {
            assert!(
                body.contains(name),
                "prom body missing `{name}`:\n{body}"
            );
        }
    }
}
