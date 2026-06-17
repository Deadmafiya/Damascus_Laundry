//! Injectable randomness.
//!
//! The deterministic path uses [`SeededRng`], a SplitMix64 generator whose entire output
//! sequence is determined by its seed — never system entropy. This keeps simulation runs
//! reproducible (DST requirement). The same trait can later wrap a real PRNG for any
//! non-replay use.

/// Source of pseudo-randomness. Object-safe.
pub trait Rng {
    /// Next 64-bit value.
    fn next_u64(&mut self) -> u64;

    /// Uniform value in `[0, bound)`. Returns 0 if `bound == 0`.
    ///
    /// Uses Lemire's rejection-free-ish multiply-shift; has negligible modulo bias for the
    /// ranges this engine uses (latency jitter buckets, sampling).
    fn next_below(&mut self, bound: u64) -> u64 {
        if bound == 0 {
            return 0;
        }
        let x = self.next_u64() as u128;
        ((x * bound as u128) >> 64) as u64
    }
}

/// Deterministic SplitMix64 generator. Reproducible from its seed.
#[derive(Debug, Clone)]
pub struct SeededRng {
    state: u64,
}

impl SeededRng {
    /// Create from a seed. Equal seeds produce identical sequences.
    pub fn new(seed: u64) -> Self {
        Self { state: seed }
    }
}

impl Rng for SeededRng {
    fn next_u64(&mut self) -> u64 {
        // SplitMix64 (Steele, Lea & Flood). Constants are the published mixing constants.
        self.state = self.state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_seed_same_sequence() {
        let mut a = SeededRng::new(42);
        let mut b = SeededRng::new(42);
        for _ in 0..1000 {
            assert_eq!(a.next_u64(), b.next_u64());
        }
    }

    #[test]
    fn different_seed_diverges() {
        let mut a = SeededRng::new(1);
        let mut b = SeededRng::new(2);
        assert_ne!(a.next_u64(), b.next_u64());
    }

    #[test]
    fn next_below_in_range() {
        let mut r = SeededRng::new(7);
        for _ in 0..1000 {
            assert!(r.next_below(10) < 10);
        }
        assert_eq!(r.next_below(0), 0);
    }
}
