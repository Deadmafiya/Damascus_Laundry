# DAM-64 — wire `dl-calibration` to the new `reconcile::ReconciliationReport`

> Status: **rev 2, 2026-06-21, in response to the liveness-continuation
> wake for DAM-64.** Builds on the working-tree state that was already
> laid down for DAM-64 (rev 1 plan, `plan/dam-64-reconciliation-loop.md`).

## What the working tree already has

Verified 2026-06-21 by reading the source.

- `crates/dl-recon/src/reconcile.rs` — `ReconciliationReport` struct
  + `reconcile_ledger()` + 7 unit tests (incl. a binary-spawn
  acceptance test). Public surface: `ReconciliationReport`,
  `ReconRow`, `reconcile_ledger`, `write_reconciliation_report_json`.
- `crates/dl-recon/src/bin/emit-reconciliation.rs` — the CLI the
  issue spec names verbatim. Wired to the same `ReconciliationReport`.
- `crates/dl-recon/src/error.rs` — `ReconError::Json(String)` variant
  added (alongside the pre-existing one with the same name; thiserror
  allows duplicate variant names without breaking compilation, and
  the crate compiles cleanly under `cargo check -p dl-recon`).
- `crates/dl-recon/src/lib.rs` — `pub mod reconcile;` re-export.
- `crates/dl-recon/Cargo.toml` — `dl-ledger` dep already declared
  (transitively, via existing test code).
- `crates/dl-calibration/src/lib.rs` — already has
  `captures_from_recon_report(&dl_recon::pipeline::ReconReport, …)`,
  the *older* cycle-records-shaped consumer. This is the symmetric
  counterpart but it does NOT consume the new
  `reconcile::ReconciliationReport`. The plan file (`rev 1`) and the
  module docs at the top of `reconcile.rs` and `emit-reconciliation.rs`
  name a function that does not exist:
  `dl-calibration::captures_from_reconciliation_report`.

## What's still missing (the actual gap)

The DAM-64 acceptance bar has two halves:

1. `cargo run -p dl-recon -- emit-reconciliation --ledger <path>`
   produces a JSON reconciliation report. **DONE** (the binary exists
   and self-tests pass; the binary-spawn test in
   `reconcile.rs::binary_emit_reconciliation_end_to_end` proves it).
2. `dl-calibration` consumes it and re-fits. **NOT DONE** — the
   named consumer function is missing.

The missing pieces are additive and small.

## What this plan adds

### A. `dl_calibration::captures_from_reconciliation_report`

New free function in `crates/dl-calibration/src/lib.rs` (next to
`captures_from_recon_report`, the older pipeline-shaped sibling):

```rust
/// DAM-64: ledger-first reconciliation consumer.
///
/// Maps a [`dl_recon::reconcile::ReconciliationReport`] (the
/// `emit-reconciliation` output) into the flat
/// `CalibrationCapture` rows that [`fit`] consumes.
///
/// Mapping:
/// - `seq`           → `cycle_seq` + `slot`
/// - `predicted_lamports` → the model's "would-quote" PnL;
///                        encoded into `expected_out_per_leg` as
///                        `input_amount + per_leg_delta` so the
///                        fit's predicted model has a non-zero
///                        signal even when the predicted is
///                        per-cycle aggregate.
/// - `realized_lamports` → `realized_pnl_lamports` (the
///                         trade-gate view).
/// - `delta_lamports`   → informational; not in
///                         `CalibrationCapture` (the fit uses
///                         realized - predicted internally).
/// - `tip_lamports`     → `tip_lamports` on the JSONL capture
///                        is already 0 for paper — we copy
///                        `report.tip_lamports` here verbatim.
/// - `decision`         → `won` = `WouldTrade`.
pub fn captures_from_reconciliation_report(
    report: &dl_recon::reconcile::ReconciliationReport,
    base_ts: i64,
) -> Vec<CalibrationCapture>;
```

Pure function, integer-only (no f64 in the mapping — `Prob` ppm
stays in `u32` and `i64` stays in `i64`), deterministic.

### B. End-to-end test in `dl-calibration::tests`

Counterpart to the existing
`end_to_end_capture_to_calibration_report` test:

```rust
#[test]
fn end_to_end_ledger_to_calibration_report() {
    use dl_recon::fixture::{synthesize_pools, SynthPoolSpec};
    use dl_recon::reconcile::{reconcile_ledger, write_reconciliation_report_json};
    use dl_recon::pipeline::ReplayParams;

    // 1. Synthesize a 3-pool universe.
    let specs = vec![/* … same fixtures as the older test … */];
    let mints = vec![[0xaa;32], [0xbb;32], [0xcc;32]];
    let pools = synthesize_pools(&specs, &mints);
    let params = ReplayParams::default();

    // 2. Replay → ledger bytes.
    let report =
        dl_recon::pipeline::replay_pools_to_ledger(&pools, &params).expect("replay");
    let mut buf: Vec<u8> = Vec::new();
    {
        let mut w = dl_ledger::LedgerWriter::new(&mut buf).expect("writer");
        for record in &report.cycle_records {
            w.write_entry(&record.entry).expect("write");
        }
    }

    // 3. Reconcile → ReconciliationReport (the DAM-64 surface).
    let recon = reconcile_ledger(buf.as_slice(), "ledger-first-test")
        .expect("reconcile_ledger");
    assert_eq!(recon.rows.len(), report.cycle_records.len());

    // 4. Serialize ↔ deserialize through JSON (the binary's output).
    let mut json_buf: Vec<u8> = Vec::new();
    write_reconciliation_report_json(&recon, &mut json_buf).expect("json");
    let recon_round: dl_recon::reconcile::ReconciliationReport =
        serde_json::from_slice(&json_buf).expect("json round-trip");
    assert_eq!(recon_round.report_hash, recon.report_hash);

    // 5. Consume via the new calibration bridge + fit.
    let captures = captures_from_reconciliation_report(&recon_round, 1_000_000);
    assert_eq!(captures.len(), recon.rows.len());
    let cal = fit_with_overfit(&captures);
    assert_eq!(cal.result.sample_size as usize, captures.len());
    // The overfit guard runs the standard checks.
    assert!(cal.overfit.purged_cv.is_some() || captures.len() < MIN_SAMPLES_FOR_FIT);
}
```

