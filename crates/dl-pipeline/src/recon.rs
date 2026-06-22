//! Daily reconciliation feed.

use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;

use chrono::Utc;
use serde::{Deserialize, Serialize};
use tracing::{debug, warn};

use crate::error::PipelineError;
use crate::warehouse::{JsonlWarehouse, Warehouse};
use crate::{DatePartition, PipelineRunId, ReconReportV1};

/// Result of one `reconcile --date` invocation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReconStats {
    pub date: String,
    pub cycles_emitted: u64,
    pub cycles_matched_in_capture: u64,
    pub cycles_unmatched: u64,
    pub gross_bps_drift_p50: i64,
    pub gross_bps_drift_p99: i64,
    pub evaluator_differs_count: u64,
    pub reports_processed: u64,
}

/// One row of `daily_recon_v1`. One per day.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DailyReconV1 {
    pub schema: String,
    pub date: String,
    pub pipeline_run_id: String,
    pub stats: ReconStats,
    pub generated_at_unix_ms: i64,
}

/// One row of `recon_report_v1` after parsing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReconRow {
    pub cycle_id: String,
    pub matched_in_capture: bool,
    pub gross_bps_drift: i64,
    pub evaluator_differs: bool,
}

/// Run the daily reconciliation for the given date. The function is
/// idempotent: re-running it overwrites the same `daily_recon_v1[date].jsonl`.
pub fn reconcile(
    warehouse: &mut JsonlWarehouse,
    date: &DatePartition,
    pipeline_run_id: &PipelineRunId,
) -> Result<DailyReconV1, PipelineError> {
    let cycles = warehouse.read_cycle_partition(date.as_str())?;
    let reports = read_all_reports(warehouse.config().root.join("recon_report_v1"))?;

    let cycles_emitted = cycles.len() as u64;
    let cycle_ids: std::collections::HashSet<String> =
        cycles.iter().map(|c| c.cycle_id.clone()).collect();

    let mut matched = 0u64;
    let mut unmatched = 0u64;
    let mut evaluator_differs = 0u64;
    let mut drifts: Vec<i64> = Vec::new();
    for r in &reports {
        for entry in &r.cycles {
            if !cycle_ids.contains(&entry.cycle_id) {
                continue;
            }
            if entry.matched_in_capture {
                matched += 1;
            } else {
                unmatched += 1;
            }
            if entry.evaluator_differs {
                evaluator_differs += 1;
            }
            drifts.push(entry.gross_bps_drift);
        }
    }

    drifts.sort_unstable();
    let p50 = percentile(&drifts, 50).unwrap_or(0);
    let p99 = percentile(&drifts, 99).unwrap_or(0);

    let stats = ReconStats {
        date: date.as_str().to_string(),
        cycles_emitted,
        cycles_matched_in_capture: matched,
        cycles_unmatched: unmatched,
        gross_bps_drift_p50: p50,
        gross_bps_drift_p99: p99,
        evaluator_differs_count: evaluator_differs,
        reports_processed: reports.len() as u64,
    };

    let row = DailyReconV1 {
        schema: "daily_recon.v1".to_string(),
        date: date.as_str().to_string(),
        pipeline_run_id: pipeline_run_id.to_string(),
        stats,
        generated_at_unix_ms: Utc::now().timestamp_millis(),
    };

    write_daily_recon(warehouse, date, &row)?;
    debug!(
        "reconcile done: date={} emitted={} matched={} unmatched={} evaldiff={}",
        row.date, row.stats.cycles_emitted, row.stats.cycles_matched_in_capture,
        row.stats.cycles_unmatched, row.stats.evaluator_differs_count
    );
    Ok(row)
}

fn read_all_reports(root: PathBuf) -> Result<Vec<ReconReportV1>, PipelineError> {
    let mut out = Vec::new();
    if !root.exists() {
        return Ok(out);
    }
    for entry in walkdir::WalkDir::new(&root)
        .into_iter()
        .filter_map(|e| e.ok())
    {
        let p = entry.path();
        if !p.is_file() || p.extension().and_then(|s| s.to_str()) != Some("jsonl") {
            continue;
        }
        let f = fs::File::open(p)?;
        for line in BufReader::new(f).lines() {
            let line = line?;
            if line.is_empty() {
                continue;
            }
            match serde_json::from_str::<ReconReportV1>(&line) {
                Ok(r) => out.push(r),
                Err(e) => {
                    warn!(
                        "read_all_reports: skipping un-parseable line in {}: {}",
                        p.display(),
                        e
                    );
                }
            }
        }
    }
    Ok(out)
}

