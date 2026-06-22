//! Negative-cycle detection on the price graph via direct DFS.
//!
//! The graph is built by [`crate::graph::build_from_pools`]. Edge weights
//! use the v1.0 linearized formulation (`1e18 - effective_rate`, signed),
//! so a *negative-weight cycle* corresponds to a *profitable round-trip*
//! through the underlying pools.
//!
//! ## Algorithm
//!
//! Direct DFS over the price graph:
//! 1. For each starting node, do a depth-limited DFS up to `max_legs`.
//! 2. At each step, follow every outgoing edge. Prune paths that
//!    revisit an intermediate node (i.e. only allow *simple* cycles).
//! 3. When the DFS returns to the start node, the path is a cycle.
//!    Compute its `weight_sum`; keep it if `weight_sum < 0`.
//! 4. Dedupe cycles across all starts by their sorted
//!    `(pool, direction)` signature.
//!
//! ## Why DFS, not Bellman-Ford?
//!
//! Bellman-Ford's predecessor chain only tracks the *best* predecessor
//! per node. For a graph with mixed positive and negative edges, the
//! best predecessor can route through a high-profit non-cycle edge and
//! "lose" the cycle entirely. DFS over the full graph explores every
//! simple path, so it recovers all negative cycles regardless of
//! predecessor quality.
//!
//! Complexity: `O(V * (V-1)^(max_legs-1))` worst case. For v1.0
//! graphs (V ≤ ~20, max_legs ≤ 4), this is trivial.
//!
//! ## `max_legs`
//!
//! Cycle recovery is capped at `max_legs`. Cycles that exceed the cap
//! are dropped — this is the v1.0 mechanism for keeping the detector
//! focused on triangle arbs (3-leg, the dominant case) and 2-leg direct
//! arbs, and avoiding 4+ leg noise.

use std::collections::BTreeSet;

use crate::cycle::{Cycle, Direction, Leg};
use crate::graph::{Edge, Graph, TokenId};

/// Find negative-weight cycles in `graph`, each recovered as a [`Cycle`]
/// with at most `max_legs` legs.
///
/// Returns an empty `Vec` if no negative cycles are found (or all found
/// cycles exceed `max_legs`).
pub fn find_negative_cycles(graph: &Graph, max_legs: usize) -> Vec<Cycle> {
    if graph.n_tokens() == 0 || max_legs < 2 {
        return Vec::new();
    }
    let n = graph.n_tokens();

    // Adjacency: for each token, list of edges leaving that token.
    // (Each edge has a `to` field; we build a list of edge-indices.)
    let mut adj: Vec<Vec<usize>> = vec![Vec::new(); n];
    for (i, e) in graph.edges.iter().enumerate() {
        adj[e.from.0 as usize].push(i);
    }

    let mut seen_signatures: BTreeSet<Vec<([u8; 32], bool)>> = BTreeSet::new();
    let mut out: Vec<Cycle> = Vec::new();

    // For each starting node, DFS to find simple paths back to start.
    for start in 0..n {
        // Path is a sequence of edge-indices.
        let mut path: Vec<usize> = Vec::with_capacity(max_legs);
        let mut path_nodes: Vec<TokenId> = vec![TokenId(start as u32)];
        dfs(
            start as u32,
            TokenId(start as u32),
            0_i64,
            max_legs,
            &adj,
            &graph.edges,
            &mut path,
            &mut path_nodes,
            &mut seen_signatures,
            &mut out,
        );
    }

    out
}

