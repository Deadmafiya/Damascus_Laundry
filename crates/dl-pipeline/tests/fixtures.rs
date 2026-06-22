//! End-to-end fixture tests for the data pipeline (DAM-46).
//!
//! The happy / missing-schema / bad-legs fixtures live under
//! `tests/fixtures/cycle/v1/`. Each test exercises the public API
//! (not the CLI) so the assertions can run in `cargo test` without
//! requiring a `dl-pipeline` binary on PATH.
//!
//! Spec contract (DAM-46 §"Test mode"):
//! - happy.jsonl           -> row count == 3, rejects == 0
//! - missing_schema.jsonl  -> row count == 0, rejects == 1, reason == schema_missing
//! - bad_legs.jsonl        -> row count == 0, rejects == 1, reason == legs_empty

use dl_pipeline::reject::RejectReason;
use dl_pipeline::warehouse::{JsonlWarehouse, Warehouse, WarehouseConfig};
use dl_pipeline::{ingest_cycle_v1, parse_cycle_v1_line, PipelineRunId};

fn make_warehouse() -> (tempfile::TempDir, JsonlWarehouse) {
    let dir = tempfile::tempdir().unwrap();
    let w = JsonlWarehouse::open(WarehouseConfig::new(dir.path())).unwrap();
    (dir, w)
}

#[test]
fn happy_fixture_writes_three_rows_with_zero_rejects() {
    let (_dir, mut w) = make_warehouse();
    let fixture = "tests/fixtures/cycle/v1/happy.jsonl";
    let run = PipelineRunId::new();
    let stats = ingest_cycle_v1(&mut w, std::path::Path::new(fixture), &run).unwrap();
    assert_eq!(stats.lines_read, 3, "expected 3 lines read");
    assert_eq!(stats.lines_written, 3, "expected 3 lines written");
    assert_eq!(stats.rejects, 0, "happy fixture must have zero rejects");
    let rows = w.read_cycle_partition("2026-06-20").unwrap();
    assert_eq!(rows.len(), 3);
}

#[test]
fn missing_schema_fixture_writes_zero_rows_with_one_reject() {
    let (_dir, mut w) = make_warehouse();
    let fixture = "tests/fixtures/cycle/v1/missing_schema.jsonl";
    let run = PipelineRunId::new();
    let stats = ingest_cycle_v1(&mut w, std::path::Path::new(fixture), &run).unwrap();
    assert_eq!(stats.lines_written, 0);
    assert_eq!(stats.rejects, 1);
    let rejects = w.read_rejects().unwrap();
    assert_eq!(rejects.len(), 1);
    assert_eq!(rejects[0].reason, RejectReason::SchemaMissing);
}

#[test]
fn bad_legs_fixture_writes_zero_rows_with_one_reject() {
    let (_dir, mut w) = make_warehouse();
    let fixture = "tests/fixtures/cycle/v1/bad_legs.jsonl";
    let run = PipelineRunId::new();
    let stats = ingest_cycle_v1(&mut w, std::path::Path::new(fixture), &run).unwrap();
    assert_eq!(stats.lines_written, 0);
    assert_eq!(stats.rejects, 1);
    let rejects = w.read_rejects().unwrap();
    assert_eq!(rejects[0].reason, RejectReason::LegsEmpty);
}

#[test]
fn parse_cycle_v1_line_accepts_happy_record() {
    let line = r#"{"schema":"cycle.v1","cycle_id":"0000000000000000000000000000000000000000000000000000000000000099","detected_at_unix_ms":1782000000000,"detected_at_slot":1,"bot_run_id":"550e8400-e29b-41d4-a716-446655440000","dexes":["raydium","orca"],"legs":[{"pool":"58oQChx4yWmvKdwLLZzBi4ChoCc2fqCUWBkwMihLYQo2","dex":"raydium","direction":"BaseToQuote","weight":3000000000000000},{"pool":"Czfq3xZZDmsdGdUyrNLtRhGc47cXcZtLG4crryfu44zE","dex":"orca","direction":"QuoteToBase","weight":-1750000000000000000}],"base_mint":"So11111111111111111111111111111111111111112","quote_mint":"EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v","gross_bps":17470,"fee_bps_sum":60,"decision":"WouldTrade","evaluator":"conservative_default","input_lamports":1000000000,"output_lamports":1174700000,"source_feed":"ws:mainnet"}"#;
    let row = parse_cycle_v1_line(line).unwrap();
    assert_eq!(row.cycle_id.len(), 64);
    assert_eq!(row.legs.len(), 2);
}
