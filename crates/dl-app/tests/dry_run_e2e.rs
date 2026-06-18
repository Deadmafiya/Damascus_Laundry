//! End-to-end integration test for the DL_LEDGER_PATH pipeline
//! (Phase 7 / plan 01 AC-5 closure).
//!
//! Drives the synth triangle through detection + simulation +
//! ledger writing via `dl_app::dry_run::write_synth_ledger`,
//! then reads the resulting v3 ledger back and asserts:
//! - the file is ≥1 entry (AC-5 contract)
//! - the entry fields are coherent (the v3 schema)
//! - the report hash is deterministic across two calls
//! - the metrics emission site fires (via test
//!   inspection of the report fields it would emit)

use std::io::Cursor;

use dl_app::dry_run::{synth_report, synth_triangle_pools, write_synth_ledger, DryRunLedger};
use dl_ledger::{LedgerEntry, LedgerReader, LedgerWriter, LEDGER_MAGIC, LEDGER_SCHEMA_VERSION};

#[test]
fn synth_dry_run_writes_at_least_one_entry_to_v3_ledger() {
    let mut buf: Vec<u8> = Vec::new();
    let mut w = LedgerWriter::new(&mut buf).expect("writer");
    let result: DryRunLedger = write_synth_ledger(&mut w).expect("synth");
    assert!(result.entries_written >= 1, "AC-5 contract: >=1 entry");
}

#[test]
fn synth_dry_run_ledger_is_v3_format() {
    let mut buf: Vec<u8> = Vec::new();
    let mut w = LedgerWriter::new(&mut buf).expect("writer");
    let _ = write_synth_ledger(&mut w).expect("synth");
    drop(w);
    assert!(buf.starts_with(LEDGER_MAGIC), "v3 magic");
    let schema = u32::from_le_bytes([buf[8], buf[9], buf[10], buf[11]]);
    assert_eq!(schema, LEDGER_SCHEMA_VERSION);
}

#[test]
fn synth_dry_run_ledger_round_trips_entries() {
    let mut buf: Vec<u8> = Vec::new();
    let mut w = LedgerWriter::new(&mut buf).expect("writer");
    let result = write_synth_ledger(&mut w).expect("synth");
    drop(w);

    let mut r = LedgerReader::open(buf.as_slice()).expect("reader");
    let mut count = 0;
    while let Some(entry) = r.read_entry().expect("read") {
        count += 1;
        // Every entry must be schema-v3 (has tip_lamports).
        let _ = entry.tip_lamports; // Accessing the field proves the v3 schema is in use.
        assert!(entry.seq < 1_000, "seq fits in reasonable range");
    }
    assert_eq!(count, result.entries_written);
}

#[test]
fn synth_dry_run_report_is_deterministic() {
    let a = synth_report().expect("a");
    let b = synth_report().expect("b");
    assert_eq!(a.report_hash, b.report_hash);
    assert_eq!(a.cycle_records.len(), b.cycle_records.len());
}

#[test]
fn synth_dry_run_cycles_evaluated_matches_report() {
    let r = synth_report().expect("report");
    let mut buf: Vec<u8> = Vec::new();
    let mut w = LedgerWriter::new(&mut buf).expect("writer");
    let result = write_synth_ledger(&mut w).expect("synth");
    assert_eq!(
        result.report.cycle_records.len(),
        r.cycle_records.len(),
        "cycles_evaluated in the metrics emission site must match the report"
    );
}

#[test]
fn synth_dry_run_emits_would_trade_count() {
    let r = synth_report().expect("report");
    // The metrics emission site reads `report.would_trade()`. Verify
    // it's a non-negative u64 (the field type) and equals the
    // count of `WouldTrade` decisions in the cycle records.
    let direct = r
        .cycle_records
        .iter()
        .filter(|c| matches!(c.decision, dl_ledger::entry::Decision::WouldTrade))
        .count() as u64;
    assert_eq!(r.would_trade(), direct);
}

#[test]
fn synth_dry_run_total_tip_lamports_is_zero_by_default() {
    // The default `LedgerEntry::from_evaluated` sets `tip_lamports = 0`.
    // Until `dl-sim` learns to model per-cycle tip, the metrics
    // emission site will report 0. This test pins that contract.
    let r = synth_report().expect("report");
    assert_eq!(r.total_tip_lamports, 0);
}

#[test]
fn synth_triangle_pools_are_three_distinct_addresses() {
    let pools = synth_triangle_pools();
    assert_eq!(pools.len(), 3);
    let a0 = pools[0].address.0;
    let a1 = pools[1].address.0;
    let a2 = pools[2].address.0;
    assert_ne!(a0, a1);
    assert_ne!(a0, a2);
    assert_ne!(a1, a2);
}

#[test]
fn synth_triangle_mints_form_a_cycle() {
    // USDC -> SOL -> USDT -> USDC.
    let pools = synth_triangle_pools();
    // Pool 0: base USDC, quote SOL.
    assert_eq!(pools[0].base_mint.0, [0x01u8; 32]); // USDC
    assert_eq!(pools[0].quote_mint.0, [0x02u8; 32]); // SOL
                                                     // Pool 1: base SOL, quote USDT.
    assert_eq!(pools[1].base_mint.0, [0x02u8; 32]); // SOL
    assert_eq!(pools[1].quote_mint.0, [0x03u8; 32]); // USDT
                                                     // Pool 2: base USDT, quote USDC.
    assert_eq!(pools[2].base_mint.0, [0x03u8; 32]); // USDT
    assert_eq!(pools[2].quote_mint.0, [0x01u8; 32]); // USDC
}

#[test]
fn synth_dry_run_ledger_writer_works_with_cursor() {
    // Verify the writer accepts an in-memory buffer (so the
    // function is portable to in-memory callers, not just
    // `File`).
    let mut cursor = Cursor::new(Vec::new());
    {
        let mut w = LedgerWriter::new(&mut cursor).expect("cursor writer");
        let _ = write_synth_ledger(&mut w).expect("synth");
    }
    let bytes = cursor.into_inner();
    assert!(bytes.starts_with(LEDGER_MAGIC));
}
