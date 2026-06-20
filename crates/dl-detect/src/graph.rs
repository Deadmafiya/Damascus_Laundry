//! Price graph: tokens as nodes, pools as directed edges.
//!
//! Each edge is weighted by `1e18 - effective_rate` in 1e-18 scale, so a
//! *negative-weight cycle* in the graph corresponds to a *positive-return*
//! cycle through the underlying pools.
//!
//! ## Weight formulation (v1.0)
//!
//! The canonical DeFiPoser-ARB formulation uses `-ln(effective_rate)` as
//! the edge weight, so that a cycle's weight sum is `-ln(product of
//! rates)` and the standard Bellman-Ford negative-cycle test recovers
//! profitable cycles. That math requires a `ln` primitive in the value
//! path, which (a) is expensive to compute to the precision we need
//! in fixed-point, and (b) is a leaky abstraction across the fixed-point
//! boundary.
//!
//! For v1.0 we use the **linearized** formulation instead:
//!
//! ```text
//! effective_rate_1e18 = (other_reserve / this_reserve) * (1 - fee_bps/10_000)   // 1e-18 scale
//! weight              = 1e18 - effective_rate_1e18                              // signed
//! ```
//!
//! Semantics are preserved:
//! - `weight < 0` ⇔ rate > 1 ⇔ "more comes out than went in" (a profit leg)
//! - `weight > 0` ⇔ rate < 1 ⇔ a loss leg
//! - `weight = 0` ⇔ rate = 1 ⇔ break-even leg
//! - a *negative-weight cycle* still corresponds to a profitable round-trip
//!
//! The substitution is **monotone** in the sense that a `-ln` cycle sum
//! being negative implies the linearized sum is also negative (AM-GM:
//! arithmetic mean of `rate_i` ≥ geometric mean, so `sum(rate_i) > n`
//! whenever `product(rate_i) > 1`). The linearized version is therefore
//! *more* sensitive than the canonical one — it can flag some cycles
//! the log version wouldn't. The per-leg simulator (Phase 4) is what
//! determines whether a flagged cycle is actually tradeable; the
//! detector's job in v1.0 is to be a sufficiently-lossless filter that
//! no profitable cycle is silently dropped.
//!
//! ## Determinism
//!
//! Token ids are assigned in the order unique mints are first seen; a
//! `BTreeMap<[u8;32], TokenId>` backs the mint→id mapping so iteration
//! order is independent of `HashMap` randomization (AC-1 determinism).
//! Pool iteration order is whatever the caller gives us — the
//! determinism contract is "same input → same graph".

use std::collections::BTreeMap;

use dl_core::fixed::mul_div_floor;
use dl_state::pool::AmmKind;
use dl_state::Pool;

use crate::error::DetectError;

/// Scale factor: 1.0 in the graph is represented as `1_000_000_000_000_000_000`.
/// Centralized here so the math below reads as `1e18` in the algebraic form.
const ONE_1E18: u128 = 1_000_000_000_000_000_000;

/// Compact node id. `u32` because Solana has on the order of 10^6 mints
/// of interest to us; the detector should never see more than 10^4 in
/// any single graph build, but `u32` leaves headroom.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TokenId(pub u32);

/// Directed, weighted edge in the price graph.
///
/// `weight` is `1e18 - effective_rate` in 1e-18 fixed-point, signed.
/// `weight < 0` = profitable leg (more out than in); `weight > 0` =
/// loss leg. The Bellman-Ford relaxation uses raw `i64` arithmetic;
/// weights are bounded to `i64` range by the builder (saturating
/// conversion from `i128`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Edge {
    pub from: TokenId,
    pub to: TokenId,
    /// Signed, 1e-18 scale. Negative = profit leg; positive = loss leg.
    pub weight: i64,
    /// Pool this edge was derived from (so the simulator can look up
    /// reserves/fees for the forward fill).
    pub pool: dl_state::Pubkey,
    /// `true` iff this edge is the `base -> quote` direction of the
    /// pool. `false` means it's the `quote -> base` direction. Lets
    /// the BF cycle-recovery step populate `Leg::direction` without a
    /// `PoolRegistry` lookup.
    pub is_base_to_quote: bool,
    /// Which AMM family this edge came from (Phase 7 / plan 02).
    /// Lets the cycle-recovery step route the cycle to the
    /// correct fill-math path: Raydium constant-product,
    /// Orca Whirlpool single-tick, Meteora DLMM per-bin.
    /// Defaults to `RaydiumAmmV4` for backward compatibility
    /// with v1.0 cycle consumers.
    pub dex_id: AmmKind,
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

