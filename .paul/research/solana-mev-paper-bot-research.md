# Solana MEV / Arbitrage Bot — Architecture & Stack Research

**Scope:** Best practices for a high-performance Solana MEV/arbitrage bot, emphasizing a **paper-trading (simulation) variant first**, evolving toward live trading later.
**Date compiled:** 2026-06-17. Sources current to 2025–2026.
**Confidence legend:** 🟢 well-supported by multiple/primary sources · 🟡 reasonable inference or single source · 🔴 opinion / unverified — treat with caution.

> **Methodological caveat (read first):** Many of the "infrastructure-is-everything" claims below come from **RPC/infrastructure vendors** (RPC Fast, Dysnix, Chainstack, QuickNode, Helius) whose business is selling exactly that infrastructure. Their latency framing is technically plausible but commercially motivated; I flag this throughout. Crate versions/dates are from crates.io and are verifiable. I could **not** independently verify profit statistics, latency numbers, or GitHub star counts (the search layer did not surface them); those are reported as claims, not facts.

---

## 1. Language choice — Rust vs TypeScript vs Python vs Go

### What the evidence says
- The field has effectively settled on a **prototype-vs-competitive split**: **TypeScript** (`@solana/web3.js`, `jito-ts`) for prototypes and non-top-tier strategies; **Rust** (`solana-sdk` + a Jito searcher/bundle crate) for sub-slot, latency-racing strategies. 🟢
- The stated reason Rust wins at the top tier is **not raw compute** but **GC pause elimination**: V8 garbage-collection pauses introduce ~10–20 ms jitter that you "can't tune away." For landing-vs-missing races measured in single-digit/low-double-digit milliseconds, that jitter is decisive. 🟡 (plausible physics; specific ms figures are vendor-sourced and unverified)
- **Python / Go** barely appear in serious-searcher discussion. Yellowstone gRPC is reachable from any gRPC language (Python, Go, JS, Rust), so Python/Go are viable for *detection/analytics*, but neither shows up as a competitive *execution* language. Go is a reasonable middle option for concurrency without GC-as-showstopper, but lacks the first-class Solana/Jito crate ecosystem Rust has. 🟡
- A common production pattern: **TS for strategy/orchestration logic + Rust backends** for the hot path. 🟢

### Ecosystem maturity (verifiable)
- `yellowstone-grpc-client` (Triton One): **actively maintained**, latest **v13.1.0** (~2 mo before compile date), Apache-2.0, ~940k all-time downloads, 48 versions. Companion `yellowstone-grpc-proto`. This is the de-facto low-latency Solana streaming client and the strongest argument for Rust. 🟢
- Jito Rust crates: `jito-sdk-rust` (JSON-RPC SDK for bundles/tx via Block Engine, `getInflightBundleStatuses` etc.); **`jito-bundle`** is newer/actively maintained (Feb 2026, requires Rust 1.85+/edition 2024) and enforces protocol limits (max **5 tx/bundle**, tip = last instruction of final tx). The original **`jito-searcher-client` is effectively abandoned** (v0.0.1, ~Apr 2023, flagged "not recommended" on lib.rs) — do not start there. 🟢

### **For a PAPER-TRADING v1, how much does language matter?**
**Very little for v1, and this is the key planning insight.** 🟡→🟢
- Paper trading **simulates** execution latency rather than racing it. The entire reason Rust dominates — GC jitter on the submission hot path — **does not apply** when there is no real submission. You can *model* latency as a configurable parameter.
- What *does* still matter in v1: ingestion throughput (handling a full Yellowstone gRPC firehose) and detection-loop speed (graph rebuild + cycle search per slot). These are demanding but achievable in any of the four languages for a single-strategy MVP.
- **Recommendation:** Pick the language you'll move to for live trading, to preserve **backtest-live code parity** (see §5). Since the live endgame is Rust, and the best streaming/bundle crates are Rust, **building the paper bot in Rust now avoids a costly rewrite later** — but if developer velocity is the binding constraint for v1, TypeScript or Python is a defensible prototype choice you accept rewriting.

---

## 2. Reference architecture: market-data → detection → (simulated) execution

The dominant pattern across mature trading engines is **event-driven, modular** (NautilusTrader, QSTrader, QTPyLib/Kinetick). 🟢 Map it to Solana MEV:

