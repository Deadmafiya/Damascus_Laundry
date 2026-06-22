//! DAM-64 — ledger-first reconciliation consumer.
//!
//! Closes the loop on the DAM-64 acceptance bar:
//!
//! > `cargo run -p dl-recon -- emit-reconciliation --ledger <path>`
//! > produces a JSON reconciliation report; `dl-calibration` consumes
//! > it and re-fits.
//!
//! The producer side (binary + report) lives in `dl-recon::reconcile`.
//! This module is the consumer side: it maps
//! `dl_recon::reconcile::ReconciliationReport` → `Vec<CalibrationCapture>`
//! so the existing `dl_calibration::fit_with_overfit` can produce a
//! `CalibrationReport`.
//!
//! ## Why a separate module
//!
//! This module is intentionally self-contained and does not depend on
//! the DAM-35 wire-up (`ReconReport` / `NicheRank` /
//! `captures_from_recon_report` / `fit_from_capture`) that lives in
//! `lib.rs`. A prior heartbeat added that plumbing to `lib.rs`; a
//! peer-agent revert later wiped it from the main working tree. The
//! DAM-64 deliverable is independent of DAM-35 and lives here so
//! the next agent can pick it up without re-discovering the lost
//! context. See `docs/delegation/dam-64-reconciliation-consumer.md`.
//!
//! ## Mapping (`ReconRow` → `CalibrationCapture`)
//!
//! | `ReconRow` field         | `CalibrationCapture` field        |
//! |--------------------------|-----------------------------------|
//! | `seq`                    | `cycle_seq`, `slot`               |
//! | `predicted_lamports`     | `expected_out_per_leg` (per-leg delta + input_amount) |
//! | `realized_lamports`      | `realized_pnl_lamports`           |
//! | `decision`               | `won` = `WouldTrade`              |
//! | `tip_lamports`           | (not in `CalibrationCapture`; documented) |
//! | (synthesized)            | `input_amount` = `predicted.abs().max(1)` |
//! | (synthesized)            | `input_mint` / `output_mint` = `source_label` |
//! | `base_ts + i`            | `ts`                              |
//!
//! ## Determinism
//!
//! `captures_from_reconciliation_report` is a pure function of
//! `(report, base_ts)`. Same input → same captures, in the same
//! order, with the same `cycle_seq`/`realized_pnl_lamports`/`won`
//! values. Only the `ts` field depends on `base_ts`.
//!
//! ## Integer-only
//!
//! No `f32` or `f64` in any value path. Ppm clamping stays in
//! `dl_sim::ev::Prob::from_ppm`. The `i128` → `i64` clamp on
//! `realized_lamports` is a saturating `as` cast; a future schema
//! bump to a wider `CalibrationCapture::realized_pnl_lamports`
//! would lift the bound.

use dl_recon::reconcile::{ReconRow, ReconciliationReport};

use crate::{CalibrationCapture, fit_with_overfit};

/// Map a [`ReconciliationReport`] (the `emit-reconciliation` output)
/// into the flat `CalibrationCapture` rows that `fit` consumes.
///
/// See the module docs for the full mapping table.
///
/// # Arguments
///
/// - `report` — a `ReconciliationReport` produced by
///   `dl_recon::reconcile::reconcile_ledger` (or its JSON
///   deserialize-then-rehydrate form).
/// - `base_ts` — the unix epoch (seconds) the cycle `ts` fields
///   are offset from. Each row's `ts` is `base_ts + i` where `i`
///   is the row's position in `report.rows`. Use the run's
///   start-of-session wall clock for the operator's daily cadence.
///
/// # Returns
///
/// A `Vec<CalibrationCapture>` of the same length as `report.rows`.
/// On an empty report the returned vector is empty.
pub fn captures_from_reconciliation_report(
    report: &ReconciliationReport,
    base_ts: i64,
) -> Vec<CalibrationCapture> {
    report
        .rows
        .iter()
        .enumerate()
        .map(|(i, row)| row_to_capture(row, &report.source_label, base_ts, i as i64))
        .collect()
}

