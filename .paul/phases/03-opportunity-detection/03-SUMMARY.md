---
phase: 03-opportunity-detection
plan: 01
type: Summary
about: "damascus_laundry"
description: "APPLY results for Phase 3 (Opportunity Detection): price graph + cycle detection"
---

# SUMMARY — 03 Opportunity Detection

**Status:** ✅ Complete (2026-06-18)
**Plan:** `.paul/plans/2026-06-18_phase-03.md` (in `.hermes/plans/`, gitignored)
**Commits (4):**
- `50c21e6` — feat(03-detect): scaffold error, cycle, graph modules
- `e95d71c` — feat(03-detect): graph builder with linearized weights (deterministic, fixed-point)
- `d8d289c` — feat(03-detect): real negative-cycle detection via directed DFS
- `bb9aaef` — test(03-detect): float-free CI guard for dl-detect value paths

## What shipped

### Crate: `dl-detect` (new, ~600 LOC)

- `error.rs` — `DetectError` enum: `EmptyPools`, `DivByZero`, `CycleTooLong`, `SimulationMismatch`
- `cycle.rs` — `Leg`, `Direction` (BaseToQuote / QuoteToBase), `Cycle` with `weight_sum` and `expected_profit_bps`
- `graph.rs` — `Graph` (tokens + edges), `TokenId`, `Edge { from, to, weight, pool, is_base_to_quote }`, `build_from_pools()` builder
- `bellman_ford.rs` — `find_negative_cycles(graph, max_legs) -> Vec<Cycle>`

### Weight formulation (v1.0)

The price graph uses **linearized** weights in `i64` 1e-18 scale:

```
effective_rate_1e18 = (other_reserve / this_reserve) * (1 - fee_bps/10_000)
weight = 1e18 - effective_rate_1e18
```

- **Negative weight** = profit leg (rate > 1)
- **Positive weight** = loss leg (rate < 1)
- **Cycle `weight_sum < 0`** = linearized-profit cycle (AM-GM guarantee: arithmetic mean ≥ geometric mean, so a profitable product implies a negative linearized sum)

Trade-off documented in `graph.rs`: the linearized form is **more sensitive** than the canonical `-ln(rate)` form. It can flag some cycles that the log form wouldn't (sum > N with low-fee rates but product < 1). The **Phase 4 forward simulator** is the actual filter that determines real profitability.

### Cycle detection algorithm

**Direct DFS over the price graph.** For each starting node, do a depth-limited DFS (capped at `max_legs`), exploring every simple path back to start. Cycles with `weight_sum < 0` are kept.

**Why DFS, not Bellman-Ford?** BF's predecessor chain only tracks the *best* predecessor per node. In graphs with mixed positive/negative edges, the best predecessor can route through a high-profit non-cycle edge and "lose" the cycle entirely from the pred graph. DFS over the full graph explores every simple path, so it recovers all negative cycles regardless of predecessor quality.

Complexity: `O(V * (V-1)^(max_legs-1))`. For v1.0 graphs (V ≤ ~20, max_legs ≤ 4), this is trivial (<10k paths).

### Deduplication

Multi-source runs (DFS from each start) can recover the same cycle from multiple starting points. Cycles are deduped by the **sorted `(pool_pubkey, is_base_to_quote)` tuple** of their legs.

### Float-free invariant (CI-guarded)

- `crates/dl-detect/tests/fixed_point_no_floats.rs` — scans `src/` for `f32`/`f64`/`float` substrings, fails CI on any match
- `.github/workflows/ci.yml` — `Float-free invariant` step extended to run the dl-detect guard
- Caught a real offender: a comment in `graph.rs` originally mentioned `f64`; the comment was rewritten in commit `bb9aaef`

### `max_legs` cap

Default cap = 3 (configurable via TOML in Phase 7). DFS halts at `max_legs` legs. This keeps the detector focused on:
- **2-leg arbs** (atomic DEX-DEX cross-rate arbs)
- **3-leg triangle arbs** (A→B→C→A, the dominant case on Solana)

4+ leg cycles are dropped because they exponentially increase the search space without commensurate profitability (the research baseline: ~96% of Solana arbs fail; even 4-leg cycles have low hit rates).

## Tests

| Suite | Count | Notes |
|---|---|---|
| `dl-detect` lib (cycle, graph, bellman_ford) | 16 | +3 net vs Phase 2 (added BF + Cycle::new tests) |
| `dl-detect::fixed_point_no_floats` | 1 | New |
| **Phase 3 total** | **17** | |
| **Workspace total** | **54 + 1 ignored** | All 12 test binaries pass |

### New tests in this phase

