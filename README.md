# damascus_laundry

> Solana MEV **paper-trading engine**. Wires real-time mainnet
> WebSocket RPC → three-DEX state (Raydium AMM v4, Orca Whirlpool,
> Meteora DLMM) → Bellman-Ford cycle detection → pessimistic
> EV evaluation → paper ledger. No private keys in the value path.

The v2.0 work adds a calibration + DL-Gate tier ladder that takes
the engine from "looks profitable" to "actually survives on
mainnet" — `dl-oracle/` for freshness scoring, `dl-calibration/`
for win-rate / decay / tip estimation, and recon overfitting
defenses. See `plan/damascus_laundry_v2.0.md` and
`docs/v2.0-operator-runbook.md` for the phased delivery.

## TL;DR for a new operator

```bash
git clone https://github.com/Deadmafiya/Damascus_Laundry.git
cd Damascus_Laundry

# 1. Pin Rust toolchain (rustup honors rust-toolchain.toml automatically)
rustup show

# 2. Build the binary
make build           # or: cargo build --release -p dl-app

# 3. Smoke test — no keys loaded, paper trading only
./target/release/dl-app --help
./target/release/dl-app config print

# 4. Run the test suite (~457 passing unit tests, see STATUS below)
cargo test --workspace --lib --no-fail-fast

# 5. (Optional) Start live paper trading
cp .env.example .env
$EDITOR .env        # set DL_LIVE_WS_URL=wss://your-quicknode/...
./scripts/start_paper_trader.sh
./scripts/status.sh
```

Paper trading only — `dl-app run --feed live` never loads a
keyfile. The `dl-signer` crate can derive keys from an encrypted
keyfile for *testing* the rate limiter and cap math; production
never touches it.

## STATUS

- **Branch**: `dam-69/chaos-drills` (active dev). `main` is the
  stable paper-trader line.
- **Latest tags**: `v1.2.3-reuseaddr-orphan-kill` →
  `v1.1.7-realistic-mode` and back down per the chaos-drill series.
- **Rust toolchain**: 1.94.1 (pinned in `rust-toolchain.toml`).
- **Tests**: 457 unit-test pass / 2 known-broken
  (`dl-state` real-mainnet Orca Whirlpool decoder snapshots need
  regeneration — see Status / Known issues below).
