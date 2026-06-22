//! Reconnect backoff policy — pure-Rust, no async, no clock.
//! Integer-only (no f32/f64) to comply with the dl-feed value-path
//! no-floats invariant.
//!
//! Used by `ws_feed::run_ws_task` on disconnect. The policy is
//! parameterized at task start; the actual `sleep(d)` is done in
//! the async task. This split keeps the policy unit-testable:
//! `next_backoff` is a pure function of `(attempt, base, cap, jitter_bps)`.
//!
//! ## Defaults
//!
//! - `base = 100ms`
//! - `cap  = 30s`
//! - `jitter_bps = 1000` (±10%) — caller may override.
//!
//! Exponential growth: `delay = min(cap, base * 2^attempt)`.
//! When `jitter_bps > 0`, the actual sleep is `delay * (bps ± r) / bps`
//! where `r` is the caller-supplied jitter seed in `[-bps, +bps]`
//! (integer; deterministic in tests).
//!
//! ## Reconnect-storm detection
//!
//! A "storm" is `> storm_threshold` reconnect attempts within
//! `storm_window` slots. The WS task calls `record_attempt(slot)`
//! on every attempt and consults `is_storm()` before the next
//! sleep — storms are surfaced via the `reconnect_storm_count`
//! metric and emitted as a tracing `warn!`.

use std::time::Duration;

/// Tunable reconnect policy. All arithmetic is u64.
#[derive(Debug, Clone)]
pub struct ReconnectPolicy {
    /// First retry sleep. Subsequent retries double, capped at `cap`.
    pub base: Duration,
    /// Maximum single sleep. (Avoids 2^attempt overflowing into
    /// multi-minute waits after sustained outages.)
    pub cap: Duration,
    /// Jitter amplitude in basis points (1/10000). `1000` = ±10%.
    /// `0` disables jitter. Range: `[0, 10000]`.
    pub jitter_bps: u64,
    /// Reconnect-storm threshold: more than this many attempts in
    /// the storm window → counted as a storm. Defaults to 5.
    pub storm_threshold: u64,
    /// Storm detection window in slots. Defaults to 150 (~60s of
    /// Solana slots at ~400ms each).
    pub storm_window: u64,
    /// Maximum number of consecutive reconnect attempts before
    /// giving up. `None` = retry forever. The default is `None`.
    pub max_attempts: Option<u64>,
}

impl Default for ReconnectPolicy {
    fn default() -> Self {
        Self {
            base: Duration::from_millis(100),
            cap: Duration::from_secs(30),
            jitter_bps: 1000,
            storm_threshold: 5,
            storm_window: 150,
            max_attempts: None,
        }
    }
}

impl ReconnectPolicy {
    /// Pure backoff computation. `attempt` is 0-indexed: 0 is the
    /// first retry, 1 the second, etc. Output is bounded by `cap`.
    /// `jitter_seed` is an integer in `[-jitter_bps, +jitter_bps]`
    /// (the caller is expected to derive it from a local RNG; tests
    /// pass fixed values).
    pub fn next_backoff(&self, attempt: u64, jitter_seed: Option<i64>) -> Duration {
        // 2^attempt saturates at u64::MAX. Clamp the shift amount
        // so the math stays u64.
        let factor = 1u64
            .checked_shl(attempt.min(63) as u32)
            .unwrap_or(u64::MAX);
        let nanos_base = self.base.as_nanos() as u64;
        let nanos_raw = nanos_base.saturating_mul(factor);
        let nanos_capped = nanos_raw.min(self.cap.as_nanos() as u64);
        if self.jitter_bps == 0 {
            return Duration::from_nanos(nanos_capped);
        }
        // Apply jitter: jitter_seed in `[-jitter_bps, +jitter_bps]`
        // scales the delay by `(bps + seed) / bps`. The "bps"
        // component gives the baseline 1.0; the "seed" component
        // adds the ±jitter. e.g. bps=1000 (10%), seed=+1000 →
        // scale = (1000+1000)/1000 = 2.0, i.e. +100% — far too
        // much. We want +10% = scale 1.1, so the right formula
        // is `(bps*10 + seed) / (bps*10)` — no, simpler: jitter
        // is defined as ±jitter_bps, so scale = (10000 + seed*10)
        // / 10000. The 10000 represents 100% in bps×10 units.
        let bps = self.jitter_bps as i64;
        let seed = jitter_seed.unwrap_or(0).clamp(-bps, bps);
        // The seed range is `[-bps, +bps]`, where bps is the
        // maximum jitter amplitude. A seed of `+bps` means "the
        // full +jitter", a seed of `-bps` means "the full
        // -jitter", and a seed of 0 means "no jitter". Concretely
        // with bps=1000 (10%): seed=1000 → scale 1.1, seed=500
        // → scale 1.05. The fixed scale denominator is 10000
        // (= 100%), so 100 units = 1%. seed/10000 = percentage
        // adjustment.
        let bps_scaled: i64 = 10000;
        let scale_num: i64 = bps_scaled.saturating_add(seed);
        let scale_den: i64 = bps_scaled;
        let nanos_i = (nanos_capped as i128) * (scale_num as i128) / (scale_den as i128);
        let nanos_capped_i = self.cap.as_nanos() as i128;
        let nanos = if nanos_i < 0 {
            0u64
        } else if nanos_i > nanos_capped_i {
            self.cap.as_nanos() as u64
        } else {
            nanos_i as u64
        };
        Duration::from_nanos(nanos)
    }

    /// True if the policy is exhausted (caller should stop retrying).
    pub fn is_exhausted(&self, attempt: u64) -> bool {
        matches!(self.max_attempts, Some(max) if attempt >= max)
    }
}

