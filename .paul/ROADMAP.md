---
description: "damascus_laundry — milestone and phase structure"
type: Roadmap
about: "damascus_laundry"
---

# Roadmap: damascus_laundry

## Overview

The journey from empty repo to an accurate, reliable Solana atomic-arbitrage
**paper-trading** engine. We build the simulator that tells the truth first — ingestion,
deterministic state, detection, AMM-accurate profit/cost, a pessimistic simulation core,
and a paper ledger — then prove its honesty against on-chain ground truth. Every layer is
shared with the eventual live bot; only the executor changes when (and if) we go live.
v1.0 ships when the engine reproduces the macro reality of Solana arbitrage (notably the
~96% failure rate) and replays deterministically.

## Current Milestone

**v1.0 Accurate Paper-Trading Engine** (v1.0.0)
Status: In progress
Phases: 1 of 7 complete

## Phases

**Phase Numbering:**
- Integer phases (1, 2, 3): planned milestone work
- Decimal phases (2.1): urgent insertions (marked [INSERTED])

Phases execute in numeric order: 1 → 2 → 3 → 4 → 5 → 6 → 7

| Phase | Name | Plans | Status | Completed |
|-------|------|-------|--------|-----------|
| 1 | Foundations | 1 | ✅ Complete | 2026-06-17 |
| 2 | Ingestion + Pool State (read-only) | TBD | Not started | - |
| 3 | Opportunity Detection | TBD | Not started | - |
| 4 | Profit / Cost / Sizing | TBD | Not started | - |
| 5 | Simulation Core + Paper Ledger | TBD | Not started | - |
| 6 | Testing, Reconciliation & Calibration | TBD | Not started | - |
| 7 | Observability & Hardening | TBD | Not started | - |

## Phase Details

### Phase 1: Foundations

**Status:** ✅ Complete (2026-06-17) — 1/1 plans
**Goal:** A Rust workspace that is deterministic-by-construction — fixed-point math,
injectable nondeterministic dependencies, structured logging, CI — so every later phase
inherits reproducibility for free.
**Depends on:** Nothing (first phase)
**Research:** Unlikely (decisions settled; internal scaffolding)

**Scope:**
- Cargo workspace + crate layout (core / feed / state / detect / sim / ledger separation)
- `u128` fixed-point math module (overflow-checked mul/div, explicit scale, decimals
  normalization) with property tests
- Pluggable `Clock`, `Rng`, `Feed` traits (deterministic + real impls)
- Structured logging + CI (fmt, clippy, test)

**Plans:**
- [x] 01-01: Workspace, fixed-point math module, pluggable traits, logging + CI

### Phase 2: Ingestion + Pool State (read-only)

**Goal:** Subscribe to live Solana state for target DEX pools, decode it into normalized
in-memory pool state, and record the raw feed to disk so it can be replayed
deterministically.
**Depends on:** Phase 1 (`Feed` trait, fixed-point, deterministic harness)
**Research:** Likely — exact program IDs / account-layout offsets are UNVERIFIED in
research (Raydium 404, Orca thin SPA). Must confirm against live IDL/SDK before decoding.
**Research topics:** Raydium AMM v4 + CLMM account layouts & program IDs; Orca Whirlpool
field layout & tick math; (optionally) Meteora DLMM bins. Confirm one CLOB leg (Phoenix /
OpenBook v2) for the "fresh quote" side.

**Scope:**
- Standard JSON-RPC WebSocket feed (`accountSubscribe`) behind the `Feed` trait
- Decoders for the first AMM type (constant-product) → normalized pool state
- A "fresh quote" venue (order book) leg for arb pairing
- Raw feed capture-to-disk + deterministic replay source

**Plans:**
- [ ] 02-01: WebSocket `Feed` impl + raw capture/replay
- [ ] 02-02: Pool decoders (constant-product AMM) → normalized state, validated vs SDK/Jupiter

### Phase 3: Opportunity Detection

**Goal:** From in-memory state, flag atomic-arbitrage opportunities and recover the trade
path; prove it by flagging known historical arb windows on replayed data.
**Depends on:** Phase 2 (normalized pool state + replay)
**Research:** Unlikely (well-documented algorithm; DeFiPoser-ARB)

**Scope:**
- Price graph: tokens = nodes, pools = edges weighted by −log(effective rate)
- Bellman-Ford / Moore negative-cycle detection + cycle-path recovery
- Handle Bellman-Ford gotchas (returns first cycle, reports % not absolute)

**Plans:**
- [ ] 03-01: Price graph builder + negative-cycle detection + path recovery

