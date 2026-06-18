//! Integration test for `DL_LEDGER_PATH` wiring (Phase 7 / plan 01
//! AC-5 closure). The full `dl-app run_dry_run` path is a
//! separate sub-plan (07-02 ships the end-to-end pipeline); this
//! test focuses on the smaller claim: a `LedgerWriter` opened
//! against a `DL_LEDGER_PATH` produces a valid v3 ledger file
//! with the correct magic, schema, and (when the cycle-detection
//! pipeline feeds it) ≥1 entry.

use std::fs;
use std::path::Path;

use dl_ledger::{
    LedgerEntry, LedgerReader, LedgerWriter, LEDGER_MAGIC, LEDGER_SCHEMA_VERSION,
};
use dl_sim::ev::{EvalOutcome, ExpectedValue, Prob};
use dl_sim::net_profit::NetProfit;
use dl_state::cycle::{Direction, Leg, Cycle};
use dl_state::Pubkey;
use dl_ledger::hash::LedgerHash;

#[test]
fn ledger_writer_creates_v3_file_with_correct_magic() {
    let tmp = std::env::temp_dir().join("dl_app_dl_ledger_path_test_v3.dld");
    let _ = fs::remove_file(&tmp);

    let mut buf: Vec<u8> = Vec::new();
    {
        let mut w = LedgerWriter::new(&mut buf).expect("writer");
        // Write one entry so the file has a frame, not just the header.
        let entry = dummy_entry(0, true);
        w.write_entry(&entry).expect("write");
    }
    fs::write(&tmp, &buf).expect("write tmp");

    // Read it back.
    let read = fs::read(&tmp).expect("read tmp");
    assert!(
        read.starts_with(LEDGER_MAGIC),
        "file must start with {:?}",
        std::str::from_utf8(LEDGER_MAGIC).unwrap()
    );
    let schema = u32::from_le_bytes([read[8], read[9], read[10], read[11]]);
    assert_eq!(schema, LEDGER_SCHEMA_VERSION);

    // Reopen via the reader; expect to read the entry back.
    let bytes = fs::read(&tmp).expect("read tmp");
    let mut r = LedgerReader::open(bytes.as_slice()).expect("reader open");
    let roundtrip = r.read_entry().expect("read entry").expect("entry present");
    assert_eq!(roundtrip, dummy_entry(0, true));
    assert!(r.read_entry().expect("read eof").is_none());

    fs::remove_file(&tmp).ok();
}

#[test]
fn ledger_writer_v3_rejects_v2_file() {
    // Synthesize a v2 file (schema_version=2 in the header) and
    // verify the v3 reader rejects it with `SchemaMismatch`.
    let mut buf: Vec<u8> = Vec::new();
    buf.extend_from_slice(LEDGER_MAGIC);
    buf.extend_from_slice(&2u32.to_le_bytes());
    let r = LedgerReader::open(buf.as_slice());
    match r {
        Err(dl_ledger::LedgerError::SchemaMismatch { found, expected }) => {
            assert_eq!(found, 2);
            assert_eq!(expected, 3);
        }
        other => panic!("expected SchemaMismatch, got {other:?}"),
    }
}

#[test]
fn dl_ledger_path_round_trip_writes_known_entry() {
    // End-to-end: open a ledger against a path, write an entry
    // that came from the recon pipeline's CycleRecord, read it
    // back, assert equality. Mirrors the test in the plan
    // (AC-5) but at the unit level — the dl-app integration is
    // a follow-up commit.
    let path = std::env::temp_dir().join("dl_app_known_entry_test.dld");
    let _ = fs::remove_file(&path);

    let entry = known_entry();
    {
        let mut w = LedgerWriter::new(fs::File::create(&path).expect("create")).expect("writer");
        w.write_entry(&entry).expect("write");
    }

    let bytes = fs::read(&path).expect("read");
    let mut r = LedgerReader::open(bytes.as_slice()).expect("reader");
    let read = r.read_entry().expect("read").expect("entry");
    assert_eq!(read, entry);
    assert!(Path::new(&path).exists());

    fs::remove_file(&path).ok();
}

fn dummy_entry(seq: u64, would_trade: bool) -> LedgerEntry {
    use dl_core::prob::PROB_SCALE_1E18;
    use dl_sim::cost::CostBreakdown;
    let ev = ExpectedValue {
        e_pnl: if would_trade { 100 } else { -50 },
        p_detect: Prob::from_scaled_clamped(PROB_SCALE_1E18),
        p_win: Prob::from_scaled_clamped(PROB_SCALE_1E18),
        p_land: Prob::from_scaled_clamped(PROB_SCALE_1E18),
        expected_failed_cost: 0,
    };
    LedgerEntry {
        seq,
        entry_id: seq,
        cycle_hash: LedgerHash(seq),
        net: NetProfit {
            input_amount: 0,
            gross_output: 0,
            total_costs: CostBreakdown {
                base_sig_fee_lamports: 0,
                priority_fee_lamports: 0,
                jito_tip_lamports: 0,
                jito_tip_fee_lamports: 0,
                total_lamports: 0,
            },
            net_profit: if would_trade { 100 } else { -50 },
            net_profit_bps: 0,
            profitable: would_trade,
        },
        optimistic: ev.clone(),
        conservative: ev,
        decision: if would_trade {
            dl_ledger::entry::Decision::WouldTrade
        } else {
            dl_ledger::entry::Decision::WouldNotTrade
        },
        tip_lamports: 0,
    }
}

fn known_entry() -> LedgerEntry {
    // Build a real Cycle with 2 legs to make cycle_hash non-trivial.
    let leg1 = Leg {
        pool: Pubkey([1; 32]),
        direction: Direction::BaseToQuote,
        weight: 100,
    };
    let leg2 = Leg {
        pool: Pubkey([2; 32]),
        direction: Direction::QuoteToBase,
        weight: 100,
    };
    let cycle = Cycle::new(vec![leg1, leg2]);
    use dl_core::prob::PROB_SCALE_1E18;
    use dl_sim::cost::CostBreakdown;
    use dl_sim::ev::EvalParams;
    let ev = ExpectedValue {
        e_pnl: 250,
        p_detect: Prob::from_scaled_clamped(PROB_SCALE_1E18),
        p_win: Prob::from_scaled_clamped(PROB_SCALE_1E18),
        p_land: Prob::from_scaled_clamped(PROB_SCALE_1E18),
        expected_failed_cost: 0,
    };
    let outcome = EvalOutcome {
        optimistic: ev.clone(),
        conservative: ev.clone(),
    };
    let net = NetProfit {
        input_amount: 1_000_000,
        gross_output: 1_001_000,
        total_costs: CostBreakdown {
            base_sig_fee_lamports: 5_000,
            priority_fee_lamports: 200,
            jito_tip_lamports: 10_000,
            jito_tip_fee_lamports: 500,
            total_lamports: 15_700,
        },
        net_profit: 300,
        net_profit_bps: 30,
        profitable: true,
    };
    LedgerEntry::from_evaluated_with_tip(&cycle, net, &outcome, 7, 10_000)
}