/// Tracks recent reconnect attempts and detects storms.
///
/// The WS task calls `record_attempt(slot)` on every attempt and
/// `is_storm()` to decide whether to surface a storm warning.
#[derive(Debug, Default, Clone)]
pub struct ReconnectStormDetector {
    threshold: u64,
    window: u64,
    /// Slots of recent attempts. Older entries are pruned on each
    /// `record_attempt`.
    attempts: Vec<u64>,
    /// Total storm events seen across the lifetime of this detector.
    storm_count: u64,
}

impl ReconnectStormDetector {
    /// Build with the policy's threshold and window. Pass `0, 0`
    /// to disable storm detection (every attempt is its own event).
    pub fn new(threshold: u64, window: u64) -> Self {
        Self {
            threshold,
            window,
            attempts: Vec::new(),
            storm_count: 0,
        }
    }

    /// Record one reconnect attempt at `slot`. Returns `true` if
    /// this attempt pushes the count over the storm threshold (i.e.
    /// a storm is now in progress).
    pub fn record_attempt(&mut self, slot: u64) -> bool {
        if self.window > 0 {
            let cutoff = slot.saturating_sub(self.window);
            self.attempts.retain(|&s| s > cutoff);
        }
        self.attempts.push(slot);
        if self.threshold > 0 && self.attempts.len() as u64 > self.threshold {
            self.storm_count += 1;
            true
        } else {
            false
        }
    }

    /// True if more than `threshold` attempts are within the window.
    pub fn is_storm(&self) -> bool {
        self.threshold > 0 && self.attempts.len() as u64 > self.threshold
    }

    /// Total storms observed so far.
    pub fn storm_count(&self) -> u64 {
        self.storm_count
    }

    /// Current attempt count within the window.
    pub fn attempts_in_window(&self) -> usize {
        self.attempts.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_doubles_each_attempt() {
        let p = ReconnectPolicy {
            base: Duration::from_millis(100),
            cap: Duration::from_secs(10),
            jitter_bps: 0,
            ..ReconnectPolicy::default()
        };
        assert_eq!(p.next_backoff(0, None), Duration::from_millis(100));
        assert_eq!(p.next_backoff(1, None), Duration::from_millis(200));
        assert_eq!(p.next_backoff(2, None), Duration::from_millis(400));
        assert_eq!(p.next_backoff(3, None), Duration::from_millis(800));
    }

    #[test]
    fn backoff_caps_at_cap() {
        let p = ReconnectPolicy {
            base: Duration::from_millis(100),
            cap: Duration::from_secs(1),
            jitter_bps: 0,
            ..ReconnectPolicy::default()
        };
        assert_eq!(p.next_backoff(4, None), Duration::from_millis(1000));
        assert_eq!(p.next_backoff(10, None), Duration::from_millis(1000));
    }

    #[test]
    fn backoff_with_jitter_is_deterministic_given_seed() {
        let p = ReconnectPolicy {
            base: Duration::from_millis(100),
            cap: Duration::from_secs(10),
            jitter_bps: 1000, // ±10%
            ..ReconnectPolicy::default()
        };
        // +10% of 100ms = 110ms (seed = +1000 bps).
        assert_eq!(p.next_backoff(0, Some(1000)), Duration::from_millis(110));
        // -10% of 100ms = 90ms (seed = -1000 bps).
        assert_eq!(p.next_backoff(0, Some(-1000)), Duration::from_millis(90));
        // r = 0 → no jitter.
        assert_eq!(p.next_backoff(0, Some(0)), Duration::from_millis(100));
        // +5% of 100ms = 105ms (seed = +500 bps).
        assert_eq!(p.next_backoff(0, Some(500)), Duration::from_millis(105));
    }

    #[test]
    fn backoff_jitter_cannot_exceed_cap() {
        let p = ReconnectPolicy {
            base: Duration::from_millis(100),
            cap: Duration::from_millis(500),
            jitter_bps: 10000, // ±100%
            ..ReconnectPolicy::default()
        };
        let d = p.next_backoff(10, Some(10000));
        assert!(d <= Duration::from_millis(500));
    }

    #[test]
    fn exhaustion_stops_at_max_attempts() {
        let p = ReconnectPolicy {
            max_attempts: Some(3),
            ..ReconnectPolicy::default()
        };
        assert!(!p.is_exhausted(0));
        assert!(!p.is_exhausted(2));
        assert!(p.is_exhausted(3));
        assert!(p.is_exhausted(100));
    }

    #[test]
    fn storm_detector_counts_attempts_in_window() {
        let mut s = ReconnectStormDetector::new(3, 100);
        assert!(!s.record_attempt(10));
        assert!(!s.record_attempt(20));
        assert!(!s.record_attempt(30));
        assert!(s.record_attempt(40));
        assert_eq!(s.storm_count(), 1);
        assert!(s.is_storm());
    }

    #[test]
    fn storm_detector_prunes_older_attempts() {
        let mut s = ReconnectStormDetector::new(2, 50);
        s.record_attempt(10);
        s.record_attempt(20);
        s.record_attempt(30);
        s.record_attempt(100);
        assert_eq!(s.attempts_in_window(), 1);
        assert!(!s.is_storm());
    }

    #[test]
    fn storm_detector_disabled_with_zero_threshold() {
        let mut s = ReconnectStormDetector::new(0, 0);
        for i in 0..1000 {
            s.record_attempt(i);
        }
        assert!(!s.is_storm());
        assert_eq!(s.storm_count(), 0);
    }
}
