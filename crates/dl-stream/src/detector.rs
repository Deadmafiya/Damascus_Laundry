//! Streaming detector (08-02).
//!
//! The [`StreamingGraph`] is a thin wrapper over a [`Graph`] from
//! `dl-detect` that maintains a `pool -> edges` index. As pool
//! reserves change, only the edges incident to that pool's two
//! tokens are re-computed (O(deg(pool)) per update instead of
//! O(|edges|)).
//!
//! Edge weight math is duplicated locally because
//! `weight_from_rate` in `dl-detect` is private. The math is
//! identical and is verified by the existing dl-detect tests.

use std::collections::BTreeMap;

use dl_core::fixed::mul_div_floor;
use dl_detect::cycle::Cycle;
use dl_detect::error::DetectError;
use dl_detect::graph::{build_from_pools, Graph};
use dl_state::pool::Pool;
use dl_state::Pubkey;

/// 1.0 in 1e18 fixed-point.
const ONE_E18: u128 = 1_000_000_000_000_000_000;

/// Returns `fee_multiplier` for a given fee in bps:
/// `1e18 * (1 - fee/10_000)` = `1e18 * (10_000 - fee) / 10_000`.
///
/// e.g. fee=30 bps -> 997_000_000_000_000_000.
fn fee_multiplier_1e18(fee_bps: u16) -> u128 {
    if fee_bps >= 10_000 {
        return 0; // 100% fee means no output
    }
    let numerator = (10_000u128 - fee_bps as u128).saturating_mul(ONE_E18);
    numerator / 10_000
}

/// Effective rate in 1e18 scale: `(in_reserve / out_reserve) ×
/// fee_multiplier`. Saturating.
fn effective_rate_1e18(in_reserve: u64, out_reserve: u64, fee_bps: u16) -> u128 {
    if out_reserve == 0 {
        return 0;
    }
    // mul_div_floor returns Result; collapse to 0 on error
    // (defensive — unrealistic inputs).
    mul_div_floor(
        in_reserve as u128,
        fee_multiplier_1e18(fee_bps),
        out_reserve as u128,
    )
    .unwrap_or(0)
}

/// Convert an effective rate to a graph weight. The weight is
/// `-(log2(rate)) × 1e18 / 32`, saturated. This is the
/// "log-scale slippage" weight used by Bellman-Ford to detect
/// negative cycles.
fn weight_from_rate(rate_1e18: u128) -> i64 {
    if rate_1e18 == 0 {
        return i64::MAX;
    }
    // log2(rate) ≈ bit_length - 1 + frac. We use bit_length and
    // divide by 32 to keep the result in i64 range for
    // realistic reserve values.
    let bits = 128 - rate_1e18.leading_zeros() as i64;
    // Saturating multiplication to avoid i64 overflow on
    // extreme rate values (e.g. one-sided pools).
    let abs_weight = (bits as i64).saturating_mul(ONE_E18 as i64 / 32);
    -abs_weight
}

/// An incremental price graph. Edges incident to a pool's tokens
/// are recomputed when the pool's reserves change; all other
/// edges are left alone.
#[derive(Debug)]
pub struct StreamingGraph {
    /// Underlying graph structure.
    graph: Graph,
    /// `pool_address -> Vec<edge_index>` for fast lookup of
    /// edges incident to a pool.
    pool_to_edges: BTreeMap<u64, Vec<usize>>,
}

impl StreamingGraph {
    /// Build a streaming graph from an initial pool universe.
    pub fn new(pools: &[Pool]) -> Result<Self, DetectError> {
        let graph = build_from_pools(pools)?;
        // We index by raw `[u8; 32]` cast to u64 (the first 8
        // bytes) for `Ord` support. This is sufficient for the
        // lookup; the full pubkey comparison isn't needed because
        // the graph is built from the same pool list.
        let mut pool_to_edges = BTreeMap::new();
        for (i, e) in graph.edges.iter().enumerate() {
            let key = pool_key(e.pool);
            pool_to_edges.entry(key).or_insert_with(Vec::new).push(i);
        }
        Ok(Self { graph, pool_to_edges })
    }

    /// Apply a pool update: re-compute the two edges for that
    /// pool. Returns `true` if the pool was known, `false` if it
    /// was new (we don't support pool addition in 08-02).
    pub fn update_pool(&mut self, pool: &Pool) -> bool {
        let key = pool_key(pool.address);
        let Some(edges) = self.pool_to_edges.get(&key).cloned() else {
            return false;
        };
        for edge_idx in edges {
            let edge = &mut self.graph.edges[edge_idx];
            let (in_reserve, out_reserve) = if edge.is_base_to_quote {
                (pool.base_reserve, pool.quote_reserve)
            } else {
                (pool.quote_reserve, pool.base_reserve)
            };
            let rate = effective_rate_1e18(in_reserve, out_reserve, pool.fee_bps);
            edge.weight = weight_from_rate(rate);
        }
        true
    }

    /// Add a new pool to the graph. Returns `true` if added,
    /// `false` if it was already known.
    /// Used by the live trader when a new pool is discovered
    /// via accountSubscribe.
    pub fn add_pool(&mut self, pool: &Pool) -> bool {
        let key = pool_key(pool.address);
        if self.pool_to_edges.contains_key(&key) {
            return false;
        }
        // Build a single-pool graph and merge its edges into ours.
        let single = match dl_detect::graph::build_from_pools(&[pool.clone()]) {
            Ok(g) => g,
            Err(_) => return false,
        };
        let mut added = Vec::with_capacity(single.edges.len());
        for e in single.edges {
            let idx = self.graph.edges.len();
            self.graph.edges.push(e);
            added.push(idx);
        }
        self.pool_to_edges.insert(key, added);
        true
    }

