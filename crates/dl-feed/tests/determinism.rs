//! AC-1: determinism. Two events streams produced by the same seeded script
//! must round-trip through capture→replay with bit-identical results.
//!
//! This is the bedrock of replay-driven testing: if captures were not
//! deterministic, the test suite in Phase 3+ would flake on different
//! machines.

use std::io::Cursor;

use dl_core::rng::{Rng, SeededRng};
use dl_core::Feed;
use dl_core::FeedEvent;
use dl_feed::capture::{CaptureWriter, CapturedFeed};

fn script(n: usize, seed: u64) -> Vec<FeedEvent> {
    let mut rng = SeededRng::new(seed);
    (0..n)
        .map(|i| {
            let slot = i as u64;
            let pk_byte: u8 = (rng.next_u64() & 0xFF) as u8;
            let data_len: usize = 1 + (rng.next_below(32) as usize);
            let mut data = Vec::with_capacity(data_len);
            for _ in 0..data_len {
                data.push((rng.next_u64() & 0xFF) as u8);
            }
            FeedEvent::AccountUpdate {
                slot,
                pubkey: [pk_byte; 32],
                data,
            }
        })
        .collect()
}

fn capture(script: &[FeedEvent]) -> Vec<u8> {
    let mut w = CaptureWriter::new(Vec::new()).unwrap();
    for ev in script {
        w.write_event(ev).unwrap();
    }
    w.into_inner().unwrap()
}

fn replay(bytes: Vec<u8>) -> Vec<FeedEvent> {
    let mut feed = CapturedFeed::open(Cursor::new(bytes)).unwrap();
    let mut out = Vec::new();
    while let Some(ev) = feed.next_event() {
        out.push(ev);
    }
    out
}

#[test]
fn same_seed_same_capture_bytes() {
    let s1 = script(500, 0xDEAD_BEEF);
    let s2 = script(500, 0xDEAD_BEEF);
    let c1 = capture(&s1);
    let c2 = capture(&s2);
    assert_eq!(c1, c2, "captures of equal scripts must be byte-identical");
}

#[test]
fn different_seed_different_capture_bytes() {
    let s1 = script(500, 0xDEAD_BEEF);
    let s2 = script(500, 0xDEAD_BEEF ^ 1);
    let c1 = capture(&s1);
    let c2 = capture(&s2);
    assert_ne!(
        c1, c2,
        "captures of different scripts must differ at the byte level"
    );
}

#[test]
fn round_trip_preserves_events() {
    let s = script(500, 0xDEAD_BEEF);
    let r = replay(capture(&s));
    assert_eq!(r, s);
}

#[test]
fn two_replays_of_same_capture_byte_identical() {
    let s = script(500, 0xDEAD_BEEF);
    let c = capture(&s);
    let r1 = replay(c.clone());
    let r2 = replay(c);
    // The replayed events must be equal (FeedEvent derives PartialEq + Eq
    // and is deterministic under bincode).
    assert_eq!(r1, r2);
    assert_eq!(r1, s);
}
