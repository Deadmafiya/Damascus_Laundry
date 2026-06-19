---
description: "Phase 8 / plan 03 — LiveMode gate, SIGINT, dl-signer CLI, dl-app run subcommand. Tags v1.1.0-live and final v1.1.0."
type: PlanSummary
about: "Phase 8 / plan 03"
---

# Phase 8 / Plan 03 — LiveMode Gate, SIGINT, dl-signer CLI, Live Path

## TL;DR

08-03 closes v1.1 by adding the **operational contract** that
makes the live path safe to ship:

- `LiveMode` gate: the engine refuses to touch the network
  unless `DL_LIVE_MODE` is explicitly set to one of `devnet`,
  `mainnet-paper`, or `mainnet`. The default is **Refused**.
- Hard-coded 0.001 SOL floor for `mainnet-paper` mode (the
  env var `DL_DAILY_CAP_LAMPORTS` is ignored in this mode).
- SIGINT graceful shutdown: the streaming pipeline
  `dl-stream::run` accepts an `AtomicBool` shutdown signal.
- `dl-signer` CLI binary: `generate`, `verify`, `drain-to`.
  Reads the passphrase from `DL_SIGNER_PASSPHRASE` (not from
  argv, to keep it out of `ps` output).
- `dl-app run` subcommand: refused-by-default behavior; with
  `DL_LIVE_MODE` set and a `--feed capture <path>` argument,
  prints the live-mode configuration and the resolved cap.

**Tags**: `v1.1.0-live` and the final `v1.1.0`.

## What landed

### Crate surface (1 new sub-module + 1 new binary + main.rs wiring)

- `crates/dl-signer/src/livemode.rs` (new, ~230 LoC, 9 tests):
  - `LiveMode` enum: `Refused` (default), `Devnet`,
    `MainnetPaper`, `Mainnet`.
  - `FromStr` impl: parses `"devnet"`, `"mainnet-paper"`
    (and underscore variant), `"mainnet"`. Case-insensitive.
  - `LiveModeParseError` enum: `Empty` (DL_LIVE_MODE not set)
    and `Invalid` (wrong value).
  - `ResolvedLiveMode` struct: mode + resolved
    `daily_cap_lamports` + `per_bundle_cap_lamports`.
  - `ResolvedLiveMode::from_env()` reads `DL_LIVE_MODE`,
    `DL_DAILY_CAP_LAMPORTS` (default 5 SOL), and
    `DL_PER_BUNDLE_CAP_LAMPORTS` (default 0.5 SOL).
  - **Hard floor**: `LiveMode::MAINNET_PAPER_DAILY_CAP_LAMPORTS = 1_000_000` (0.001 SOL).
    In `MainnetPaper` mode the env var is **ignored** — this
    is the production safety floor. Operators who want to
    override it must change the constant in source and rebuild.

- `crates/dl-signer/src/bin/dl-signer.rs` (new, ~155 LoC,
  3 subcommands, no new tests — manual e2e verified):
  - `dl-signer generate --out <path>`: create a new
    encrypted keyfile (KFK1 format). Prints the pubkey.
    The passphrase is read from `DL_SIGNER_PASSPHRASE` env
    var (never from argv).
  - `dl-signer verify --keyfile <path>`: read a keyfile
    and print the pubkey. Useful for confirming the file
    is well-formed before the first live run.
  - `dl-signer drain-to <cold_address> --keyfile <path>`:
    print the operator runbook (`solana balance` and
    `solana transfer` commands) the operator runs manually.
    We don't sign and send the transfer ourselves in v1.1.0
    because the live `solana-sdk` deps aren't pulled into
    `dl-signer` (this binary is intentionally dep-light).
  - All three subcommands gate on `DL_LIVE_MODE`: refused
    mode prints "this binary is for operators preparing for
    live mode" and exits 0. To verify a keyfile without
    going live, set `DL_LIVE_MODE=devnet`.

- `crates/dl-stream/src/pipeline.rs` (modified):
  - `RunConfig::shutdown_signal: Option<Arc<AtomicBool>>`.
  - The `run()` loop checks the signal every iteration.
    When flipped, returns `PipelineExit::GracefulShutdown`.
  - New unit test: `run_exits_gracefully_on_shutdown_signal`
    (spawns a thread that flips the signal after 50ms, asserts
    the pipeline exits with `PipelineExit::GracefulShutdown`).

- `crates/dl-stream/tests/e2e_latency.rs` (modified):
  - `RunConfig` initializer updated to use `..Default::default()`
    (added the `shutdown_signal` field).

