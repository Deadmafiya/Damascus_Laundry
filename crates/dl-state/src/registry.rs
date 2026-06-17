//! In-memory pool store. Single-writer, multi-reader-friendly via
//! snapshotting; not concurrent-mutation-safe (we don't need that yet —
//! the engine loops over a single writer thread and reads on others).
//!
//! For v1.0 the registry is just a `HashMap`. When the detector starts
//! querying by mint pair, we may add a secondary index, but YAGNI.

use std::collections::HashMap;

use crate::pool::Pool;

#[derive(Debug, Default, Clone)]
pub struct PoolRegistry {
    by_address: HashMap<[u8; 32], Pool>,
}

impl PoolRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Insert or replace a pool. Returns the previous value if any.
    pub fn insert(&mut self, p: Pool) -> Option<Pool> {
        self.by_address.insert(p.address.0, p)
    }

    /// Lookup by pool address.
    pub fn get(&self, addr: &[u8; 32]) -> Option<&Pool> {
        self.by_address.get(addr)
    }

    /// Iterate over all `(pool_address, pool)` pairs.
    pub fn iter(&self) -> impl Iterator<Item = (&[u8; 32], &Pool)> {
        self.by_address.iter()
    }

    pub fn len(&self) -> usize {
        self.by_address.len()
    }

    pub fn is_empty(&self) -> bool {
        self.by_address.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pool::{AmmKind, Pubkey};

    fn sample_pool(addr: [u8; 32]) -> Pool {
        Pool {
            address: Pubkey(addr),
            kind: AmmKind::RaydiumAmmV4,
            base_mint: Pubkey([1u8; 32]),
            quote_mint: Pubkey([2u8; 32]),
            base_decimals: 9,
            quote_decimals: 6,
            base_reserve: 100_000_000_000, // 100 SOL in lamports
            quote_reserve: 15_000_000_000, // 15,000 USDC
            fee_bps: 25,
            last_update_slot: 1,
        }
    }

    #[test]
    fn insert_and_lookup() {
        let mut r = PoolRegistry::new();
        let p = sample_pool([7u8; 32]);
        assert!(r.insert(p.clone()).is_none());
        assert_eq!(r.get(&[7u8; 32]), Some(&p));
    }

    #[test]
    fn replace_returns_previous() {
        let mut r = PoolRegistry::new();
        r.insert(sample_pool([7u8; 32]));
        let mut p2 = sample_pool([7u8; 32]);
        p2.quote_reserve = 16_000_000_000;
        let prev = r.insert(p2.clone()).unwrap();
        assert_eq!(prev.quote_reserve, 15_000_000_000);
        assert_eq!(r.get(&[7u8; 32]), Some(&p2));
    }

    #[test]
    fn missing_returns_none() {
        let r = PoolRegistry::new();
        assert!(r.get(&[0u8; 32]).is_none());
    }

    #[test]
    fn len_tracks_insertions() {
        let mut r = PoolRegistry::new();
        assert!(r.is_empty());
        r.insert(sample_pool([1u8; 32]));
        r.insert(sample_pool([2u8; 32]));
        assert_eq!(r.len(), 2);
    }

    #[test]
    fn mid_price_scaled_1e9_computes() {
        let p = sample_pool([7u8; 32]);
        // mid_price = quote * 1e9 / base (in raw base units, scaled by 1e-9)
        // = 15_000_000_000 * 1_000_000_000 / 100_000_000_000
        // = 1.5e18 / 1e11 = 1.5e8
        // Human-readable price (USDC per SOL, decimals-normalized) is 150,
        // but the function returns the raw 1e-9-scaled value: 1.5e8.
        let mid = p.mid_price_scaled_1e9().unwrap();
        assert_eq!(mid, 150_000_000);
    }

    #[test]
    fn mid_price_zero_base_reserve_errors() {
        let mut p = sample_pool([7u8; 32]);
        p.base_reserve = 0;
        assert!(p.mid_price_scaled_1e9().is_err());
    }
}
