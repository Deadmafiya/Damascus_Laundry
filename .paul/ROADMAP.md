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
**Current Milestone**
- **v1.0 Accurate Paper-Trading Engine** (v1.0.0)
- Status: In progress
- Phases: 5 of 7 complete

## Phases

**Phase Numbering:**
- Integer phases (1, 2, 3): planned milestone work
- Decimal phases (2.1): urgent insertions (marked [INSERTED])

Phases execute in numeric order: 1 → 2 → 3 → 4 → 5 → 6 → 7

| Phase | Name | Plans | Status | Completed |
|-------|------|-------|--------|-----------|
| 1 | Foundations | 1 | ✅ Complete | 2026-06-17 |
| 2 | Ingestion + Pool State (read-only) | 2 | ✅ Complete | 2026-06-18 |
| 3 | Opportunity Detection | 1 | ✅ Complete | 2026-06-18 |
| 4 | Profit / Cost / Sizing | 1 | ✅ Complete | 2026-06-18 |
| 5 | Simulation Core + Paper Ledger | 2 | Complete (2/2) | - |
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

**Status:** ✅ Complete (2026-06-18) — 2/2 plans
**Goal:** Subscribe to live Solana state for target DEX pools, decode it into normalized
in-memory pool state, and record the raw feed to disk so it can be replayed
deterministically.
**Depends on:** Phase 1 (`Feed` trait, fixed-point, deterministic harness)
**Research:** ✅ Complete — Raydium AMM v4 program ID + 752-byte AmmInfo layout verified
against `raydium-io/raydium-amm` master. Orca Whirlpool deferred to Phase 3 (or skip for v1.0).

**Scope:**
- Standard JSON-RPC WebSocket feed (`accountSubscribe`) behind the `Feed` trait ✅
- Decoders for the first AMM type (constant-product) → normalized pool state ✅ (Raydium AMM v4)
- A "fresh quote" venue (order book) leg for arb pairing — deferred to Phase 3
- Raw feed capture-to-disk + deterministic replay source ✅

**Plans:**
- [x] 02-01: WebSocket `Feed` impl + raw capture/replay
- [x] 02-02: Pool decoders (constant-product AMM) → normalized state, validated vs SDK/Jupiter

### Phase 3: Opportunity Detection

**Status:** ✅ Complete (2026-06-18) — 1/1 plan
**Goal:** From in-memory state, flag atomic-arbitrage opportunities and recover the trade
path; prove it by flagging known historical arb windows on replayed data.
**Depends on:** Phase 2 (normalized pool state + replay)
**Research:** Unlikely (well-documented algorithm; DeFiPoser-ARB)

**Scope:**
- Price graph: tokens = nodes, pools = edges weighted by −log(effective rate)
- Bellman-Ford / Moore negative-cycle detection + cycle-path recovery
- Handle Bellman-Ford gotchas (returns first cycle, reports % not absolute)

**Plans:**
- [x] 03-01: Price graph builder + negative-cycle detection + path recovery
  - **Implementation note:** ended up using **DFS over the full graph** (not Bellman-Ford pred-chain) because BF's "best predecessor" tracking loses cycles in graphs with mixed positive/negative edges. The 3-leg triangle test (2 loss legs + 1 profit leg) was the key case that exposed this. Documented in `graph.rs` and `03-SUMMARY.md`.

### Phase 4: Profit / Cost / Sizing

**Status:** ✅ Complete (2026-06-18) — 1/1 plan
**Goal:** Turn a detected cycle into an honest per-opportunity net-profit estimate —
AMM-curve-accurate fills, optimal sizing, full cost netting.
**Depends on:** Phase 3 (detected cycles)
**Research:** Unlikely (AMM math known; verify exact fee tiers/curves per pool)

**Scope:**
- AMM-aware output math (fill each leg against real reserves/curve, sequential
  state mutation across legs — never mid-price); CLMM tick-walking where applicable ✅
- Optimal-input sizing against the convex slippage curve (marginal revenue = 0) ✅
  — **Implementation note:** ended up using **golden-section search** with inverse
  golden ratio `1/φ = 0.618` (NOT `φ = 1.618`, which overflows `u128` for `span < 1.6× boundary`).
  The `gross_output(input)` curve is concave-down (constant-product slippage is monotone
  diminishing-returns), so the sum `gross − input − cost` is unimodal on any closed
  interval. 64 iters × ~1 µs per fill = ~64 µs/cycle. The closed-form analytical inverse
  was rejected as fragile to per-leg fee differences.
