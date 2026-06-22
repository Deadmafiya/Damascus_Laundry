//! Staleness guard — halts trading on stale pool state.
//!
//! Phase 3 hardening: if a subscribed pool/vault fails to produce
//! an `AccountUpdate` for longer than `max_staleness_slots`, the
//! guard trips and the `WsFeed` background task emits a
//! `FeedEvent::StalePoolHalt` and stops sending further events.
//! Trading must be halted on a `StalePoolHalt` because pricing data
//! is no longer trustworthy (a half-updated pool is worse than no
//! data — it produces false arbitrage opportunities).
//!
//! ## Pure Rust, no clock
//!
//! The guard does not look at wall time. Slot-based staleness is
//! the only correct metric: a connection that's "alive" but stuck
//! in a 5-minute-slot outage is just as bad as a dropped connection.
//!
//! ## Stale-detection rule
//!
//! A pool is "stale" when `(now_slot - last_seen_slot) > max_staleness_slots`.
//! `now_slot` is the most recent slot seen on *any* subscribed
//! account, advanced by `notify_slot_advance(slot)`. The guard
//! also trips immediately if `now_slot` advances past the threshold
//! with no notification at all (the "all silent" case).
//!
//! ## Trip semantics
//!
//! Once tripped, the guard stays tripped (one-shot). `tripped()` and
//! `tripped_at()` report the trip; `update()` and `notify_*()` are
//! no-ops after trip. This matches the safety principle: a halt is
//! sticky; recovery requires operator action (a config reload +
//! process restart is the expected path).

/// Outcome of a staleness check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StalenessResult {
    /// The pool(s) are fresh — no halt.
    Fresh,
    /// A specific pool's last update is older than the threshold.
    Stale {
        /// Pubkey of the pool (vault or pool account) that went stale.
        pubkey: [u8; 32],
        /// Slot at which the stale pool was last seen.
        last_seen_slot: u64,
        /// Number of slots since the last update.
        staleness_slots: u64,
    },
}

/// Tracks last-seen slots for a set of subscribed accounts and
/// detects "stale" pools (no update within `max_staleness_slots`).
#[derive(Debug, Clone)]
pub struct StalenessGuard {
    max_staleness_slots: u64,
    /// Last-seen slot per pubkey. BTreeMap for deterministic
    /// iteration in tests.
    last_seen: std::collections::BTreeMap<[u8; 32], u64>,
    /// Most recent slot observed on any subscription. Defaults to 0
    /// (bot just started, no data yet — guard does not trip until
    /// a slot has actually been seen).
    now_slot: u64,
    /// Set after the first trip. Sticky.
    tripped: bool,
    /// The pubkey that caused the trip, if any.
    tripped_for: Option<[u8; 32]>,
    /// Slot at which the trip occurred.
    tripped_at_slot: u64,
    /// Number of slots the tripper had been silent.
    tripped_staleness: u64,
    /// Total trips in the lifetime of this guard. Drives the
    /// `stale_pool_count` metric.
    trip_count: u64,
}

impl StalenessGuard {
    /// Build a guard with the given max-staleness threshold in slots.
    /// `0` disables the guard (every update is fresh).
    pub fn new(max_staleness_slots: u64) -> Self {
        Self {
            max_staleness_slots,
            last_seen: std::collections::BTreeMap::new(),
            now_slot: 0,
            tripped: false,
            tripped_for: None,
            tripped_at_slot: 0,
            tripped_staleness: 0,
            trip_count: 0,
        }
    }

    /// Register a new subscribed account. The pubkey starts at slot
    /// 0 (never seen); the guard will not trip on it until at least
    /// one update arrives.
    pub fn register(&mut self, pubkey: [u8; 32]) {
        self.last_seen.entry(pubkey).or_insert(0);
    }

    /// Number of registered subscriptions.
    pub fn registered(&self) -> usize {
        self.last_seen.len()
    }

    /// Record an `AccountUpdate` for `pubkey` at `slot`. Returns
    /// `true` if this update trips the guard (i.e. a different
    /// subscribed account was already older than the threshold).
    /// After a trip, subsequent updates return `false` (sticky).
    pub fn update(&mut self, pubkey: [u8; 32], slot: u64) -> bool {
        if self.tripped {
            return false;
        }
        if slot > self.now_slot {
            self.now_slot = slot;
        }
        self.last_seen.insert(pubkey, slot);
        self.check_inner()
    }

    /// Advance the global slot clock without updating any specific
    /// account. Returns `true` if this advance trips the guard.
    /// Used when a `FeedEvent::Slot` arrives with no associated
    /// account update.
    pub fn notify_slot_advance(&mut self, slot: u64) -> bool {
        if self.tripped {
            return false;
        }
        if slot > self.now_slot {
            self.now_slot = slot;
        }
        self.check_inner()
    }

    /// Run the staleness check against the current `now_slot`.
    fn check_inner(&mut self) -> bool {
        if self.tripped {
            return false;
        }
        if self.max_staleness_slots == 0 {
            return false;
        }
        if self.now_slot == 0 {
            return false;
        }
        let mut worst: Option<([u8; 32], u64)> = None;
        for (&pk, &last) in &self.last_seen {
            if last == 0 {
                continue;
            }
            let age = self.now_slot.saturating_sub(last);
            if age > self.max_staleness_slots {
                match worst {
                    Some((_, w_last)) if last >= w_last => {}
                    _ => worst = Some((pk, last)),
                }
            }
        }
        if let Some((pk, last)) = worst {
            let staleness = self.now_slot.saturating_sub(last);
            self.trip(pk, last, staleness);
            true
        } else {
            false
        }
    }