/// Depth-limited DFS from `start` (the cycle's start node) following
/// edges in `adj`. `cur` is the current node; `weight_so_far` is the
/// cumulative edge weight of the current path.
#[allow(clippy::too_many_arguments)]
fn dfs(
    start: u32,
    cur: TokenId,
    weight_so_far: i64,
    max_legs: usize,
    adj: &[Vec<usize>],
    edges: &[Edge],
    path: &mut Vec<usize>,
    path_nodes: &mut Vec<TokenId>,
    seen_signatures: &mut BTreeSet<Vec<([u8; 32], bool)>>,
    out: &mut Vec<Cycle>,
) {
    // Try every outgoing edge from `cur`.
    for &edge_idx in &adj[cur.0 as usize] {
        let edge = &edges[edge_idx];
        let to = edge.to;

        // We want to find a path that returns to `start` of length 2..=max_legs.
        // If `to == start` and we have at least 1 leg, this closes the cycle.
        if to.0 == start && !path.is_empty() {
            // Check that we haven't exceeded max_legs.
            if path.len() + 1 > max_legs {
                continue;
            }
            let total_weight = match weight_so_far.checked_add(edge.weight) {
                Some(w) => w,
                None => continue,
            };
            if total_weight < 0 {
                // Found a negative cycle. Build legs from `path` + closing edge.
                let mut legs: Vec<Leg> = Vec::with_capacity(path.len() + 1);
                let mut ws: i64 = 0;
                for &eidx in path.iter() {
                    let e = &edges[eidx];
                    let dir = if e.is_base_to_quote {
                        Direction::BaseToQuote
                    } else {
                        Direction::QuoteToBase
                    };
                    ws = match ws.checked_add(e.weight) {
                        Some(v) => v,
                        None => break,
                    };
                    legs.push(Leg {
                        pool: e.pool,
                        direction: dir,
                        weight: e.weight,
                    });
                }
                let dir = if edge.is_base_to_quote {
                    Direction::BaseToQuote
                } else {
                    Direction::QuoteToBase
                };
                ws = match ws.checked_add(edge.weight) {
                    Some(v) => v,
                    None => continue,
                };
                legs.push(Leg {
                    pool: edge.pool,
                    direction: dir,
                    weight: edge.weight,
                });

                if ws < 0 {
                    let mut cycle = Cycle::new(legs);
                    cycle.compute_expected_profit_bps();
                    let sig: Vec<([u8; 32], bool)> = cycle
                        .legs
                        .iter()
                        .map(|l| (l.pool.0, matches!(l.direction, Direction::BaseToQuote)))
                        .collect();
                    let mut sorted_sig = sig.clone();
                    sorted_sig.sort();
                    if seen_signatures.insert(sorted_sig) {
                        out.push(cycle);
                    }
                }
            }
            // Don't extend past `start` (that would just go through the
            // cycle again, producing non-simple paths).
            continue;
        }

        // Don't revisit intermediate nodes (simple paths only).
        if path_nodes.contains(&to) {
            continue;
        }

        // Don't exceed max_legs.
        if path.len() + 1 >= max_legs {
            continue;
        }

        // Recurse.
        let new_weight = match weight_so_far.checked_add(edge.weight) {
            Some(w) => w,
            None => continue,
        };
        path.push(edge_idx);
        path_nodes.push(to);
        dfs(
            start,
            to,
            new_weight,
            max_legs,
            adj,
            edges,
            path,
            path_nodes,
            seen_signatures,
            out,
        );
        path.pop();
        path_nodes.pop();
    }
    let _ = path_nodes; // silence unused (we use it via the recursion)
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
            ..Default::default()
        }
    }

    #[test]
    fn finds_2leg_arb_in_two_pools_same_pair() {
        // Two SOL/USDC pools with different prices. Buying SOL on the
        // cheap pool and selling on the expensive pool is a 2-leg arb.
        let pools = [
            pool(1, 2, 3, 100_000_000_000, 200_000_000_000, 25), // 100 SOL, 200k USDC
            pool(4, 2, 3, 100_000_000_000, 220_000_000_000, 25), // 100 SOL, 220k USDC
        ];
        let g = build_from_pools(&pools).unwrap();
        let cycles = find_negative_cycles(&g, 4);
        assert!(
            !cycles.is_empty(),
            "expected to find at least one 2-leg cycle"
        );
        let c = &cycles[0];
        assert_eq!(c.n_legs(), 2, "expected 2 legs, got {}", c.n_legs());
        assert!(
            c.weight_sum < 0,
            "weight_sum should be negative for a profitable cycle"
        );
        assert!(
            c.expected_profit_bps > 0,
            "expected_profit_bps should be positive"
        );
    }

    #[test]
    fn finds_3leg_triangle_arb() {
        // Triangle: A/B, B/C, C/A. Two pools have 1:1 reserves and a
        // 30 bps fee, so the 2-leg round-trip on each is a real loss.
        // The third pool (C/A) is set favorable (100 C : 110 A) so the
        // *3-leg* triangle is the only profitable cycle.
        let pools = [
            pool(1, 2, 3, 100_000, 100_000, 30), // A/B, 1:1
            pool(2, 3, 5, 100_000, 100_000, 30), // B/C, 1:1
            pool(3, 5, 2, 100_000, 110_000, 30), // C/A, 100:110
        ];
        let g = build_from_pools(&pools).unwrap();
        let cycles = find_negative_cycles(&g, 4);
        let three_leg: Vec<&Cycle> = cycles.iter().filter(|c| c.n_legs() == 3).collect();
        assert!(
            !three_leg.is_empty(),
            "expected at least one 3-leg triangle arb, got {} cycles (legs: {:?})",
            cycles.len(),
            cycles.iter().map(|c| c.n_legs()).collect::<Vec<_>>()
        );
        let c = three_leg[0];
        assert!(c.weight_sum < 0);
        let unique_pools: std::collections::BTreeSet<_> =
            c.legs().iter().map(|l| l.pool.0).collect();
        assert_eq!(unique_pools.len(), 3, "expected all 3 pools in the cycle");
    }

    #[test]
    fn finds_no_2leg_arb_when_fees_erode_profit() {
        // Two pools with 1:1 reserves and 30 bps fees. The 2-leg
        // round-trip has rate = (1 - 0.003)^2 = 0.994, a real loss.
        let pools = [
            pool(1, 2, 3, 100_000, 100_000, 30),
            pool(4, 2, 3, 100_000, 100_000, 30),
        ];
        let g = build_from_pools(&pools).unwrap();
        let cycles = find_negative_cycles(&g, 4);
        assert!(
            cycles.is_empty(),
            "expected no cycles on 1:1 reserves with fees, got {} cycles (legs: {:?})",
            cycles.len(),
            cycles.iter().map(|c| c.n_legs()).collect::<Vec<_>>()
        );
    }

    #[test]
    fn respects_max_legs() {
        // 4-pool cycle that requires max_legs=4. With max_legs=3, the
        // 4-leg cycle is dropped.
        let pools = [
            pool(1, 2, 3, 100_000, 100_000, 30), // 2->3 = 0.997 (loss)
            pool(2, 3, 5, 100_000, 100_000, 30), // 3->5 = 0.997 (loss)
            pool(3, 5, 7, 100_000, 100_000, 30), // 5->7 = 0.997 (loss)
            pool(4, 7, 2, 100_000, 110_000, 30), // 7->2 = 1.04685 (profit)
        ];
        let g = build_from_pools(&pools).unwrap();
        let cycles4 = find_negative_cycles(&g, 4);
        let four_leg: Vec<&Cycle> = cycles4.iter().filter(|c| c.n_legs() == 4).collect();
        assert!(
            !four_leg.is_empty(),
            "expected 4-leg cycle with max_legs=4, got {} cycles (legs: {:?})",
            cycles4.len(),
            cycles4.iter().map(|c| c.n_legs()).collect::<Vec<_>>()
        );
        let cycles3 = find_negative_cycles(&g, 3);
        let four_leg_at_3: Vec<&Cycle> = cycles3.iter().filter(|c| c.n_legs() >= 4).collect();
        assert!(
            four_leg_at_3.is_empty(),
            "expected no 4-leg cycles with max_legs=3, got {}",
            four_leg_at_3.len()
        );
    }
}