- Cost netting: base sig fee, priority fee (`CU_limit × CU_price/1e6`), Jito tip + 5% fee ✅
- Optional: `simulateTransaction` integration as the exact gross-edge oracle — deferred
  to Phase 6 (calibration)

**Plans:**
- [x] 04-01: AMM fill math + optimal sizing + cost model + NetProfit boundary object
  - **Implementation note:** `Cycle`/`Leg`/`Direction` types relocated from `dl-detect::cycle`
    to `dl-state::cycle` to break the dl-detect ↔ dl-sim cyclic dep. `dl-detect::cycle`
    re-exports the types for backward compatibility. The new `simulate_through_pools` is a
    free function (not an `impl Cycle` block) — Rust's orphan rule forbids inherent impls
    on types defined in another crate.
  - **Cost baseline:** `default_busy` (1M-lamport tip, 6 sigs, 600k CU, 50k µlamports/CU)
    totals **1,110,000 lamports**. The plan had 1,080,000; off-by-1e3 in the priority-fee
    math, corrected in commit `f7451d6`. `default_min` (10k tip) totals 26,200 lamports.
  - **Float-free invariant:** new CI guard `crates/dl-sim/tests/fixed_point_no_fractional.rs`
    scans `dl-sim/src/` for `f32`/`f64`/`float` substrings, fails on any match. Section
    headers renamed "Float-free invariant" → "Integer-only invariant" to avoid the
    substring self-match.

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
- [x] 05-01: Simulation model (EV decomposition, latency re-check, winner's-curse haircut) — ✅ 2026-06-18
- [x] 05-02: Paper ledger (DLD-LDG1 format, FNV-1a 64 cycle hash, append-only bincode frames, decision gate) — ✅ 2026-06-18

**Decisions (planning):**
- 05-01 models latency as a `p_land` haircut; real state-advance re-score deferred to Phase 6 (needs pool-bearing captures).
- 05-02 ledger = append-only length-prefixed file (schema v2, mirrors Phase 2 capture); no new dependency.

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
- [x] 06-01: Golden-file replay + DST fault injection + invariants
- [x] 06-02: On-chain reconciliation, calibration to macro anchors, overfitting metrics
- [x] **07-01 PLANNED** (`.paul/phases/07-observability-and-hardening/07-01-PLAN.md`):
- [x] 07-02: Multi-pool scale-up + v1.0 release docs

**Goal:** Make the engine operable and trustworthy over long runs — dashboards,
config-driven params, multi-pool/multi-DEX scale-up. (Ships v1.0.)
**Depends on:** Phase 6 (validated, calibrated engine)
**Research:** Likely — multi-DEX math needs verification against Orca/Meteora SDKs

**Scope:**
- Metrics dashboards: opps/sec, detection latency, hit rate, drift, paper PnL
- Config-driven strategy params (no recompile to retune)
- Scale to multiple pools / multiple DEXs (Orca Whirlpool + Meteora DLMM)
- v1.0 release: documentation + reproducible run over a captured window

**Plans:**
- [x] **07-01 PLANNED** (`.paul/phases/07-observability-and-hardening/07-01-PLAN.md`):
  Metrics/observability (`MetricsSink` trait, `tracing` adapter) +
  config-driven params (`EngineConfig` TOML loader) +
  closes `DL_LEDGER_PATH` deferral from 05-02 +
  per-cycle tip in ledger (closes 3 of 6 placeholder
  `engine_aggregate()` mappings) +
  `LedgerSummary` gains `median` / `p95`.
- [x] **07-02 PLANNED** (`.paul/phases/07-observability-and-hardening/07-02-PLAN.md`):
  Orca Whirlpool + Meteora DLMM decoders +
  Prometheus / OTel metrics adapter +
  `reproduce_paper_pnl.sh` script +
  `docs/v1.0.md` + `CHANGELOG.md` + `v1.0.0` tag.
  **Research gate**: `.paul/research/multi-dex-math.md` must
  be committed before 07-02 starts; the doc covers Orca and
  Meteora SDK source links and the Prometheus-vs-OTel choice.

---
*Roadmap created: 2026-06-17*
*Last updated: 2026-06-18 (Phase 5 COMPLETE — 05-01 EV core + 05-02 paper ledger; new dl-ledger crate (DLD-LDG1 schema v2, 7 src files, 3 test files, 41 new tests); 211 tests pass; 5 float-free guards; dl-app wiring deferred to phase 6/7)*
