//! Fault-injection middlewares (Phase 6, plan 06-01).
//!
//! The recon harness is deterministic by construction (every input
//! yields a byte-identical report). To stress the harness's robustness
//! to lossy / out-of-order / corrupted inputs, these middlewares wrap
//! a [`Feed`] and apply a configurable transformation on the fly.
//!
//! Every middleware is a pure `Feed` impl — no I/O, no system entropy,
//! no `unsafe`. Each accepts a [`JitterRng`] (deterministic, seeded)
//! so a test can reproduce a fault exactly.
//!
//! ## The five middlewares
//!
//! 1. [`BoundedDrop`] — drop the first `n` events, then pass through.
//!    Models late-arriving subscriptions.
//! 2. [`BoundedCorrupt`] — with probability `p`, replace the payload
//!    of the next `AccountUpdate` event with a random `len`-byte
//!    buffer. Models CRC failure / schema mismatch mid-stream.
//! 3. [`JitteredSlot`] — increment each emitted `Slot` event's slot
//!    number by a random amount in `[0, jitter]`. Models clock skew.
//! 4. [`Reorder`] — buffer `n` events and emit them in a permuted
//!    order. Models out-of-order delivery.
//! 5. [`Capped`] — terminate after `n` events (a hard ceiling).
//!
//! Plus [`FaultConfig`] — a single config struct bundling all five —
//! and [`JitterRng`] — a deterministic LCG seeded from a `u64`.
//!
//! ## Integer-only invariant
//!
//! No `f32` / `f64`. Probabilities are passed as `u32` parts-per-million
//! (mirror `dl_sim::ev::PPM_ONE`). Jitter bounds are `u32`.

use dl_core::{Feed, FeedEvent};

/// Deterministic linear-congruential RNG, seeded from a `u64`.
///
/// Used for fault injection. Implements `Rng` enough to drive
/// Deterministic linear-congruential RNG, seeded from a `u64`.
///
/// Used for fault injection. Implements `Rng` enough to drive
/// `u32`/`u64` draws; same seed → same sequence (invariant I-1).
#[derive(Debug, Clone)]
pub struct JitterRng {
    state: u64,
}

impl JitterRng {
    /// Seed from a `u64`. The seed is masked to avoid the LCG's
    /// all-zero fixed point.
    pub fn from_seed(seed: u64) -> Self {
        Self { state: seed | 1 }
    }

    /// Draw a `u64`.
    pub fn next_u64(&mut self) -> u64 {
        // Numerical Recipes LCG constants. Same constants as `pcg`
        // and `wyrand` test vectors use, so tests can swap seeds
        // without surprises.
        self.state = self
            .state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1);
        self.state
    }

    /// Draw a `u32`.
    pub fn next_u32(&mut self) -> u32 {
        (self.next_u64() >> 32) as u32
    }

    /// Draw a `u32` in `[0, bound)`. `bound == 0` returns `0`.
    pub fn next_bounded(&mut self, bound: u32) -> u32 {
        if bound == 0 {
            return 0;
        }
        (self.next_u64() % bound as u64) as u32
    }

    /// Draw a `u32` in `[0, ppm_one]`. `ppm_one` is typically
    /// [`crate::ppm::PPM_ONE`]; we hard-code `1_000_000` here to
    /// avoid a circular dep on dl-sim. Mirrors `dl_sim::ev::PPM_ONE`.
    pub fn next_ppm(&mut self) -> u32 {
        (self.next_u64() % 1_000_000) as u32
    }
}

// ---------------------------------------------------------------------------
// Fault 1: BoundedDrop — drop the first N events
// ---------------------------------------------------------------------------

/// Drop the first `drop_count` events, then pass through unchanged.
///
/// Models a subscription that started late: the consumer doesn't
/// see the earliest events. EOF on the inner feed terminates as usual.
#[derive(Debug)]
pub struct BoundedDrop<F: Feed> {
    inner: F,
    drop_count: u64,
    dropped: u64,
}

impl<F: Feed> BoundedDrop<F> {
    pub fn new(inner: F, drop_count: u64) -> Self {
        Self {
            inner,
            drop_count,
            dropped: 0,
        }
    }

    pub fn dropped(&self) -> u64 {
        self.dropped
    }
}

impl<F: Feed> Feed for BoundedDrop<F> {
    fn next_event(&mut self) -> Option<FeedEvent> {
        while self.dropped < self.drop_count {
            if self.inner.next_event().is_none() {
                return None;
            }
            self.dropped += 1;
        }
        self.inner.next_event()
    }
}

