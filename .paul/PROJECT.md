---
description: "Accurate, reliable paper-trading engine for Solana atomic DEX-DEX arbitrage MEV"
type: Project
about: "damascus_laundry"
---

# damascus_laundry

## What This Is

A Solana MEV paper-trading bot. **damascus_laundry_v1.0** ingests real-time Solana
market state, detects atomic DEX-DEX arbitrage opportunities, and **simulates**
whether each opportunity would have been profitable — without ever submitting a real
transaction. The whole point of v1.0 is a simulator so accurate it does not lie to us:
it must reproduce *losing* (the ~96% atomic-arb failure rate) as faithfully as it spots
gross opportunities.

The architecture is deliberately built so that going live later changes **only the
executor module** — everything else (ingestion, state, detection, profit/cost
estimation, ledger) is shared between paper and live.

## Core Value

**Reliable, honest profitability estimation.** A detected opportunity is only counted as
profit if it survives latency, competition, landing probability, and fees — modeled
pessimistically by default. Accuracy and precision over optimism. If the engine says a
strategy is +EV, we should be able to trust it before risking real capital.

## Current State

| Attribute | Value |
|-----------|-------|
| Type | Software (Rust trading-engine / simulator) |
| Version | 0.0.0 |
| Status | Phase 1 (Foundations) complete — building Phase 2 (Ingestion) |
| Last Updated | 2026-06-17 |

## Requirements

### Core Features / Deliverables

- **Real-time ingestion** of Solana DEX pool state (Phase 1: standard JSON-RPC
  WebSocket, free; behind a pluggable `Feed` trait so Yellowstone gRPC drops in later).
- **In-memory pool state** for target AMMs, normalized across token decimals.
- **Opportunity detection** — price graph + Bellman-Ford negative-cycle search across
  pools (DeFiPoser-ARB loop).
- **Profit/cost estimation** — AMM-curve-accurate fills (never mid-price), optimal trade
  sizing against the slippage curve, full cost netting (base fee, priority fee, Jito tip
  incl. 5% fee).
- **Accurate simulation core** — multiplicative EV decomposition
  `E[PnL] = p_detect × p_win × p_land × (gross − costs) − E[failed_costs]`, latency
  re-check at projected landing slot, winner's-curse haircut, optimistic + conservative
  bounds reported together.
- **Paper ledger** — paper portfolio, position tracking, realized/unrealized PnL,
  per-opportunity PnL attribution.
- **Deterministic replay** — capture feeds to disk; re-run identical detection/sizing
  code for golden-file parity (backtest = live = replay).
- **Metrics & observability** — opps/sec, detection latency, hit rate, drift
  (modeled-vs-on-chain), PnL.

### Validated (Settled) Decisions

See Key Decisions below.

### Validated (Shipped)

- ✓ **Deterministic-by-construction foundation** — `u128` fixed-point math (overflow-safe
  `mul_div` with 256-bit intermediate), injectable `Clock`/`Rng`/`Feed` traits, seed-reproducible
  replay harness. — Phase 1
- ✓ **Float-free value path** enforced (only an isolated `display` module uses floats). — Phase 1
- ✓ **CI + structured logging** (`tracing`, fmt/clippy/test on every push). — Phase 1

## Target Users

The project owner / operator (single-operator searcher). The bot is for authorized,
educational, defensive-research paper trading — evaluating strategy viability before any
live capital. Not a product for third parties in v1.0.

## Constraints

- **Paper trading only in v1.0** — no real transaction submission, no funded wallet,
  no private keys in the hot path. (Live execution is explicitly Phase 7+, out of v1.0.)
- **No sandwiching, ever** — extractive, gated behind opaque private mempools, and being
  engineered out (Jito BAM TEEs, `jitodontfront`). Excluded from the strategy set.
- **No floating-point in any value/balance/PnL path** — `u128` fixed-point base units,
  overflow-checked, explicit scale tracking. Floats only for display.
- **All nondeterministic dependencies must be injectable** (Clock, RNG, Feed) from day
  one — deterministic simulation testing is impractical to retrofit.
- **Abstract `read-state` and `submit` layers** — the block-building stack is in flux
  (mempool removal 2024 → Jito BAM 2025 → Firedancer 2025-26); do not hard-code one
  client's ordering/timing behavior.
- **Budget-conscious v1** — start on free RPC; no paid data feed required to validate the
  engine.
- **Treat every dollar/percentage figure from research as point-in-time** — re-pull from
  live on-chain data before using in P&L math.

## Key Decisions

