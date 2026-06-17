//! Bellman-Ford negative-cycle detection on the price graph.
//!
//! v1.0 stub: the real implementation lands in task 03-03. For now we
//! return an empty cycle list so downstream code can be wired up and
//! the rest of the scaffolding compiles & tests.
//!
//! ## Algorithm sketch (for 03-03)
//!
//! DeFiPoser-ARB:
//! 1. Initialize `dist[v] = 0` for all `v` and `pred[v] = None`.
//! 2. Relax all edges `|V|` times.
//! 3. After the `|V|`th pass, any edge `(u, v)` with
//!    `dist[u] + w(u,v) < dist[v]` lies on (or is reachable from) a
//!    negative-weight cycle.
//! 4. Walk predecessors `|V|` steps from `v` to land inside the cycle,
//!    then recover the cycle by walking `pred` until we revisit a node.
//!
//! The hard parts (which 03-03 has to solve):
//! - bounding the search by `max_legs` so we don't recover 200-leg cycles,
//! - recovering the cycle as a `Vec<Leg>` rather than just node ids,
//! - rejecting cycles that don't actually go through a profit-making
//!   edge (the relaxation finds reachable negative cycles too).

use crate::cycle::Cycle;
use crate::graph::Graph;

/// Find all (or up to some cap of) negative-weight cycles in `graph`,
/// each recovered as a [`Cycle`] with at most `max_legs` legs.
///
/// **v1.0 stub:** always returns an empty `Vec`. The real
/// Bellman-Ford-Moore implementation is in task 03-03.
pub fn find_negative_cycles(_graph: &Graph, _max_legs: usize) -> Vec<Cycle> {
    // Real implementation deferred to 03-03.
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::build_from_pools;
    use dl_state::{AmmKind, Pool, Pubkey};

    fn pool(addr: u8, base: u8, quote: u8, br: u64, qr: u64, fee_bps: u16) -> Pool {
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
    fn stub_returns_empty() {
        // Build a tiny graph: one pool, two tokens, two edges.
        // Even with a real BF we wouldn't expect a cycle (no closed walk
        // through a single edge), but for the v1.0 stub we just verify
        // the function runs and returns an empty Vec.
        let pools = [pool(1, 2, 3, 1_000_000, 2_000_000, 25)];
        let g = build_from_pools(&pools).unwrap();
        let cycles = find_negative_cycles(&g, 4);
        assert!(cycles.is_empty(), "stub returned {cycles:?}");
    }

    #[test]
    fn stub_returns_empty_on_multi_pool_graph() {
        // Triangle of pools: A/B, B/C, C/A. Real BF could find a cycle
        // here in 03-03; the stub returns empty regardless.
        let pools = [
            pool(1, 2, 3, 1_000, 2_000, 25),
            pool(4, 3, 5, 5_000, 7_000, 30),
            pool(7, 5, 2, 4_000, 3_500, 20),
        ];
        let g = build_from_pools(&pools).unwrap();
        assert_eq!(g.n_tokens(), 3);
        assert_eq!(g.n_edges(), 6);
        let cycles = find_negative_cycles(&g, 8);
        assert!(cycles.is_empty(), "stub returned {cycles:?}");
    }
}