/// `r = 1 - fee_bps/10_000` as a 1e-18-scaled u128. 25 bps → 997_500e12.
/// Uses `saturating_sub` so `fee_bps > 10_000` clamps the multiplier to
/// 0 (a degenerate 100%-fee pool) rather than underflowing.
fn fee_multiplier_1e18(fee_bps: u16) -> u128 {
    debug_assert!(fee_bps <= 10_000);
    // fee_bps/10_000 in 1e-18 = fee_bps * 1e14.
    let fee_frac_1e18 = (fee_bps as u128).saturating_mul(100_000_000_000_000u128);
    ONE_1E18.saturating_sub(fee_frac_1e18)
}

/// `(other_reserve / this_reserve) * fee_mult`, all in 1e-18 scale.
///
/// This is the spot price (no slippage) of "1 unit of `this` → `other`"
/// after the trading fee. Constant-product AMMs have no slippage at
/// infinitesimal trade size, so this is the right asymptotic limit
/// for the *edge weight* in v1.0; the per-leg simulator in Phase 4 is
/// where slippage actually shows up (via the full `getAmountOut`).
fn effective_rate_1e18(
    other_reserve: u64,
    this_reserve: u64,
    fee_mult_1e18: u128,
) -> Result<u128, DetectError> {
    if this_reserve == 0 {
        return Err(DetectError::InvalidMath(dl_core::MathError::DivByZero));
    }
    // Step 1: (other / this) in 1e-18 scale. `mul_div_floor` widens to
    // 256 bits when `other * 1e18` would overflow u128.
    let rate_no_fee = mul_div_floor(other_reserve as u128, ONE_1E18, this_reserve as u128)?;
    // Step 2: multiply by fee multiplier. Result is still ≤ rate_no_fee.
    let rate_with_fee = mul_div_floor(rate_no_fee, fee_mult_1e18, ONE_1E18)?;
    Ok(rate_with_fee)
}

/// `weight = 1e18 - rate_1e18`, signed and clamped to `i64`.
/// Negative → profitable leg; positive → loss leg.
fn weight_from_rate(rate_1e18: u128) -> i64 {
    let diff: i128 = ONE_1E18 as i128 - rate_1e18 as i128;
    diff.clamp(i64::MIN as i128, i64::MAX as i128) as i64
}

