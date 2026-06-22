//! DAM-64: ledger-first reconciliation.
//!
//! Pure offline transform: read a `.dlg` paper ledger end-to-end and
//! emit a per-cycle predicted-vs-realized PnL report (in lamports).
//! The report is JSON-serializable so `dl-recon`'s
//! `emit-reconciliation` binary can dump it to disk and
//! `dl-calibration::captures_from_reconciliation_report` can consume
//! it without going through bincode.
//!
//! ## Predicted vs realized — the mapping
//!
//! Each [`LedgerEntry`] (schema v3) carries two EV bounds:
//!
//! - `optimistic.e_pnl` — the "predicted" EV. Computed under
//!   `p_detect = p_win = p_land = 1.0` and no failed-cost haircut.
//!   This is the EV the *uninformed* model would quote for the
//!   cycle.
//! - `conservative.e_pnl` — the "realized-aware" EV. The trade gate
//!   is `conservative.e_pnl > 0`. In the paper-trading path this
//!   bound *is* the realized PnL the bot would have booked, because
//!   no on-chain delta exists to compare against — the conservative
//!   model is the best available stand-in.
//!
//! The `delta` field is `predicted - realized`, i.e.
//! `optimistic.e_pnl - conservative.e_pnl`. A large positive delta
//! means the optimistic bound is loose; a small delta means the two
//! bounds agree. This is the same sign convention
//! `dl-calibration::reconcile` uses (predicted_pnl - realized_pnl).
//!
//! ## Extension point for live on-chain realization
//!
//! When the executor / dl-assert path starts writing a per-cycle
//! `realized_pnl_lamports: i64` field into `LedgerEntry` (schema v4,
//! tracked as a follow-up child issue), this module picks it up
//! without a report-shape change: the per-row `realized_lamports`
//! field will take the on-chain value verbatim. The report struct
//! already uses `i128` for the realized field to absorb both
//! sources.
//!
//! ## Integer-only
//!
//! No `f32` or `f64` in any value path. Hashing uses FNV-1a 64,
//! matching `dl-recon::pipeline::hash_records` for consistency.
//!
//! ## Determinism
//!
//! `reconcile_ledger` is a pure function of the ledger bytes. Two
//! calls with the same ledger produce the same `report_hash`
//! (invariant I-1). Any change to a row's observable fields changes
//! the hash (invariant I-6).
//!
//! ## Crate surface
//!
//! - [`ReconciliationReport`]: aggregate output.
//! - [`ReconRow`]: one row per ledger entry.
//! - [`reconcile_ledger`]: read a `Read` source, return a
//!   [`ReconciliationReport`].
//! - [`write_reconciliation_report_json`]: serialize a report to
//!   pretty JSON (used by the binary and by tests).

#![deny(unsafe_code)]

use std::io::{Read, Write};

use dl_ledger::{Decision, LedgerReader};
use serde::{Deserialize, Serialize};

use crate::error::ReconError;

// ---------------------------------------------------------------------------
// FNV-1a 64 (mirrors `dl-recon::pipeline::hash_records`)
// ---------------------------------------------------------------------------

const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

// ---------------------------------------------------------------------------
// Report shape
// ---------------------------------------------------------------------------

/// One row in the reconciliation report. Mirrors a single
/// `LedgerEntry` (schema v3) projected to predicted-vs-realized
/// fields. `i128` for the lamports fields to absorb the future
/// `realized_pnl_lamports: i64` field without a report-shape change.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReconRow {
    /// Sequence number from the source ledger (monotonic, starts at
    /// 0). `seq` is the natural key the calibration crate
    /// aggregates by.
    pub seq: u64,
    /// Cycle hash from the source ledger. Stable across replay
    /// (FNV-1a 64 over the cycle's leg sequence).
    pub cycle_hash: u64,
    /// Optimistic-bound `e_pnl` from the source ledger. This is the
    /// "predicted" PnL the un-haircut EV model quotes.
    pub predicted_lamports: i128,
    /// Realized-aware `e_pnl` from the source ledger. In the
    /// paper-trading path this is the conservative bound; on the
    /// live path this will be the on-chain delta. (See module docs.)
    pub realized_lamports: i128,
    /// `predicted - realized`, signed. A large positive value
    /// indicates the optimistic bound is loose; a small value
    /// indicates the two bounds agree.
    pub delta_lamports: i128,
    /// Trade gate derived from the conservative bound: `WouldTrade`
    /// iff `conservative.e_pnl > 0`.
    pub decision: Decision,
    /// Per-cycle Jito tip in lamports (schema v3).
    pub tip_lamports: u64,
}