    fn trip(&mut self, pubkey: [u8; 32], last_seen_slot: u64, staleness: u64) {
        self.tripped = true;
        self.tripped_for = Some(pubkey);
        self.tripped_at_slot = self.now_slot;
        self.tripped_staleness = staleness;
        self.trip_count += 1;
    }

    /// True if the guard has tripped (sticky after first trip).
    pub fn tripped(&self) -> bool {
        self.tripped
    }

    /// Pubkey that caused the trip, if any.
    pub fn tripped_for(&self) -> Option<[u8; 32]> {
        self.tripped_for
    }

    /// Slot at which the trip occurred.
    pub fn tripped_at_slot(&self) -> u64 {
        self.tripped_at_slot
    }

    /// Number of slots the tripper was silent at trip time.
    pub fn tripped_staleness(&self) -> u64 {
        self.tripped_staleness
    }

    /// Total trips observed so far. Drives the `stale_pool_count`
    /// metric.
    pub fn trip_count(&self) -> u64 {
        self.trip_count
    }

    /// Current slot clock.
    pub fn now_slot(&self) -> u64 {
        self.now_slot
    }

    /// Run an explicit freshness check at a synthetic `now_slot`
    /// without mutating the clock. Returns the staleness result.
    /// Used by tests that want to drive the guard with scripted
    /// slot sequences.
    pub fn check_at(&self, now_slot: u64) -> StalenessResult {
        if self.max_staleness_slots == 0 || now_slot == 0 {
            return StalenessResult::Fresh;
        }
        let mut worst: Option<([u8; 32], u64)> = None;
        for (&pk, &last) in &self.last_seen {
            if last == 0 {
                continue;
            }
            let age = now_slot.saturating_sub(last);
            if age > self.max_staleness_slots {
                match worst {
                    Some((_, w_last)) if last >= w_last => {}
                    _ => worst = Some((pk, last)),
                }
            }
        }
        match worst {
            Some((pk, last)) => StalenessResult::Stale {
                pubkey: pk,
                last_seen_slot: last,
                staleness_slots: now_slot.saturating_sub(last),
            },
            None => StalenessResult::Fresh,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pks() -> Vec<[u8; 32]> {
        vec![[0xA1u8; 32], [0xB2u8; 32], [0xC3u8; 32]]
    }

    #[test]
    fn fresh_guard_does_not_trip_on_initial_register() {
        let mut g = StalenessGuard::new(50);
        for pk in pks().iter() {
            g.register(*pk);
        }
        assert!(!g.tripped());
        assert_eq!(g.trip_count(), 0);
    }

    #[test]
    fn fresh_update_does_not_trip() {
        let mut g = StalenessGuard::new(50);
        let pks = pks();
        for pk in &pks {
            g.register(*pk);
        }
        assert!(!g.update(pks[0], 100));
        assert!(!g.tripped());
    }

    #[test]
    fn trips_when_one_pool_goes_silent() {
        let mut g = StalenessGuard::new(10);
        let pks = pks();
        for pk in &pks {
            g.register(*pk);
        }
        g.update(pks[0], 100);
        g.update(pks[1], 100);
        // Pool 1 advances to 150; pool 0 silent.
        // age = 150 - 100 = 50 > 10 → stale.
        assert!(g.update(pks[1], 150));
        assert!(g.tripped());
        assert_eq!(g.tripped_for(), Some(pks[0]));
        assert_eq!(g.tripped_staleness(), 50);
        assert_eq!(g.trip_count(), 1);
    }

    #[test]
    fn trips_on_slot_advance_with_no_updates() {
        let mut g = StalenessGuard::new(5);
        let pks = pks();
        for pk in &pks {
            g.register(*pk);
        }
        g.update(pks[0], 100);
        assert!(g.notify_slot_advance(106));
        assert!(g.tripped());
        assert_eq!(g.tripped_for(), Some(pks[0]));
    }

    #[test]
    fn trip_is_sticky() {
        let mut g = StalenessGuard::new(5);
        let pks = pks();
        for pk in &pks {
            g.register(*pk);
        }
        g.update(pks[0], 100);
        assert!(g.notify_slot_advance(200));
        assert!(g.tripped());
        // Subsequent updates do not reset the trip and do not
        // re-increment the count.
        assert!(!g.update(pks[1], 1000));
        assert_eq!(g.trip_count(), 1);
    }

    #[test]
    fn disabled_guard_with_zero_threshold_never_trips() {
        let mut g = StalenessGuard::new(0);
        let pks = pks();
        for pk in &pks {
            g.register(*pk);
        }
        g.update(pks[0], 1);
        g.update(pks[1], 1_000_000);
        assert!(!g.tripped());
    }

    #[test]
    fn check_at_pure_function() {
        let mut g = StalenessGuard::new(10);
        let pks = pks();
        for pk in &pks {
            g.register(*pk);
        }
        g.update(pks[0], 50);
        let res = g.check_at(60);
        assert!(matches!(res, StalenessResult::Fresh));
        let res = g.check_at(70);
        match res {
            StalenessResult::Stale { pubkey, last_seen_slot, staleness_slots } => {
                assert_eq!(pubkey, pks[0]);
                assert_eq!(last_seen_slot, 50);
                assert_eq!(staleness_slots, 20);
            }
            other => panic!("expected stale, got {other:?}"),
        }
        // check_at is pure: guard is still untripped.
        assert!(!g.tripped());
    }
}
