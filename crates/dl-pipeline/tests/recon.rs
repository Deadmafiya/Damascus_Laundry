//! Daily reconciliation feed tests for the data pipeline (DAM-46).

use std::fs;
use std::io::Write;

use dl_pipeline::recon::{reconcile, DailyReconV1};
use dl_pipeline::warehouse::{JsonlWarehouse, Warehouse, WarehouseConfig};
use dl_pipeline::{
    CycleV1, DatePartition, PipelineRunId, ReconCycleEntryV1, ReconReportV1,
};

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

fn write_recon_report(dir: &std::path::Path, run_id: &str, body: &str) {
    let p = dir.join("recon_report_v1").join(run_id);
    fs::create_dir_all(&p).unwrap();
    let mut f = fs::File::create(p.join("report.jsonl")).unwrap();
    f.write_all(body.as_bytes()).unwrap();
}

#[test]
fn reconcile_synthetic_one_day_writes_daily_recon() {
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
        cycles: vec![ReconCycleEntryV1 {
            cycle_id: cid.clone(),
            matched_in_capture: true,
            gross_bps_drift: -7,
            evaluator_differs: false,
        }],
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
    let p = dir.path().join("daily_recon_v1").join("2026-06-20.jsonl");
    assert!(p.exists(), "daily_recon row must be written");
    let body = fs::read_to_string(&p).unwrap();
    let parsed: DailyReconV1 = serde_json::from_str(body.trim()).unwrap();
    assert_eq!(parsed.date, "2026-06-20");
}

#[test]
fn reconcile_is_idempotent_on_rerun() {
    let dir = tempfile::tempdir().unwrap();
    let mut w = JsonlWarehouse::open(WarehouseConfig::new(dir.path())).unwrap();
    let cid = "b".repeat(64);
    w.insert_cycle(&make_cycle(&cid, 1781913600000)).unwrap();
    let report = ReconReportV1 {
        schema: "recon_report.v1".to_string(),
        report_id: "r1".to_string(),
        bot_run_id: "550e8400-e29b-41d4-a716-446655440000".to_string(),
        captured_at_unix_ms: 1781913600000,
        reconciled_at_unix_ms: 1782000001000,
        cycles: vec![ReconCycleEntryV1 {
            cycle_id: cid.clone(),
            matched_in_capture: true,
            gross_bps_drift: 3,
            evaluator_differs: false,
        }],
    };
    write_recon_report(
        dir.path(),
        "550e8400-e29b-41d4-a716-446655440000",
        &format!("{}\n", serde_json::to_string(&report).unwrap()),
    );
    let date = DatePartition::parse("2026-06-20").unwrap();
    let run = PipelineRunId::new();
    let first = reconcile(&mut w, &date, &run).unwrap();
    let second = reconcile(&mut w, &date, &run).unwrap();
    assert_eq!(first.stats.cycles_emitted, second.stats.cycles_emitted);
    assert_eq!(first.stats.cycles_matched_in_capture, second.stats.cycles_matched_in_capture);
    assert_eq!(first.stats.gross_bps_drift_p50, second.stats.gross_bps_drift_p50);
}