/// Per-row mapping. Pulled out so the unit tests can exercise the
/// edge cases (clamp, decision) without re-constructing a full
/// `ReconciliationReport`.
fn row_to_capture(
    row: &ReconRow,
    source_label: &str,
    base_ts: i64,
    index: i64,
) -> CalibrationCapture {
    // input_amount: stable proxy. The `ReconRow` does not carry the
    // original cycle's input amount (it is a predicted-vs-realized
    // projection), so we synthesize one from `predicted.abs().max(1)`.
    // This keeps `niche_score`'s `SizeBucket` classification
    // non-degenerate on zero-prediction rows. Cast to u64 with a
    // saturating clamp — for a sane paper path the predicted PnL
    // fits in u64; the i128 -> u64 cast is documented as lossy
    // for the *future* live on-chain `realized_pnl_lamports: i64`
    // field, which we never use here.
    let input_amount: u64 = {
        let abs = row.predicted_lamports.unsigned_abs();
        if abs > u64::MAX as u128 {
            u64::MAX
        } else {
            abs.max(1) as u64
        }
    };

    // Per-leg expected_out: encode the predicted as
    // `input_amount + per_leg_delta` so `fit_with_overfit`'s
    // `niche_score` sees a positive signal. `ReconRow` does not
    // carry a leg count, so we treat it as 1 leg per row; the
    // downstream `niche_score` does not consume `expected_out_per_leg`
    // for `SizeBucket` (it uses `input_amount`).
    let per_leg_delta: i64 = if row.predicted_lamports == 0 {
        0
    } else {
        // 1 leg → delta is the full predicted.
        row.predicted_lamports.min(i64::MAX as i128) as i64
    };
    let expected_out_per_leg: Vec<u64> = vec![
        (input_amount as i64)
            .saturating_add(per_leg_delta)
            .max(0) as u64,
    ];

    // Realized clamps to i64. The per-row totals in the report are
    // i128 to absorb an additive future `realized_pnl_lamports: i64`
    // field without a report-shape change; today the conservative
    // `e_pnl` is i128, but its magnitude is bounded by what a single
    // trade can pay out, so the clamp is saturating and lossless.
    let realized_pnl_lamports: i64 = row
        .realized_lamports
        .clamp(i64::MIN as i128, i64::MAX as i128) as i64;

    let won = matches!(row.decision, dl_ledger::Decision::WouldTrade);

    CalibrationCapture {
        ts: base_ts.saturating_add(index),
        cycle_seq: row.seq,
        slot: row.seq,
        // The reconcile report does not name the mints (its source
        // is a `.dlg` ledger that records only the cycle hash).
        // Use the source label as a stand-in so the niche selector's
        // mint-prefix checks see a non-empty string and the consumer
        // logs are traceable. A future schema bump carrying the
        // real mints through is a follow-up.
        input_mint: source_label.to_string(),
        output_mint: source_label.to_string(),
        input_amount,
        expected_out_per_leg,
        jito_bundle_id: format!("recon-{}", row.seq),
        realized_pnl_lamports,
        won,
    }
}

/// DAM-64 acceptance bar in code form: replay a synth universe
/// through ledger → reconcile_ledger → JSON → JSON-decode →
/// `captures_from_reconciliation_report` → `fit_with_overfit`.
///
/// Pure function over the synth fixtures; deterministic; no I/O.
/// The four sub-tests below exercise this and the per-row edge
/// cases.
pub fn fit_from_reconciliation_report(
    report: &ReconciliationReport,
    base_ts: i64,
) -> (Vec<CalibrationCapture>, crate::CalibrationReport) {
    let captures = captures_from_reconciliation_report(report, base_ts);
    let fitted = fit_with_overfit(&captures);
    (captures, fitted)
}

