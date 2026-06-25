# `dl-calibration` — Phase 2 EV-model calibration

`dl-calibration` fits the three multiplicative probabilities
`p_detect · p_win · p_land` that the EV model in `dl-sim::ev`
multiplies into the per-cycle expected PnL. It also runs an
overfit guard (DSR + PBO + purged walk-forward CV) over the
realized PnL series and writes a single `calibration.json` file
that `EvalParams::from_calibration` consumes at startup.

DAM-35 wires the crate to the `dl-recon` paper-ledger path: the
`calibrate` binary now has a `--from-capture` mode that opens a
`.dlf` capture, replays it through `dl-recon`'s harness, derives
`CalibrationCapture` rows, fits, and writes the report — end to
end with no hand-curated JSONL.

## Modes

### 1. From a JSONL captures file (live paper trader)

The live pipeline (`dl-app`) appends one `CalibrationCapture` per
landed bundle to a JSONL file. Run:

```bash
cargo run --release -p dl-calibration --bin calibrate -- \
    --captures ./dl-calibration/captures.jsonl \
    --out     ./dl-calibration/calibration.json
```

Cold-start (file missing or under `MIN_SAMPLES_FOR_FIT` = 30
captures) returns the Laplace-smoothed `0.5` for all three
probabilities and flags `is_overfit_risk: true`.

### 2. From a `dl-recon` `.dlf` capture (DAM-35)

The recon harness produces `.dlf` captures (one of the canonical
Phase 6 / 9 artifacts). Run:

```bash
# 1. Generate a synthetic .dlf for local end-to-end smoke-testing.
cargo run --release -p dl-recon --example dump_capture -- \
    /tmp/example.dlf

# 2. Replay it through recon, fit, and write the calibration report.
cargo run --release -p dl-calibration --bin calibrate -- \
    --from-capture /tmp/example.dlf \
    --out          ./dl-calibration/calibration.json
```

The `captures_from_recon_report` function maps every detected
cycle in the `ReconReport` to a `CalibrationCapture`:

- `realized_pnl_lamports` ← the conservative-bound `e_pnl`
  (signed i64). The optimistic bound is **not** used; using it
  would inflate `p_win` and silently widen the trade gate.
- `won` ← the trade gate (`decision == WouldTrade`).
- `input_mint` / `output_mint` ← the first / last leg's pool
  pubkey. Real mint labels are wired in Phase 3 when the live
  pipeline carries them through.
- `input_amount` ← the cycle's optimal input (saturated to u64).
- `expected_out_per_leg` ← input plus an equal share of the net
  profit. The fit is invariant to this approximation.

`base_ts` is the wall-clock start time; per-cycle ts =
`base_ts + seq` so captures are monotonically increasing. The
exact value doesn't affect the fit, only the order does.

## Overfit guard

`OverfitReport::from_returns` runs three checks on the realized
PnL series:

- **Deflated Sharpe Ratio (DSR)** — corrects the observed
  Sharpe for the number of trials. Uses `dl-recon-overfit`'s
  `deflated_sharpe` against a single strategy (v1.0).
- **Probability of Backtest Overfitting (PBO)** — bootstraps
  `PBO_N_CONFIGS` (= 8) IS/OOS rank pairs by two-block-splitting
  the realized PnL series at deterministic 50–60% cut points,
  then calls `dl-recon-overfit::pbo`. PBO > 0.5 means the
  in-sample winners systematically underperform OOS.
- **Purged walk-forward CV** — 5 folds, 5% embargo. Mean OOS
  Sharpe < 0 is a regression signal.

`is_overfit_risk` flips to `true` if DSR ≤ 0 **or** PBO > 0.5
**or** sample size < `MIN_SAMPLES_FOR_FIT` (= 30).

Cold-start (n < 30) returns `pbo: null` and flags
`is_overfit_risk: true` without running the bootstrap.

## Defensive defaults

- Empty capture set → returns `p = 0.5` for all three (Laplace
  smoothing with α=1). Cold-start is paper-mode-identical.
- Sample size < `MIN_SAMPLES_FOR_FIT` (30) → returns the same
  Laplace-0.5 default and emits a warning.
- Corrupt JSONL line → skipped + logged; never aborts the fit.

## Re-running end-to-end

```bash
# Generate a synthetic .dlf
cargo run --release -p dl-recon --example dump_capture -- /tmp/example.dlf

# Fit and write the report
cargo run --release -p dl-calibration --bin calibrate -- \
    --from-capture /tmp/example.dlf \
    --out          ./dl-calibration/calibration.json

# Inspect the report
cat ./dl-calibration/calibration.json
```

The report schema is:

```json
{
  "result": {
    "p_detect": 500000000000000000,
    "p_win":    500000000000000000,
    "p_land":   500000000000000000,
    "sample_size": 0,
    "fitted_at": 1782023735
  },
  "overfit": {
    "dsr": null,
    "pbo": null,
    "pbo_n_configs": 0,
    "purged_cv": null,
    "is_overfit_risk": true
  }
}
```

Probabilities are stored in `dl-sim::ev::Prob` (ppm, 1e18 scale).
A value of `500_000_000_000_000_000` is the Laplace-0.5 default
(50% probability).

## Tests

```bash
cargo test -p dl-calibration --lib
```

The DAM-35 test set covers:

- `overfit_pbo_runs_on_synthetic_dataset` — PBO produces a real
  `PboResult` on a 60-cycle synthetic return series (n_configs =
  `PBO_N_CONFIGS`, PBO in [0, 1]).
- `overfit_pbo_cold_start_returns_none` — short-circuits to
  `pbo: None` when the return series is under `MIN_SAMPLES_FOR_FIT`.
- `end_to_end_capture_to_calibration_report` — synthesizes a
  pool universe, replays it through `dl-recon`, derives
  `CalibrationCapture` rows, and verifies the wire: one capture
  per detected cycle, conservative `e_pnl` mapped to
  `realized_pnl_lamports`, trade gate mirrored in `won`, ts
  monotonically increasing.
- `fit_from_capture_writes_and_returns_report` — opens a `.dlf`
  capture, replays it, writes `CalibrationReport` to disk, and
  round-trips the JSON back to an equal in-memory report. The
  on-disk JSON must contain the `"pbo"` field (regression guard
  against a future change that drops the field).
