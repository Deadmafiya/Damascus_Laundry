//! Injectable clock abstraction.
//!
//! The engine never reads wall-clock time directly; it goes through [`Clock`] so that
//! deterministic replays use a [`MockClock`] whose time and slot advance only when the
//! harness says so. Solana produces slots roughly every 400 ms.

/// A Solana slot number.
pub type Slot = u64;

/// Source of time and slot progression. Object-safe so callers can hold `&dyn Clock`.
pub trait Clock {
    /// Milliseconds since an arbitrary fixed epoch (monotonic within a run).
    fn now_millis(&self) -> u64;

    /// Current slot.
    fn slot(&self) -> Slot;
}

/// Real clock backed by the OS monotonic timer. Slot is derived from elapsed time using
/// the nominal ~400 ms slot cadence and a configurable genesis slot/instant.
pub struct SystemClock {
    start: std::time::Instant,
    genesis_slot: Slot,
}

impl SystemClock {
    /// Create a system clock, anchoring `slot()` at `genesis_slot` for "now".
    pub fn new(genesis_slot: Slot) -> Self {
        Self {
            start: std::time::Instant::now(),
            genesis_slot,
        }
    }
}

impl Default for SystemClock {
    fn default() -> Self {
        Self::new(0)
    }
}

impl Clock for SystemClock {
    fn now_millis(&self) -> u64 {
        self.start.elapsed().as_millis() as u64
    }

    fn slot(&self) -> Slot {
        // ~400 ms per slot (nominal); integer division keeps this deterministic-ish but
        // SystemClock is only for live use, never for replay assertions.
        self.genesis_slot + (self.now_millis() / 400)
    }
}

/// Deterministic clock: time and slot advance only via explicit calls. Two `MockClock`s
/// driven by the same calls produce identical readings — the basis for replay.
pub struct MockClock {
    millis: u64,
    slot: Slot,
}

impl MockClock {
    /// Start at the given millis and slot.
    pub fn new(start_millis: u64, start_slot: Slot) -> Self {
        Self {
            millis: start_millis,
            slot: start_slot,
        }
    }

    /// Advance time by `delta_millis` (does not change the slot).
    pub fn advance_millis(&mut self, delta_millis: u64) {
        self.millis = self.millis.saturating_add(delta_millis);
    }

    /// Advance by one slot and the nominal 400 ms.
    pub fn tick_slot(&mut self) {
        self.slot = self.slot.saturating_add(1);
        self.advance_millis(400);
    }

    /// Jump directly to a specific slot (e.g. when replaying a captured `Slot` event),
    /// advancing time by the nominal cadence for any slots skipped.
    pub fn set_slot(&mut self, slot: Slot) {
        if slot > self.slot {
            let delta = slot - self.slot;
            self.advance_millis(delta.saturating_mul(400));
        }
        self.slot = slot;
    }
}

impl Default for MockClock {
    fn default() -> Self {
        Self::new(0, 0)
    }
}

impl Clock for MockClock {
    fn now_millis(&self) -> u64 {
        self.millis
    }

    fn slot(&self) -> Slot {
        self.slot
    }
}
