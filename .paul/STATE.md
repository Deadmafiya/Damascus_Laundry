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
**Current focus:** v1.0 Accurate Paper-Trading Engine — Phase 2 (Ingestion + Pool State)

## Current Position

Milestone: v1.0 Accurate Paper-Trading Engine (v1.0.0)
Phase: 2 of 7 (Ingestion + Pool State) — Ready to plan
Plan: None yet (Phase 1 complete)
Status: Phase 1 complete, ready to plan Phase 2
Last activity: 2026-06-17 — Phase 1 unified & transitioned; committed

Progress:
- Milestone: [█░░░░░░░░░] ~14% (1 of 7 phases complete)
- Phase 2: [░░░░░░░░░░] 0%

## Loop Position

Current loop state:
```
PLAN ──▶ APPLY ──▶ UNIFY
  ✓        ✓        ✓     [Loop complete — ready for next PLAN (Phase 2)]
```

## Performance Metrics

**Velocity:**
- Total plans completed: 1
- Average duration: ~1 session
- Total execution time: n/a (single session)

**By Phase:**

| Phase | Plans | Total Time | Avg/Plan |
|-------|-------|------------|----------|
| 01-foundations | 1/1 | 1 session | 1 session |
*Updated after each plan completion*

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

### Deferred Issues

| Issue | Origin | Effort | Revisit |
|-------|--------|--------|---------|
| Verify Raydium/Orca program IDs & account offsets (UNVERIFIED in research) | Research | M | Phase 2 |
| Empirically calibrate p_win / tip-to-win curves (no published constants) | Research | L | Phase 6 |
| Deflated-Sharpe exact formula not verified inline | Research | S | Phase 6 |
| Re-pull all point-in-time $/% figures from live data before P&L math | Research | S | Phase 4/6 |

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

Last commit: 2c95d55 — feat(01-foundations): deterministic Rust workspace + fixed-point core
Branch: main
Feature branches merged: none

## Session Continuity

Last session: 2026-06-17
Stopped at: Phase 1 (Foundations) complete — unified & transitioned. Workspace builds; fmt/clippy/test green; 17 tests pass; CI added; PROJECT/ROADMAP evolved; phase committed.
Next action: /paul:plan for Phase 2 (Ingestion + Pool State). Phase 2 needs research: confirm Raydium/Orca program IDs & account layouts against live IDL/SDK before decoding.
Resume file: .paul/ROADMAP.md (Phase 2 details)
Resume context: dl-core has real code (fixed-point + Clock/Rng/Feed + FeedEvent). dl-feed/state/detect/sim/ledger are placeholders. Phase 2 fills dl-feed (JSON-RPC WS Feed impl + raw capture/replay) and dl-state (constant-product AMM decoders → normalized state).

---
*STATE.md — Updated after every significant action*
*Size target: <100 lines (digest, not archive)*