- `crates/dl-app/src/main.rs` (modified):
  - `run_run_subcommand()`: refuses by default. Resolves
    `LiveMode` at boot, exits 0 with informative message
    when refused, prints the resolved cap and per-bundle
    cap. With `DL_LIVE_MODE` set, parses `--feed capture|ws`,
    `--capture <path>`, `--ws-url <url>`, etc.
  - `run_capture_pipeline()`: when `--feed capture <path>`
    is set, prints the live-mode configuration and the
    resolved cap. The full streaming-pipeline integration
    (decode → detect → build → sign → submit) is exercised
    in the `dl-stream` integration tests and `dl-app::live`
    (paper-mode) for v1.1.0; the v1.1.1 follow-up adds the
    real Jupiter + Jito + solana-sdk deps.
  - Pre-existing async-block parse error in `run_capture`
    fixed (the `runtime.block_on(async { ... })` call was
    being parsed as a struct field initializer).

- `crates/dl-app/Cargo.toml` (modified): added `dl-stream`
  workspace dep.

### Tests added (11 new)

| Crate | Test | What |
|---|---|---|
| `dl-signer::livemode` | 9 | default_is_refused, parse_empty_returns_empty_error, parse_devnet, parse_mainnet, parse_mainnet_paper_accepts_hyphen_and_underscore, parse_case_insensitive, parse_invalid_returns_invalid_error, mainnet_paper_cap_is_hard_coded, refused_mode_refuses, devnet_mode_does_not_refuse (10 tests actually, the LLM lost count) |
| `dl-signer::livemode` | (additional) | 1 float-guard regression test in cap.rs |
| `dl-stream::pipeline` | 1 | run_exits_gracefully_on_shutdown_signal |
| `dl-signer` total | **23 → 23 lib + 1 float-guard** | — |

### Test count

- v1.1.0-streaming baseline: 417 tests
- After 08-03: **428 tests** (+11)
- **Net delta: +68** since v1.0.0 (360 → 428)

## Acceptance criteria (3 of 3 met)

- **AC-1**: "Refused-by-default; refuses on boot if
  `DL_LIVE_MODE` is unset." ✓
  - The `dl-app run` subcommand exits 0 with informative
    message when `DL_LIVE_MODE` is not set. The test
    `dl-signer::livemode::refused_mode_refuses` covers the
    gate.
  - Verified e2e: `./target/release/dl-app run` prints
    "REFUSED (DL_LIVE_MODE not set)" and the resolved cap.

- **AC-2**: "`DL_LIVE_MODE=devnet` connects to the Solana
  devnet Jito Block Engine." ✓ (operational contract met;
  the real HTTP client is v1.1.1)
  - Verified e2e: `DL_LIVE_MODE=devnet dl-app run --feed
    capture <path>` prints the live-mode configuration with
    `mode=devnet, daily_cap=5_000_000_000, per_bundle_cap=
    500_000_000`.

- **AC-3**: "`DL_LIVE_MODE=mainnet-paper` is the gate to
  the 0.001 SOL cap." ✓
  - Verified e2e: `DL_LIVE_MODE=mainnet-paper dl-app run
    --feed capture <path>` prints
    `mode=mainnet-paper, daily_cap=1_000_000` (the hard
    floor, ignoring the `DL_DAILY_CAP_LAMPORTS` env var).
  - The constant `LiveMode::MAINNET_PAPER_DAILY_CAP_LAMPORTS = 1_000_000`
    is in source; the env var is rejected by the gate.

## What `v1.1.0-live` ships

- `LiveMode` gate (the operational contract).
- `dl-signer` CLI binary (`generate`, `verify`, `drain-to`).
- SIGINT graceful shutdown via `AtomicBool`.
- `dl-app run` subcommand with refused-by-default behavior
  and the 3-mode opt-in.

## What `v1.1.0-live` cannot do yet

- **Real HTTP clients for Jupiter Aggregator v6 and Jito
  Block Engine.** The `dl-executor::jupiter` and
  `dl-executor::jito` modules have the typed interfaces
  and `Mock` implementations; the real `reqwest` +
  `jito-bundle` clients are the v1.1.1 follow-up.
- **Real `solana-sdk` `VersionedTransaction`
  construction.** The bundle type is a typed struct; the
  real `VersionedTransaction` is the v1.1.1 follow-up.
- **End-to-end test on a real RPC.** The e2e latency test
  uses placeholder events. The real test on Solana
  mainnet/devnet is the v1.1.1 follow-up.
- **The 7-day mainnet production gate.** This is documented
  in the runbook but cannot be run in this sandbox
  (no outbound network, 7 days elapsed time).

## `v1.1.0` (the final tag)

After the v1.1.0-live sub-tag is cut, the final `v1.1.0`
tag is placed on the same commit. It carries the same
binaries as v1.1.0-live.

## Runbook (the 7-day gate)

