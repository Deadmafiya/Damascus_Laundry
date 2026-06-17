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
**Current focus:** v1.0 Accurate Paper-Trading Engine — Phase 3 (Opportunity Detection)

## Current Position

Milestone: v1.0 Accurate Paper-Trading Engine (v1.0.0)
Phase: 3 of 7 (Opportunity Detection) — Complete
Plan: 03 (price graph + cycle detection) — single plan, 4 commits
Status: Phase 3 complete, ready to plan Phase 4 (Profit / Cost / Sizing)
Last activity: 2026-06-18 — Phase 3 complete; sub-agent model mixed, in-session fallback for BF/DFS impl

Progress:
- Milestone: [████░░░░░░] ~42% (3 of 7 phases complete)
- Phase 3: [██████████] 100%

## Loop Position

Current loop state:
```
PLAN ──▶ APPLY ──▶ UNIFY
  ✓        ✓        ✓     [Loop complete — ready for next PLAN (Phase 4)]
```

## Performance Metrics

**Velocity:**
- Total plans completed: 3 (01-01, 02-01, 02-02)
- Average duration: ~1 session each
- Total execution time: 3 sessions

**By Phase:**

| Phase | Plans | Total Time | Avg/Plan |
|-------|-------|------------|----------|
| 01-foundations | 1/1 | 1 session | 1 session |
| 02-ingestion-pool-state | 2/2 | 2 sessions | 1 session |
| 03-opportunity-detection | 1/1 | 1 session | 1 session |

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

Last commit: bb9aaef — test(03-detect): float-free CI guard for dl-detect value paths
Branch: main
Feature branches merged: none
Phase 3 commits: 4 (50c21e6, e95d71c, d8d289c, bb9aaef) — 2 sub-agent + 2 in-session

## Session Continuity

Last session: 2026-06-18
Stopped at: Phase 3 (Opportunity Detection) complete. Workspace builds; fmt/clippy/test green; 54 tests pass + 1 ignored; 3 float-free CI guards pass. dl-detect crate real: graph builder (linearized weights in i64 1e-18 scale), DFS-based negative-cycle detection, max_legs cap, dedup by (pool, direction). Sub-agent model proved unreliable for complex algorithmic impl (3 of 4 attempts hit 600s/reasoning budget with zero/partial work); in-session execution took over for the BF/DFS design.
Next action: /paul:plan for Phase 4 (Profit / Cost / Sizing). Phase 4 needs: real constant-product AMM fill math (`dy = (y * dx) / (x + dx) * (1 - fee)`), optimal input sizing (closed-form marginal revenue = 0), cost model (sig fee + priority fee + Jito tip), wire `Cycle::simulate_through_pools` to return real fill output. `dl-sim` is still a placeholder.
Resume file: .paul/ROADMAP.md (Phase 4 details)
Resume context: dl-feed (live WS), dl-state (real Raydium AMM v4 decoder), dl-detect (real graph + DFS cycle detection) are working code. dl-sim, dl-ledger are still placeholders. 60-s slot-only capture fixture at `crates/dl-feed/tests/fixtures/sample_capture.bincode`; for Phase 4 sim testing we still need a future capture with pool AmmInfo + vault AccountUpdates.

---
*STATE.md — Updated after every significant action*
*Size target: <100 lines (digest, not archive)*
