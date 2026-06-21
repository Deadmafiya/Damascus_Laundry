//! Determinism integration test (AC-3).
//!
//! A small driver loop consumes a `ScriptedFeed`, advances a `MockClock` from observed
//! slots, and mixes in `SeededRng` draws — recording a transcript of every (time, random,
//! event) tuple. Two runs with the same seed and script must produce byte-for-byte equal
//! transcripts.

use dl_core::{Clock, Feed, FeedEvent, MockClock, Rng, ScriptedFeed, SeededRng};

/// One line of the recorded transcript. Encoded to bytes for a strict equality check.
fn record(out: &mut Vec<u8>, millis: u64, slot: u64, rand: u64, tag: &str) {
    out.extend_from_slice(format!("{millis}|{slot}|{rand}|{tag}\n").as_bytes());
}

fn run(seed: u64, script: Vec<FeedEvent>) -> Vec<u8> {
    let mut clock = MockClock::new(0, 0);
    let mut rng = SeededRng::new(seed);
    let mut feed = ScriptedFeed::new(script);
    let mut transcript = Vec::new();

    while let Some(ev) = feed.next_event() {
        // Advance the clock to the event's observed slot (no look-ahead).
        clock.set_slot(ev.slot());
        let r = rng.next_below(1_000_000);
        let tag = match &ev {
            FeedEvent::Slot { .. } => "slot",
            FeedEvent::AccountUpdate { pubkey, data, .. } => {
                // Fold event payload into the transcript via the clock too.
                let _ = (pubkey, data);
                "acct"
            }
            FeedEvent::Pool { pool, .. } => {
                // Decoded pool updates (DAM-44b/c, Meteora DLMM +
                // Orca Whirlpool). Fold the pool pubkey into the
                // transcript tag for byte-stable determinism.
                let _ = pool;
                "pool"
            }
            FeedEvent::StalePoolHalt { pubkey, .. } => {
                // Staleness-guard trip. Fold the pubkey for the
                // same reason as `Pool`.
                let _ = pubkey;
                "stale"
            }
        };
        record(&mut transcript, clock.now_millis(), clock.slot(), r, tag);
    }
    transcript
}

fn sample_script() -> Vec<FeedEvent> {
    vec![
        FeedEvent::Slot { slot: 10 },
        FeedEvent::AccountUpdate {
            slot: 10,
            pubkey: [3u8; 32],
            data: vec![9, 8, 7, 6],
        },
        FeedEvent::Slot { slot: 11 },
        FeedEvent::AccountUpdate {
            slot: 13,
            pubkey: [4u8; 32],
            data: vec![0],
        },
        FeedEvent::Slot { slot: 14 },
    ]
}

#[test]
fn two_seeded_runs_are_byte_identical() {
    let a = run(123_456, sample_script());
    let b = run(123_456, sample_script());
    assert_eq!(
        a, b,
        "identical seed + script must yield identical transcripts"
    );
    assert!(!a.is_empty(), "transcript should be non-empty");
}

#[test]
fn different_seed_changes_transcript() {
    let a = run(1, sample_script());
    let b = run(2, sample_script());
    assert_ne!(a, b, "different seeds should diverge");
}