/// Aggregate reconciliation report over a single ledger.
///
/// Totals are signed `i128` so a losing session doesn't overflow a
/// `u64`. `report_hash` is FNV-1a 64 over the bincode of `rows`;
/// `source_ledger_hash` is FNV-1a 64 over the entry `(seq,
/// cycle_hash)` pairs (a stable fingerprint of which ledger was
/// read, independent of the projection).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReconciliationReport {
    /// Source ledger path (informational; not part of the hash).
    pub source_label: String,
    /// One row per ledger entry, ordered by `seq`.
    pub rows: Vec<ReconRow>,
    /// Sum of `rows[i].predicted_lamports`.
    pub total_predicted_lamports: i128,
    /// Sum of `rows[i].realized_lamports`.
    pub total_realized_lamports: i128,
    /// `total_predicted - total_realized`, signed.
    pub total_delta_lamports: i128,
    /// Sum of `rows[i].tip_lamports`.
    pub total_tip_lamports: u64,
    /// Count of `WouldTrade` rows.
    pub n_traded: u64,
    /// Count of `WouldNotTrade` rows.
    pub n_not_traded: u64,
    /// FNV-1a 64 over the entry `(seq, cycle_hash)` pairs.
    pub source_ledger_hash: u64,
    /// FNV-1a 64 over the bincode of `rows`.
    pub report_hash: u64,
}

impl ReconciliationReport {
    /// Number of rows in this report.
    pub fn len(&self) -> usize {
        self.rows.len()
    }

    /// True iff no entries were read.
    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }
}

// ---------------------------------------------------------------------------
// Core entry point
// ---------------------------------------------------------------------------

/// Read a `dl-ledger` paper ledger from `source` and return a
/// [`ReconciliationReport`]. The `Read` source is consumed to EOF
/// (per dl-ledger's invariant I-5: no terminator frame, EOF is
/// clean). `source_label` is recorded on the report for operator
/// audit; it is not part of `report_hash`.
///
/// Pure function of the ledger bytes: two calls with the same
/// source return equal `ReconciliationReport`s.
pub fn reconcile_ledger<R: Read>(
    source: R,
    source_label: impl Into<String>,
) -> Result<ReconciliationReport, ReconError> {
    let source_label = source_label.into();
    let mut reader = LedgerReader::open(source)?;
    let mut rows: Vec<ReconRow> = Vec::new();
    let mut source_hash = FNV_OFFSET;
    let mut total_predicted: i128 = 0;
    let mut total_realized: i128 = 0;
    let mut total_tip: u64 = 0;
    let mut n_traded: u64 = 0;
    let mut n_not_traded: u64 = 0;

    while let Some(entry) = reader.read_entry()? {
        // Mix seq + cycle_hash into the source fingerprint.
        for byte in entry.seq.to_le_bytes() {
            source_hash ^= byte as u64;
            source_hash = source_hash.wrapping_mul(FNV_PRIME);
        }
        for byte in entry.cycle_hash.0.to_le_bytes() {
            source_hash ^= byte as u64;
            source_hash = source_hash.wrapping_mul(FNV_PRIME);
        }

        let predicted = entry.optimistic.e_pnl;
        let realized = entry.conservative.e_pnl;
        let delta = predicted.saturating_sub(realized);
        total_predicted = total_predicted.saturating_add(predicted);
        total_realized = total_realized.saturating_add(realized);
        total_tip = total_tip.saturating_add(entry.tip_lamports);
        match entry.decision {
            Decision::WouldTrade => n_traded = n_traded.saturating_add(1),
            Decision::WouldNotTrade => n_not_traded = n_not_traded.saturating_add(1),
        }

        rows.push(ReconRow {
            seq: entry.seq,
            cycle_hash: entry.cycle_hash.0,
            predicted_lamports: predicted,
            realized_lamports: realized,
            delta_lamports: delta,
            decision: entry.decision,
            tip_lamports: entry.tip_lamports,
        });
    }

    let report_hash = {
        let mut h = FNV_OFFSET;
        for row in &rows {
            let bytes = bincode::serialize(row).expect("ReconRow bincode");
            for byte in bytes {
                h ^= byte as u64;
                h = h.wrapping_mul(FNV_PRIME);
            }
        }
        h
    };

    Ok(ReconciliationReport {
        source_label,
        rows,
        total_predicted_lamports: total_predicted,
        total_realized_lamports: total_realized,
        total_delta_lamports: total_predicted.saturating_sub(total_realized),
        total_tip_lamports: total_tip,
        n_traded,
        n_not_traded,
        source_ledger_hash: source_hash,
        report_hash,
    })
}

