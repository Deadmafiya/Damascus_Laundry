---
description: "damascus_laundry — current position and accumulated context"
type: ProjectState
about: "damascus_laundry"
---

# Project State

## Project Reference

See: .paul/PROJECT.md (updated 2026-06-17)

**Core value:** Reliable, honest profitability estimation — count profit only after
latency, competition, landing probability, and fees are modeled pessimistically.
**Current focus:** v1.0 Accurate Paper-Trading Engine — Phase 4 (Profit / Cost / Sizing) — Complete

## Current Position

Milestone: v1.0 Accurate Paper-Trading Engine (v1.0.0)
Phase: 4 of 7 (Profit / Cost / Sizing) — Complete
Plan: 04 (real AMM-curve fills, optimal input sizing, all costs, NetProfit boundary object) — single plan, 4 commits
Status: Phase 4 complete, ready to plan Phase 5 (Simulation Core + Paper Ledger)
Last activity: 2026-06-18 — Phase 4 complete; sub-agent model failed for Tasks 4+5 (timeout twice), in-session landed Tasks 4-8

Progress:
- Milestone: [█████░░░░░] ~57% (4 of 7 phases complete)
- Phase 4: [██████████] 100%

## Loop Position

Current loop state:
```
PLAN ──▶ APPLY ──▶ UNIFY
  ✓        ✓        ✓     [Loop complete — ready for next PLAN (Phase 4)]
```

## Performance Metrics

**Velocity:**
- Total plans completed: 4 (01-01, 02-01, 02-02, 04-01)
- Average duration: ~1 session each
- Total execution time: 4 sessions

**By Phase:**

| Phase | Plans | Total Time | Avg/Plan |
|-------|-------|------------|----------|
| 01-foundations | 1/1 | 1 session | 1 session |
| 02-ingestion-pool-state | 2/2 | 2 sessions | 1 session |
| 03-opportunity-detection | 1/1 | 1 session | 1 session |
| 04-profit-cost-sizing | 1/1 | 1 session | 1 session |

## Accumulated Context

### Decisions

| Decision | Phase | Impact |
|----------|-------|--------|
| Language: Rust | Pre-init | All code; preserves backtest-live parity |
| Free JSON-RPC WS first, gRPC-ready | Pre-init | Phase 2 feed; pluggable `Feed` trait |
| Atomic DEX-DEX arbitrage only | Pre-init | Scopes detection/sim to one clean strategy |
| Model losing first (~96% fail) | Pre-init | Accuracy target for sim core (Phase 5/6) |
| Shadow/replay primary method | Pre-init | Drives deterministic-replay design (Phase 1/2) |
| Use jito-foundation/jito-solana as Jito-mechanics reference | Pre-init | Spec for bundle/tip/auction modeling (Phase 5); live node (Phase 7+) |
| Isolate floats in a dedicated `dl-core::display` module | Phase 1 | Keeps value path float-free per AC-2; helper out of the value path |
| Raydium AMM v4 program ID + AmmInfo layout pinned from upstream | Phase 2 | All decoding uses verified 752-byte layout from `raydium-io/raydium-amm` master |
| Sync `Feed` trait + async bridge via `std::sync::mpsc` | Phase 2 | Default `dl-feed` build is async-free; WS code is `#[cfg(feature = "ws")]` |
| Length-prefixed bincode capture format, schema v1 | Phase 2 | Bit-identical replay (AC-1); single-byte magic + u32 schema header + frames |
| Config-driven pool universe (TOML) | Phase 3 | Q1 user decision; pool config loaded at runtime, not hardcoded |
| Cycle leg cap = 3 default, configurable | Phase 3 | Q2 user decision; 3-leg max, override in TOML |
| **DFS for cycle detection (not Bellman-Ford)** | Phase 3 | New: BF pred-only-best chain loses cycles in mixed-positive/negative graphs; DFS over full graph recovers all. Documented in `graph.rs`. |
| Multi-source DFS (one per start node) | Phase 3 | O(V^2 * E) worst case; V≤20 in v1.0 so trivial |
| Linearized weight formulation (1e-18 scale) | Phase 3 | Avoids `ln()` in value path; documented in `graph.rs` |
| **Sub-agent model mixed reliability** | Phase 3 (process) | Sub-agents work for 1-2 well-scoped tasks per spawn; complex algorithmic design with debugging is faster in-session. Logged in EXPERIENCE.md. |
| **Golden-section sizer, not analytical inverse** | Phase 4 | Robust to fee changes and per-leg reserve differences; 64 iters × ~1 µs = ~64 µs/cycle. `1/φ = 0.618` (NOT `φ = 1.618` — that overflows `u128` for `span < 1.6× boundary`). |
| **Cost model: 4 components in u64 lamports** | Phase 4 | base_sig_fee + priority_fee + jito_tip + jito_5%_fee. `default_busy` baseline = 1,110,000 lamports (was 1,080,000 in plan; off-by-1e3 corrected in commit `f7451d6`). |
| **Cycle type relocated to dl-state (orphan-rule workaround)** | Phase 4 | `dl-sim` consumes `Cycle`s; `dl-detect` re-exports from `dl-state::cycle` for backward compat. New `simulate_through_pools` is a free function (not `impl Cycle`), so the orphan rule doesn't bite. |
| **In-session execution for complex algorithmic tasks** | Phase 4 (process) | Sub-agent 2-task limit confirmed; Tasks 4+5 (sizing + NetProfit) needed 3 fix-iterations — sub-agents timed out twice at 600s. In-session landed all 5 remaining tasks in one session. |