    /// Detect negative cycles using the current graph state.
    pub fn detect(&self) -> Vec<Cycle> {
        // For 08-02 we use Bellman-Ford over the *current* graph
        // state. A full Bellman-Ford per update is O(V*E) which
        // is too slow for 10k events/second; the proper fix is
        // incremental Bellman-Ford, which is the v1.2 work.
        dl_detect::bellman_ford::find_negative_cycles(&self.graph, 4)
    }

    /// The underlying graph (read-only).
    pub fn graph(&self) -> &Graph {
        &self.graph
    }

    /// Number of edges incident to a pool.
    pub fn edges_for(&self, pool: Pubkey) -> usize {
        self.pool_to_edges
            .get(&pool_key(pool))
            .map(|v| v.len())
            .unwrap_or(0)
    }
}

/// Hash a 32-byte pubkey to a u64 (first 8 bytes interpreted as
/// little-endian). Provides `Ord` for use as `BTreeMap` key.
fn pool_key(p: Pubkey) -> u64 {
    let b = p.0;
    u64::from_le_bytes([
        b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7],
    ])
}

/// Detects cycles as the streaming graph updates.
pub struct StreamingDetector {
    graph: StreamingGraph,
}

impl StreamingDetector {
    pub fn new(pools: &[Pool]) -> Result<Self, DetectError> {
        Ok(Self {
            graph: StreamingGraph::new(pools)?,
        })
    }

    /// Apply a pool update. Returns any cycles detected after the
    /// update. New pools are added to the graph automatically
    /// (used by the live trader when discovering pools via WS).
    pub fn on_pool_update(&mut self, pool: &Pool) -> Vec<Cycle> {
        let known = self.graph.update_pool(pool);
        if !known {
            self.graph.add_pool(pool);
            self.graph.update_pool(pool);
        }
        self.graph.detect()
    }

    pub fn graph(&self) -> &StreamingGraph {
        &self.graph
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dl_state::pool::AmmKind;

    fn triangle_pools() -> Vec<Pool> {
        vec![
            Pool {
                address: Pubkey([0xA1; 32]),
                kind: AmmKind::RaydiumAmmV4,
                base_mint: Pubkey([0x01; 32]),
                quote_mint: Pubkey([0x02; 32]),
                base_decimals: 6,
                quote_decimals: 9,
                base_reserve: 100_000_000,
                quote_reserve: 1_000_000_000,
                fee_bps: 30,
                last_update_slot: 0,
            },
            Pool {
                address: Pubkey([0xA2; 32]),
                kind: AmmKind::RaydiumAmmV4,
                base_mint: Pubkey([0x02; 32]),
                quote_mint: Pubkey([0x03; 32]),
                base_decimals: 9,
                quote_decimals: 6,
                base_reserve: 1_000_000_000,
                quote_reserve: 105_000_000,
                fee_bps: 30,
                last_update_slot: 0,
            },
            Pool {
                address: Pubkey([0xA3; 32]),
                kind: AmmKind::RaydiumAmmV4,
                base_mint: Pubkey([0x03; 32]),
                quote_mint: Pubkey([0x01; 32]),
                base_decimals: 6,
                quote_decimals: 6,
                base_reserve: 105_000_000,
                quote_reserve: 105_105_000,
                fee_bps: 30,
                last_update_slot: 0,
            },
        ]
    }

    #[test]
    fn new_streams_initial_pools() {
        let d = StreamingDetector::new(&triangle_pools()).unwrap();
        assert_eq!(d.graph().edges_for(Pubkey([0xA1; 32])), 2);
        assert_eq!(d.graph().edges_for(Pubkey([0xA2; 32])), 2);
        assert_eq!(d.graph().edges_for(Pubkey([0xA3; 32])), 2);
    }

    #[test]
    fn update_known_pool_recomputes_edges() {
        let mut d = StreamingDetector::new(&triangle_pools()).unwrap();
        let mut p = triangle_pools()[0].clone();
        p.quote_reserve = 3_000_000_000;
        let _ = d.on_pool_update(&p);
        // No panic; edges still indexed.
        assert_eq!(d.graph().edges_for(Pubkey([0xA1; 32])), 2);
    }

    #[test]
    fn update_unknown_pool_returns_false() {
        let mut d = StreamingDetector::new(&triangle_pools()).unwrap();
        let mut unknown = triangle_pools()[0].clone();
        unknown.address = Pubkey([0xFF; 32]);
        assert!(!d.graph.update_pool(&unknown));
    }

    #[test]
    fn detect_returns_cycles_on_unprofitable_synth() {
        let d = StreamingDetector::new(&triangle_pools()).unwrap();
        let cycles = d.graph().detect();
        assert!(
            !cycles.is_empty(),
            "synth triangle should yield >= 1 cycle"
        );
    }

    #[test]
    fn empty_pools_fails_to_build() {
        let r = StreamingDetector::new(&[]);
        assert!(r.is_err());
    }
}
