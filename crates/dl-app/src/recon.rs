//! `dl-app recon` subcommand (Phase 6, plan 02).
//!
//! Reads a capture file, runs the recon pipeline, compares against an
//! anchor dataset, optionally calibrates `EvalParams`, and prints a
//! human-readable report.
//!
//! Usage:
//!   dl-app recon --capture <path> --anchors <path.jsonl> [--calibrate]
//!
//! Exit codes:
//!   0 — clean run, all anchors within tolerance
//!   1 — at least one anchor exceeds tolerance
//!   2 — runtime error (file missing, decode failure, etc.)

use std::env;
use std::io::{Read, Write};
use std::path::Path;
use std::process::ExitCode;

use dl_recon::onchain::{reconcile, AnchorDataset, AnchorDivergence, AnchorName};
use dl_recon::pipeline::{replay_capture_to_ledger, ReplayParams};
use dl_sim::ev::EvalParams;
use tracing::{info, warn};

use crate::init_tracing;

/// Result of running `dl-app recon`.
#[derive(Debug)]
pub enum ReconCliResult {
    Ok,
    Divergences(Vec<AnchorDivergence>),
    Error(String),
}

pub fn run(args: &[String]) -> ReconCliResult {
    let opts = match parse_args(args) {
        Ok(o) => o,
        Err(e) => return ReconCliResult::Error(e),
    };

    info!(
        capture = %opts.capture_path,
        anchors = ?opts.anchors_path,
        report_json = ?opts.report_json,
        calibrate = opts.calibrate,
        "starting recon"
    );

    // 1. Open capture.
    let mut file = match std::fs::File::open(&opts.capture_path) {
        Ok(f) => f,
        Err(e) => return ReconCliResult::Error(format!("open capture: {e}")),
    };
    let mut buf = Vec::new();
    if let Err(e) = file.read_to_end(&mut buf) {
        return ReconCliResult::Error(format!("read capture: {e}"));
    }
    let cursor = std::io::Cursor::new(buf);

    // 2. Replay.
    let report = match replay_capture_to_ledger(cursor, &opts.params) {
        Ok(r) => r,
        Err(e) => return ReconCliResult::Error(format!("replay: {e}")),
    };
    info!(
        cycles = report.cycle_records.len(),
        would_trade = report.summary.would_trade(),
        feed_events = report.feed_events_consumed,
        "replay complete"
    );

    // 2b. Optionally dump the report as JSON. The script
    // (`scripts/reproduce_paper_pnl.sh`) uses this to capture
    // the recon outcome without going through the anchor
    // compare step.
    if let Some(ref path) = opts.report_json {
        let json = match serde_json::to_string_pretty(&report) {
            Ok(j) => j,
            Err(e) => {
                return ReconCliResult::Error(format!("report-json serialize: {e}"));
            }
        };
        if let Err(e) = std::fs::write(path, &json) {
            return ReconCliResult::Error(format!("report-json write: {e}"));
        }
        info!(path = %path, "report-json written");
    }

    // 3. Load anchors (optional when --report-json was given).
    if opts.anchors_path.is_none() {
        // No compare step requested; the report is the deliverable.
        return ReconCliResult::Ok;
    }
    let dataset = match AnchorDataset::load_jsonl(Path::new(opts.anchors_path.as_deref().unwrap()))
    {
        Ok(d) => d,
        Err(e) => return ReconCliResult::Error(format!("anchors: {e}")),
    };

    // 4. Compare (and optionally calibrate).
    if opts.calibrate {
        let current = EvalParams::conservative_default();
        match reconcile(&dataset, &report, &current) {
            Ok(fit) => {
                print_calibration_report(&fit);
                let remaining: Vec<AnchorDivergence> = fit
                    .divergences_remaining
                    .into_iter()
                    .filter(|d| d.exceeds_tolerance)
                    .collect();
                if remaining.is_empty() {
                    ReconCliResult::Ok
                } else {
                    ReconCliResult::Divergences(remaining)
                }
            }
            Err(e) => ReconCliResult::Error(format!("calibrate: {e}")),
        }
    } else {
        match dataset.compare(&report) {
            Ok(divs) => {
                print_divergences(&divs);
                let bad: Vec<AnchorDivergence> =
                    divs.into_iter().filter(|d| d.exceeds_tolerance).collect();
                if bad.is_empty() {
                    ReconCliResult::Ok
                } else {
                    ReconCliResult::Divergences(bad)
                }
            }
            Err(e) => ReconCliResult::Error(format!("compare: {e}")),
        }
    }
}

#[derive(Debug)]
struct ReconOpts {
    capture_path: String,
    anchors_path: Option<String>,
    report_json: Option<String>,
    calibrate: bool,
    params: ReplayParams,
}

fn parse_args(args: &[String]) -> Result<ReconOpts, String> {
    let mut capture_path: Option<String> = None;
    let mut anchors_path: Option<String> = None;
    let mut report_json: Option<String> = None;
    let mut calibrate = false;

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--capture" | "-c" => {
                i += 1;
                capture_path = Some(args.get(i).ok_or("--capture: missing value")?.clone());
            }
            "--anchors" | "-a" => {
                i += 1;
                anchors_path = Some(args.get(i).ok_or("--anchors: missing value")?.clone());
            }
            "--report-json" => {
                i += 1;
                report_json = Some(args.get(i).ok_or("--report-json: missing value")?.clone());
            }
            "--calibrate" => calibrate = true,
            "--help" | "-h" => {
                print_help();
                std::process::exit(0);
            }
            other => return Err(format!("unknown arg: {other}")),
        }
        i += 1;
    }

    let capture_path = capture_path.ok_or("--capture <path> is required")?;
    // --anchors is optional when --report-json is supplied
    // (the recon report is written without the compare step).
    if anchors_path.is_none() && report_json.is_none() {
        return Err(
            "--anchors <path.jsonl> is required (or use --report-json to skip the compare step)"
                .to_string(),
        );
    }

    Ok(ReconOpts {
        capture_path,
        anchors_path,
        report_json,
        calibrate,
        params: ReplayParams::default(),
    })
}