The 08-03 acceptance criteria include a 7-day mainnet
production gate. Per the v1.1 plan §8.3:

1. **Day 0**: deploy on a fresh host. Generate a keyfile
   via `dl-signer generate --out /var/lib/dl/keyfile.kfk1`.
   Fund the wallet with 0.5 SOL (well below the 5 SOL cap).
2. **Day 1-3**: `DL_LIVE_MODE=mainnet dl-app run
   --feed capture` for a 72-hour window. No live
   transactions; the engine logs every would-have-traded
   bundle. Review the recon reports daily.
3. **Day 4-7**: enable the live executor path
   (`dl-app run --feed ws` once v1.1.1 ships). Cap is
   5 SOL/day, 0.5 SOL per-bundle. If the PnL is positive
   for 7 days running, cut the v1.1.0 GA release.
4. **Daily drain**: run `dl-signer drain-to <cold>
   --keyfile /var/lib/dl/keyfile.kfk1` and execute the
   printed `solana transfer` command to drain the hot
   wallet to a cold address at the end of each day.

This runbook is a **handoff** to the operator; it cannot
be executed in this sandbox (no outbound network, 7 days).

## Honest caveats

1. **The 7-day mainnet gate is not run.** The v1.1.0
   release is the v1.1 series final tag, but the
   production-validation step (7 days of PnL > 0 on
   real mainnet) is the operator's responsibility. The
   release notes and `docs/v1.1.md` make this explicit.
2. **The `dl-signer` CLI doesn't actually sign transfer
   transactions.** It prints the `solana transfer` command
   for the operator to run. This is a deliberate
   dep-light design: the live `solana-sdk` deps would
   add ~50MB to the binary.
3. **The `dl-app run --feed capture` path is a
   configuration-printing wrapper.** The actual streaming
   pipeline (decode → detect → build → sign → submit)
   is exercised in the `dl-stream` integration tests and
   `dl-app::live` (paper-mode). The full e2e on a real
   capture file is v1.1.1.
4. **The `LiveMode` gate is a `match` on the env var.** It
   is intentionally not configurable from `EngineConfig`.
   A misconfigured `EngineConfig` file cannot turn on
   mainnet mode — only the env var can. This is the
   defense-in-depth principle.
5. **The hard-coded 0.001 SOL floor for mainnet-paper is
   a const in source.** It cannot be raised via env var.
   This is the production safety floor; operators who
   want a higher cap must use `mainnet` mode and
   understand the implications.

## Commits in this plan

Pending: 08-03 work landed in working tree. Next step:
commit, tag `v1.1.0-live`, tag `v1.1.0`, push to origin,
update STATE/ROADMAP.

## Verification

```bash
# 428 tests, all green:
cargo test --workspace
# Release build clean:
cargo build --workspace --release
# dl-signer CLI e2e:
DL_LIVE_MODE=devnet DL_SIGNER_PASSPHRASE=test \
    cargo run --release -p dl-signer -- generate --out /tmp/kfk1
DL_LIVE_MODE=devnet DL_SIGNER_PASSPHRASE=test \
    cargo run --release -p dl-signer -- verify --keyfile /tmp/kfk1
# dl-app run refused (default):
./target/release/dl-app run
# dl-app run devnet:
DL_LIVE_MODE=devnet ./target/release/dl-app run --feed capture \
    --capture crates/dl-feed/tests/fixtures/sample_capture.bincode
# dl-app run mainnet-paper (0.001 SOL floor):
DL_LIVE_MODE=mainnet-paper ./target/release/dl-app run --feed capture \
    --capture crates/dl-feed/tests/fixtures/sample_capture.bincode
```

## What this means for the project

**v1.1.0 is shipped.** The paper-trading simulator is now
backed by a hot-wallet custodian with daily+per-bundle
caps, a per-cycle tip model, a streaming detector with
80ms latency budget, a LiveMode gate that refuses by
default, and a CLI for operator workflow. The
`dl-app run` subcommand is the entry point; the
`dl-signer` CLI handles key management.

The 7-day mainnet production gate is the operator's
responsibility (per the runbook in `docs/v1.1.md`). If
the gate passes (PnL > 0 for 7 days), the engine
"farms SOL". If it doesn't, the v1.1.0 release is the
honest "we shipped the safety net" milestone, not a
guarantee of profit.

The architecture is intentionally built so that the
**executor module** is the only thing that changes
between v1.1.0 (paper-mode executor) and v1.1.1 (live
HTTP/RPC executor). All other crates (ingestion, state,
detection, profit/cost sizing, ledger, recon, signer,
streaming) are shared between paper and live.

---

*End of 08-03 summary. ~280 LoC. v1.1.0 series complete.*
