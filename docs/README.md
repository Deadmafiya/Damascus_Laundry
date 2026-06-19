# Project overview

`damascus_laundry` is a Solana MEV **paper-trading engine** that:

1. Connects to live mainnet WebSocket RPC (`wss://api.mainnet-beta.solana.com` or a paid RPC).
2. Subscribes to AMM pool accounts (Raydium AMM v4, Orca Whirlpool, Meteora DLMM) and their SPL-token vault accounts.
3. Maintains an in-memory price graph where each pool is a node and each swap direction is an edge.
4. Runs Bellman-Ford cycle detection on every pool update.
5. For each detected negative-weight cycle, evaluates the conservative-bound EV (`EvalParams::conservative_default()`) and writes a paper trade to `wallet.json` only when `WouldTrade` is returned.
6. Bridges detected cycles to ArbiNexus (`vendor/arbinexus`) for oracle-gated paper-trade simulation with a 30% win rate.

It holds **no private keys in the value path**. Paper mode only. The `dl-signer` crate *can* derive keys from an encrypted keyfile for testing, but `dl-app run --feed live` never loads a keyfile.

The simulation is honest: it models latency, competition decay, tip cost, and Jito auction window pessimistically, so a high PnL in the simulator is more meaningful than a high gross-spread count.

## Current state

- **Latest tag**: `v1.1.7-realistic-mode` (commit `abc5747` on `main`).
- **Tests**: 441 passing.
- **Branch**: `main`.
- **Rust toolchain**: 1.94.1 (pinned in `rust-toolchain.toml`).

The v1.1 series added (in order): paper-mode executor + hot-wallet signer (`v1.1.0-executor`), streaming detector + latency benchmark (`v1.1.0-streaming`), LiveMode gate + `dl-signer` CLI (`v1.1.0`), live mainnet wire (`v1.1.2-mainnet-wire`), conservative-bound detector gate (`v1.1.3-detector-gate`), vault subscriptions + dynamic pool addition (`v1.1.4-mainnet-vaults`), optimistic paper mode (`v1.1.5-paper-mode-optimistic`), ArbiNexus bridge (`v1.1.6-arbinexus-bridge`), realistic 30% win rate (`v1.1.7-realistic-mode`).

## How the engine works (one paragraph)

A pool update arrives via WebSocket. The `dl-app` bin parses it into a `Pool` (mints, decimals, fee, reserves for Raydium from vault subscriptions). The `dl-stream::StreamingDetector` adds the pool to a `Graph` and runs `dl_detect::bellman_ford::find_negative_cycles`. Each cycle is evaluated by `dl_sim::ev::evaluate` with `EvalParams::optimistic()` (paper mode) or `conservative_default()` (realistic mode). If the decision is `WouldTrade`, the trade is appended to `wallet.json` and `wallet.cycles.jsonl`. The ArbiNexus bridge reads `wallet.cycles.jsonl`, applies its oracle confidence gate, and writes `wallet_paper.json` with a 30% win-rate model.

## Workspace layout

| Crate | Responsibility | Phase |
|-------|----------------|-------|
| `dl-core` | Fixed-point math (`u128`), injectable `Clock` / `Rng` / `Feed` traits, shared types | 1 |
| `dl-feed` | WebSocket + scripted feed; capture/replay; account & program subscribe | 2 |
| `dl-state` | Pool / cycle / pubkey types; AMM-state decoders for Raydium v4, Orca Whirlpool, Meteora DLMM | 3 |
| `dl-detect` | Bellman-Ford negative-cycle detection; Graph build | 4 |
| `dl-sim` | Fill math (constant-product, Orca CL, Meteora bin), cost stack, sizing, EV evaluation | 5 |
| `dl-ledger` | v3 paper ledger (header + entries + summary) with integer-only percentiles | 6 |
| `dl-recon` | Recon pipeline: replay capture to ledger + report JSON + markdown summary | 6 |
| `dl-recon-overfit` | Deflated Sharpe Ratio (DSR) and Probability of Backtest Overfitting (PBO) | 6 |
| `dl-signer` | AES-256-GCM + Argon2id keyfile; daily + per-bundle cap; rate limit | 8 |
| `dl-executor` | Jupiter + Jito bundle builder (mocked clients) | 8 |
| `dl-stream` | StreamingDetector (incremental Bellman-Ford) + LatencyHistogram | 8 |
| `dl-paper` | `PaperWallet` (JSON-backed, atomic-write, stats helpers) | 9 |
| `dl-app` | Binary entry point. Subcommands: `run`, `metrics prom`, `dry-run`, `recon`, `config` | end-to-end |

## Top-level directories

| Path | What's in it |
|------|--------------|
| `crates/` | Workspace members listed above |
| `scripts/` | Operator scripts (start, stop, status, seed, full-pipeline, arbinexus-bridge) |
| `vendor/arbinexus/` | Git submodule: `rigocrypto/arbinexus`, oracle-gated paper-trade simulator |
| `graphify-out/` | Knowledge graph (`graph.html`, `GRAPH_REPORT.md`, `graph.json`) |
| `docs/` | This directory — onboarding, runbook, architecture |
| `.env.example` | Template for `.env` (gitignored) |
| `target/` | Cargo build artifacts (gitignored) |