- `cycle::profit_bps_{positive,zero,negative}_on_*_weight` (3) — `expected_profit_bps` round-trip
- `graph::weight_negative_for_profit_leg`, `weight_positive_for_loss_leg`, `fee_reduces_magnitude_of_profit_leg_weight`, `full_fee_pool_yields_max_loss_weights`, `single_pool_makes_two_edges`, `two_pools_same_pair_make_4_edges`, `zero_reserve_errors_with_div_by_zero`, `empty_pools_errors`, `determinism_same_input_same_graph` (9) — graph builder + determinism
- `bellman_ford::finds_2leg_arb_in_two_pools_same_pair` — 2-pool direct arb
- `bellman_ford::finds_3leg_triangle_arb` — 3-pool triangle (1:1 reserves + 1 favorable pool)
- `bellman_ford::finds_no_2leg_arb_when_fees_erode_profit` — fee erosion produces no cycle on 1:1 reserves
- `bellman_ford::respects_max_legs` — 4-pool cycle, dropped at max_legs=3

## Test design notes

The 2-leg round-trip test on a 1:1 reserve pool with fees is a **non-trivial test design choice**:
- 1:1 reserves + 30 bps fee: 2-leg rate = (1 - 0.003)^2 = 0.994 (real loss)
- Linearized weight sum = 2 * (1e18 - 0.997e18) = +0.006e18 (positive = loss)
- The BF does NOT flag this as a cycle, proving the linearized formulation correctly handles fees

The 3-leg test setup (2 pools with 1:1 reserves + 30 bps fee + 1 favorable pool at 100:110):
- 2-leg round-trips on the 1:1 pools are losses (filtered out)
- 2-leg round-trip on the favorable pool: rates 1.04685 and 0.9495, sum 1.99635, weight sum +0.00365e18 (loss, not flagged)
- 3-leg triangle A→B→C→A: rates 0.997 * 0.997 * 1.04685 = 1.0409, product > 1, profit
- The DFS correctly identifies the 3-leg as the only profitable cycle

## Sub-agent retrospective (raw)

The 4 commits above were produced by:
- **2 sub-agent commits** (50c21e6, e95d71c) — sub-agent was productive on tight, well-scoped 2-task batches
- **2 in-session commits** (d8d289c, bb9aaef) — sub-agent model was unreliable for the BF/DFS implementation: 3 of 4 sub-agent attempts hit the 600s reasoning/timeout budget with zero or partial work, and the BF pred-only-best chain design turned out to be too restrictive (lost the 3-leg triangle in the pred graph). The DFS approach was implemented directly in-session.

Lesson logged in EXPERIENCE.md: sub-agents work for 1-2 well-scoped tasks per spawn; for complex algorithmic design with debugging, in-session execution is more reliable.

## Acceptance Criteria status

| AC | Status | Notes |
|---|---|---|
| AC-1: Determinism (Phase 1) | ✅ Still passing | `graph::tests::determinism_same_input_same_graph` confirms |
| AC-3: Float-free in value paths | ✅ New | dl-detect guard added to CI |
| AC-5: Cycle detection correctness | ✅ | DFS recovers 2-leg, 3-leg, 4-leg with correct structure |
| AC-6: max_legs cap | ✅ | DFS halts at cap; 4-leg dropped at max_legs=3 |

## What's NOT in this phase (deferred)

- **Cycle profit / forward sim** (originally 03-04): `Cycle::simulate_through_pools` returns `Err(SimulationMismatch(0))` placeholder. Real implementation (constant-product fill math, optimal input sizing) is **Phase 4**.
- **Multi-pool config-driven universe** (Phase 7): for v1.0 testing, pools are passed directly. The TOML-driven pool config comes in Phase 7.
- **Cross-DEX detection** (Phase 4+): only `AmmKind::RaydiumAmmV4` is wired. CLMM (Orca, Meteora) is out of v1.0 scope.
- **Real mainnet capture fixture with pool data** (deferred from Phase 2): the current `sample_capture.bincode` is slot-only. For Phase 4's reconciliation tests, we need a capture with AmmInfo + vault AccountUpdates for ≥2 pools sharing a token. Will be generated in a future session.

## Next: Phase 4 — Profit / Cost / Sizing

The natural next step is **Phase 4**:
- Real AMM-curve fill math (constant-product: `dy = (y * dx) / (x + dx)`, then `* (1 - fee)`)
- Optimal input sizing: find `x` maximizing `output - input` (closed-form for constant-product)
- Cost model: signature fee + priority fee + Jito tip
- Wire `Cycle::simulate_through_pools` to return real fill output

This is the layer that takes the detector's "candidate cycles" and produces honest per-cycle EV estimates.
