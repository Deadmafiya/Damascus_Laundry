# `dl-app recon --report-json` — JSON contract

This document is the JSON contract for the reconciliation harness
output and the operator one-liner that consumes it. It is the source
of truth that DAM-60 (and DAM-77.A's recon re-authoring) commit to.

Owners:
- Pipeline: `crates/dl-recon/src/pipeline.rs` (`ReconReport`),
  `crates/dl-recon/src/onchain.rs` (`AnchorDataset`,
  `AnchorDivergence`).
- CLI: `crates/dl-app/src/recon.rs` (`run`, `parse_args`,
  `dispatch`).
- Operator entry: `scripts/recon_bundle.sh`.

When the schema changes, update this doc in the same PR that changes
the Rust types. The recon report is a `ReconReport` serialized via
`serde_json::to_string_pretty`; the field names below are the wire
names.

---

## 1. The ship gate

For a recorded mainnet-paper bundle, the operator must answer a single
question before the paper-PnL figure is "trust the engine":

> Did the realized PnL match the predicted PnL within 0.001 SOL?

The gate is:

```
gap_sol   = realized_pnl_sol - predicted_pnl_sol
ship iff  |gap_sol| <= 0.001
```

- `predicted_pnl_sol` comes from the recon report's
  `summary.sum_conservative_e_pnl`, expressed in SOL (the field is
  the 1e18-scaled signed integer from `LedgerSummary`; the operator
  one-liner converts to SOL with `value / 1e18`).
- `realized_pnl_sol` is the on-chain realized PnL for the bundle,
  recorded by the live trade landing log. **The realized-PnL bridge is
  owned by DAM-58.** Until DAM-58 lands, `realized_pnl_sol` is `0.0`
  and the operator is expected to hand-fill it from the landing log;
  the gate then reduces to "did the conservative e_pnl stay within
  ±0.001 SOL of zero?" which is the conservative-by-default Phase 1c
  contract.
- The per-anchor basis-points divergence (see §4) is a *calibration
  signal* that feeds into DAM-35 (calibration fit). It is *not* the
  Phase 1c ship gate; the SOL-scaled `gap_sol` is.

---

## 2. `dl-app recon --report-json <path>` — output shape

`dl-app recon` is the CLI that produces the JSON. Invoked as:

```
dl-app recon --capture <capture.dlf> \
             --anchors <anchors.jsonl> \
             --report-json <out.json>   \
             [--calibrate]
```

It writes the `ReconReport` to `<out.json>` and the operator
one-liner `scripts/recon_bundle.sh` consumes that file.

The JSON shape is the `serde_json` view of
`crates/dl-recon/src/pipeline.rs::ReconReport` plus the
`AnchorDataset::compare` divergences. Top-level fields:

| Field                  | Type           | Source                                              | Notes                                                                  |
|------------------------|----------------|-----------------------------------------------------|------------------------------------------------------------------------|
| `params`               | object         | `ReplayParams` (`pipeline.rs`)                      | The two `EvalParams` (optimistic + conservative) + cost + sizing caps. |
| `cycle_records`        | array<object>  | `Vec<CycleRecord>`                                  | Per-detected-cycle evaluation. See §2.1.                               |
| `summary`              | object         | `LedgerSummary` (`dl-ledger/src/summary.rs`)        | Counts + aggregate PnL. See §2.2.                                      |
| `divergences`          | array<object>  | `Vec<Divergence>` (`pipeline.rs`)                   | Always empty in 06-01. See §3.                                         |
| `report_hash`          | integer (u64)  | FNV-1a 64 over bincode of records (`pipeline.rs`)   | Determinism invariant I-6.                                             |
| `feed_events_consumed` | integer (u64)  | `dl_recon::pipeline::pools_from_feed`               | 0 in pool-only path.                                                   |
| `total_tip_lamports`   | integer (u64)  | sum of `LedgerEntry::tip_lamports`                  | 0 in pool-only path.                                                   |
| `anchor_divergences`   | array<object>  | `AnchorDataset::compare` (`onchain.rs`)             | One entry per anchor the engine can produce. See §4.                   |

### 2.1 `cycle_records[i]`

| Field      | Type           | Source                                  |
|------------|----------------|-----------------------------------------|
| `seq`      | integer (u64)  | Sequence number, 0-based.                |
| `cycle`    | object         | `Cycle` (legs in order, `weight_sum`).  |
| `net`      | object         | `NetProfit` (gross / fees / net).       |
| `outcome`  | object         | `EvalOutcome` (optimistic + conservative `e_pnl`, `p_land`). |
| `decision` | string         | `WouldTrade` \| `WouldNotTrade`         |
| `entry`    | object         | `LedgerEntry` (full ledger record).     |

### 2.2 `summary`

The `LedgerSummary::from_entries` aggregate. The fields are private
on the Rust type but the `Serialize` impl exposes the underlying
storage as `snake_case` JSON keys:

| Field                          | Type           | Source                                |
|--------------------------------|----------------|---------------------------------------|
| `total`                        | integer (u64)  | `entries.len()`                       |
| `would_trade`                  | integer (u64)  | count of `Decision::WouldTrade`       |
| `would_not_trade`              | integer (u64)  | `total - would_trade`                 |
| `sum_optimistic_e_pnl`         | integer (i128) | sum of `optimistic.e_pnl`             |
| `sum_conservative_e_pnl`       | integer (i128) | sum of `conservative.e_pnl`           |
| `sum_conservative_p_land`      | integer (u128) | sum of `conservative.p_land.scaled()` |
| `median_conservative_e_pnl`    | integer (i128) | 50th pct of `conservative.e_pnl`      |
| `p95_conservative_e_pnl`       | integer (i128) | 95th pct of `conservative.e_pnl`      |

All `e_pnl` fields are on the 1e18 scale (`Prob`-scaled integer).
Conversion to SOL is `value / 1e18`.

---

## 3. `divergences` (06-01 stub)

`Divergence` is the structured diff against a *prior* ledger. In
the 06-01 single-source-of-truth build, the freshly-built report IS
the only report, so the array is always empty. The shape is reserved
here for 06-02 (when the recon report is compared against a recorded
baseline). Per `dl_recon::pipeline::diff_against_ledger` (no-op
stub):

```json
{
  "seq": 0,
  "original_decision": "WouldTrade",
  "re_decision": "WouldNotTrade",
  "original_e_pnl": "0",
  "re_e_pnl": "0",
  "delta_e_pnl": "0"
}
```

---

## 4. `anchor_divergences` — per-anchor bps diff (DAM-35 calibration signal)

The on-chain macro anchors. `AnchorDataset::load_jsonl` reads a
`.jsonl` of `AnchorEntry`; `AnchorDataset::compare` produces one
`AnchorDivergence` per anchor the engine can produce. Source:
`crates/dl-recon/src/onchain.rs`.

`AnchorDivergence` shape:

```json
{
  "name": "LandedArbCount",
  "engine_value": 12,
  "anchor_value": 11,
  "divergence_bps": 909,
  "tolerance_bps": 500,
  "exceeds_tolerance": true
}
```

- `name` — one of the `AnchorName` enum variants
  (`AttemptCount`, `LandedArbCount`, `MeanTipLamports`,
  `MedianWinnerPnlSol`, `P95WinnerPnlSol`, `TipAsPctOfMev`).
- `engine_value` / `anchor_value` — fixed-point in the anchor's
  `unit` (lamports, bundles, bps, etc.). `u128` on the wire.
- `divergence_bps` — `(engine - anchor) / anchor * 10_000`, signed.
  Positive = engine over-estimated.
- `tolerance_bps` — per-anchor tolerance (see `AnchorName::tolerance_bps`).
- `exceeds_tolerance` — `|divergence_bps| > tolerance_bps`.

The `dl-app recon` CLI exits 1 if any anchor in this list has
`exceeds_tolerance == true`. The operator one-liner
(`scripts/recon_bundle.sh`) does **not** consume this list directly;
it treats the CLI exit code as a precondition and applies the
SOL-scaled `gap_sol` gate from §1.

The per-anchor bps diff is the DAM-35 calibration signal. DAM-35
takes the `anchor_divergences` list, fits new `EvalParams`, and
produces a `CalibrationFit` (`improved_params` + remaining
divergences). It does *not* decide ship/no-ship — that decision is
the SOL-scaled `gap_sol` gate above.

---

## 5. `scripts/recon_bundle.sh` — summary output

The operator one-liner reads the JSON written by `dl-app recon
--report-json`, applies the §1 gate, and prints a structured summary
to stdout. The summary has these fields (snake_case, matching the
issue's spec):

| Field                | Type     | Source                                                          |
|----------------------|----------|-----------------------------------------------------------------|
| `bundle_id`          | string   | argv[1]                                                         |
| `capture`            | string   | resolved capture path                                           |
| `anchors`            | string   | `DL_ANCHORS_FILE`                                               |
| `recon`              | string   | `DL_RECON_OUT`                                                  |
| `tolerance_sol`      | number   | `DL_TOLERANCE_SOL` (default 0.001)                              |
| `predicted_pnl_sol`  | number   | `summary.sum_conservative_e_pnl / 1e18`                         |
| `realized_pnl_sol`   | number   | `DL_REALIZED_PNL_SOL` (default 0.0; set by DAM-58 once shipped) |
| `gap_sol`            | number   | `realized - predicted`                                          |
| `within_tolerance`   | boolean  | `|gap_sol| <= tolerance_sol`                                    |
| `would_trade`        | boolean  | `summary.would_trade > 0`                                       |
| `feed_events`        | integer  | `feed_events_consumed`                                          |
| `report_hash`        | integer  | `report_hash`                                                   |
| `total_tip_lamports` | integer  | `total_tip_lamports`                                            |

The script's exit code is the gate outcome; see §6.

---

## 6. Exit codes

`scripts/recon_bundle.sh` exits:

| Code | Meaning                                                                                     |
|------|---------------------------------------------------------------------------------------------|
| 0    | Pass — `|gap_sol| <= DL_TOLERANCE_SOL`.                                                     |
| 1    | Fail — `|gap_sol| >  DL_TOLERANCE_SOL`. Anchor divergences in the recon CLI are pre-recorded. |
| 2    | Runtime error — no manifest, no matching capture, `dl-app recon` exit != {0,1}, missing JSON, etc. |

Underlying `dl-app recon` exits:

| Code | Meaning                                                                |
|------|------------------------------------------------------------------------|
| 0    | Clean — all anchors within tolerance.                                  |
| 1    | At least one anchor exceeds tolerance (per-anchor bps signal).         |
| 2    | Runtime error (missing capture, decode failure, etc.).                 |

The one-liner translates `dl-app recon`'s exit 1 into a non-fatal
"continue to JSON read" path (so the operator still sees the summary
even when anchors diverge) and translates any other non-zero into
exit 2.

---

## 7. Resolution rules for `<bundle_id>` → capture path

The one-liner resolves `<bundle_id>` to a capture on disk via:

1. `captures/manifest.json` — a JSON object keyed by `bundle_id`,
   values are paths (relative to `DL_CAPTURE_DIR` or absolute). The
   script reads with `jq -r --arg id "$BUNDLE_ID" '.[$id] // empty'`.
2. Flat search fallback — `captures/${BUNDLE_ID}.dlf`,
   `captures/${BUNDLE_ID}.bincode`, `captures/${BUNDLE_ID}` in that
   order.

If both miss, the script exits 2 with:

```
recon_bundle: no manifest entry for '<bundle_id>' and no captures/<bundle_id>*.{dlf,bincode} found
```

---

## 8. Diff against a prior run

The `divergences` field (§3) is the diff between a freshly-built
report and a *previously-recorded* ledger. It is empty in 06-01 and
non-empty in 06-02.

`anchor_divergences` (§4) is the per-anchor bps diff between the
engine aggregate and the on-chain macro anchor dataset
(Jito Block Explorer API + Dune cross-check). It is the calibration
signal that DAM-35 consumes.

The SOL-scaled `gap_sol` (§1) is the Phase 1c ship gate. The
per-anchor bps is a calibration signal. They are intentionally
separate dimensions: the gate is binary, the calibration is
continuous.

---

## 9. Notes for the next agent

- DAM-58 owns the realized-PnL bridge. Until it ships, `gap_sol`
  is `predicted_pnl_sol - 0.0` and the operator is expected to
  hand-fill `DL_REALIZED_PNL_SOL` from the landing log.
- The recon report is `serde_json::to_string_pretty` of `ReconReport`,
  so any field added to `ReconReport` will appear on the wire
  automatically. Update this doc in the same PR.
- The script is `set -euo pipefail` and mode 0755. It is CI-safe:
  it does no live network calls. All paths are operator-supplied
  via env vars.
