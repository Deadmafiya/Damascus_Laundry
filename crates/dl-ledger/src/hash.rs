//! Deterministic 64-bit hash over a `Cycle`'s leg sequence.
//!
//! Used to identify a cycle across runs without storing the full leg
//! list in every ledger entry. Two equal cycles (same pool sequence and
//! same directions) must hash to the same value; two different cycles
//! should almost certainly hash to different values (FNV-1a 64 has
//! no formal collision guarantee, but is good enough for v1.0 audit
//! trails).
//!
//! ## Choice: FNV-1a 64
//!
//! Picked over `DefaultHasher` (randomized per process, would break
//! AC-1 determinism) and over `crc32` (32 bits is too narrow for a
//! per-cycle id). Hand-rolled because adding `fnv` or `seahash` is
//! not worth ~10 lines of code.

use dl_state::cycle::Cycle;

/// Deterministic 64-bit cycle id.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(transparent)]
pub struct LedgerHash(pub u64);

impl LedgerHash {
    /// Sentinel for the empty cycle (zero legs). Should never appear in a
    /// real ledger entry — a real cycle has at least 2 legs (Roundtrip 2-3).
    pub const ZERO: LedgerHash = LedgerHash(0);

    /// Compute the hash of a cycle.
    ///
    /// Mix order: for each leg in `cycle.legs`, XOR the 32 pool pubkey
    /// bytes (each `h ^= byte; h = h.wrapping_mul(PRIME)`), then XOR a
    /// single direction byte (0 for BaseToQuote, 1 for QuoteToBase).
    /// This is FNV-1a 64 with the standard offset basis and prime.
    pub fn from_cycle(cycle: &Cycle) -> Self {
        // FNV-1a 64 constants.
        const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
        const PRIME: u64 = 0x0000_0100_0000_01b3;

        let mut h = OFFSET;
        for leg in &cycle.legs {
            for byte in leg.pool.0 {
                h ^= byte as u64;
                h = h.wrapping_mul(PRIME);
            }
            // Direction discriminator.
            h ^= match leg.direction {
                dl_state::cycle::Direction::BaseToQuote => 0u8,
                dl_state::cycle::Direction::QuoteToBase => 1u8,
            } as u64;
            h = h.wrapping_mul(PRIME);
        }
        LedgerHash(h)
    }
}

impl From<LedgerHash> for u64 {
    fn from(h: LedgerHash) -> u64 {
        h.0
    }
}

impl std::ops::Deref for LedgerHash {
    type Target = u64;
    fn deref(&self) -> &u64 {
        &self.0
    }
}

impl std::fmt::Display for LedgerHash {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Lowercase hex with no 0x prefix. Stable, grep-able.
        write!(f, "{:016x}", self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dl_state::cycle::{Cycle, Direction, Leg};

    fn leg(pool_byte: u8, dir: Direction) -> Leg {
        let mut pk = [0u8; 32];
        pk[31] = pool_byte;
        Leg {
            pool: dl_state::Pubkey(pk),
            direction: dir,
            weight: 0,
        }
    }

    #[test]
    fn from_cycle_is_deterministic() {
        let c = Cycle::new(vec![
            leg(1, Direction::BaseToQuote),
            leg(2, Direction::QuoteToBase),
        ]);
        let h1 = LedgerHash::from_cycle(&c);
        let h2 = LedgerHash::from_cycle(&c);
        assert_eq!(h1, h2, "same cycle must hash the same way");
    }

    #[test]
    fn from_cycle_differs_on_pool_change() {
        let c1 = Cycle::new(vec![
            leg(1, Direction::BaseToQuote),
            leg(2, Direction::QuoteToBase),
        ]);
        let c2 = Cycle::new(vec![
            leg(1, Direction::BaseToQuote),
            leg(3, Direction::QuoteToBase),
        ]);
        assert_ne!(LedgerHash::from_cycle(&c1), LedgerHash::from_cycle(&c2));
    }

    #[test]
    fn from_cycle_differs_on_direction_change() {
        let c1 = Cycle::new(vec![
            leg(1, Direction::BaseToQuote),
            leg(2, Direction::QuoteToBase),
        ]);
        let c2 = Cycle::new(vec![
            leg(1, Direction::QuoteToBase),
            leg(2, Direction::QuoteToBase),
        ]);
        assert_ne!(LedgerHash::from_cycle(&c1), LedgerHash::from_cycle(&c2));
    }

    #[test]
    fn empty_cycle_hashes_to_nonzero() {
        // FNV-1a 64 of zero bytes is the offset basis, not zero. So
        // the empty cycle gets a non-ZERO hash. This is fine: empty
        // cycles should never appear in a real ledger entry.
        let c = Cycle::new(vec![]);
        let h = LedgerHash::from_cycle(&c);
        assert_ne!(h, LedgerHash::ZERO);
    }

    #[test]
    fn display_is_16_hex_digits() {
        let h = LedgerHash(0xdead_beef_1234_5678);
        assert_eq!(format!("{}", h), "deadbeef12345678");
        assert_eq!(format!("{}", h).len(), 16);
    }

    #[test]
    fn deref_to_u64() {
        let h = LedgerHash(42);
        assert_eq!(*h, 42u64);
    }
}
