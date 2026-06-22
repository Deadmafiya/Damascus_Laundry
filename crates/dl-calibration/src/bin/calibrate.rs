//! `dl-calibrate` — Phase 2c binary.
//!
//! Reads the JSONL calibration log, fits `p_detect / p_win / p_land`,
//! runs the overfit-guard check, and writes `calibration.json`.
//!
//! Usage:
//!   cargo run --release -p dl-calibration --bin dl-calibrate -- \
//!       --captures ./dl-calibration/captures.jsonl \
//!       --out ./dl-calibration/calibration.json
//!
//! DAM-64 (gated by `--features dam64`):
//!   cargo run --release -p dl-calibration --features dam64 --bin dl-calibrate -- \
//!       --from-reconciliation ./recon.json \
//!       --out ./dl-calibration/calibration.json
//!
//! The `--from-reconciliation` flag consumes the JSON report
//! produced by `cargo run -p dl-recon -- emit-reconciliation
//! --ledger <path> --out <path>` and re-fits the calibration
//! probabilities from the reconciled rows. This is the operator's
//! daily-cadence handoff: `emit-reconciliation | dl-calibrate`.

use std::path::PathBuf;
use std::process::ExitCode;

use dl_calibration::{
    fit_with_overfit, read_jsonl, write_calibration_report, MIN_SAMPLES_FOR_FIT,
};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    let mut captures_path: Option<PathBuf> = None;
    let mut out_path = PathBuf::from("./dl-calibration/calibration.json");
    let mut from_reconciliation: Option<PathBuf> = None;
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--captures" => {
                if let Some(v) = args.get(i + 1) {
                    captures_path = Some(PathBuf::from(v));
                }
                i += 2;
            }
            "--out" => {
                if let Some(v) = args.get(i + 1) {
                    out_path = PathBuf::from(v);
                }
                i += 2;
            }
            "--from-reconciliation" => {
                if let Some(v) = args.get(i + 1) {
                    from_reconciliation = Some(PathBuf::from(v));
                }
                i += 2;
            }
            _ => i += 1,
        }
    }

    // DAM-64 path: consume a `ReconciliationReport` JSON file and
    // map it to captures before fitting. Gated on the `dam64` feature
    // so the default build still compiles.
    #[cfg(feature = "dam64")]
    if let Some(recon_path) = from_reconciliation {
        return run_from_reconciliation(&recon_path, &out_path);
    }
    #[cfg(not(feature = "dam64"))]
    if from_reconciliation.is_some() {
        eprintln!(
            "dl-calibrate: --from-reconciliation requires the `dam64` feature; \
             rebuild with `cargo build -p dl-calibration --features dam64`"
        );
        return ExitCode::from(2);
    }

    let captures_path = captures_path
        .unwrap_or_else(|| PathBuf::from("./dl-calibration/captures.jsonl"));
    let caps = read_jsonl(&captures_path);
    eprintln!(
        "dl-calibrate: read {} captures from {}",
        caps.len(),
        captures_path.display()
    );
    let report: dl_calibration::CalibrationReport = fit_with_overfit(&caps);
    let cal: &dl_calibration::CalibrationResult = &report.result;
    let guard: &dl_calibration::OverfitReport = &report.overfit;
    let min_samples: u64 = MIN_SAMPLES_FOR_FIT as u64;
    let cold_start = (caps.len() as u64) < min_samples;
    if cold_start {
        eprintln!(
            "dl-calibrate: WARNING: sample_size {} < {}; fit defaults to Laplace (0.5)",
            caps.len(), min_samples
        );
    }
    match write_calibration_report(&report, &out_path) {
        Ok(()) => {
            println!(
                "dl-calibrate: wrote {} (p_detect={} p_win={} p_land={} n={} dsr={:?} cv={:?} overfit_risk={})",
                out_path.display(),
                cal.p_detect.to_ppm(),
                cal.p_win.to_ppm(),
                cal.p_land.to_ppm(),
                cal.sample_size,
                guard.dsr.as_ref().map(|d| d.dsr),
                guard.purged_cv.as_ref().map(|c| (c.n_folds, c.mean_oos_sharpe)),
                guard.is_overfit_risk,
            );
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("dl-calibrate: write failed: {e}");
            ExitCode::from(1)
        }
    }
}

/// DAM-64 handler. Reads a `ReconciliationReport` JSON file, maps
/// it to `CalibrationCapture` rows, fits, and writes the report.
#[cfg(feature = "dam64")]
fn run_from_reconciliation(recon_path: &PathBuf, out_path: &PathBuf) -> ExitCode {
    use dl_calibration::captures_from_reconciliation_report;
    let raw = match std::fs::read_to_string(recon_path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!(
                "dl-calibrate: read {} failed: {e}",
                recon_path.display()
            );
            return ExitCode::from(1);
        }
    };
    let report: dl_recon::reconcile::ReconciliationReport = match serde_json::from_str(&raw) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("dl-calibrate: parse {} failed: {e}", recon_path.display());
            return ExitCode::from(1);
        }
    };
    // base_ts: use the report's own rows (they're already ts-ordered);
    // fall back to 0 for empty reports. The ts of the first row is
    // the run's effective start.
    let base_ts: i64 = report
        .rows
        .first()
        .map(|r| r.seq as i64)
        .unwrap_or(0);
    let caps = captures_from_reconciliation_report(&report, base_ts);
    eprintln!(
        "dl-calibrate: read {} rows from {} (source_label={})",
        caps.len(),
        recon_path.display(),
        report.source_label
    );
    let fitted = fit_with_overfit(&caps);
    let cal: &dl_calibration::CalibrationResult = &fitted.result;
    let guard: &dl_calibration::OverfitReport = &fitted.overfit;
    match write_calibration_report(&fitted, out_path) {
        Ok(()) => {
            println!(
                "dl-calibrate: wrote {} (p_detect={} p_win={} p_land={} n={} dsr={:?} cv={:?} overfit_risk={})",
                out_path.display(),
                cal.p_detect.to_ppm(),
                cal.p_win.to_ppm(),
                cal.p_land.to_ppm(),
                cal.sample_size,
                guard.dsr.as_ref().map(|d| d.dsr),
                guard.purged_cv.as_ref().map(|c| (c.n_folds, c.mean_oos_sharpe)),
                guard.is_overfit_risk,
            );
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("dl-calibrate: write failed: {e}");
            ExitCode::from(1)
        }
    }
}