### Deferred Issues

| Issue | Origin | Effort | Revisit |
|-------|--------|--------|---------|
| Verify Orca Whirlpool account layout & tick math | Research | L | Phase 3 (or skip for v1.0) |
| Empirically calibrate p_win / tip-to-win curves (no published constants) | Research | L | Phase 6 |
| Deflated-Sharpe exact formula not verified inline | Research | S | Phase 6 |
| Re-pull all point-in-time $/% figures from live data before P&L math | Research | S | Phase 4/6 |
| Multi-account pool assembly (AmmInfo + 2 vault AccountUpdates per pool) | Phase 2 | M | Phase 3 |

### Blockers/Concerns

| Blocker | Impact | Resolution Path |
|---------|--------|-----------------|
| Block-building stack in flux (BAM/Firedancer) | Submit/read-state assumptions may shift | Abstract feed/submit layers (constraint in PROJECT.md) |

## Boundaries (Active)

Protected elements (carried forward; reaffirm in each PLAN):

- Do not introduce floating-point into any value/balance/PnL path (floats only in `dl-core::display`)
- Keep nondeterministic dependencies (Clock/Rng/Feed) behind injectable traits
- Keep the `Feed` abstraction intact so JSON-RPC WS and gRPC are interchangeable (Phase 2)

### Git State

Last commit: d0c7f9e — feat(04-sim): optimal input sizing (golden-section) + NetProfit boundary object
Branch: main
Feature branches merged: none
Phase 4 commits: 4 (9b556c7, c023e29, f7451d6, d0c7f9e) — 2 sub-agent + 2 in-session

## Session Continuity

Last session: 2026-06-18
Stopped at: Phase 4 (Profit / Cost / Sizing) complete. Workspace builds; fmt/clippy/test green; ~110 tests pass + 1 ignored; 4 float-free CI guards (dl-feed, dl-state, dl-detect, dl-sim). `dl-sim` crate is real: fill_constant_product, simulate_cycle (multi-leg + reserve mutation), CostModel (4 components in u64), find_optimal_input (golden-section with 1/φ = 0.618, convexity pre-check, 64 iters), NetProfit boundary object. `dl-detect::cycle::simulate_through_pools` wired to `dl-sim` (free function, not method — orphan rule after the dl-state::cycle re-export). `Cycle`/`Leg`/`Direction` types relocated from `dl-detect::cycle` to `dl-state::cycle` to break the dl-detect ↔ dl-sim cyclic dep. Sub-agent model proved unreliable again (2 attempts, 0 commits on Tasks 4+5); in-session took over and landed Tasks 4-8 in one session.
Next action: /paul:plan for Phase 5 (Simulation Core + Paper Ledger). Phase 5 needs: probability decomposition (p_detect × p_win × p_land), paper-trade gate (when to log a trade), paper ledger (SQLite or similar, schema v2), per-cycle EV computation. The Phase 4/5 boundary object `NetProfit` is the input. dl-feed (WS), dl-state (decoder + registry), dl-detect (graph + cycle detection + free-function sim), dl-sim (fill + cost + size + NetProfit) are all working code. dl-ledger is still a placeholder.
Resume file: .paul/ROADMAP.md (Phase 5 details)
Resume context: 60-s slot-only capture fixture at `crates/dl-feed/tests/fixtures/sample_capture.bincode`; for Phase 5 sim testing we still need a future capture with pool AmmInfo + vault AccountUpdates.

---
*STATE.md — Updated after every significant action*
*Size target: <100 lines (digest, not archive)*
