//! `dl-niches` — Phase 2e binary.
//!
//! Reads daily reconciliation reports, scores each niche class
//! by realized net PnL per unit gas, and writes `niches.json`.
//!
//! The niche taxonomy:
//! - Dex: Raydium | Orca | Meteora
//! - Pool age: New (<1h) | Young (<24h) | Mature (≥24h)
//! - Time of day (UTC): Peak | Normal | OffPeak
//! - Input size: Small (<1 SOL) | Medium | Large (≥10 SOL)
//!
//! Default enabled rule: `sample_size >= 30 AND realized_pnl_per_unit > 0`.

use std::path::PathBuf;
use std::process::ExitCode;

use dl_calibration::{
    niche_score, niches_from_scores, read_recon_reports, write_niches_json, NicheConfig,
};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    let mut recon_glob = "./dl-recon/recon-*.json".to_string();
    let mut out_path = PathBuf::from("./dl-recon/niches.json");
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--recon-glob" => {
                if let Some(v) = args.get(i + 1) {
                    recon_glob = v.clone();
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

    let paths = match glob_paths(&recon_glob) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("dl-niches: glob error: {e}");
            return ExitCode::from(2);
        }
    };
    if paths.is_empty() {
        eprintln!(
            "dl-niches: no recon files matched {} (run reconcile first)",
            recon_glob
        );
        return ExitCode::from(1);
    }

    let reports = read_recon_reports(&paths);
    eprintln!("dl-niches: loaded {} recon files", reports.len());
    let scores = niche_score(&reports);
    let cfg: NicheConfig = niches_from_scores(&scores);
    match write_niches_json(&cfg, &out_path) {
        Ok(()) => {
            let enabled = cfg.enabled_classes.len();
            println!(
                "dl-niches: wrote {} ({} enabled classes of {} total)",
                out_path.display(),
                enabled,
                cfg.scores.len()
            );
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("dl-niches: write failed: {e}");
            ExitCode::from(1)
        }
    }
}

/// Minimal glob expansion (handles `*.json` suffix only; sufficient
/// Minimal glob expansion (handles `dir/prefix*suffix` only;
/// anything with multiple wildcards or recursive `**` falls through
/// to returning the pattern unchanged). Phase 2 L7: `**`-style
/// recursion and `?` single-char wildcards are TODO — the v1.0 use
/// case (recon file glob) only needs the simple prefix*suffix form.
fn glob_paths(pattern: &str) -> std::io::Result<Vec<PathBuf>> {
    let Some((dir, file_pat)) = pattern.rsplit_once('/') else {
        return Ok(Vec::new());
    };
    if !file_pat.contains('*') {
        return Ok(vec![PathBuf::from(pattern)]);
    }
    let prefix = file_pat.split('*').next().unwrap_or("");
    let suffix = file_pat.split('*').last().unwrap_or("");
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let name = entry.file_name();
        let Some(name_str) = name.to_str() else { continue };
        if !name_str.starts_with(prefix) || !name_str.ends_with(suffix) {
            continue;
        }
        out.push(entry.path());
    }
    out.sort();
    Ok(out)
}
