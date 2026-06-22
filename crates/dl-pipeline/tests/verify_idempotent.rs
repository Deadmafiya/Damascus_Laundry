//! Verify path tests for the data pipeline (DAM-46).

use dl_pipeline::warehouse::{JsonlWarehouse, Warehouse, WarehouseConfig};
use dl_pipeline::CycleV1;

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
fn seal_then_verify_matches() {
    let dir = tempfile::tempdir().unwrap();
    let mut w = JsonlWarehouse::open(WarehouseConfig::new(dir.path())).unwrap();
    w.insert_cycle(&make_cycle(&"c".repeat(64), 1781913600000)).unwrap();
    w.seal_partition("2026-06-20", "test-run").unwrap();
    let (live, stored) = w.verify_partition("2026-06-20").unwrap();
    let s = stored.expect("stored checksum must exist after seal");
    assert_eq!(s.row_count, 1);
    assert_eq!(s.row_set_blake3, live.row_set_blake3);
}

#[test]
fn verify_idempotent_on_partition_unchanged() {
    let dir = tempfile::tempdir().unwrap();
    let mut w = JsonlWarehouse::open(WarehouseConfig::new(dir.path())).unwrap();
    w.insert_cycle(&make_cycle(&"d".repeat(64), 1781913600000)).unwrap();
    w.seal_partition("2026-06-20", "test-run").unwrap();
    let (live_a, stored_a) = w.verify_partition("2026-06-20").unwrap();
    let (live_b, stored_b) = w.verify_partition("2026-06-20").unwrap();
    assert_eq!(live_a.row_set_blake3, live_b.row_set_blake3);
    assert_eq!(live_a.row_count, live_b.row_count);
    assert_eq!(stored_a.unwrap().row_set_blake3, stored_b.unwrap().row_set_blake3);
}

#[test]
fn verify_returns_none_when_unsealed() {
    let dir = tempfile::tempdir().unwrap();
    let mut w = JsonlWarehouse::open(WarehouseConfig::new(dir.path())).unwrap();
    w.insert_cycle(&make_cycle(&"e".repeat(64), 1781913600000)).unwrap();
    let (live, stored) = w.verify_partition("2026-06-20").unwrap();
    assert!(stored.is_none(), "no stored checksum before seal_partition");
    assert_eq!(live.row_count, 1);
}
