//! Inspection test: read the v3 ledger produced by `DL_LEDGER_PATH`
//! and verify entry fields are sensible.

use std::fs;
use std::path::Path;

use dl_ledger::{LedgerReader, LEDGER_MAGIC, LEDGER_SCHEMA_VERSION};

#[test]
fn inspect_dry_run_ledger_v3() {
    // The v3 file from the manual DL_LEDGER_PATH run. If absent,
    // the test is a no-op.
    let path = std::env::temp_dir().join("07_01_full_e2e.dld");
    if !path.exists() {
        eprintln!("no ledger at {}; skipping", path.display());
        return;
    }
    let bytes = fs::read(&path).expect("read ledger");
    assert!(
        bytes.starts_with(LEDGER_MAGIC),
        "file must start with {:?}",
        std::str::from_utf8(LEDGER_MAGIC).unwrap()
    );
    let schema = u32::from_le_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]);
    assert_eq!(schema, LEDGER_SCHEMA_VERSION);

    let mut r = LedgerReader::open(bytes.as_slice()).expect("reader");
    let mut n = 0usize;
    let mut total_e_pnl = 0i128;
    let mut total_tip = 0u64;
    while let Some(entry) = r.read_entry().expect("read") {
        n += 1;
        total_e_pnl += entry.conservative.e_pnl;
        total_tip += entry.tip_lamports;
        eprintln!(
            "entry seq={} cycle_hash={:#x} e_pnl={} tip={}",
            entry.seq, entry.cycle_hash.0, entry.conservative.e_pnl, entry.tip_lamports
        );
    }
    eprintln!("total: {n} entries, e_pnl={total_e_pnl}, tip={total_tip}");
    assert!(n > 0, "ledger must have ≥1 entry");
    assert!(
        total_tip == 0,
        "tip is 0 by default (per-cycle tip in dl-sim is a follow-up)"
    );
    // Show file exists
    assert!(Path::new(&path).exists());
}
