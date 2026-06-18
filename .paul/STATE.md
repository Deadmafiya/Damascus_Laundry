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
**Current focus:** v1.0 Accurate Paper-Trading Engine — Phase 5 (Simulation Core + Paper Ledger)

## Current Position

Milestone: v1.0 Accurate Paper-Trading Engine (v1.0.0)
Phase: 5 of 7 (Simulation Core + Paper Ledger) — Applying
Plan: 05-01 applied (EV decomposition + dual bounds); 05-02 (paper ledger) next
Status: PLAN 05-01 ✅ DONE, ready for 05-02
Last activity: 2026-06-18 — Applied Phase 5 / plan 01 (EV core)

Progress:
- Milestone: [██████░░░░] ~71% (5 of 7 phases complete, Phase 5 in progress)
- Phase 5: [███░░░░░░░] applying (1/2 plans)

## Loop Position

Current loop state:
```
PLAN ──▶ APPLY ──▶ UNIFY
  ◉        ◉        ◑     [05-01 applied; awaiting unify for ledger]
```

## Performance Metrics

**Velocity:**
- Total plans completed: 5 (01-01, 02-01, 02-02, 04-01, 05-01)
- Average duration: ~1 session each
- Total execution time: 5 sessions

**By Phase:**

| Phase | Plans | Total Time | Avg/Plan |
|-------|-------|------------|----------|
| 01-foundations | 1/1 | 1 session | 1 session |
| 02-ingestion-pool-state | 2/2 | 2 sessions | 1 session |
| 03-opportunity-detection | 1/1 | 1 session | 1 session |
| 04-profit-cost-sizing | 1/1 | 1 session | 1 session |
| 05-simulation-core-paper-ledger | 1/2 | 1 session | 1 session |

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
| **05-01 latency = p_land haircut (not state-advance)** | Phase 5 | User decision. Δt → lower p_land, parameterized + tested now. Real "advance state to projected landing slot + re-score" deferred to Phase 6 (needs pool-bearing captures). |
| **Paper ledger = append-only file (schema v2)** | Phase 5 | User decision. Length-prefixed bincode mirroring Phase 2 capture; zero new deps; deterministic; Phase 6 reads it for reconciliation. (Applies to 05-02.) |

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

Last commit: 6b258ed — feat(05-sim): EV decomposition + dual bounds (pessimistic-by-default)
Branch: main
Feature branches merged: none
Phase 5 commits: 1 (6b258ed) — 1 in-session

## Session Continuity

Last session: 2026-06-18
Stopped at: Phase 5 plan 01 (EV decomposition + dual bounds) applied. Workspace builds; fmt/clippy/test green; 170 tests pass + 1 ignored; 4 float-free CI guards (dl-feed, dl-state, dl-detect, dl-sim) green. Next: 05-02 (paper ledger — append-only file, schema v2, zero new deps). Then Phase 6 (reconciliation + calibration).
Next action: Plan 05-02 (paper ledger), then /paul:apply.
Resume file: .paul/phases/05-simulation-core-paper-ledger/05-02-PLAN.md (to be created)
Resume context: dl-sim::ev::ExpectedValue { e_pnl, p_detect, p_win, p_land, expected_failed_cost } is the row schema 05-02 will write. Default values are Phase-6 calibration targets.

---
*STATE.md — Updated after every significant action*
*Size target: <100 lines (digest, not archive)*