/// Build a `Graph` from a slice of pools.
///
/// For each pool, two directed edges are added:
/// - `base -> quote` weighted by `1e18 - rate(quote/base * (1 - fee))`
/// - `quote -> base` weighted by `1e18 - rate(base/quote * (1 - fee))`
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
/// - `InvalidMath` (`DivByZero`) on zero reserve, or `Overflow` on
///   intermediate rate math (extremely unlikely with realistic reserve
///   magnitudes — u64 reserves maxed at ~1.8e19 → rate_1e18 maxes at
///   ~1.8e37, but `mul_div_floor`'s wide path handles even that).
pub fn build_from_pools(pools: &[Pool]) -> Result<Graph, DetectError> {
    if pools.is_empty() {
        return Err(DetectError::EmptyGraph);
    }

    let mut g = Graph::new();
    for pool in pools {
        let base_id = g.intern_token(pool.base_mint);
        let quote_id = g.intern_token(pool.quote_mint);
        let fee_mult = fee_multiplier_1e18(pool.fee_bps);

        let rate_b2q = effective_rate_1e18(pool.quote_reserve, pool.base_reserve, fee_mult)?;
        let rate_q2b = effective_rate_1e18(pool.base_reserve, pool.quote_reserve, fee_mult)?;

        g.push_edge(Edge {
            from: base_id,
            to: quote_id,
            weight: weight_from_rate(rate_b2q),
            pool: pool.address,
            is_base_to_quote: true,
            dex_id: pool.kind,
        });
        g.push_edge(Edge {
            from: quote_id,
            to: base_id,
            weight: weight_from_rate(rate_q2b),
            pool: pool.address,
            is_base_to_quote: false,
            dex_id: pool.kind,
        });
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
            ..Default::default()
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

    /// Two pools with identical base/quote mints (different addresses
    /// and reserves) should still produce only 2 unique token nodes,
    /// but 4 directed edges (2 per pool) — and every weight must be
    /// non-zero (no real AMM has rate exactly 1.0).
    #[test]
    fn two_pools_same_pair_make_4_edges() {
        let pools = vec![
            sample_pool(1, 2, 3, 1_000_000_000, 2_000_000_000, 25),
            sample_pool(99, 2, 3, 1_500_000_000, 3_000_000_000, 25),
        ];
        let g = build_from_pools(&pools).unwrap();
        assert_eq!(g.n_tokens(), 2);
        assert_eq!(g.n_edges(), 4);

        // Tokens 2 and 3 are interned once. Every edge endpoint must
        // be one of them.
        let b = g.token_id(&[2u8; 32]).unwrap();
        let q = g.token_id(&[3u8; 32]).unwrap();
        for e in &g.edges {
            assert!(
                (e.from == b && e.to == q) || (e.from == q && e.to == b),
                "edge has unexpected token endpoints: {e:?}"
            );
            assert_ne!(e.weight, 0, "weight must be non-zero for a real AMM rate");
        }

        // Both pools have identical base:quote reserves ratio (2:1) so
        // both contribute the same pair of weights. After sorting, the
        // multiset of weights should be {w, w, w', w'} — i.e. exactly
        // two distinct values, each appearing twice. We don't pin the
        // exact values here (other tests do that); we just assert the
        // duplicate structure.
        let mut ws: Vec<i64> = g.edges.iter().map(|e| e.weight).collect();
        ws.sort();
        let mut counts: BTreeMap<i64, usize> = BTreeMap::new();
        for w in &ws {
            *counts.entry(*w).or_insert(0) += 1;
        }
        let counts: Vec<(i64, usize)> = counts.into_iter().collect();
        assert_eq!(
            counts.len(),
            2,
            "two pools with the same base:quote ratio should produce exactly two distinct weight values, got {ws:?}"
        );
        for (_w, n) in &counts {
            assert_eq!(*n, 2, "each weight should appear twice, got {counts:?}");
        }

        // The pool addresses must all be present and unique.
        let mut addrs: Vec<Pubkey> = g.edges.iter().map(|e| e.pool).collect();
        addrs.sort_by_key(|p| p.0);
        addrs.dedup();
        assert_eq!(addrs.len(), 2, "expected 2 distinct pool addresses");
    }

    /// `weight = 1e18 - rate`. If rate < 1 the difference is positive
    /// (a "loss" leg — 1 unit of `from` returns less than 1 unit of
    /// `to`). Synthetic test: a pool with quote < base, fee=0, so the
    /// base→quote rate is exactly 0.5.
    #[test]
    fn weight_positive_for_loss_leg() {
        // quote < base, fee=0 → base→quote rate = 0.5
        // → weight_base_to_quote = 1e18 - 0.5e18 = +0.5e18
        let p = sample_pool(1, 2, 3, /*br*/ 2, /*qr*/ 1, /*fee*/ 0);
        let g = build_from_pools(&[p]).unwrap();
        assert_eq!(g.edges.len(), 2);
        // Edge 0: base → quote
        assert!(
            g.edges[0].weight > 0,
            "base→quote weight should be positive for rate<1, got {}",
            g.edges[0].weight
        );
        // 0.5e18 in i64 fixed-point. mul_div_floor on
        // (1 * 1e18 / 2) gives floor(0.5e18) = 500_000_000_000_000_000.
        assert_eq!(g.edges[0].weight, 500_000_000_000_000_000);
        // Edge 1: quote → base, rate = 2.0 → weight = -1e18 (profit leg)
        assert_eq!(g.edges[1].weight, -1_000_000_000_000_000_000);
    }

    /// Symmetric to the loss-leg test: a pool with rate > 1 produces
    /// a *negative* weight (more out than in, ignoring fees). Same
    /// pool, swapped reserve roles: quote = 2, base = 1.
    #[test]
    fn weight_negative_for_profit_leg() {
        let p = sample_pool(1, 2, 3, /*br*/ 1, /*qr*/ 2, /*fee*/ 0);
        let g = build_from_pools(&[p]).unwrap();
        assert_eq!(g.edges.len(), 2);
        // Edge 0: base → quote, rate = 2.0 → weight = -1e18
        assert_eq!(g.edges[0].weight, -1_000_000_000_000_000_000);
        // Edge 1: quote → base, rate = 0.5 → weight = +0.5e18
        assert_eq!(g.edges[1].weight, 500_000_000_000_000_000);
    }

    /// With non-zero fees the weight is slightly less negative (or
    /// more positive) than the zero-fee case. Sanity-check the
    /// magnitude direction.
    #[test]
    fn fee_reduces_magnitude_of_profit_leg_weight() {
        // Same rate-b2q = 2.0 pool, once with 0 fee and once with
        // 25 bps. The 25 bps case must be strictly less negative
        // (closer to zero) than the 0 fee case.
        let zero = build_from_pools(&[sample_pool(1, 2, 3, 1, 2, 0)]).unwrap();
        let with_fee = build_from_pools(&[sample_pool(2, 2, 3, 1, 2, 25)]).unwrap();
        let w_zero = zero.edges[0].weight;
        let w_fee = with_fee.edges[0].weight;
        assert!(
            w_zero < 0,
            "zero-fee weight should be negative (profit leg)"
        );
        assert!(w_fee < 0, "with-fee weight should still be negative");
        assert!(
            w_fee > w_zero,
            "fee should pull weight toward zero (less negative): w_zero={w_zero}, w_fee={w_fee}"
        );
    }

    /// Zero reserve → `DivByZero` from the rate computation. The pool
    /// is malformed on-chain, but the builder must surface the error
    /// rather than silently producing a weight=0 edge.
    #[test]
    fn zero_reserve_errors_with_div_by_zero() {
        let p = sample_pool(1, 2, 3, 0, 1_000_000, 25);
        let g = build_from_pools(&[p]);
        assert!(matches!(
            g,
            Err(DetectError::InvalidMath(dl_core::MathError::DivByZero))
        ));
    }

    /// Fee = 100% clamps the fee multiplier to 0 → every effective
    /// rate becomes 0 → every weight becomes +1e18 (max loss leg).
    /// Not an error: it's a degenerate-but-coherent pool.
    #[test]
    fn full_fee_pool_yields_max_loss_weights() {
        let p = sample_pool(1, 2, 3, 1_000_000, 2_000_000, 10_000);
        let g = build_from_pools(&[p]).unwrap();
        for e in &g.edges {
            assert_eq!(e.weight, ONE_1E18 as i64);
        }
    }

    /// AC-4: a triangle involving one Raydium, one Orca, and one
    /// Meteora pool around a common token triplet is built into
    /// a graph whose edges carry the per-DEX `dex_id` field. The
    /// detection step (Bellman-Ford) is exercised separately;
    /// this test pins the per-DEX edge labeling.
    ///
    /// Triangle: USDC (mint 1) -> SOL (mint 2) -> USDT (mint 3)
    /// -> USDC (mint 1), one pool per DEX.
    #[test]
    fn multi_dex_triangle_dex_id_labeling() {
        let usdc: u8 = 1;
        let sol: u8 = 2;
        let usdt: u8 = 3;
        // Raydium: USDC/SOL. 100 USDC = 1 SOL (price = 100).
        let raydium = Pool {
            address: Pubkey([0xA1; 32]),
            kind: AmmKind::RaydiumAmmV4,
            base_mint: Pubkey([usdc; 32]),
            quote_mint: Pubkey([sol; 32]),
            base_decimals: 6,
            quote_decimals: 9,
            base_reserve: 100_000_000,    // 100 USDC
            quote_reserve: 1_000_000_000, // 1 SOL
            fee_bps: 30,
            last_update_slot: 0,
            ..Default::default()
        };
        // Orca Whirlpool: SOL/USDT. Price-edge of 5% on top
        // of the Raydium 100x for a profitable round-trip.
        // We model this as: 1 SOL = 105 USDT (constant-product
        // approximation; the real Orca fill uses Q64.64).
        let orca = Pool {
            address: Pubkey([0xA2; 32]),
            kind: AmmKind::OrcaWhirlpool,
            base_mint: Pubkey([sol; 32]),
            quote_mint: Pubkey([usdt; 32]),
            base_decimals: 9,
            quote_decimals: 6,
            base_reserve: 1_000_000_000, // 1 SOL
            quote_reserve: 105_000_000,  // 105 USDT
            fee_bps: 30,
            last_update_slot: 0,
            ..Default::default()
        };
        // Meteora DLMM: USDT/USDC. 1.001 USDC = 1 USDT.
        let meteora = Pool {
            address: Pubkey([0xA3; 32]),
            kind: AmmKind::MeteoraDlmm,
            base_mint: Pubkey([usdt; 32]),
            quote_mint: Pubkey([usdc; 32]),
            base_decimals: 6,
            quote_decimals: 6,
            base_reserve: 1_001_000_000_000, // 1.001M USDT
            quote_reserve: 1_000_000_000,    // 1M USDC
            fee_bps: 30,
            last_update_slot: 0,
            ..Default::default()
        };

        let g = build_from_pools(&[raydium, orca, meteora]).expect("graph");
        // Each edge must carry the dex_id matching its pool.
        let mut seen_per_dex = Vec::new();
        for e in &g.edges {
            let expected = match e.pool.0[0] {
                0xA1 => AmmKind::RaydiumAmmV4,
                0xA2 => AmmKind::OrcaWhirlpool,
                0xA3 => AmmKind::MeteoraDlmm,
                other => panic!("unexpected pool byte {other}"),
            };
            assert_eq!(
                e.dex_id, expected,
                "dex_id mismatch for pool {:?}",
                e.pool.0[0]
            );
            if !seen_per_dex.contains(&e.dex_id) {
                seen_per_dex.push(e.dex_id);
            }
        }
        // All three DEXs represented.
        assert_eq!(seen_per_dex.len(), 3);
        assert!(seen_per_dex.contains(&AmmKind::RaydiumAmmV4));
        assert!(seen_per_dex.contains(&AmmKind::OrcaWhirlpool));
        assert!(seen_per_dex.contains(&AmmKind::MeteoraDlmm));
    }
}