fn write_daily_recon(
    warehouse: &JsonlWarehouse,
    date: &DatePartition,
    row: &DailyReconV1,
) -> Result<(), PipelineError> {
    let path = warehouse.daily_recon_path(date.as_str());
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("jsonl.tmp");
    let mut f = fs::File::create(&tmp)?;
    let mut s = serde_json::to_string(row)?;
    s.push('\n');
    f.write_all(s.as_bytes())?;
    fs::rename(&tmp, &path)?;
    Ok(())
}

/// Integer percentile over a sorted `&[i64]`. `pct` is 0..=100.
fn percentile(sorted: &[i64], pct: u32) -> Option<i64> {
    if sorted.is_empty() {
        return None;
    }
    let n = sorted.len();
    let rank = ((pct as u64 * n as u64 + 99) / 100).max(1) as usize;
    let idx = (rank - 1).min(n - 1);
    Some(sorted[idx])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::warehouse::{JsonlWarehouse, Warehouse, WarehouseConfig};
    use crate::{CycleV1, ReconReportV1, ReconCycleEntryV1};
    use std::io::Write;
    use std::path::Path;

    fn write_recon_report(dir: &Path, run_id: &str, body: &str) {
        let p = dir.join("recon_report_v1").join(run_id);
        fs::create_dir_all(&p).unwrap();
        let mut f = fs::File::create(p.join("report.jsonl")).unwrap();
        f.write_all(body.as_bytes()).unwrap();
    }

    fn make_cycle(id: &str, ts: i64) -> CycleV1 {
        CycleV1 {
            schema: "cycle.v1".to_string(),
            cycle_id: id.to_string(),
            detected_at_unix_ms: ts,
            detected_at_slot: 1,
            bot_run_id: "550e8400-e29b-41d4-a716-446655440000".to_string(),
            dexes: vec!["raydium".to_string(), "orca".to_string()],
            legs: vec![],
            base_mint: "".to_string(),
            quote_mint: "".to_string(),
            gross_bps: 100,
            fee_bps_sum: 60,
            decision: "WouldTrade".to_string(),
            evaluator: "conservative_default".to_string(),
            input_lamports: 1_000_000_000,
            output_lamports: 1_000_500_000,
            source_feed: "ws:mainnet".to_string(),
        }
    }

    #[test]
    fn reconcile_joins_emitted_and_reported() {
        let dir = tempfile::tempdir().unwrap();
        let mut w = JsonlWarehouse::open(WarehouseConfig::new(dir.path())).unwrap();
        let cid = "a".repeat(64);
        w.insert_cycle(&make_cycle(&cid, 1781913600000)).unwrap();
        let report = ReconReportV1 {
            schema: "recon_report.v1".to_string(),
            report_id: "r1".to_string(),
            bot_run_id: "550e8400-e29b-41d4-a716-446655440000".to_string(),
            captured_at_unix_ms: 1781913600000,
            reconciled_at_unix_ms: 1782000001000,
            cycles: vec![
                ReconCycleEntryV1 {
                    cycle_id: cid.clone(),
                    matched_in_capture: true,
                    gross_bps_drift: -12,
                    evaluator_differs: false,
                },
                ReconCycleEntryV1 {
                    cycle_id: "b".repeat(64),
                    matched_in_capture: false,
                    gross_bps_drift: 999,
                    evaluator_differs: true,
                },
            ],
        };
        write_recon_report(
            dir.path(),
            "550e8400-e29b-41d4-a716-446655440000",
            &format!("{}\n", serde_json::to_string(&report).unwrap()),
        );
        let date = DatePartition::parse("2026-06-20").unwrap();
        let run = PipelineRunId::new();
        let row = reconcile(&mut w, &date, &run).unwrap();
        assert_eq!(row.stats.cycles_emitted, 1);
        assert_eq!(row.stats.cycles_matched_in_capture, 1);
        assert_eq!(row.stats.cycles_unmatched, 0);
        assert_eq!(row.stats.gross_bps_drift_p50, -12);
    }

    #[test]
    fn percentile_nearest_rank() {
        let s = vec![1, 2, 3, 4, 5, 6, 7, 8, 9, 10];
        assert_eq!(percentile(&s, 50), Some(5));
        assert_eq!(percentile(&s, 99), Some(10));
        assert_eq!(percentile(&s, 0), Some(1));
        assert_eq!(percentile(&[], 50), None);
    }
}