```
┌─────────────┐   ┌──────────────┐   ┌──────────────┐   ┌──────────────┐   ┌──────────────┐
│ Ingestion   │──▶│ State Mgmt   │──▶│ Opportunity  │──▶│ Profit/Cost  │──▶│ Simulated    │
│ (gRPC feed) │   │ (pool state) │   │ Detection    │   │ Estimation   │   │ Execution    │
└─────────────┘   └──────────────┘   └──────────────┘   └──────────────┘   └──────┬───────┘
                                                                                   │
                          ┌────────────────────────────────────────────────────────┘
                          ▼
                  ┌──────────────┐   ┌──────────────────────────┐
                  │ Paper Ledger │──▶│ Persistence / Metrics /   │
                  │ (PnL, posns) │   │ Observability             │
                  └──────────────┘   └──────────────────────────┘
```

**Ingestion layer.** Push-based **Yellowstone gRPC** (account/program state streams), not polling websockets. Vendor-claimed latency ~1–5 ms (gRPC) vs ~50–200 ms (WS) 🟡(vendor). For live you'd later add **ShredStream** (pre-confirmation shred access, claimed ~50–200 ms detection edge 🟡vendor). Use `fromSlot` replay to recover events missed during brief disconnects (limited replay buffer). 🟢 For paper v1, a single Yellowstone subscription filtered to your target DEX programs is enough.

**State management.** In-memory, type-safe pool/account state keyed by pool address; maintain reserves/liquidity per AMM and reconstruct the price graph from current state. NautilusTrader's model — `Clock`, `Cache`, `MessageBus`, `Portfolio`, `Actors`, optional Redis-backed persistence — is a strong reference for thread-safe state. 🟢 Decouple capture from strategy (QTPyLib/Kinetick "Blotter + ZeroMQ pub/sub") so you can record feeds even when strategies are off. 🟢

**Opportunity detection.** Model tokens as graph nodes, pool exchange rates as edges weighted by **−log(effective rate)**; a **negative cycle = product of rates > 1 = profitable loop** (Bellman-Ford/Moore). 🟢 Complexity O(EV); currency graphs are dense (E≈O(V²)). The DeFiPoser-ARB loop is the canonical recipe: (i) build graph from spot prices, (ii) detect cycle, (iii) build path & size trade, (iv) execute & update state. 🟢

**Profit/cost estimation.** Spot prices only hold for infinitesimal trades; AMM `x·y=k` makes edge weights **slippage-dependent**. Size the trade by **increasing input until marginal revenue stops rising** (optimize against the convex slippage curve), not the linear spot estimate. 🟢 Subtract DEX fees (relative, e.g. 0.3%) **and** absolute costs (priority fees, Jito tips). Known Bellman-Ford gotchas: it returns the **first** cycle found (a $10k and a $0.10 arb are equally likely to be returned), and reports **percentage** not absolute profit — so absolute gas/tip accounting must be layered on top. 🟢

**Simulated execution & ledger (paper).** Apply the chosen opportunity to a **paper portfolio**: deduct simulated input, credit modeled output (post-slippage, post-fee), record position deltas and realized/unrealized PnL. Model an **execution-latency parameter** and an **inclusion-probability/landing model** so paper results aren't naively optimistic (you didn't actually win the race). 🟡

**Persistence, metrics, observability.** Append-only event log of feed → detection → decision → fill; time-series metrics (opportunities/sec, detection latency, paper PnL, hit rate). NautilusTrader uses optional Redis-backed state persistence; for v1 a local time-series/SQLite + structured logs suffice. 🟢

---

## 3. Open-source repos worth studying — with strong caveats

⚠️ **Critical warning:** The Solana-arb GitHub landscape is **heavily polluted with low-quality clones, star-farming, and likely malware**. A cluster of near-identical repos (`OnlyForward0613`, `senior106`, `ChangeYourself0613`, `kelvin-1013`) share copy-pasted READMEs, stub code (`// Implementation`-only Anchor functions), Telegram contact links, and "give me stars" messaging — classic scam/wallet-drainer signals. **Never run any downloaded crypto bot with a funded wallet or real private key without a full line-by-line audit; use devnet and throwaway keys.** 🟢

