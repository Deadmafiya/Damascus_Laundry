//! Price graph: tokens as nodes, pools as directed edges.
//!
//! Each edge is weighted by `-log(effective rate)` in 1e-18 scale, so a
//! *negative-weight cycle* in the graph corresponds to a *positive-
//! return* cycle through the underlying pools (DeFiPoser-ARB).
//!
//! Token ids are assigned deterministically: insertion order of unique
//! mints is preserved, and a `BTreeMap<[u8;32], TokenId>` backs the
//! mapping so iteration order is independent of `HashMap` randomization
//! (AC-1 determinism).
//!
//! For v1.0, the effective rate used is the *constant-product spot rate
//! after fee*. The `build_from_pools` builder pre-computes the edge
//! weight as `i64` (signed!) with 1.0 = 1e18. This means edges are
//! direction-sensitive: the `base -> quote` edge has a different
//! weight from the `quote -> base` edge on the same pool.

use std::collections::BTreeMap;

use dl_core::fixed::mul_div_floor;
use dl_state::Pool;

use crate::error::DetectError;

/// Compact node id. `u32` because Solana has on the order of 10^6 mints
/// of interest to us; the detector should never see more than 10^4 in
/// any single graph build, but `u32` leaves headroom.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TokenId(pub u32);

/// Directed, weighted edge in the price graph.
///
/// `weight` is `-log(effective rate)` in 1e-18 fixed-point. Negative
/// weight = profitable leg (more out than in, ignoring the log math
/// itself). The Bellman-Ford relaxation uses raw `i64` arithmetic;
/// weights are bounded to `i64` range by the builder.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Edge {
    pub from: TokenId,
    pub to: TokenId,
    /// 1e-18 scaled. `1.0` is represented as `1_000_000_000_000_000_000`.
    pub weight: i64,
    /// Pool this edge was derived from (so the simulator can look up
    /// reserves/fees for the forward fill).
    pub pool: dl_state::Pubkey,
}

/// The price graph itself. `n` is the number of distinct mints; the
/// `tokens` vec and `token_index` map are kept in sync.
#[derive(Debug, Default, Clone)]
pub struct Graph {
    /// `tokens[i]` is the mint pubkey for `TokenId(i)`.
    pub tokens: Vec<dl_state::Pubkey>,
    /// `token_index.get(&mint_bytes) -> Some(TokenId)` for O(log n) lookup.
    pub token_index: BTreeMap<[u8; 32], TokenId>,
    /// All directed edges, including the reverse edges produced by the
    /// builder. Stored in insertion order (which is deterministic
    /// given a deterministic input).
    pub edges: Vec<Edge>,
}

impl Graph {
    /// New empty graph. Use [`build_from_pools`] to construct a
    /// populated one.
    pub fn new() -> Self {
        Self::default()
    }

    /// Number of distinct tokens (== number of nodes).
    pub fn n_tokens(&self) -> usize {
        self.tokens.len()
    }

    /// Number of directed edges (== 2 * number of pools, normally).
    pub fn n_edges(&self) -> usize {
        self.edges.len()
    }

    /// Lookup a token id by its mint pubkey bytes.
    pub fn token_id(&self, mint: &[u8; 32]) -> Option<TokenId> {
        self.token_index.get(mint).copied()
    }

    /// Insert a new unique mint and return its id. No-op (returns the
    /// existing id) if the mint is already present.
    fn intern_token(&mut self, mint: dl_state::Pubkey) -> TokenId {
        if let Some(&id) = self.token_index.get(mint.as_ref()) {
            return id;
        }
        let id = TokenId(self.tokens.len() as u32);
        self.token_index.insert(mint.0, id);
        self.tokens.push(mint);
        id
    }

    /// Add a directed edge. Caller is responsible for assigning weights
    /// in the right scale; this is a low-level helper.
    pub fn push_edge(&mut self, edge: Edge) {
        self.edges.push(edge);
    }
}

/// `r = 1 - fee_bps/10_000` as a 1e-18-scaled u128. 25 bps → 9_975e14.
fn fee_multiplier_1e18(fee_bps: u16) -> u128 {
    debug_assert!(fee_bps <= 10_000);
    let fee_frac_1e18 = (fee_bps as u128).saturating_mul(100_000_000_000_000_000u128 / 10_000);
    1_000_000_000_000_000_000u128.saturating_sub(fee_frac_1e18)
}

