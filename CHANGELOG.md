# Changelog

All notable changes to `damascus_laundry` are documented here.
The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [v1.0.0] — 2026-XX-XX

### Added
- **Multi-DEX decoding + fill math** for Raydium AMM v4
  (constant-product), Orca Whirlpool (concentrated-liquidity,
  Q64.64 fixed-point), and Meteora DLMM (per-bin,
  `SCALE_OFFSET = 1e12`). New modules: `dl-state/src/decoder/
  orca_whirlpool.rs`, `dl-state/src/decoder/meteora_dlmm.rs`,
  `dl-sim/src/fill_orca.rs`, `dl-sim/src/fill_meteora.rs`.
- **Per-DEX edge labeling** in `dl-detect`: `Edge::dex_id`
  lets multi-DEX triangles (Raydium + Orca + Meteora)
  enumerate and route each leg to the correct fill-math
  path. AC-4 contract pinned by a synthetic-fixture test.
- **`EngineConfig`** (TOML loader + 20+ env-var overrides)
  with `dl-app config print` subcommand.
- **`MetricsSink` trait + `Counter` / `Gauge` / `Histogram`
  types** in `dl-app::metrics`, with a `MetricsRegistry`
  that supports multi-sink fan-out.
- **Prometheus text-format adapter** (`dl-app::metrics_prom`)
  + `dl-app metrics prom [--port N]` subcommand. Hand-rolled
  emitter (no `prometheus` crate dep) to keep the workspace
  integer-only. AC-5 verified by `curl /metrics` returning
  5 distinct metric names.
- **DL_LEDGER_PATH env-var** wiring: `run_dry_run` opens a
  v3 ledger file and writes the synth triangle's
  `LedgerEntry` records (4 entries per dry-run).
- **`reproduce_paper_pnl.sh`** script: single-command
  reproduction. Refuses to run without `--capture` (CI-safe).
  Produces `ledger.dld`, `report.json`, `PAPER_PNL_REPORT.md`.
  AC-6 verified end-to-end against the sample fixture.
- **Per-cycle tip in `LedgerEntry`**: schema bumped 2 → 3
  (adds `tip_lamports: u64`). v3 readers reject v2 files.
- **`LedgerSummary` percentiles**: 50th and 95th percentile
  of `conservative.e_pnl` via `select_nth_unstable` (integer-
  only, O(n) average).
- **`ReconReport::total_tip_lamports`** field, populated
  from the sum of `LedgerEntry::tip_lamports` over cycle
  records.
- **`engine_aggregate()` real values** for 4 of 6 anchor
  mappings: `MeanTipLamports`, `MedianWinnerPnlSol`,
  `P95WinnerPnlSol`, `TipAsPctOfMev` (using a documented
  first-order approximation).
- **On-chain anchor dataset research gate**:
  `.paul/research/onchain-arb-anchor-dataset.md` and
  `.paul/research/multi-dex-math.md`. Both committed
  before their dependent decoder / parser / fill-math
  tasks.
- **Deflated Sharpe / purged walk-forward CV / PBO** in
  `dl-recon-overfit`. The only `f64` site in the workspace
  (separate crate to keep the float-free invariant clean).

### Changed
- **Schema versioning policy**: v3 readers reject v2 files
  via `LedgerError::SchemaMismatch`. Downward compat
  intentionally not preserved.
- **Float-free invariant**: extended to the new
  `orca_whirlpool` and `meteora_dlmm` decoders (verified by
  `dl-state/tests/fixed_point_no_floats.rs`).
- **Synthetic anchor dataset** renamed from
  `synthetic_anchors.jsonl` to `anchors.v0.jsonl`; provenance
  upgraded to `jito-bot.constants.v0+helius-report.2025`.
- **Golden hash** for the canonical triangle pool universe
  bumped to v3 (`9917465376805268376`). The v2 value
  (`9565092578115491832`) is preserved as `GOLDEN_HASH_V2`.

### Deprecated
- None. v1.0 is the first tagged release.

### Removed
- None. v1.0 is the first tagged release.

### Fixed
- `DL_LEDGER_PATH` deferral from Phase 5 / plan 02 (the
  `dry-run` path now produces a real v3 ledger, not a
  header-only stub).
- 6 honest-deferral items from Phase 6 / plan 02 (per-cycle
  tip, percentile tracking, real engine-aggregate values,
  `DL_LEDGER_PATH` wiring) are closed.

### Security
- No private keys in the value path; the engine is
  paper-trading only. No `unsafe` (`#![deny(unsafe_code)]`
  in every crate).

## [v0.x] — pre-release development

Pre-1.0 development. Plans `02-01` through `07-01` of the
project's `.paul/phases/` directory cover the full
progression. No semantic-versioning semantics applied.