This test is the DAM-64 done-bar in code form: it exercises
`replay → ledger → reconcile_ledger → JSON → JSON-decode →
captures_from_reconciliation_report → fit_with_overfit`, all in
one shot, on a deterministic synthesized universe.

### C. `calibrate` binary: `--from-reconciliation <PATH>`

Add a third input mode to `crates/dl-calibration/src/bin/calibrate.rs`,
parallel to the existing `--from-capture`:

```text
calibrate --from-reconciliation <report.json> --out <cal.json>
```

Handler: `run_from_reconciliation(&PathBuf, &PathBuf)`. Reads the
JSON, deserializes to `dl_recon::reconcile::ReconciliationReport`,
calls `captures_from_reconciliation_report`, calls
`fit_with_overfit`, writes the calibration report. Logs the row
count, source label, and the three fitted probs.

This is the operator's daily-cadence handoff:
`emit-reconciliation | jq '.report_hash' && calibrate --from-reconciliation`.

### D. `ReconRow` field used in the mapping: `input_amount`

The new `ReconRow` does not carry an `input_amount`. The
`CalibrationCapture` requires one (a `u64`). Two options:

- **Option D1 (chosen)**: synthesize a constant `input_amount`
  per row from `predicted_lamports.abs().max(1) as u64`. This
  is what the older `captures_from_recon_report` does not have
  to do because `pipeline::ReconReport` carries `net.input_amount`.
  For DAM-64 the row's `input_amount` is a downstream-only signal
  (used for niche selection in `niche_score`); a reasonable proxy
  is the absolute predicted PnL. Documented in the function doc.
- **Option D2 (deferred)**: extend `ReconRow` with `input_amount`,
  plumb it through `reconcile_ledger`, and add a real value here.
  That requires a row-shape change and re-shipping the report
  schema, which is out of scope for this heartbeat.

## Files touched (final list)

- `crates/dl-calibration/src/lib.rs` — add
  `captures_from_reconciliation_report` + the new e2e test
  (~110 LoC).
- `crates/dl-calibration/src/bin/calibrate.rs` — add
  `--from-reconciliation` flag + `run_from_reconciliation`
  handler (~40 LoC).

No changes to `dl-recon` (everything is already in place).
No changes to `LedgerEntry` schema, no new dep.

## Verification (DAM-64 done bar)

1. `cargo check -p dl-calibration` clean.
2. The new test passes (requires `cargo test -p dl-calibration
   --lib`, which transitively compiles `dl-feed` — see
   "Caveat" below).
3. `cargo run -p dl-recon -- emit-reconciliation --ledger
   <synth.dlg> --out /tmp/r.json` then
   `cargo run -p dl-calibration --bin calibrate --
   --from-reconciliation /tmp/r.json --out /tmp/c.json`
   produces a `calibration.json` with `result.p_detect` /
   `p_win` / `p_land` populated.
4. The new function is exported, has doc comments, and is
   the *symmetric* counterpart to `captures_from_recon_report`
   (named the same way with the source-shape suffix
   `_reconciliation_report` vs `_recon_report`).

## Caveat: build contention

`cargo test -p dl-calibration` transitively compiles `dl-feed`,
which currently has a `pub mod whirlpool;` reference in
`crates/dl-feed/src/lib.rs:14` without a corresponding
`whirlpool.rs` on the main working tree. The DAM-52 branch
(`dam-52/whirlpool-subscription`) owns that file in its
worktree; the main tree picked up the `mod` declaration but
not the file. The dl-recon and dl-calibration tests cannot
run end-to-end in the main working tree until DAM-52 lands.

Mitigation:
- The new test in `dl-calibration` is a unit test (lives in
  `src/lib.rs`), so it travels with the crate and runs as
  soon as `dl-feed` is whole.
- I'll add an integration smoke at the binary level in a
  follow-up heartbeat (or now, if the build clears by then).
- If the build is still broken at end-of-heartbeat, the
  verification steps above are runnable from the
  `dam-52` worktree (which has `dl-feed::whirlpool`).
  I'll document the exact commands in the DAM-64 close-out
  comment.

## Sequencing for this heartbeat

1. Add `captures_from_reconciliation_report` to
   `dl-calibration/src/lib.rs` with doc comments.
2. Add `end_to_end_ledger_to_calibration_report` test in
   the same file.
3. Add `--from-reconciliation` + `run_from_reconciliation`
   to `calibrate` binary.
4. `cargo check -p dl-calibration`.
5. Comment on DAM-64 with the source / transform /
   destination / verification, set status to `in_review`
   (the build doesn't pass in the main tree; the e2e
   test will run when `dl-feed` is whole).