- **Workspace**: 16 crates (see [Workspace layout](#workspace-layout)).
- **CI**: `.github/workflows/ci.yml` runs fmt + clippy + test +
  float-free invariants on every PR.
- **Submodule**: `vendor/arbinexus` (git submodule, oracle-gated
  realistic bridge; clone with `git clone --recurse-submodules`).

### Known issues (Phase 1c / 2 work in `dam-69/chaos-drills`)

- `crates/dl-state` — 2 unit tests fail in
  `decoder::orca_whirlpool::tests` with mainnet real extracts
  (`copy_from_slice: source slice length (9) does not match
  destination slice length (8)`). Decoder binary-shape drift;
  fix is regenerate the test fixtures from a fresh mainnet
  snapshot. Doesn't affect any other crate.
- `crates/dl-feed/tests/whirlpool_subscription.rs` and
  `crates/dl-app/tests/cycle_writer_schema.rs` reference modules
  that are not yet `pub mod`-wired in their parents' `lib.rs`.
  Wires-up tracked under `dam-69/chaos-drills` follow-ups.
- `crates/dl-recon/tests/dam64_e2e_smoke.rs` needs
  `dl-calibrate` on `PATH` — runs in CI in
  `crates/dl-calibration/`.

None of these block `make build`, `cargo test --workspace --lib`,
or starting the paper trader.

## Engineering invariants (do not break)

These are non-negotiable. Every PR keeps them, or it's reverted.

1. **Integer-only math in the value path.** All trade math uses
   `u128`/`i128`/`u64`. No `f64`/`f32` in the simulator or
   detector. Whitelisted exceptions: `dl-recon-overfit/` (DSR/PBO
   require float), `dl-signer/ratelimit.rs` (token-bucket time
   decay). `tests/no_floats.rs` run in CI per crate.
2. **No private keys in the value path.** `dl-app run --feed live`
   never imports or signs with a keyfile. The key exists for
   unit-testing the cap / rate-limit math only.
3. **LiveMode gate is mandatory.** `LiveMode::Refused` (default)
   refuses to start unless `DL_LIVE_MODE` is explicitly
   devnet / mainnet-paper / mainnet.
4. **Conservative-bound defaults.** Every evaluator uses
   `EvalParams::conservative_default()` unless explicitly
   overridden. Paper-mode optimistic overrides must be clearly
   commented.
5. **Plugin-free code graph.** `graphify-out/` is gitignored —
   re-run with `graphify update .` after the code changes if you
   commit it back.

## Workspace layout

| Crate | Responsibility |
|-------|----------------|
| `dl-core` | Fixed-point math (`u128`), injectable `Clock` / `Rng` / `Feed` traits, shared types |
| `dl-feed` | WebSocket + scripted feed; capture/replay; account & program subscribe |
| `dl-state` | Pool / cycle / pubkey types; AMM-state decoders for Raydium v4, Orca Whirlpool, Meteora DLMM |
| `dl-detect` | Bellman-Ford negative-cycle detection; Graph build |
| `dl-sim` | Fill math (constant-product, Orca CL, Meteora bin), cost stack, sizing, EV evaluation |
| `dl-ledger` | v3 paper ledger (header + entries + summary) with integer-only percentiles |
| `dl-recon` | Recon pipeline: replay capture → ledger → report JSON + markdown summary |
| `dl-recon-overfit` | Deflated Sharpe Ratio (DSR) and Probability of Backtest Overfitting (PBO) |
| `dl-signer` | AES-256-GCM + Argon2id keyfile; daily + per-bundle cap; rate limit (testing only) |
| `dl-executor` | Jupiter + Jito bundle builder (mocked clients) |
| `dl-stream` | StreamingDetector (incremental Bellman-Ford) + LatencyHistogram |
| `dl-paper` | `PaperWallet` (JSON-backed, atomic-write, stats helpers) |
| `dl-oracle` | DL-Gate freshness oracle (slot / age / quorum scoring) |
| `dl-calibration` | Win-rate + decay + tip calibration; DL-tier ladder (T0–T3) |
| `dl-assert-sdk` | On-chain assert program SDK (off the workspace default build path) |
| `dl-app` | Binary entry point. Subcommands: `run`, `metrics prom`, `dry-run`, `recon`, `config` |

The BPF program `dl-assert-program` is **excluded** from the
workspace. Build with `cargo build-sbf` (Solana SDK install
required, `scripts/verify_assert_program_deploy.sh` covers it).

## Repository layout

| Path | What's in it |
|------|--------------|
| `crates/` | Workspace members (above) |
| `crates/dl-app/dl-calibration/captures.jsonl` | Runtime artifact — gitignored |
| `scripts/` | Operator scripts (start / stop / status / seed / arbi-bridge / chaos drills) |
| `dashboard/index.html`, `dashboard/live.html` | Dashboard HTML (served by `damascus_laundry_dashboard`) |
| `damascus_laundry_dashboard` | Tiny Python HTTP server serving dashboard + wallet proxy |
| `vendor/arbinexus/` | Submodule: rigocrypto/arbinexus (oracle-gated paper-trade simulator) |
| `docs/` | Architecture, runbooks, intel, delegation notes, observability |
| `docs/runbook-map.md` | Live-runbook ↔ v2.0-runbook disambiguation (single source of truth) |
| `docs/intel/` | Daily briefs, decision log, glossary, timeline, vision |
| `docs/observability/alerts.yml` | Prometheus alert rules (SLO-tracked) |
| `docs/delegation/` | Sub-agent delegation handoffs (DAM-46, DAM-54, DAM-67, etc.) |
| `.env.example` | Template for `.env` (gitignored) |
| `.paul/` | Phase plans + research notes |
| `plan/` | v2.0 plan + decision records |
| `graphify-out/` | Generated code graph — gitignored, regenerate with `graphify update .` |
| `target/` | Cargo build artifacts — gitignored |

## Build & test

```bash
# All commands assume repo root.
make build                      # cargo build --release -p dl-app
cargo build --workspace         # all 16 crates (dl-app binary + 15 libs)
cargo test --workspace --lib --no-fail-fast
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings

# Binary
./target/release/dl-app --help
./target/release/dl-app config print
./target/release/dl-app metrics prom --port 9090
```

## Run the paper trader

```bash
# 1. Edit .env (gitignored) with your settings — see .env.example
cp .env.example .env
# DL_LIVE_MODE=mainnet-paper
# DL_LIVE_WS_URL=wss://your-quicknode.solana-mainnet.quiknode.pro/<key>/
# DL_LIVE_POOL_PUBKEYS=58oQChx4yWmvKdwLLZzBi4ChoCc2fqCUWBkwMihLYQo2,...
# DL_PAPER_MODE=realistic

# 2. Start (writes ./trader.pid, logs to ./trader.log)
./scripts/start_paper_trader.sh

# 3. Status (wallet balance, PnL, last trades)
./scripts/status.sh

# 4. Web dashboard
./damascus_laundry_dashboard   # http://localhost:PORT

# 5. (Optional) Realistic-mode bridge via ArbiNexus
./scripts/run_arbinexus_bridge.sh
# → creates ./wallet_paper.json with 30%-win-rate + tip-cost model

# 6. Stop
./scripts/stop_paper_trader.sh
```

## Runbook map

There are two runbooks in `docs/`. They cover overlapping
territory. `docs/runbook-map.md` is the single source of truth
for "which file owns which step."

- **`docs/live-runbook.md`** — operator SOPs (hot-wallet funding,
  kill-switch recovery, devnet airdrop, daily recon).
- **`docs/v2.0-operator-runbook.md`** — phase-gated checklist per
  phase (1c / 1d / 2 / 3 / 4) of the v2.0 plan.
- **`docs/runbook.md`** — legacy paper-trader runbook. Superseded
  by `live-runbook.md` for live-mode operations.

For phase structure and cap values → `plan/damascus_laundry_v2.0.md`.

## Chaos drills (Phase 1c resilience)

```bash
# Drill 1 — kill process mid-bundle (18 preload sites across crates)
./scripts/chaos/kill_process_mid_bundle.sh

# Drill 2 — undeterministic RPC kill mid-trade
./scripts/chaos/kill_rpc_mid_trade.sh
```

Backed by integration tests:
- `crates/dl-app/tests/chaos_kill_process.rs`
- `crates/dl-app/tests/chaos_kill_rpc.rs`

Wired in via DAM-69 / DAM-84 closure variant. See
`docs/v2.0-operator-runbook.md` for the resilience contract.

## Environment variables (most used)

| Var | Default | Meaning |
|-----|---------|---------|
| `DL_LIVE_MODE` | unset (Refused) | `devnet` / `mainnet-paper` / `mainnet` |
| `DL_LIVE_WS_URL` | unset | `wss://...` QuickNode / Helius / Triton |
| `DL_LIVE_POOL_PUBKEYS` | unset | comma-separated mainnet pool addresses |
| `DL_LIVE_DURATION_SECS` | `3600` | wall-clock cap in seconds |
| `DL_PAPER_MODE` | `optimistic` | `optimistic` (100% win) or `realistic` (30% win) |
| `DL_LEDGER_PATH` | unset | output path for v3 ledger |
| `DL_TIP_LAMPORTS` | `10000` | Jito tip per bundle |
| `DL_MAINNET_KEYFILE` | unset | hot-wallet keyfile (live mode only) |

Full set in `docs/dl-app.env.example.md` and `docs/live-runbook.md`.

## Free mainnet RPC options

| Provider | Free WebSocket? | Notes |
|----------|----------------|-------|
| Helius | yes | Includes Sender / gRPC / Jito bundle simulation |
| Triton | yes | Rate-limited; gRPC available |
| QuickNode | yes (Build, 15 RPS) | Best DX; some tiers restrict WS |
| Public `api.mainnet-beta.solana.com` | rate-limited | Disconnects sustained WS after ~60s — not for overnight runs |

## Submit a PR

1. Branch from `dam-69/chaos-drills` (default dev) or `main`.
2. Keep the 5 engineering invariants intact.
3. `cargo fmt --all --check && cargo clippy --workspace --all-targets -- -D warnings`
4. `cargo test --workspace --lib --no-fail-fast` — 457/457.
5. Update `CHANGELOG.md` and the relevant docs/
   subdirectory. Don't add to `graphify-out/` — it's gitignored.

## Where to read next

- `docs/README.md` — full project overview + how the engine works.
- `docs/architecture.md` — code-level architecture walkthrough.
- `docs/v2.0-operator-runbook.md` — phase-gated live checklist.
- `docs/live-runbook.md` — operator SOPs.
- `docs/runbook-map.md` — runbook disambiguation.
- `docs/intel/vision-and-roadmap.md` — strategic trajectory.
- `plan/damascus_laundry_v2.0.md` — the v2.0 plan (source of truth).
- `CHANGELOG.md` — version history.

## License

Apache-2.0 — see `LICENSE`.
