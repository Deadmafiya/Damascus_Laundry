//! `emit-reconciliation` — DAM-64 reconciliation CLI.
//!
//! Reads a `.dlg` paper ledger and emits a JSON reconciliation
//! report (predicted vs realized PnL, in lamports). The report
//! shape is `dl_recon::reconcile::ReconciliationReport`; the
//! downstream `dl-calibration::captures_from_reconciliation_report`
//! deserializes it directly.
//!
//! ## Usage
//!
//! ```text
//! cargo run --release -p dl-recon --bin emit-reconciliation -- \
//!     --ledger <PATH> [--out <PATH>]
//! ```
//!
//! - `--ledger <path>` (required): the source `.dlg` paper ledger.
//! - `--out <path>` (optional): output JSON path. Defaults to
//!   `<ledger-stem>.reconciliation.json` next to the input. The
//!   special value `-` writes pretty JSON to stdout (no summary
//!   line on stderr in that case; the JSON is the only output).
//!
//! ## Exit codes
//!
//! - `0`: success — report written.
//! - `1`: ledger could not be opened / read / parsed.
//! - `2`: bad CLI usage (missing `--ledger`).
//!
//! ## Output shape
//!
//! Pretty JSON with integer-only fields; `serde_json::to_string_pretty`
//! is the only serializer. Top-level keys: `source_label`, `rows`,
//! `total_predicted_lamports`, `total_realized_lamports`,
//! `total_delta_lamports`, `total_tip_lamports`, `n_traded`,
//! `n_not_traded`, `source_ledger_hash`, `report_hash`. See
//! [`dl_recon::reconcile::ReconciliationReport`].
//!
//! ## Operator usage
//!
//! ```bash
//! cargo run --release -p dl-recon --bin emit-reconciliation -- \
//!     --ledger ./data/dl-app/mainnet.dlg \
//!     --out   ./data/dl-recon/mainnet.dlg.reconciliation.json
//! ```
//!
//! Then pipe the JSON into `dl-calibration`'s fitter (or just hand
//! it to the operator console — the file is the handoff).

use std::fs::File;
use std::io::{BufWriter, Write as _};
use std::path::PathBuf;
use std::process::ExitCode;

use dl_recon::reconcile::{
    reconcile_ledger, write_reconciliation_report_json, ReconciliationReport,
};
use dl_recon::ReconError;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    let mut ledger_path: Option<PathBuf> = None;
    let mut out_path: Option<PathBuf> = None;
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--ledger" => {
                if let Some(v) = args.get(i + 1) {
                    ledger_path = Some(PathBuf::from(v));
                }
                i += 2;
            }
            "--out" => {
                if let Some(v) = args.get(i + 1) {
                    out_path = Some(PathBuf::from(v));
                }
                i += 2;
            }
            "-h" | "--help" => {
                eprintln!("{}", usage());
                return ExitCode::SUCCESS;
            }
            other => {
                eprintln!("emit-reconciliation: unknown argument: {other}");
                eprintln!("{}", usage());
                return ExitCode::from(2);
            }
        }
    }

    let Some(ledger) = ledger_path else {
        eprintln!("emit-reconciliation: --ledger is required");
        eprintln!("{}", usage());
        return ExitCode::from(2);
    };

    let report = match run(&ledger) {
        Ok(r) => r,
        Err(e) => {
            eprintln!(
                "emit-reconciliation: failed to read ledger {}: {e}",
                ledger.display()
            );
            return ExitCode::from(1);
        }
    };

    let out_target = out_path.unwrap_or_else(|| default_out_path(&ledger));

    if out_target == PathBuf::from("-") {
        // Stdout: just the JSON, no summary line. The consumer is a
        // pipe; printing extra to stdout would corrupt the stream.
        let stdout = std::io::stdout();
        let mut handle = stdout.lock();
        if let Err(e) = write_reconciliation_report_json(&report, &mut handle) {
            eprintln!("emit-reconciliation: write stdout failed: {e}");
            return ExitCode::from(1);
        }
        return ExitCode::SUCCESS;
    }

    if let Some(parent) = out_target.parent() {
        if !parent.as_os_str().is_empty() {
            if let Err(e) = std::fs::create_dir_all(parent) {
                eprintln!(
                    "emit-reconciliation: failed to create parent dir {}: {e}",
                    parent.display()
                );
                return ExitCode::from(1);
            }
        }
    }
    let file = match File::create(&out_target) {
        Ok(f) => f,
        Err(e) => {
            eprintln!(
                "emit-reconciliation: failed to create {}: {e}",
                out_target.display()
            );
            return ExitCode::from(1);
        }
    };
    let mut writer = BufWriter::new(file);
    if let Err(e) = write_reconciliation_report_json(&report, &mut writer) {
        eprintln!("emit-reconciliation: write failed: {e}");
        return ExitCode::from(1);
    }
    if let Err(e) = writer.flush() {
        eprintln!("emit-reconciliation: flush failed: {e}");
        return ExitCode::from(1);
    }

    // Summary on stderr — the JSON file is the primary output; the
    // summary is operator-facing and must not pollute the file.
    eprintln!(
        "emit-reconciliation: wrote {} (rows={} n_traded={} n_not_traded={} \
         total_predicted_lamports={} total_realized_lamports={} \
         total_delta_lamports={} total_tip_lamports={} report_hash={})",
        out_target.display(),
        report.rows.len(),
        report.n_traded,
        report.n_not_traded,
        report.total_predicted_lamports,
        report.total_realized_lamports,
        report.total_delta_lamports,
        report.total_tip_lamports,
        report.report_hash,
    );
    ExitCode::SUCCESS
}

fn run(ledger: &PathBuf) -> Result<ReconciliationReport, ReconError> {
    let file = File::open(ledger).map_err(|e| {
        ReconError::Io(std::io::Error::new(
            e.kind(),
            format!("open {}: {e}", ledger.display()),
        ))
    })?;
    reconcile_ledger(file, ledger.display().to_string())
}

fn default_out_path(ledger: &PathBuf) -> PathBuf {
    let stem = ledger
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("ledger");
    let parent = ledger.parent().unwrap_or_else(|| std::path::Path::new("."));
    parent.join(format!("{stem}.reconciliation.json"))
}

fn usage() -> &'static str {
    "\
Usage: emit-reconciliation --ledger <PATH> [--out <PATH>|-]

Reads a .dlg paper ledger and writes a JSON reconciliation report
(predicted vs realized PnL, in lamports).

  --ledger <PATH>   source ledger file (required)
  --out   <PATH>    output JSON path (default: <stem>.reconciliation.json)
                    Use `-` to write JSON to stdout.

Exit codes: 0 = success, 1 = ledger read/parse failure, 2 = bad CLI."
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_out_path_appends_suffix() {
        let p = PathBuf::from("/tmp/example.dlg");
        let out = default_out_path(&p);
        assert_eq!(out, PathBuf::from("/tmp/example.reconciliation.json"));
    }

    #[test]
    fn default_out_path_handles_named_stem() {
        let p = PathBuf::from("session-2026-06-21.dlg");
        let out = default_out_path(&p);
        assert_eq!(out, PathBuf::from("session-2026-06-21.reconciliation.json"));
    }
}