| Repo | What it claims | Lesson / caveat |
|---|---|---|
| **nautilus_trader** — `github.com/nautechsystems/nautilus_trader` | Production-grade **Rust-native** event-driven engine; Python API; nanosecond backtests; backtest-live parity; `high-precision` 128-bit feature flag; `defi` feature | **Best architectural reference here.** Not Solana-MEV-specific, but the gold standard for deterministic, event-driven, backtest=live design. LGPL-3.0, maintained by Nautech Systems. 🟢 |
| **ARBProtocol/ARB-V2** — `github.com/ARBProtocol/ARB-V2` | Monitors Meteora/Raydium/Orca; on-chain "HARBR" program fails the tx if it would be a loss; Rust; local/VPS | Most legitimate-looking arb repo found; the "fail-on-loss guard program" is a useful safety pattern. Verify code/license/commit history yourself. 🟡 |
| **AV1080p/Solana-Arbitrage-Bot** | 7+ DEXes; **Yellowstone gRPC** ingestion; optional Jito bundling; configurable thresholds | Architecturally illustrative of the ingestion+Jito pattern; provenance unverified — audit before trusting. 🔴 |
| `OnlyForward0613` / `senior106` / `ChangeYourself0613` / `kelvin-1013` clones | "Cross-DEX Raydium/Orca/Meteora/Jupiter, Jito-MEV" | **Avoid as code.** One README is candidly useful on *constraints* (front-running, CU/tx-size limits, off-chain detection, MEV-aware RPC) — read for concepts only. 🔴 |
| `awesome-systematic-trading` — `github.com/paperswithbacktest/awesome-systematic-trading` | Curated list of systematic-trading libs/strategies | Good jumping-off index for engines/patterns (not Solana-specific). 🟢 |

Other non-Solana reference engines worth borrowing patterns from: **QSTrader** (signal generation decoupled from portfolio/risk/execution/accounting), **Zipline** (event-driven backtester), **Freqtrade** (crypto bot, Python), **vectorbt** (vectorized discovery). 🟢

> **Market context (claims, unverified):** "In 2025, arbitrage ≈ 50% of Solana DEX volume; 90M+ successful arb txns via Jito's detection generating $142.8M profit." Sourced from a vendor blog — directionally consistent with Solana arb being dominated by professional searchers, but **treat exact figures as unverified.** 🔴

---

## 4. Performance & precision engineering