fn print_help() {
    eprintln!("dl-app recon — Phase 6 reconciliation harness");
    eprintln!();
    eprintln!("USAGE:");
    eprintln!("    dl-app recon --capture <path> --anchors <path.jsonl> [--calibrate]");
    eprintln!();
    eprintln!("OPTIONS:");
    eprintln!("    -c, --capture <path>     path to a capture file (bincode)");
    eprintln!("    -a, --anchors <path>     path to anchor dataset (JSONL)");
    eprintln!("        --calibrate          fit EvalParams to close divergences");
    eprintln!("    -h, --help               show this help");
}

fn print_divergences(divs: &[AnchorDivergence]) {
    println!();
    println!("anchor divergences ({} entries):", divs.len());
    println!();
    println!(
        "  {:<24}  {:>14}  {:>14}  {:>10}  {:>8}",
        "name", "engine", "anchor", "bps", "tol"
    );
    println!("  {}", "-".repeat(78));
    for d in divs {
        let name = format!("{:?}", d.name);
        println!(
            "  {:<24}  {:>14}  {:>14}  {:>+10}  {:>7}  {}",
            name,
            d.engine_value,
            d.anchor_value,
            d.divergence_bps,
            d.tolerance_bps,
            if d.exceeds_tolerance { "EXCEEDS" } else { "ok" },
        );
    }
}

fn print_calibration_report(fit: &dl_recon::onchain::CalibrationFit) {
    println!();
    println!("calibration fit:");
    println!();
    println!("  adjustment_bps: {}", fit.adjustment_bps);
    println!(
        "  base_win_ppm: {} -> {}",
        fit.input_divergences.len(),
        fit.improved_params.competition.base_win_ppm
    );
    println!();
    println!(
        "  remaining divergences ({}):",
        fit.divergences_remaining.len()
    );
    for d in &fit.divergences_remaining {
        let name = format!("{:?}", d.name);
        println!(
            "    {:<24}  {:>+8} bps  (tol {})",
            name, d.divergence_bps, d.tolerance_bps
        );
    }
}

/// Run `dl-app recon` as a subprocess, returning its exit code.
pub fn dispatch() -> ! {
    let args: Vec<String> = std::env::args().skip(2).collect();
    match run(&args) {
        ReconCliResult::Ok => std::process::exit(0),
        ReconCliResult::Divergences(bad) => {
            for d in bad {
                let _ = writeln!(std::io::stderr(), "{:?}: {} bps", d.name, d.divergence_bps);
            }
            std::process::exit(1);
        }
        ReconCliResult::Error(msg) => {
            eprintln!("recon error: {msg}");
            std::process::exit(2);
        }
    }
}

/// Run `dl-app recon` and return a structured result.
pub fn run_dispatch(args: &[String]) -> ReconCliResult {
    run(args)
}

/// Entry used by tests: the onchain integration test calls this
/// helper to exercise the CLI without spawning a process.
pub fn run_for_test(capture: Vec<u8>, anchors_path: &Path, calibrate: bool) -> ReconCliResult {
    let cursor = std::io::Cursor::new(capture);
    let report = match replay_capture_to_ledger(cursor, &ReplayParams::default()) {
        Ok(r) => r,
        Err(e) => return ReconCliResult::Error(format!("replay: {e}")),
    };
    let dataset = match AnchorDataset::load_jsonl(anchors_path) {
        Ok(d) => d,
        Err(e) => return ReconCliResult::Error(format!("anchors: {e}")),
    };
    if calibrate {
        let current = EvalParams::conservative_default();
        match reconcile(&dataset, &report, &current) {
            Ok(fit) => {
                print_calibration_report(&fit);
                let bad: Vec<AnchorDivergence> = fit
                    .divergences_remaining
                    .into_iter()
                    .filter(|d| d.exceeds_tolerance)
                    .collect();
                if bad.is_empty() {
                    ReconCliResult::Ok
                } else {
                    ReconCliResult::Divergences(bad)
                }
            }
            Err(e) => ReconCliResult::Error(format!("calibrate: {e}")),
        }
    } else {
        match dataset.compare(&report) {
            Ok(divs) => {
                let bad: Vec<AnchorDivergence> =
                    divs.into_iter().filter(|d| d.exceeds_tolerance).collect();
                if bad.is_empty() {
                    ReconCliResult::Ok
                } else {
                    ReconCliResult::Divergences(bad)
                }
            }
            Err(e) => ReconCliResult::Error(format!("compare: {e}")),
        }
    }
}

/// Silent variant of run for tests — returns the divergence list.
pub fn run_silent(capture: Vec<u8>, anchors_path: &Path) -> Vec<AnchorDivergence> {
    let cursor = std::io::Cursor::new(capture);
    let report = match replay_capture_to_ledger(cursor, &ReplayParams::default()) {
        Ok(r) => r,
        Err(_) => return Vec::new(),
    };
    let dataset = match AnchorDataset::load_jsonl(anchors_path) {
        Ok(d) => d,
        Err(_) => return Vec::new(),
    };
    dataset.compare(&report).unwrap_or_default()
}

/// Look up the AttemptCount value from a divergence list (for tests).
pub fn first_attempt_count(divs: &[AnchorDivergence]) -> Option<u128> {
    divs.iter()
        .find(|d| d.name == AnchorName::AttemptCount)
        .map(|d| d.engine_value)
}