// ---------------------------------------------------------------------------
// JSON helpers
// ---------------------------------------------------------------------------

/// Serialize a [`ReconciliationReport`] as pretty JSON. Integer-only
/// fields throughout — no `f64` ever appears in the output. Used by
/// the `emit-reconciliation` binary and by tests.
pub fn write_reconciliation_report_json<W: Write>(
    report: &ReconciliationReport,
    mut sink: W,
) -> Result<(), ReconError> {
    let json = serde_json::to_string_pretty(report)
        .map_err(|e| ReconError::Json(format!("reconciliation report serialize: {e}")))?;
    sink.write_all(json.as_bytes())?;
    sink.write_all(b"\n")?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixture::{ReconFixture, SynthPoolSpec};
    use crate::pipeline::ReplayParams;
    use dl_core::prob::PROB_SCALE_1E18;
    use dl_ledger::hash::LedgerHash;
    use dl_sim::cost::CostBreakdown;
    use dl_sim::ev::{ExpectedValue, Prob};
    use dl_sim::net_profit::NetProfit;
    use dl_state::cycle::{Cycle, Direction, Leg};
    use dl_state::Pubkey;

    fn one_leg(pool_byte: u8, dir: Direction) -> Leg {
        let mut pk = [0u8; 32];
        pk[31] = pool_byte;
        Leg {
            pool: Pubkey(pk),
            direction: dir,
            weight: 0,
        }
    }

    fn two_leg_cycle() -> Cycle {
        Cycle::new(vec![
            one_leg(1, Direction::BaseToQuote),
            one_leg(2, Direction::QuoteToBase),
        ])
    }

    fn zero_net(p: i128) -> NetProfit {
        NetProfit {
            input_amount: 1,
            gross_output: 0,
            total_costs: CostBreakdown {
                base_sig_fee_lamports: 0,
                priority_fee_lamports: 0,
                jito_tip_lamports: 0,
                jito_tip_fee_lamports: 0,
                total_lamports: 0,
            },
            net_profit: p,
            net_profit_bps: 0,
            profitable: p > 0,
        }
    }

    fn zero_ev(p: i128) -> ExpectedValue {
        ExpectedValue {
            e_pnl: p,
            p_detect: Prob::from_scaled_clamped(PROB_SCALE_1E18),
            p_win: Prob::from_scaled_clamped(PROB_SCALE_1E18),
            p_land: Prob::from_scaled_clamped(PROB_SCALE_1E18),
            expected_failed_cost: 0,
            tip_lamports: 0,
        }
    }

    fn ledger_bytes_with_outcomes(outcomes: &[(i128, i128, u64)]) -> Vec<u8> {
        // (predicted, realized, tip)
        let cycle = two_leg_cycle();
        let cycle_hash = LedgerHash::from_cycle(&cycle);
        let mut buf = Vec::new();
        {
            let mut w = dl_ledger::LedgerWriter::new(&mut buf).expect("writer open");
            for (i, (predicted, realized, tip)) in outcomes.iter().enumerate() {
                let net = zero_net(*predicted);
                let entry = dl_ledger::LedgerEntry {
                    seq: i as u64,
                    entry_id: i as u64,
                    cycle_hash,
                    net,
                    optimistic: zero_ev(*predicted),
                    conservative: zero_ev(*realized),
                    decision: if *realized > 0 {
                        Decision::WouldTrade
                    } else {
                        Decision::WouldNotTrade
                    },
                    tip_lamports: *tip,
                };
                w.write_entry(&entry).expect("write entry");
            }
        }
        buf
    }

    #[test]
    fn empty_ledger_yields_empty_report() {
        let buf = ledger_bytes_with_outcomes(&[]);
        let report = reconcile_ledger(buf.as_slice(), "empty").expect("reconcile");
        assert!(report.is_empty());
        assert_eq!(report.n_traded, 0);
        assert_eq!(report.n_not_traded, 0);
        assert_eq!(report.total_predicted_lamports, 0);
        assert_eq!(report.total_realized_lamports, 0);
        assert_eq!(report.total_delta_lamports, 0);
        // Empty rows: FNV offset is the identity. Locks the empty
        // hash so future readers can detect a degenerate case.
        assert_eq!(report.report_hash, FNV_OFFSET);
    }

    #[test]
    fn mixed_ledger_totals() {
        let buf = ledger_bytes_with_outcomes(&[
            (1000, 500, 10), // WouldTrade (realized > 0)
            (-200, -300, 0), // WouldNotTrade
            (2000, 1500, 5), // WouldTrade
            (-50, -50, 0),   // WouldNotTrade (realized == 0)
        ]);
        let report = reconcile_ledger(buf.as_slice(), "mixed").expect("reconcile");
        assert_eq!(report.rows.len(), 4);
        assert_eq!(report.n_traded, 2);
        assert_eq!(report.n_not_traded, 2);
        assert_eq!(report.total_predicted_lamports, 1000 - 200 + 2000 - 50);
        assert_eq!(report.total_realized_lamports, 500 - 300 + 1500 - 50);
        assert_eq!(
            report.total_delta_lamports,
            report.total_predicted_lamports - report.total_realized_lamports
        );
        assert_eq!(report.total_tip_lamports, 15);
        // delta == predicted - realized, per row.
        for row in &report.rows {
            assert_eq!(
                row.delta_lamports,
                row.predicted_lamports - row.realized_lamports
            );
        }
    }

    #[test]
    fn same_ledger_same_report_hash() {
        let buf = ledger_bytes_with_outcomes(&[(100, 50, 1), (-10, -20, 0)]);
        let a = reconcile_ledger(buf.as_slice(), "x").expect("reconcile a");
        let b = reconcile_ledger(buf.as_slice(), "x").expect("reconcile b");
        assert_eq!(a, b);
        assert_eq!(a.report_hash, b.report_hash);
        assert_eq!(a.source_ledger_hash, b.source_ledger_hash);
    }

    #[test]
    fn different_ledger_different_report_hash() {
        let buf_a = ledger_bytes_with_outcomes(&[(100, 50, 1)]);
        let buf_b = ledger_bytes_with_outcomes(&[(200, 50, 1)]);
        let a = reconcile_ledger(buf_a.as_slice(), "a").expect("a");
        let b = reconcile_ledger(buf_b.as_slice(), "b").expect("b");
        assert_ne!(a.report_hash, b.report_hash);
    }

    #[test]
    fn source_label_does_not_affect_report_hash() {
        let buf = ledger_bytes_with_outcomes(&[(100, 50, 1)]);
        let a = reconcile_ledger(buf.as_slice(), "label-a").expect("a");
        let b = reconcile_ledger(buf.as_slice(), "label-b").expect("b");
        // The label is informational only; the report hash is over
        // the bincode of `rows`.
        assert_eq!(a.report_hash, b.report_hash);
        assert_eq!(a.source_ledger_hash, b.source_ledger_hash);
        assert_ne!(a.source_label, b.source_label);
    }

    #[test]
    fn json_round_trips_via_serde_json() {
        let buf = ledger_bytes_with_outcomes(&[(100, 50, 1), (-20, -30, 0)]);
        let report = reconcile_ledger(buf.as_slice(), "rt").expect("reconcile");
        let json = serde_json::to_string(&report).expect("to_string");
        let parsed: ReconciliationReport = serde_json::from_str(&json).expect("from_str");
        assert_eq!(parsed, report);
    }

    #[test]
    fn write_reconciliation_report_json_emits_valid_json() {
        let buf = ledger_bytes_with_outcomes(&[(100, 50, 1)]);
        let report = reconcile_ledger(buf.as_slice(), "json-out").expect("reconcile");
        let mut sink: Vec<u8> = Vec::new();
        write_reconciliation_report_json(&report, &mut sink).expect("write");
        let text = String::from_utf8(sink).expect("utf8");
        // Smoke test: top-level keys are present in the pretty JSON.
        assert!(text.contains("\"rows\""));
        assert!(text.contains("\"total_predicted_lamports\""));
        assert!(text.contains("\"total_realized_lamports\""));
        assert!(text.contains("\"total_delta_lamports\""));
        assert!(text.contains("\"total_tip_lamports\""));
        assert!(text.contains("\"n_traded\""));
        assert!(text.contains("\"n_not_traded\""));
        assert!(text.contains("\"source_ledger_hash\""));
        assert!(text.contains("\"report_hash\""));
        // And it's parseable back into the same report.
        let parsed: ReconciliationReport = serde_json::from_str(text.trim()).expect("parse back");
        assert_eq!(parsed, report);
    }

    #[test]
    fn reconciles_synthesized_ledger() {
        // End-to-end: synthesize a small ledger via the recon
        // fixture, run `reconcile_ledger` on the resulting bytes,
        // and verify the report is internally consistent.
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
        let fx = ReconFixture::build(&specs, &mints, &ReplayParams::default());
        let report = reconcile_ledger(fx.ledger.as_slice(), "synth").expect("reconcile");
        let report2 = reconcile_ledger(fx.ledger.as_slice(), "synth").expect("reconcile 2");
        assert_eq!(report.report_hash, report2.report_hash);
        // Totals are internally consistent.
        let sum_pred: i128 = report.rows.iter().map(|r| r.predicted_lamports).sum();
        let sum_real: i128 = report.rows.iter().map(|r| r.realized_lamports).sum();
        assert_eq!(report.total_predicted_lamports, sum_pred);
        assert_eq!(report.total_realized_lamports, sum_real);
        assert_eq!(
            report.n_traded + report.n_not_traded,
            report.rows.len() as u64
        );
    }

    /// Acceptance: invoke the `emit-reconciliation` binary against
    /// a synthesized ledger and verify the JSON it produces. This
    /// is the DAM-64 acceptance bar end-to-end. Lives in the lib
    /// tests (rather than `tests/emit_reconciliation.rs`) so it
    /// doesn't need a separate integration test directory and
    /// doesn't re-trigger dl-feed compilation.
    #[test]
    fn binary_emit_reconciliation_end_to_end() {
        // 1. Synthesize a small .dlg ledger on disk.
        let tmpdir = std::env::temp_dir().join("dl-recon-binary-acceptance");
        std::fs::create_dir_all(&tmpdir).expect("mkdir tmp");
        let ledger_path = tmpdir.join("acceptance.dlg");
        if ledger_path.exists() {
            std::fs::remove_file(&ledger_path).expect("remove old");
        }
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
        let fx = ReconFixture::build(&specs, &mints, &ReplayParams::default());
        std::fs::write(&ledger_path, &fx.ledger).expect("write ledger");

        // 2. Resolve the binary path. `CARGO_BIN_EXE_<name>` is set
        // by Cargo for the integration-test harness; for unit tests
        // in src/, fall back to the debug target path.
        let bin = option_env!("CARGO_BIN_EXE_emit-reconciliation")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| {
                std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                    .join("../../target/debug/emit-reconciliation")
            });

        // 3. Run the binary.
        let out = std::process::Command::new(&bin)
            .arg("--ledger")
            .arg(&ledger_path)
            .arg("--out")
            .arg("-")
            .output()
            .expect("spawn emit-reconciliation");
        assert!(
            out.status.success(),
            "emit-reconciliation failed: stderr={}",
            String::from_utf8_lossy(&out.stderr)
        );
        let json = String::from_utf8(out.stdout).expect("utf8 stdout");

        // 4. Parse the JSON report and assert shape.
        let report: ReconciliationReport =
            serde_json::from_str(json.trim()).expect("parse json report");
        assert_eq!(report.source_label, ledger_path.display().to_string());
        let sum_pred: i128 = report.rows.iter().map(|r| r.predicted_lamports).sum();
        let sum_real: i128 = report.rows.iter().map(|r| r.realized_lamports).sum();
        assert_eq!(report.total_predicted_lamports, sum_pred);
        assert_eq!(report.total_realized_lamports, sum_real);
        // report_hash is stable across runs.
        let json2 = std::process::Command::new(&bin)
            .arg("--ledger")
            .arg(&ledger_path)
            .arg("--out")
            .arg("-")
            .output()
            .expect("spawn 2");
        let report2: ReconciliationReport = serde_json::from_str(
            String::from_utf8(json2.stdout).unwrap().trim(),
        )
        .expect("parse 2");
        assert_eq!(report.report_hash, report2.report_hash);

        // 5. Cleanup.
        let _ = std::fs::remove_dir_all(&tmpdir);
    }
}