## Engineering constraints (read this before changing code)

These are non-negotiable project invariants — every PR must keep them:

1. **Integer-only math in the value path.** All trade math uses `u128`/`i128`/`u64`. No `f64` in the simulator. Two exceptions are allowed and tracked:
   - `dl-recon-overfit/` — Deflated Sharpe + PBO math (mathematically requires float).
   - `dl-signer/ratelimit.rs` — token-bucket time decay.
2. **No private keys in the value path.** The keyfile format (`dl-signer`) exists for testing only; `dl-app run --feed live` never loads one.
3. **Float-free CI guards.** Each crate's `tests/` has a `no_floats.rs` test that grep-gates any new `f64`/`f32` import outside the whitelisted files.
4. **LiveMode gate is mandatory.** `LiveMode::Refused` (default) refuses to start the engine unless `DL_LIVE_MODE` is explicitly set to `devnet`, `mainnet-paper`, or `mainnet`.
5. **Conservative-bound defaults.** Every evaluator uses `EvalParams::conservative_default()` unless explicitly overridden. Paper-mode optimistic overrides must be clearly commented.

## Build & test

```bash
# All commands assume repo root.
cargo build --release
cargo test --workspace
cargo build --release -p dl-app
./target/release/dl-app --help
```

Test count trajectory: 360 (v1.0.0) → 441 (post v1.1.7).

## Runbook (operator quick-reference)

### Start paper trading

```bash
# 1. Edit .env (gitignored) with your settings:
#    DL_LIVE_MODE=mainnet
#    DL_LIVE_WS_URL=wss://your-quicknode-or-helius-rpc/...
#    DL_LIVE_POOL_PUBKEYS=58oQChx4yWmvKdwLLZzBi4ChoCc2fqCUWBkwMihLYQo2
#    DL_LIVE_DURATION_SECS=28800
#    DL_PAPER_MODE=realistic
#    (see .env.example for full template)

# 2. Start the bot in background.
./scripts/start_paper_trader.sh

# 3. Watch the dashboard.
./scripts/status.sh

# 4. (Optional) Run the ArbiNexus bridge for realistic PnL with
#    oracle confidence gating.
./scripts/run_arbinexus_bridge.sh
```

### Useful commands

```bash
tail -f trader.log                            # raw event log
./scripts/status.sh                           # wallet dashboard (SOL + USD)
./scripts/stop_paper_trader.sh                # graceful SIGTERM then SIGKILL
./scripts/seed_wallet.sh 1.0                  # reset wallet to 1 SOL
cat wallet.json | jq .                        # full wallet JSON
cat wallet_paper.json | jq '.summary'         # ArbiNexus realistic PnL
```

### Common env vars

| Var | Default | Meaning |
|-----|---------|---------|
| `DL_LIVE_MODE` | unset (Refused) | `devnet` / `mainnet-paper` / `mainnet` |
| `DL_LIVE_WS_URL` | unset | `wss://...` QuickNode / Helius / Triton |
| `DL_LIVE_POOL_PUBKEYS` | unset | comma-separated mainnet pool addresses |
| `DL_LIVE_DURATION_SECS` | `3600` | wall-clock cap in seconds |
| `DL_PAPER_MODE` | `optimistic` | `optimistic` (100% win) or `realistic` (30% win) |
| `DL_LEDGER_PATH` | unset | output path for v3 ledger |
| `DL_TIP_LAMPORTS` | `10000` | Jito tip per bundle |

## Free mainnet RPC options

| Provider | Free tier WebSocket? | Notes |
|----------|---------------------|-------|
| Helius | yes | Includes Sender / gRPC / Jito bundle simulation |
| Triton | yes | Rate-limited; gRPC available |
| QuickNode | yes (Build plan, 15 RPS) | Best DX; some tiers restrict WS |
| Public `api.mainnet-beta.solana.com` | rate-limited | Disconnects sustained WS after ~60s; not for overnight runs |

## Where to read next

- [docs/architecture.md](architecture.md) — code-level architecture walkthrough
- [docs/runbook.md](runbook.md) — detailed operator guide
- [docs/testing.md](testing.md) — how to add a test that won't get reverted
- [docs/known-limitations.md](known-limitations.md) — what's intentionally out of scope
- [graphify-out/GRAPH_REPORT.md](../graphify-out/GRAPH_REPORT.md) — code map with god nodes and community labels
- [CHANGELOG.md](../CHANGELOG.md) — version history

## Code map quick-reference

The 10 most-connected nodes in the codebase (from `graphify-out/`):

1. `replay_pools_to_ledger()` — 30 edges
2. `MetricsRegistry` — 29
3. `build_from_pools()` — 27
4. `simulate_cycle()` — 22
5. `run_paper_live()` — 20
6. `evaluate()` — 20
7. `fill_constant_product()` — 16
8. `decode_amm_info()` — 16
9. `write_synth_ledger()` — 15
10. `find_optimal_input()` — 15

Start your investigation with the god node most relevant to your change; follow `calls` and `references` edges from there.

## License

See `LICENSE` (MIT).
