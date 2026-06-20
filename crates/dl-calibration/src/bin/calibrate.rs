//! `dl-calibrate` — Phase 2c binary.
//!
//! Reads the JSONL calibration log, fits `p_detect / p_win / p_land`,
//! runs the overfit-guard check, and writes `calibration.json`.
//!
//! Usage:
//!   cargo run --release -p dl-calibration --bin dl-calibrate -- \
//!       --captures ./dl-calibration/captures.jsonl \
//!       --out ./dl-calibration/calibration.json

use std::path::PathBuf;
use std::process::ExitCode;

use dl_calibration::{
    fit_with_overfit, read_jsonl, write_calibration_report, MIN_SAMPLES_FOR_FIT,
};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    let mut captures_path = PathBuf::from("./dl-calibration/captures.jsonl");
    let mut out_path = PathBuf::from("./dl-calibration/calibration.json");
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--captures" => {
                if let Some(v) = args.get(i + 1) {
                    captures_path = PathBuf::from(v);
                }
                i += 2;
            }
            "--out" => {
                if let Some(v) = args.get(i + 1) {
                    out_path = PathBuf::from(v);
                }
                i += 2;
            }
            _ => i += 1,
        }
    }

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
