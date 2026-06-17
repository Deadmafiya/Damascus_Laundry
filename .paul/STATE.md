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
Phase: 2 of 7 (Ingestion + Pool State) — Complete
Plan: 02-01 (WS feed + capture/replay) + 02-02 (AMM decoders + dry-run) both DONE
Status: Phase 2 complete, ready to plan Phase 3
Last activity: 2026-06-18 — Phase 2 unified & transitioned; 15 commits on top of b094547

Progress:
- Milestone: [███░░░░░░░] ~28% (2 of 7 phases complete)
- Phase 2: [██████████] 100%

## Loop Position

Current loop state:
```
PLAN ──▶ APPLY ──▶ UNIFY
  ✓        ✓        ✓     [Loop complete — ready for next PLAN (Phase 3)]
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

Last commit: 32f1822 — feat(02-app): dry-run mode replays sample capture through PoolRegistry decoder
Branch: main
Feature branches merged: none
Phase 2 commits: 15 (197e92a, d90e570, 25bb71e, b17ac3b, 72ec64d, b6e0f23, 89f748a, ac63cd8, 76457f1, e592aae, fbf47c8, 37830b3, 321214a, 32f1822 + 1 hash-recording chore)

## Session Continuity

Last session: 2026-06-18
Stopped at: Phase 2 (Ingestion + Pool State) complete — unified & transitioned. Workspace builds; fmt/clippy/test green; 60 tests pass + 2 ignored (live WS + live AmmInfo); 2 float-free CI guards pass; live AmmInfo decode against real Raydium SOL/USDC pool verified.
Next action: /paul:plan for Phase 3 (Opportunity Detection). Phase 3 needs: price graph (pools as edges, tokens as nodes) + Bellman-Ford negative-cycle detection + cycle-path recovery. The captured sample from 02-01-07 contains slot-only data; for cycle-detection tests we need a future capture that includes a pool's AmmInfo + vault account updates.
Resume file: .paul/ROADMAP.md (Phase 3 details)
Resume context: dl-feed (WsFeed, CaptureWriter/Reader, CapturingFeed tee, capture format v1) and dl-state (Pool, PoolRegistry, Raydium AMM v4 decoder) are real code. dl-detect, dl-sim, dl-ledger are still placeholders. 60-s slot-only capture fixture at `crates/dl-feed/tests/fixtures/sample_capture.bincode`; verified real pool `3sjNoCnkkhWPVXYGDtem8rCciHSGc9jSFZuUAzKbvRVp` decodes end-to-end (reserves 15.6T lamports / 40.5T micro-USDC, 25 bps fee, 9/6 decimals).

---
*STATE.md — Updated after every significant action*
*Size target: <100 lines (digest, not archive)*