// ---------------------------------------------------------------------------
// Fault 2: BoundedCorrupt — with probability ppm/ppm_one, replace payload
// ---------------------------------------------------------------------------

/// With `corrupt_prob_ppm / 1_000_000` probability per event, replace
/// the next `AccountUpdate` payload with random bytes of the same
/// length. `Slot` events are passed through (they have no payload to
/// corrupt). The deterministic RNG drives the corruption decision.
#[derive(Debug)]
pub struct BoundedCorrupt<F: Feed> {
    inner: F,
    corrupt_prob_ppm: u32,
    corrupt_len: usize,
    rng: JitterRng,
    corrupted: u64,
    passed: u64,
}

impl<F: Feed> BoundedCorrupt<F> {
    pub fn new(inner: F, corrupt_prob_ppm: u32, corrupt_len: usize, rng: JitterRng) -> Self {
        Self {
            inner,
            corrupt_prob_ppm,
            corrupt_len,
            rng,
            corrupted: 0,
            passed: 0,
        }
    }

    pub fn corrupted(&self) -> u64 {
        self.corrupted
    }

    pub fn passed(&self) -> u64 {
        self.passed
    }
}

impl<F: Feed> Feed for BoundedCorrupt<F> {
    fn next_event(&mut self) -> Option<FeedEvent> {
        let event = self.inner.next_event()?;
        let roll = self.rng.next_ppm();
        if roll < self.corrupt_prob_ppm {
            self.corrupted += 1;
            match event {
                FeedEvent::AccountUpdate { slot, pubkey, .. } => {
                    // Corrupt by overwriting the payload with random
                    // bytes of `corrupt_len`. The harness downstream
                    // will see UnknownAccountSize (or a decoder error)
                    // and report a divergence.
                    let mut junk = vec![0u8; self.corrupt_len];
                    let mut i = 0;
                    while i < self.corrupt_len {
                        let word = self.rng.next_u64().to_le_bytes();
                        for &b in &word {
                            if i < self.corrupt_len {
                                junk[i] = b;
                                i += 1;
                            } else {
                                break;
                            }
                        }
                    }
                    Some(FeedEvent::AccountUpdate {
                        slot,
                        pubkey,
                        data: junk,
                    })
                }
                other => {
                    // Slot events have no payload to corrupt; pass through.
                    self.passed += 1;
                    Some(other)
                }
            }
        } else {
            self.passed += 1;
            Some(event)
        }
    }
}

// ---------------------------------------------------------------------------
// Fault 3: JitteredSlot — jitter each Slot event's slot number
// ---------------------------------------------------------------------------

/// Increment each emitted `Slot` event's slot number by a uniform
/// random amount in `[0, jitter_slots]`. The harness downstream sees
/// a monotonic slot stream that's been skewed per-event; its derived
/// ledger entries still produce a valid (deterministic) report, but
/// slot-ordering invariants may be violated.
#[derive(Debug)]
pub struct JitteredSlot<F: Feed> {
    inner: F,
    jitter_slots: u32,
    rng: JitterRng,
}

impl<F: Feed> JitteredSlot<F> {
    pub fn new(inner: F, jitter_slots: u32, rng: JitterRng) -> Self {
        Self {
            inner,
            jitter_slots,
            rng,
        }
    }
}

impl<F: Feed> Feed for JitteredSlot<F> {
    fn next_event(&mut self) -> Option<FeedEvent> {
        let event = self.inner.next_event()?;
        match event {
            FeedEvent::Slot { slot } => {
                let jitter = self.rng.next_bounded(self.jitter_slots.saturating_add(1));
                Some(FeedEvent::Slot {
                    slot: slot.saturating_add(u64::from(jitter)),
                })
            }
            other => Some(other),
        }
    }
}

// ---------------------------------------------------------------------------
// Fault 4: Reorder — buffer N events and emit a permuted order
// ---------------------------------------------------------------------------

/// Mode for [`Reorder`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReorderMode {
    /// Buffer `n` events and emit them in reverse order. Models a
    /// peer that flushes its write buffer backward on every commit.
    Reverse,
    /// Buffer `n` events and emit them in the next permutation driven
    /// by `rng`. Models a network that occasionally re-orders packets.
    Permute,
}

/// Buffer `n` events and emit them in a non-natural order. The buffer
/// fills from the inner feed; once full, it's drained in the chosen
/// order and the cycle repeats. EOF terminates cleanly.
#[derive(Debug)]
pub struct Reorder<F: Feed> {
    inner: F,
    window: usize,
    mode: ReorderMode,
    rng: JitterRng,
    buffer: Vec<FeedEvent>,
}