/// Build a `Graph` from a slice of pools.
///
/// For each pool, two directed edges are added:
/// - `base -> quote` weighted by `-log(quote_per_base * (1 - fee))`
/// - `quote -> base` weighted by `-log(base_per_quote * (1 - fee))`
///
/// In v1.0 the *spot* constant-product rate is used as the effective
/// rate (the price you'd get for an infinitesimally small trade, less
/// fees). This is the right asymptotic limit and keeps the BF math
/// consistent with the per-leg simulator. Optimal-input sizing against
/// the actual slippage curve is a Phase 4 deliverable; the detector's
/// job here is to *find* cycles, not size them.
///
/// Errors:
/// - `EmptyGraph` if `pools` is empty.
/// - `InvalidMath` on `DivByZero` (zero reserve) or arithmetic overflow.
pub fn build_from_pools(pools: &[Pool]) -> Result<Graph, DetectError> {
    if pools.is_empty() {
        return Err(DetectError::EmptyGraph);
    }

    let mut g = Graph::new();
    // `BTreeMap` gives us deterministic iteration in `pools` order
    // because we intern tokens in the order they appear — but the
    // *pool* order is whatever the caller gave us. We do not sort
    // pools in this commit; the determinism guarantee is:
    // "same `pools` slice → same graph".
    for pool in pools {
        let base_id = g.intern_token(pool.base_mint);
        let quote_id = g.intern_token(pool.quote_mint);
        // TODO(03-04): record `Leg::pool` direction for simulator.
        // For now, just push the edges with weight=0; the builder
        // is fleshed out in 03-02/03-03.
        g.push_edge(Edge {
            from: base_id,
            to: quote_id,
            weight: 0,
            pool: pool.address,
        });
        g.push_edge(Edge {
            from: quote_id,
            to: base_id,
            weight: 0,
            pool: pool.address,
        });
        // The constants `mul_div_floor` and `fee_multiplier_1e18` are
        // referenced here only to keep them from going "unused" before
        // 03-02/03-03 land. Remove this `let _` once the real builder
        // is in place.
        let _ = (mul_div_floor, fee_multiplier_1e18);
    }
    Ok(g)
}

#[cfg(test)]
mod tests {
    use super::*;
    use dl_state::{AmmKind, Pool, Pubkey};

    fn sample_pool(addr: u8, base: u8, quote: u8, br: u64, qr: u64, fee_bps: u16) -> Pool {
        Pool {
            address: Pubkey([addr; 32]),
            kind: AmmKind::RaydiumAmmV4,
            base_mint: Pubkey([base; 32]),
            quote_mint: Pubkey([quote; 32]),
            base_decimals: 9,
            quote_decimals: 6,
            base_reserve: br,
            quote_reserve: qr,
            fee_bps,
            last_update_slot: 1,
        }
    }

    #[test]
    fn empty_pools_errors() {
        let g = build_from_pools(&[]);
        assert!(matches!(g, Err(DetectError::EmptyGraph)));
    }

    #[test]
    fn single_pool_makes_two_edges() {
        let p = sample_pool(1, 2, 3, 1_000_000_000, 2_000_000_000, 25);
        let g = build_from_pools(&[p]).unwrap();
        assert_eq!(g.n_tokens(), 2);
        assert_eq!(g.n_edges(), 2);
        let b = g.token_id(&[2u8; 32]).unwrap();
        let q = g.token_id(&[3u8; 32]).unwrap();
        assert_eq!(g.edges[0].from, b);
        assert_eq!(g.edges[0].to, q);
        assert_eq!(g.edges[1].from, q);
        assert_eq!(g.edges[1].to, b);
    }

    #[test]
    fn determinism_same_input_same_graph() {
        let pools = vec![
            sample_pool(1, 2, 3, 1_000, 2_000, 25),
            sample_pool(4, 3, 5, 5_000, 7_000, 30),
        ];
        let g1 = build_from_pools(&pools).unwrap();
        let g2 = build_from_pools(&pools).unwrap();
        assert_eq!(g1.tokens, g2.tokens);
        assert_eq!(g1.edges.len(), g2.edges.len());
        for (a, b) in g1.edges.iter().zip(g2.edges.iter()) {
            assert_eq!(a, b);
        }
    }
}