// ─── Tests ───────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use dl_ledger::Decision;
    use dl_recon::fixture::{synthesize_pools, SynthPoolSpec};
    use dl_recon::pipeline::{replay_pools_to_ledger, ReplayParams};
    use dl_recon::reconcile::{reconcile_ledger, write_reconciliation_report_json};

    fn synth_specs() -> (Vec<SynthPoolSpec>, Vec<[u8; 32]>) {
        let specs = vec![
            SynthPoolSpec {
                address: [1u8; 32],
                base_reserve: 1_000_000,
                quote_reserve: 1_000_000,
                fee_bps: 30,
            },
            SynthPoolSpec {
                address: [2u8; 32],
                base_reserve: 1_000_000,
                quote_reserve: 1_000_000,
                fee_bps: 30,
            },
            SynthPoolSpec {
                address: [3u8; 32],
                base_reserve: 1_000_000,
                quote_reserve: 1_100_000,
                fee_bps: 30,
            },
        ];
        let mints = vec![[0xaa; 32], [0xbb; 32], [0xcc; 32]];
        (specs, mints)
    }

    /// Round-trip: ledger bytes → ReconciliationReport → JSON →
    /// ReconciliationReport (with matching `report_hash`) →
    /// `Vec<CalibrationCapture>` → `fit_with_overfit`.
    #[test]
    fn end_to_end_ledger_to_calibration_report() {
        let (specs, mints) = synth_specs();
        let pools = synthesize_pools(&specs, &mints);
        let params = ReplayParams::default();
        let report = replay_pools_to_ledger(&pools, &params).expect("replay");
        assert!(!report.cycle_records.is_empty());

        // 1. Write the per-cycle records out as a `.dlg` ledger.
        let mut buf: Vec<u8> = Vec::new();
        {
            let mut w = dl_ledger::LedgerWriter::new(&mut buf).expect("writer");
            for record in &report.cycle_records {
                w.write_entry(&record.entry).expect("write entry");
            }
        }

        // 2. Reconcile the ledger (the DAM-64 surface).
        let recon = reconcile_ledger(buf.as_slice(), "ledger-first-test")
            .expect("reconcile_ledger");
        assert_eq!(recon.rows.len(), report.cycle_records.len());
        assert_eq!(recon.n_traded + recon.n_not_traded, recon.rows.len() as u64);

        // 3. Serialize ↔ deserialize through JSON (the binary's
        //    actual output). The hash must be stable across this
        //    round-trip.
        let mut json_buf: Vec<u8> = Vec::new();
        write_reconciliation_report_json(&recon, &mut json_buf).expect("json");
        let recon_round: ReconciliationReport =
            serde_json::from_slice(&json_buf).expect("json round-trip");
        assert_eq!(recon_round.report_hash, recon.report_hash);
        assert_eq!(recon_round.source_ledger_hash, recon.source_ledger_hash);

        // 4. Consume via the new calibration bridge + fit.
        let (captures, cal) = fit_from_reconciliation_report(&recon_round, 1_000_000);
        assert_eq!(captures.len(), recon.rows.len());
        // Order is preserved: seq is the natural sort key.
        for (i, c) in captures.iter().enumerate() {
            assert_eq!(c.cycle_seq, recon.rows[i].seq);
        }
        assert_eq!(cal.result.sample_size as usize, captures.len());
        // The overfit guard runs the standard checks; we don't
        // assert on `is_overfit_risk` because the synth universe
        // is too small to satisfy MIN_SAMPLES_FOR_FIT and the
        // guard intentionally flags it.
    }

    /// `captures_from_reconciliation_report` is a pure function of
    /// `(report, base_ts)`. Same input → same output, byte-equal.
    /// Different `base_ts` changes only the `ts` field.
    #[test]
    fn captures_from_reconciliation_report_is_pure() {
        let (specs, mints) = synth_specs();
        let pools = synthesize_pools(&specs, &mints);
        let params = ReplayParams::default();
        let report = replay_pools_to_ledger(&pools, &params).expect("replay");
        let mut buf: Vec<u8> = Vec::new();
        {
            let mut w = dl_ledger::LedgerWriter::new(&mut buf).expect("writer");
            for record in &report.cycle_records {
                w.write_entry(&record.entry).expect("write");
            }
        }
        let recon = reconcile_ledger(buf.as_slice(), "purity").expect("reconcile");
        let a = captures_from_reconciliation_report(&recon, 1_000_000);
        let b = captures_from_reconciliation_report(&recon, 1_000_000);
        assert_eq!(a, b);
        let c = captures_from_reconciliation_report(&recon, 2_000_000);
        assert_eq!(a.len(), c.len());
        for (x, y) in a.iter().zip(c.iter()) {
            assert_ne!(x.ts, y.ts);
            assert_eq!(x.cycle_seq, y.cycle_seq);
            assert_eq!(x.realized_pnl_lamports, y.realized_pnl_lamports);
            assert_eq!(x.won, y.won);
        }
    }

    /// Empty report → empty captures, no panic. `fit_with_overfit`
    /// returns the Laplace 0.5 default.
    #[test]
    fn captures_from_empty_reconciliation_report() {
        let report = ReconciliationReport {
            source_label: "empty".to_string(),
            rows: Vec::new(),
            total_predicted_lamports: 0,
            total_realized_lamports: 0,
            total_delta_lamports: 0,
            total_tip_lamports: 0,
            n_traded: 0,
            n_not_traded: 0,
            source_ledger_hash: 0,
            report_hash: 0,
        };
        let caps = captures_from_reconciliation_report(&report, 0);
        assert!(caps.is_empty());
        let cal = fit_with_overfit(&caps);
        // Laplace-smoothed 0/2 == 0.5.
        assert_eq!(cal.result.p_detect.to_ppm(), 500_000);
        assert_eq!(cal.result.p_win.to_ppm(), 500_000);
        assert_eq!(cal.result.p_land.to_ppm(), 500_000);
    }

    /// `won` flag is driven by `Decision::WouldTrade`. `Decision`
    /// is the trade-gate verdict the paper path records in the
    /// ledger's `conservative.e_pnl > 0` rule.
    #[test]
    fn captures_won_flag_matches_decision() {
        let row_trade = ReconRow {
            seq: 0,
            cycle_hash: 1,
            predicted_lamports: 1_000,
            realized_lamports: 500,
            delta_lamports: 500,
            decision: Decision::WouldTrade,
            tip_lamports: 10,
        };
        let row_skip = ReconRow {
            decision: Decision::WouldNotTrade,
            ..row_trade.clone()
        };
        let report = ReconciliationReport {
            source_label: "dec".to_string(),
            rows: vec![row_trade, row_skip],
            total_predicted_lamports: 1_000,
            total_realized_lamports: 500,
            total_delta_lamports: 500,
            total_tip_lamports: 10,
            n_traded: 1,
            n_not_traded: 1,
            source_ledger_hash: 0,
            report_hash: 0,
        };
        let caps = captures_from_reconciliation_report(&report, 0);
        assert_eq!(caps.len(), 2);
        assert!(caps[0].won);
        assert!(!caps[1].won);
    }
}