| Decision | Rationale | Date |
|----------|-----------|------|
| Language: **Rust** | Live-trading endgame; only first-class crates (`yellowstone-grpc-client` v13.x, `jito-bundle`); preserves backtest-live parity → no rewrite when going live. | 2026-06-17 |
| Feed: **free JSON-RPC WS first, gRPC-ready** | Validate the engine at $0; pluggable `Feed` trait lets Yellowstone gRPC drop in for accuracy/latency later. | 2026-06-17 |
| Strategy: **atomic DEX-DEX arbitrage only** | Single clean strategy, fully reconstructable from on-chain state, atomic (no inventory risk); tightest scope to nail the accurate-simulation engine first. | 2026-06-17 |
| Primary method: **shadow/replay**, forward-paper as validation gate | MEV opportunities are fleeting + latency-bound and Solana has no mempool; "what you knew" = "what your node's stream saw, when". Kills look-ahead bias structurally. | 2026-06-17 |
| **Model losing first** (~96% fail) | A sim that doesn't reproduce the failure rate is optimistic by construction and worthless. | 2026-06-17 |
| `simulateTransaction` as **gross-edge oracle**, haircut on top | Gives exact fills vs real state but assumes you're alone; competition/landing/latency are separate pessimistic multipliers. | 2026-06-17 |
| Excluded from v1: **CEX-DEX, new-pool sniping, JIT, sandwiching** | Depend on off-chain fills / extreme latency / private mempools that can't be honestly paper-simulated at first (sandwiching also excluded permanently). | 2026-06-17 |
| **Use `jito-foundation/jito-solana` as Jito-mechanics reference** | It is Jito's MEV fork of the Agave validator (Rust). v1 reads it as the authoritative spec for bundle/tip/relayer/auction behavior the sim core must model; we do not compile/run a validator for paper trading. Reserved as the live-node for the deferred Phase 7+ live/ShredStream path. | 2026-06-17 |

## Success Metrics

- **Simulation honesty:** aggregate simulated outcomes reproduce the macro anchors —
  attempt-failure rate in the ~96% neighborhood, average *winner* near the single-dollar
  order of magnitude (not orders larger), tips at 20-50% of available MEV. If the sim's
  average winner is much larger, fills/competition/survivorship are mis-modeled.
- **Determinism:** a captured feed stream replays bit-identically (golden-file parity);
  integer math is bit-reproducible across runs/machines.
- **Calibration:** for arbitrages competitors landed on-chain, the engine reproduces the
  winner's realized profit (fill model) and would have detected the opportunity in time
  (p_detect), within tight tolerance.
- **Dual bounds:** every strategy result reported with both optimistic and conservative
  EV; we act only on the conservative bound. The gap is tracked as a risk signal.
- **Overfitting defense:** every parameter trial logged; Deflated-Sharpe / PBO reported,
  not just best Sharpe; final untouched out-of-sample window held out; conclusions require
  thousands of detected *opportunities*, not dozens of wins.

## Tech Stack / Tools

| Layer | Technology | Notes |
|-------|------------|-------|
| Language | Rust (edition 2021+, `jito-bundle` later needs 1.85+/2024) | No GC; fixed-point; live-path-ready |
| Ingestion (v1) | Standard JSON-RPC WebSocket (`accountSubscribe`/`logsSubscribe`) | Free; behind pluggable `Feed` trait |
| Ingestion (later) | Yellowstone gRPC (`yellowstone-grpc-client` v13.x) — Helius LaserStream / Triton | Production feed; deferred |
| Decode | `anchor`/IDL structs; cross-check vs Jupiter `/order` + native DEX SDKs | Local decoders for target pools |
| Sim oracle | RPC `simulateTransaction` (`replaceRecentBlockhash`, `accounts`, `unitsConsumed`) | Exact gross fills; upper bound |
| Jito reference | `jito-foundation/jito-solana` (Jito's MEV fork of the Agave validator; Rust, Apache-2.0) | **Source-of-truth** for bundle/tip/relayer/auction mechanics the sim core models; also the node we'd run for the deferred live/ShredStream path |
| Math | `u128` fixed-point base units, overflow-checked, explicit scale | No floats in value path |
| Detection | Price graph + Bellman-Ford negative-cycle (DeFiPoser-ARB) | Slippage-aware sizing |
| Architecture | Event-driven, modular (NautilusTrader-style Clock/Cache/MessageBus/Portfolio) | All nondeterministic deps injected |
| Persistence/metrics | Local time-series + structured append-only event log (SQLite ok for v1) | Redis-backed state only if needed |
| Live add-ons (deferred) | `jito-bundle` (≤5 tx, tip-last), ShredStream | Phase 7+, out of v1.0 |
## Reference Research

Deep-research findings (compiled 2026-06-17, sourced + confidence-tagged) live in:
- `.paul/research/solana-mev-landscape.md` — domain map (no-mempool, Jito/BAM/Firedancer, strategies, DEXs, risks)
- `.paul/research/solana-mev-data-stack-research.md` — ingestion, DEX decoding math, simulateTransaction, fees, SDKs
- `.paul/research/solana-mev-paper-trading-research.md` — accurate-simulation principles (the 11 principles), metrics, overfitting
- `.paul/research/solana-mev-paper-bot-research.md` — architecture, language, repos, precision, phased build order

**Key external repo:** [`jito-foundation/jito-solana`](https://github.com/jito-foundation/jito-solana)
— Jito's MEV fork of the Agave Solana validator (Rust, Apache-2.0). Used in v1.0 as the
authoritative reference for Jito bundle/tip/relayer/auction mechanics that the simulation
core models (Phase 5); the eventual live/ShredStream path (Phase 7+) would run this client.
Build docs: https://jito-foundation.gitbook.io/mev/jito-solana/building-the-software

---
*PROJECT.md — Updated when requirements or context change*
*Last updated: 2026-06-17 after Phase 1*
