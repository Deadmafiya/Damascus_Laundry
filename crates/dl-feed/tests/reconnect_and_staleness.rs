//! DAM-31.D acceptance: end-to-end test of the new
//! reconnect/registry/guard modules **without** a live WebSocket.
//!
//! Drives:
//! 1. `ReconnectPolicy::next_backoff` produces a sequence with the
//!    expected exponential growth and cap.
//! 2. `ReconnectStormDetector` flips to "storm" after the
//!    threshold.
//! 3. `StalenessGuard` trips on a silent pool and emits a
//!    `FeedEvent::StalePoolHalt` when the WS task integrates it.
//! 4. `SubscriptionRegistry` iterates in deterministic order for
//!    resubscribe.
//!
//! All four are also unit-tested inside their own modules; this
//! integration test exists to demonstrate they compose correctly
//! and to satisfy the issue's "use a scripted/mock feed"
//! requirement (the integration test never opens a socket).

use dl_core::FeedEvent;
use dl_feed::reconnect::{ReconnectPolicy, ReconnectStormDetector};
use dl_feed::registry::SubscriptionRegistry;
use dl_feed::staleness::StalenessGuard;

#[test]
fn full_resubscribe_and_staleness_flow() {
    // Step 1: Build a registry of three vault pubkeys. The bot
    // discovered these in initial pool enumeration.
    let mut registry = SubscriptionRegistry::new();
    registry.insert([0xA1u8; 32]);
    registry.insert([0xB2u8; 32]);
    registry.insert([0xC3u8; 32]);
    assert_eq!(registry.len(), 3);

    // Step 2: WS drops. Reconnect policy begins exponential
    // backoff. Three attempts at base=10ms cap=80ms produce
    // 10, 20, 40ms.
    let policy = ReconnectPolicy {
        base: std::time::Duration::from_millis(10),
        cap: std::time::Duration::from_millis(80),
        jitter_bps: 0,
        storm_threshold: 5,
        storm_window: 100,
        max_attempts: Some(10),
    };
    assert_eq!(
        policy.next_backoff(0, None),
        std::time::Duration::from_millis(10)
    );
    assert_eq!(
        policy.next_backoff(1, None),
        std::time::Duration::from_millis(20)
    );
    assert_eq!(
        policy.next_backoff(2, None),
        std::time::Duration::from_millis(40)
    );
    assert_eq!(
        policy.next_backoff(3, None),
        std::time::Duration::from_millis(80)
    );

    // Step 3: Storm detector counts attempts.
    let mut storm = ReconnectStormDetector::new(2, 100);
    assert!(!storm.record_attempt(50));
    assert!(!storm.record_attempt(60));
    // 3rd attempt pushes over threshold.
    assert!(storm.record_attempt(70));
    assert_eq!(storm.storm_count(), 1);

    // Step 4: Reconnect succeeds. The bot re-issues every entry
    // in the registry. The order is deterministic (sorted), which
    // matches the issue's "resubscribes to all vault accounts
    // from the registry" requirement.
    let resubscribe_order: Vec<[u8; 32]> = registry.to_vec();
    assert_eq!(
        resubscribe_order,
        vec![[0xA1u8; 32], [0xB2u8; 32], [0xC3u8; 32]]
    );

    // Step 5: Staleness guard. Two pools update at slot 100;
    // only one keeps updating. After the second update at slot
    // 150, the guard trips and reports the silent pool.
    let mut guard = StalenessGuard::new(10);
    for pk in registry.iter() {
        guard.register(pk);
    }
    assert!(!guard.update(registry.iter().next().unwrap(), 100));
    let second_pk = {
        let mut it = registry.iter();
        it.next();
        it.next().unwrap()
    };
    assert!(!guard.update(second_pk, 100));
    // Pool 0 (first pubkey) goes silent; pool 1 advances to 150.
    // age = 150 - 100 = 50 > 10 → trip.
    assert!(guard.update(second_pk, 150));
    assert!(guard.tripped());
    assert_eq!(guard.tripped_for(), Some(registry.iter().next().unwrap()));
    assert_eq!(guard.tripped_staleness(), 50);
    assert_eq!(guard.trip_count(), 1);

    // Step 6: The WS task would emit a `FeedEvent::StalePoolHalt`
    // carrying the trip details. Build it and assert its shape.
    let halt = FeedEvent::StalePoolHalt {
        last_seen_slot: guard.tripped_at_slot(),
        pubkey: guard.tripped_for().unwrap(),
        staleness_slots: guard.tripped_staleness(),
    };
    match halt {
        FeedEvent::StalePoolHalt {
            last_seen_slot,
            pubkey,
            staleness_slots,
        } => {
            assert_eq!(last_seen_slot, 150);
            assert_eq!(pubkey, registry.iter().next().unwrap());
            assert_eq!(staleness_slots, 50);
        }
        other => panic!("expected StalePoolHalt, got {other:?}"),
    }
}

#[test]
fn reconnect_policy_never_exceeds_cap_under_extreme_jitter() {
    let p = ReconnectPolicy {
        base: std::time::Duration::from_millis(1),
        cap: std::time::Duration::from_millis(50),
        jitter_bps: 10000, // 100% jitter
        storm_threshold: 0,
        storm_window: 0,
        max_attempts: None,
    };
    for attempt in 0..30 {
        let d = p.next_backoff(attempt, Some(10000));
        assert!(
            d <= std::time::Duration::from_millis(50),
            "attempt={attempt} produced {d:?} > cap"
        );
    }
}

#[test]
fn storm_detector_window_pruning_does_not_underflow() {
    // Defensive: pruning at slot=0 with window=10 should not
    // underflow the cutoff calculation. After recording slot=0,
    // recording at slot=5 (cutoff=0) is treated as a fresh
    // attempt because the slot=0 entry is pruned. Threshold=1
    // means we need 2+ attempts; the second pushes us into storm.
    let mut s = ReconnectStormDetector::new(1, 10);
    s.record_attempt(0);
    s.record_attempt(5);
    // After pruning: the slot=0 entry is gone; only slot=5
    // remains. attempts_in_window = 1, not a storm.
    assert!(!s.is_storm());
    // Add a third. Now 2 entries in window, > threshold → storm.
    s.record_attempt(8);
    assert!(s.is_storm());
    assert_eq!(s.storm_count(), 1);
}