impl<F: Feed> Reorder<F> {
    pub fn new(inner: F, window: usize, mode: ReorderMode, rng: JitterRng) -> Self {
        Self {
            inner,
            window: window.max(1),
            mode,
            rng,
            buffer: Vec::with_capacity(window.max(1)),
        }
    }

    fn refill(&mut self) -> bool {
        self.buffer.clear();
        for _ in 0..self.window {
            match self.inner.next_event() {
                Some(ev) => self.buffer.push(ev),
                None => break,
            }
        }
        if self.buffer.is_empty() {
            return false;
        }
        match self.mode {
            ReorderMode::Reverse => self.buffer.reverse(),
            ReorderMode::Permute => {
                // Fisher–Yates with our LCG.
                for i in (1..self.buffer.len()).rev() {
                    let j = self.rng.next_bounded((i + 1) as u32) as usize;
                    self.buffer.swap(i, j);
                }
            }
        }
        true
    }
}

impl<F: Feed> Feed for Reorder<F> {
    fn next_event(&mut self) -> Option<FeedEvent> {
        if self.buffer.is_empty() && !self.refill() {
            return None;
        }
        if self.buffer.is_empty() {
            return None;
        }
        Some(self.buffer.remove(0))
    }
}

// ---------------------------------------------------------------------------
// Fault 5: Capped — hard ceiling on event count
// ---------------------------------------------------------------------------

/// Stop emitting events after `cap` events. Models a feed that
/// disconnects mid-stream.
#[derive(Debug)]
pub struct Capped<F: Feed> {
    inner: F,
    cap: u64,
    emitted: u64,
}

impl<F: Feed> Capped<F> {
    pub fn new(inner: F, cap: u64) -> Self {
        Self {
            inner,
            cap,
            emitted: 0,
        }
    }

    pub fn emitted(&self) -> u64 {
        self.emitted
    }
}

impl<F: Feed> Feed for Capped<F> {
    fn next_event(&mut self) -> Option<FeedEvent> {
        if self.emitted >= self.cap {
            return None;
        }
        let ev = self.inner.next_event()?;
        self.emitted += 1;
        Some(ev)
    }
}

// ---------------------------------------------------------------------------
// FaultConfig — bundle all five for one-shot test setup
// ---------------------------------------------------------------------------

/// Configuration bundle for the five middlewares. Each field is
/// `None` to disable that fault. Default is all-`None` (no faults).
#[derive(Debug, Clone, Default)]
pub struct FaultConfig {
    /// BoundedDrop: drop the first N events.
    pub drop_count: Option<u64>,
    /// BoundedCorrupt: probability (ppm) of payload corruption.
    pub corrupt_prob_ppm: Option<u32>,
    /// BoundedCorrupt: replacement payload length.
    pub corrupt_len: Option<usize>,
    /// JitteredSlot: max jitter added to each slot.
    pub slot_jitter: Option<u32>,
    /// Reorder: window size.
    pub reorder_window: Option<usize>,
    /// Reorder: permutation mode.
    pub reorder_mode: Option<ReorderMode>,
    /// Capped: hard ceiling on events.
    pub cap: Option<u64>,
    /// RNG seed (used by every fault that has randomness).
    pub rng_seed: u64,
}

impl FaultConfig {
    pub fn none() -> Self {
        Self::default()
    }

    pub fn with_rng_seed(mut self, seed: u64) -> Self {
        self.rng_seed = seed;
        self
    }
}

/// Trait alias-like: a builder that applies a [`FaultConfig`] to an
/// inner feed, returning the outermost wrapped feed.
pub trait FaultLayer {
    fn apply<F: Feed>(&self, inner: F) -> Self::Out;
    type Out: Feed;
}

// ---------------------------------------------------------------------------
// Re-exports
// ---------------------------------------------------------------------------

// (Lib.rs re-exports individual names.)

#[cfg(test)]
mod tests {
    use super::*;
    use dl_core::feed::ScriptedFeed;

    fn scripted(events: Vec<FeedEvent>) -> ScriptedFeed {
        ScriptedFeed::new(events)
    }

    #[test]
    fn rng_is_deterministic() {
        let mut a = JitterRng::from_seed(42);
        let mut b = JitterRng::from_seed(42);
        for _ in 0..100 {
            assert_eq!(a.next_u64(), b.next_u64());
        }
    }