### Phase 4: Profit / Cost / Sizing

**Goal:** Turn a detected cycle into an honest per-opportunity net-profit estimate —
AMM-curve-accurate fills, optimal sizing, full cost netting.
**Depends on:** Phase 3 (detected cycles)
**Research:** Unlikely (AMM math known; verify exact fee tiers/curves per pool)

**Scope:**
- AMM-aware output math (fill each leg against real reserves/curve, sequential
  state mutation across legs — never mid-price); CLMM tick-walking where applicable
- Optimal-input sizing against the convex slippage curve (marginal revenue = 0)
- Cost netting: base sig fee, priority fee (`CU_limit × CU_price/1e6`), Jito tip + 5% fee
- Optional: `simulateTransaction` integration as the exact gross-edge oracle

**Plans:**
- [ ] 04-01: AMM fill math + optimal sizing
- [ ] 04-02: Cost model + (optional) simulateTransaction gross-edge oracle

### Phase 5: Simulation Core + Paper Ledger

**Goal:** The accuracy heart of v1.0 — apply opportunities to a paper portfolio through a
pessimistic-by-default simulation model, and track PnL with attribution.
**Depends on:** Phase 4 (net-profit estimate per opportunity)
**Research:** Unlikely (principles settled in research; calibration values deferred to P6)
**Reference:** Model Jito bundle/tip/relayer/auction semantics against
`jito-foundation/jito-solana` (Jito's validator fork) as the authoritative spec.

**Scope:**
- Multiplicative EV decomposition `E[PnL] = p_detect × p_win × p_land × (gross − costs) − E[failed_costs]`
- Latency model: re-check viability at projected landing slot (advance replayed state by
  Δt distribution; anchor to ~200 ms Jito window / 50 ms ticks)
- Winner's-curse haircut: `p_win` *decreases* with opportunity richness
- Landing/fee accounting per path (spam: sig fee on every included tx incl. reverts, 50%
  priority-fee burn; Jito: tip only on won bundles)
- Optimistic + conservative bounds reported together
- Paper ledger: portfolio, positions, realized/unrealized PnL, per-opportunity attribution

**Plans:**
- [ ] 05-01: Simulation model (EV decomposition, latency re-check, winner's-curse haircut)
- [ ] 05-02: Paper ledger + PnL attribution + dual-bound reporting

### Phase 6: Testing, Reconciliation & Calibration

**Goal:** Prove the engine tells the truth. Golden-file determinism, fault injection, and
calibration against on-chain macro anchors + competitor-landed arbs.
**Depends on:** Phase 5 (end-to-end paper PnL)
**Research:** Likely — calibrating `p_win` / tip-to-win curves needs fresh on-chain data
(no published constants; must be empirically derived).
**Research topics:** competitor-landed arb dataset (Jito arb explorer / Dune); current
macro anchors (avg winner $, failure rate, tip-as-%-of-MEV); Deflated-Sharpe exact formula.

**Scope:**
- Golden-file replay suite (captured stream → identical output baseline)
- Deterministic Simulation Testing: fault injection (disconnects, reordered/late slots,
  stale pool state); invariant assertions (portfolio never negative; value conserved
  modulo modeled fees)
- Reconcile modeled output vs on-chain ground truth; drift metric + alerting
- Calibrate to macro anchors (~96% fail, ~$1.58 avg winner, 20-50% tip) before trusting
  micro output
- Overfitting defense: trial logging, Deflated Sharpe / PBO, purged walk-forward CV,
  held-out window

**Plans:**
- [ ] 06-01: Golden-file replay + DST fault injection + invariants
- [ ] 06-02: On-chain reconciliation, calibration to macro anchors, overfitting metrics

### Phase 7: Observability & Hardening

**Goal:** Make the engine operable and trustworthy over long runs — dashboards,
config-driven params, multi-pool/multi-DEX scale-up. (Ships v1.0.)
**Depends on:** Phase 6 (validated, calibrated engine)
**Research:** Unlikely (internal patterns)

**Scope:**
- Metrics dashboards: opps/sec, detection latency, hit rate, drift, paper PnL
- Config-driven strategy params (no recompile to retune)
- Scale to multiple pools / multiple DEXs
- v1.0 release: documentation + reproducible run over a captured window

**Plans:**
- [ ] 07-01: Metrics/observability + config-driven params
- [ ] 07-02: Multi-pool scale-up + v1.0 release docs

---
*Roadmap created: 2026-06-17*
*Last updated: 2026-06-17 (Phase 1 complete)*