🟢 **Use fixed-point integer math, never floats, for any value affecting balances/PnL.**
- Binary floats can't exactly represent decimals (0.1, etc.) and have **magnitude-relative precision** — large token balances lose low-order digits. This is precisely why DeFi represents amounts as **large integers in the token's smallest unit** (the `decimals` field is the fixed scale, like ERC-20's 18).
- **Track scale explicitly through operations.** Multiplication sums the fractional digit counts; products of large `u64` token amounts × high-precision prices **overflow 64 bits**, which is the core reason to compute in **`u128`** (or wider). NautilusTrader's optional `high-precision` mode = 128-bit value types — same principle. 🟢
- **Decimals normalization:** normalize every token to a common internal scale on ingestion; carry the per-token `decimals` in pool state and convert at the boundary. Only convert to float for **display**, never for order/balance math. 🟢
- **Deterministic replay** depends on this: integer math is bit-reproducible across runs/machines; float accumulation order is not. Fixed-point is a prerequisite for the golden-file replay in §5. 🟡

Practical rule for the engine: amounts as `u128` base units; prices as fixed-point (e.g. a Q-format or scaled integer); overflow-checked `mul`/`div` helpers with explicit scale tracking.

---

## 5. Testing, accuracy & reliability

🟢 **Deterministic Simulation Testing (DST)** is the rigorous frame (FoundationDB/TigerBeetle lineage; explicitly cited as excelling at *financial transaction engines*):
1. **Make all nondeterministic components pluggable** — clock, RNG, network, and *feed source* injected as interfaces. (Build this in from day one; retrofitting onto a live system is "generally impractical.")
2. **Control entropy with a seed** so runs are random-appearing yet perfectly reproducible.
3. **Explore the state space** with fault injection (disconnects, reordered/late slots, stale pool state).
4. **Assert invariants** (e.g. paper portfolio never goes negative; ledger conserves value modulo modeled fees).

🟢 **Golden-file replay (backtest-live parity):** capture real Yellowstone feed streams, re-run them through the same detection+sizing code path, assert output matches a known-good baseline — directly verifies determinism. Use the trading distinction: **vectorized fast runs for discovery, event-driven tick-accurate replay for validation.**

🟡 **Property-based testing** (Hypothesis/QuickCheck/proptest): generate seeded random pool states and assert invariants (no-arb graph never yields a cycle; AMM output math is monotonic in input; round-tripping decimals is lossless). Evidence base here was thinner than for DST/replay — treat as recommended-by-analogy.

🟢 **Reconcile paper vs on-chain ground truth & monitor drift:** periodically compare your modeled output for an opportunity against what the chain actually did that slot. Be aware of the backtest pitfalls that wreck PnL realism — **lookahead bias, data leakage, and optimistic slippage**; live Sharpe often falls *far* below backtested. For thin pools, slippage/spread move fast. Track a drift metric (modeled-vs-actual fill error) and alert when it grows. Discipline: **data versioning, parameter provenance, deterministic builds, seeded Monte Carlo.**

---

## 6. Recommended stack & phased build order

### Recommended stack (planning recommendation — opinion grounded in evidence) 🟡
- **Language:** **Rust** for the engine. Rationale: it's the live-trading endgame, has the only first-class crates (`yellowstone-grpc-client`, `jito-bundle`/`jito-sdk-rust`), and choosing it now preserves backtest-live parity and avoids a rewrite. *Accept TS/Python only if v1 velocity strictly outweighs the eventual rewrite cost.*
- **Ingestion:** `yellowstone-grpc-client` (v13.x), filtered subscriptions to target DEX programs; `fromSlot` for gap recovery.
- **Math:** `u128` fixed-point base units, overflow-checked, explicit scale tracking. No floats in the value path.
- **Architecture:** event-driven, modular, NautilusTrader-style (`Clock`/`Cache`/`MessageBus`/`Portfolio`/`Actors`), with **all nondeterministic deps injected** for DST.
- **Detection:** price graph + Bellman-Ford negative-cycle (DeFiPoser-ARB loop); AMM-aware trade sizing against the slippage curve.
- **Persistence/metrics:** local time-series + structured event log for v1; Redis-backed state if/when needed.
- **Live add-ons (deferred):** Jito bundles (≤5 tx, tip-last), ShredStream, redundant submission paths (Jito → Astralane/bloXroute fallback).

### Phased build order

**Phase 0 — Foundations.** Repo + CI; `u128` fixed-point math module with property tests; pluggable Clock/RNG/Feed interfaces (DST-ready from the start); structured logging.

**Phase 1 — Ingestion + state (read-only).** Yellowstone gRPC subscription to 2–3 DEX programs (start one AMM type, e.g. constant-product); decode pool accounts into normalized in-memory state; record raw feed to disk for replay. *Exit:* can replay a captured stream deterministically.

**Phase 2 — Detection.** Build price graph from state; Bellman-Ford negative-cycle detection; recover the cycle path. *Exit:* flags known historical arb windows on replayed data.

**Phase 3 — Profit/cost + sizing.** AMM-aware output math; optimal-input sizing against slippage curve; subtract relative fees + modeled absolute costs (priority fee/tip placeholder). *Exit:* per-opportunity net-profit estimate.

**Phase 4 — Paper execution & ledger.** Apply opportunities to a paper portfolio; latency + inclusion-probability models so results aren't naively optimistic; track positions and realized/unrealized PnL. *Exit:* end-to-end paper PnL over a replayed day.

**Phase 5 — Testing & reconciliation.** Golden-file replay suite; DST fault injection (disconnects/stale slots); reconcile modeled vs on-chain ground truth; drift monitoring + alerting.

**Phase 6 — Observability & hardening.** Metrics dashboards (opps/sec, detection latency, hit rate, drift, PnL); config-driven strategy params; multi-pool/multi-DEX scale-up.

**Phase 7 (later, live) — Execution path.** Swap the simulated executor for `jito-bundle` submission, ShredStream ingestion, redundant submission/failover, on-chain fail-on-loss guard (cf. ARB-V2 "HARBR"). Backtest-live parity means **only the executor module changes.**

---

## Sources
- [Solana Arbitrage Bot Setup Guide 2026 — RPC Fast](https://rpcfast.com/blog/solana-arbitrage-bot-setup) *(vendor)*
- [Solana arbitrage bot setup: why most fail — Daniel Yavorovych / Medium](https://yavorovych.medium.com/solana-arbitrage-bot-setup-why-most-fail-before-they-start-1c24d8d72593)
- [Solana trading infrastructure 2026 — Chainstack](https://chainstack.com/solana-trading-infrastructure-2026/) *(vendor)*
- [How to Build a Solana Arbitrage Bot in 2026 — Dysnix](https://dysnix.com/blog/solana-arbitrage-bot-setup) *(vendor)*
- [Best Solana RPC Providers for MEV in 2026 — Dysnix](https://dysnix.com/blog/solana-rpc-for-mev) *(vendor)*
- [MEV Protection on Solana in 2026 (Jito Bundles, Astralane) — DEV.to](https://dev.to/gerus_team/mev-protection-on-solana-in-2026-jito-bundles-astralane-and-what-actually-works-3gbc)
- [yellowstone-grpc — GitHub (Triton/rpcpool)](https://github.com/rpcpool/yellowstone-grpc)
- [yellowstone-grpc-client — crates.io](https://crates.io/crates/yellowstone-grpc-client)
- [Yellowstone gRPC Quickstart — Helius Docs](https://www.helius.dev/docs/grpc/quickstart) *(vendor)*
- [Making Yellowstone Geyser gRPC Requests with Rust — QuickNode Docs](https://www.quicknode.com/docs/solana/yellowstone-grpc/overview/rust) *(vendor)*
- [jito-sdk-rust — crates.io](https://crates.io/crates/jito-sdk-rust)
- [jito-bundle — crates.io](https://crates.io/crates/jito-bundle)
- [jito-searcher-client — crates.io](https://crates.io/crates/jito-searcher-client) / [lib.rs (not recommended)](https://lib.rs/crates/jito-searcher-client)
- [jito-client — crates.io](https://crates.io/crates/jito-client/0.1.0)
- [ARBProtocol/ARB-V2 — GitHub](https://github.com/ARBProtocol/ARB-V2)
- [AV1080p/Solana-Arbitrage-Bot — GitHub](https://github.com/AV1080p/Solana-Arbitrage-Bot)
- [nautilus_trader — GitHub](https://github.com/nautechsystems/nautilus_trader) / [nautilustrader.io](https://nautilustrader.io/) / [nautilus-trading — crates.io](https://crates.io/crates/nautilus-trading)
- [awesome-systematic-trading — GitHub](https://github.com/paperswithbacktest/awesome-systematic-trading/)
- [Cyclic Arbitrage in DEXs — EmergentMind](https://www.emergentmind.com/topics/cyclic-arbitrage-in-decentralized-exchanges-dexs)
- [Bellman-Ford in Cryptocurrency Arbitrage — Nilkumar Patel / Medium](https://medium.com/@23bt04107/bellman-ford-in-cryptocurrency-arbitrage-detecting-profitable-trade-cycles-2a6264a409b3)
- [Graph algorithms and currency arbitrage, part 2 — Reasonable Deviations](https://reasonabledeviations.com/2019/04/21/currency-arbitrage-graphs-2/)
- [Arbitrage using Bellman-Ford — The Algorists](https://www.thealgorists.com/Algo/ShortestPaths/Arbitrage)
- [Detect a negative cycle in a Graph (Bellman-Ford) — GeeksforGeeks](https://www.geeksforgeeks.org/dsa/detect-negative-cycle-graph-bellman-ford/)
- [On the Just-In-Time Discovery of Profit-Generating Transactions in DeFi (DeFiPoser) — arXiv:2103.02228](https://arxiv.org/pdf/2103.02228)
- [Triangular Arbitrage With Crypto DEXs, Part Two — Alex Ford / Coinmonks](https://medium.com/coinmonks/triangular-arbitrage-with-crypto-dexs-part-two-f6e6ff66fb87)
- [Fixed point vs Floating point — Microcontroller Tips](https://www.microcontrollertips.com/difference-between-fixed-and-floating-point/)
- [Float vs Double vs Fixed Point — Electronics-ed](https://www.electronics-ed.com/2026/04/float-vs-double-vs-fixed-point.html)
- [Deterministic simulation testing — Antithesis Docs](https://antithesis.com/docs/resources/deterministic_simulation_testing/)
- [Design an Automated Trading Platform — Ankit K. Srivastava / Medium](https://medium.com/@ankitviddya/design-an-automated-trading-platform-16e57a640310)
- [Architectural Design Patterns for HFT Algo Trading Bots — James Hall / Medium](https://medium.com/@halljames9963/architectural-design-patterns-for-high-frequency-algo-trading-bots-c84f5083d704)
- [Backtesting AI Crypto Strategies Safely — Blockchain Council](https://www.blockchain-council.org/cryptocurrency/backtesting-ai-crypto-trading-strategies-avoiding-overfitting-lookahead-bias-data-leakage/)
- [QTPyLib / pytrade docs](https://docs.pytrade.org/trading)