    #[test]
    fn rng_differs_per_seed() {
        let mut a = JitterRng::from_seed(1);
        let mut b = JitterRng::from_seed(2);
        // Vanishingly unlikely to collide on the first draw.
        assert_ne!(a.next_u64(), b.next_u64());
    }

    #[test]
    fn bounded_drop_drops_first_n_then_passes() {
        let events: Vec<FeedEvent> = (0..5).map(|s| FeedEvent::Slot { slot: s }).collect();
        let mut f = BoundedDrop::new(scripted(events), 2);
        assert_eq!(f.next_event().unwrap(), FeedEvent::Slot { slot: 2 });
        assert_eq!(f.next_event().unwrap(), FeedEvent::Slot { slot: 3 });
        assert_eq!(f.next_event().unwrap(), FeedEvent::Slot { slot: 4 });
        assert!(f.next_event().is_none());
        assert_eq!(f.dropped(), 2);
    }

    #[test]
    fn bounded_drop_zero_drops_passes_through() {
        let events = vec![FeedEvent::Slot { slot: 7 }];
        let mut f = BoundedDrop::new(scripted(events), 0);
        assert_eq!(f.next_event().unwrap(), FeedEvent::Slot { slot: 7 });
    }

    #[test]
    fn bounded_corrupt_rolls_under_threshold() {
        // Always corrupt (1_000_000 ppm == 100%).
        let events = vec![FeedEvent::AccountUpdate {
            slot: 1,
            pubkey: [0xaa; 32],
            data: vec![0u8; 165],
        }];
        let mut f = BoundedCorrupt::new(scripted(events), 1_000_000, 165, JitterRng::from_seed(1));
        let ev = f.next_event().unwrap();
        match ev {
            FeedEvent::AccountUpdate { data, .. } => {
                assert_eq!(data.len(), 165);
                // With seed 1, the LCG draws non-zero bytes; data
                // is unlikely to be the original all-zeros. Verify
                // by drawing the same RNG state and checking that
                // *some* bytes differ.
                let mut rng = JitterRng::from_seed(1);
                let _ = rng.next_ppm();
                let mut junk = vec![0u8; 165];
                let mut i = 0;
                while i < 165 {
                    let word = rng.next_u64().to_le_bytes();
                    for &b in &word {
                        if i < 165 {
                            junk[i] = b;
                            i += 1;
                        } else {
                            break;
                        }
                    }
                }
                assert_eq!(data, junk);
            }
            _ => panic!("expected AccountUpdate"),
        }
        assert_eq!(f.corrupted(), 1);
    }

    #[test]
    fn jittered_slot_clamps_at_bound() {
        let events = vec![FeedEvent::Slot { slot: 100 }];
        // jitter_slots = 10 → slot becomes 100..=110.
        let mut f = JitteredSlot::new(scripted(events), 10, JitterRng::from_seed(0));
        match f.next_event().unwrap() {
            FeedEvent::Slot { slot: s } => assert!((100..=110).contains(&s)),
            _ => panic!("expected Slot"),
        }
    }

    #[test]
    fn reorder_reverse_emits_backwards() {
        let events: Vec<FeedEvent> = (0..4).map(|s| FeedEvent::Slot { slot: s }).collect();
        let mut f = Reorder::new(
            scripted(events),
            4,
            ReorderMode::Reverse,
            JitterRng::from_seed(0),
        );
        assert_eq!(f.next_event().unwrap(), FeedEvent::Slot { slot: 3 });
        assert_eq!(f.next_event().unwrap(), FeedEvent::Slot { slot: 2 });
        assert_eq!(f.next_event().unwrap(), FeedEvent::Slot { slot: 1 });
        assert_eq!(f.next_event().unwrap(), FeedEvent::Slot { slot: 0 });
    }

    #[test]
    fn capped_terminates_at_cap() {
        let events: Vec<FeedEvent> = (0..10).map(|s| FeedEvent::Slot { slot: s }).collect();
        let mut f = Capped::new(scripted(events), 3);
        assert!(f.next_event().is_some());
        assert!(f.next_event().is_some());
        assert!(f.next_event().is_some());
        assert!(f.next_event().is_none());
        assert_eq!(f.emitted(), 3);
    }

    #[test]
    fn fault_config_default_is_empty() {
        let cfg = FaultConfig::none();
        assert!(cfg.drop_count.is_none());
        assert!(cfg.corrupt_prob_ppm.is_none());
        assert!(cfg.slot_jitter.is_none());
        assert!(cfg.reorder_window.is_none());
        assert!(cfg.cap.is_none());
    }
}
