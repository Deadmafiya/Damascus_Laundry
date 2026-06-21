//! `calibrate` — DAM-35: end-to-end calibration binary.

use std::fs::File;
use std::io::BufReader;
use std::path::PathBuf;
use std::process::ExitCode;

use dl_calibration::{
    fit_from_capture, fit_with_overfit, read_jsonl, write_calibration_report, MIN_SAMPLES_FOR_FIT,
};
use dl_recon::pipeline::ReplayParams;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    let mut captures_path = PathBuf::from("./dl-calibration/captures.jsonl");
    let mut out_path = PathBuf::from("./dl-calibration/calibration.json");
    let mut from_capture_path: Option<PathBuf> = None;
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--captures" => { if let Some(v) = args.get(i + 1) { captures_path = PathBuf::from(v); } i += 2; }
            "--out" => { if let Some(v) = args.get(i + 1) { out_path = PathBuf::from(v); } i += 2; }
            "--from-capture" => { if let Some(v) = args.get(i + 1) { from_capture_path = Some(PathBuf::from(v)); } i += 2; }
            _ => i += 1,
        }
    }
    if let Some(cap_path) = from_capture_path {
        return run_from_capture(&cap_path, &out_path);
    }
    run_from_captures(&captures_path, &out_path)
}

fn run_from_captures(captures_path: &PathBuf, out_path: &PathBuf) -> ExitCode {
    let caps = read_jsonl(captures_path);
    eprintln!("calibrate: read {} captures from {}", caps.len(), captures_path.display());
    let report: dl_calibration::CalibrationReport = fit_with_overfit(&caps);
    let cal = &report.result;
    let guard = &report.overfit;
    let min_samples: u64 = MIN_SAMPLES_FOR_FIT as u64;
    let cold_start = (caps.len() as u64) < min_samples;
    if cold_start {
        eprintln!("calibrate: WARNING: sample_size {} < {}; fit defaults to Laplace (0.5)", caps.len(), min_samples);
    }
    match write_calibration_report(&report, out_path) {
        Ok(()) => {
            println!("calibrate: wrote {} (p_detect={} p_win={} p_land={} n={} dsr={:?} pbo={:?} cv={:?} overfit_risk={})",
                out_path.display(), cal.p_detect.to_ppm(), cal.p_win.to_ppm(), cal.p_land.to_ppm(),
                cal.sample_size,
                guard.dsr.as_ref().map(|d| d.dsr),
                guard.pbo.as_ref().map(|p| (p.n_configs, p.pbo)),
                guard.purged_cv.as_ref().map(|c| (c.n_folds, c.mean_oos_sharpe)),
                guard.is_overfit_risk);
            ExitCode::SUCCESS
        }
        Err(e) => { eprintln!("calibrate: write failed: {e}"); ExitCode::from(1) }
    }
}

fn run_from_capture(capture_path: &PathBuf, out_path: &PathBuf) -> ExitCode {
    let file = match File::open(capture_path) {
        Ok(f) => f,
        Err(e) => { eprintln!("calibrate: failed to open capture {}: {e}", capture_path.display()); return ExitCode::from(1); }
    };
    let reader = BufReader::new(file);
    let params = ReplayParams::default();
    let base_ts: i64 = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
    match fit_from_capture(reader, &params, base_ts, out_path) {
        Ok(report) => {
            let cal = &report.result;
            let guard = &report.overfit;
            println!("calibrate: wrote {} from capture {} (p_detect={} p_win={} p_land={} n={} dsr={:?} pbo={:?} cv={:?} overfit_risk={})",
                out_path.display(), capture_path.display(),
                cal.p_detect.to_ppm(), cal.p_win.to_ppm(), cal.p_land.to_ppm(),
                cal.sample_size,
                guard.dsr.as_ref().map(|d| d.dsr),
                guard.pbo.as_ref().map(|p| (p.n_configs, p.pbo)),
                guard.purged_cv.as_ref().map(|c| (c.n_folds, c.mean_oos_sharpe)),
                guard.is_overfit_risk);
            ExitCode::SUCCESS
        }
        Err(e) => { eprintln!("calibrate: from-capture failed: {e}"); ExitCode::from(1) }
    }
}
