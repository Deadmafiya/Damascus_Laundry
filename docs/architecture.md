# Architecture

This document describes the runtime architecture for someone modifying
the codebase. Read `README.md` first for orientation.

## Data flow

```
   ┌──────────────────────────────────────────────────────────────────────┐
   │                       Solana mainnet (WebSocket)                       │
   └──────────────────────────────────────────────────────────────────────┘
                                │
                                │  AccountUpdate events
                                │  (program-subscribe + account-subscribe)
                                ▼
   ┌──────────────────────────────────────────────────────────────────────┐
   │  dl-feed::ws_feed::WsFeed                                             │
   │    - subscribes to AMM program IDs (Raydium/Orca/Meteora)             │
   │    - subscribes to specific vault accounts (when mainnet allows)     │
   │    - emits FeedEvent::AccountUpdate { pubkey, data }                  │
   └──────────────────────────────────────────────────────────────────────┘
                                │
                                │  Feed trait (sync next_event / async recv)
                                ▼
   ┌──────────────────────────────────────────────────────────────────────┐
   │  dl-app::run_live_paper  (or  run_dry_run  for batch replay)          │
   │                                                                       │
   │  1. decode_amm_info / decode_whirlpool / decode_lb_pair              │
   │     → Pool { address, mints, decimals, reserves, fee_bps }          │
   │                                                                       │
   │  2. if reserves are 0 (AmmInfo-only):                                 │
   │     subscribe to base_vault and quote_vault,                         │
   │     update reserves when SplTokenAccount events arrive               │
   │                                                                       │
   │  3. detector.on_pool_update(&pool) → Vec<Cycle>                      │
   └──────────────────────────────────────────────────────────────────────┘
                                │
                                │  Vec<Cycle>  (each cycle has Vec<Leg>)
                                ▼
   ┌──────────────────────────────────────────────────────────────────────┐
   │  dl-stream::detector::StreamingDetector                              │
   │    - holds pool_to_edges index                                        │
   │    - on new pool: graph.add_pool()                                    │
   │    - on update: graph.update_pool() recomputes 2 affected edges      │
   │    - detect() runs dl_detect::bellman_ford::find_negative_cycles      │
   └──────────────────────────────────────────────────────────────────────┘
                                │
                                │  Vec<Cycle>
                                ▼
   ┌──────────────────────────────────────────────────────────────────────┐
   │  dl_sim::ev::evaluate                                                │
   │    inputs: NetProfit, EvalParams                                     │
   │    - conservative_default(): 30% win rate + 10bp decay                │
   │    - optimistic():              100% win, no decay                    │
   │    output: EvalOutcome { optimistic, conservative }                  │
   │           + Decision (WouldTrade | WouldNotTrade)                    │
   └──────────────────────────────────────────────────────────────────────┘
                                │
                                │  WouldTrade cycles
                                ▼
   ┌──────────────────────────────────────────────────────────────────────┐
   │  dl_paper::PaperWallet                                               │
   │    - JSON-backed (wallet.json)                                       │
   │    - atomic write via tempfile + rename                              │
   │    - per-bundle fill execution                                       │
   │    - also writes wallet.cycles.jsonl for ArbiNexus bridge            │
   └──────────────────────────────────────────────────────────────────────┘
```

## Crate responsibilities

| Crate | Layer | Public API used by `dl-app` |
|-------|-------|----------------------------|
| `dl-core` | types & math | `Clock`, `Rng`, `Feed`, `FeedEvent`, `Prob` |
| `dl-state` | pool/cycle domain | `Pool`, `Cycle`, `AmmKind`, `Pubkey`, decoder functions |
| `dl-feed` | I/O abstraction | `WsFeed::connect`, `Feed::next_event`, `CaptureWriter` |
| `dl-detect` | graph algorithm | `find_negative_cycles`, `build_from_pools` |
| `dl-sim` | math | `fill_constant_product`, `simulate_cycle`, `evaluate`, `NetProfit`, `CostBreakdown`, `EvalParams` |
| `dl-ledger` | on-disk format | `LedgerWriter`, `LedgerEntry`, `LedgerSummary` |
| `dl-recon` | pipeline | `replay_capture_to_ledger`, `recon_report` |
| `dl-recon-overfit` | stats | `deflated_sharpe`, `probability_of_backtest_overfitting` |
| `dl-signer` | key custody | `KeyStore::load`, `CapState::try_charge` (paper mode: unused) |
| `dl-executor` | bundle assembly | `JupiterClient::quote`, `JitoClient::submit_bundle` (paper mode: unused) |
| `dl-stream` | live pipeline | `StreamingDetector::on_pool_update`, `LatencyHistogram` |
| `dl-paper` | ledger-of-trades | `PaperWallet::new`, `PaperWallet::execute`, `PaperWallet::save` |

## Bin: `dl-app`

The single binary entry point. Subcommands:

| Subcommand | Purpose |
|------------|---------|
| `dl-app run` | Live paper trading (`--feed capture` for offline replay, `--feed live` for mainnet WS) |
| `dl-app metrics prom` | Serve Prometheus metrics on a port |
| `dl-app dry-run` | Replay a captured `bincode` file through detection → ledger |
| `dl-app recon` | Run `dl-recon` pipeline and write report.json + PAPER_PNL_REPORT.md |
| `dl-app config print` | Show resolved `EngineConfig` |

## Tests as architecture documentation

Each crate's test directory doubles as worked examples. Read these first:

- `crates/dl-detect/tests/` — graph + cycle math
- `crates/dl-sim/tests/` — fill math per DEX
- `crates/dl-recon/tests/` — replay pipeline end-to-end
- `crates/dl-stream/tests/` — streaming detector
- `crates/dl-paper/tests/` — wallet lifecycle

## Float-free invariant

The `tests/no_floats.rs` files in each value-path crate enforce a grep gate on `f64`/`f32`. The whitelist is intentional and tracked:

| File | Why allowed |
|------|-------------|
| `dl-recon-overfit/src/*.rs` | DSR/PBO are inherently float-based; integer version exists but is less accurate |
| `dl-signer/src/ratelimit.rs` | Token-bucket time decay; integer version adds code for no behavior change |

Any new `f64` import elsewhere must be justified or removed.

## Where to start reading the code

1. `crates/dl-app/src/main.rs` — `run_live_paper` is the orchestrator (~150 lines of comments + logic).
2. `crates/dl-stream/src/detector.rs` — `StreamingDetector::on_pool_update` is the cycle-detection engine.
3. `crates/dl-detect/src/bellman_ford.rs` — pure algorithm.
4. `crates/dl-sim/src/ev.rs` — `evaluate` is the conservative-bound gate.
5. `crates/dl-feed/src/ws_feed.rs` — WebSocket lifecycle and channel plumbing.

Each file is short. Read top-to-bottom.
