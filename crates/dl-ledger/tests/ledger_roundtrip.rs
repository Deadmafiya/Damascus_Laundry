//! Round-trip and corruption tests for the paper ledger file format.
//!
//! Mirrors `crates/dl-feed/tests/capture_roundtrip.rs`:
//!   - Write N entries, read them back, assert byte-equality.
//!   - Test magic / schema / truncation / corruption failure modes.
//!   - Lock the `format_spec()` text against drift by asserting
//!     it mentions the key fields.

use dl_core::prob::PROB_SCALE_1E18;
use dl_ledger::entry::Decision;
use dl_ledger::hash::LedgerHash;
use dl_ledger::{
    format_spec, LedgerEntry, LedgerReader, LedgerWriter, LEDGER_MAGIC, LEDGER_SCHEMA_VERSION,
};
use dl_sim::cost::CostBreakdown;
use dl_sim::ev::{EvalOutcome, ExpectedValue, Prob};
use dl_sim::net_profit::NetProfit;

fn empty_costs() -> CostBreakdown {
    CostBreakdown {
        base_sig_fee_lamports: 0,
        priority_fee_lamports: 0,
        jito_tip_lamports: 0,
        jito_tip_fee_lamports: 0,
        total_lamports: 0,
    }
}

fn one_p() -> Prob {
    Prob::from_scaled_clamped(PROB_SCALE_1E18)
}

fn ev(e_pnl: i128) -> ExpectedValue {
    ExpectedValue {
        e_pnl,
        p_detect: one_p(),
        p_win: one_p(),
        p_land: one_p(),
        expected_failed_cost: 0,
    }
}

fn build_entry(seq: u64, opt: i128, con: i128, trade: bool) -> LedgerEntry {
    LedgerEntry {
        seq,
        entry_id: seq,
        cycle_hash: LedgerHash(seq.wrapping_mul(0x9e37_79b9_7f4a_7c15)),
        net: NetProfit {
            input_amount: 1,
            gross_output: 0,
            total_costs: empty_costs(),
            net_profit: opt,
            net_profit_bps: 0,
            profitable: opt > 0,
        },
        optimistic: ev(opt),
        conservative: ev(con),
        decision: if trade {
            Decision::WouldTrade
        } else {
            Decision::WouldNotTrade
        },
    }
}

#[test]
fn write_then_read_round_trip_n_entries() {
    let entries: Vec<LedgerEntry> = (0..10)
        .map(|i| build_entry(i as u64, (i as i128) * 100, (i as i128) * 50, i % 2 == 0))
        .collect();

    let mut buf = Vec::new();
    {
        let mut w = LedgerWriter::new(&mut buf).expect("writer new");
        for e in &entries {
            w.write_entry(e).expect("write entry");
        }
        assert_eq!(w.frames_written(), entries.len() as u64);
    }

    let mut r = LedgerReader::open(buf.as_slice()).expect("reader open");
    for expected in &entries {
        let got = r.read_entry().expect("read ok").expect("entry present");
        assert_eq!(
            &got, expected,
            "round-trip mismatch for seq={}",
            expected.seq
        );
    }
    assert_eq!(r.read_entry().expect("final read ok"), None, "EOF");
    assert_eq!(r.entries_read(), entries.len() as u64);
}

#[test]
fn format_spec_locks_key_fields() {
    let spec = format_spec();
    assert!(spec.contains("DLD-LDG1"), "spec missing magic: {}", spec);
    assert!(spec.contains("u32"), "spec missing u32: {}", spec);
    assert!(spec.contains("bincode"), "spec missing bincode: {}", spec);
    assert!(
        spec.contains("LedgerEntry"),
        "spec missing LedgerEntry: {}",
        spec
    );
    assert!(
        spec.contains("payload_len"),
        "spec missing payload_len: {}",
        spec
    );
    assert!(
        spec.contains("Schema version 2"),
        "spec missing schema v2: {}",
        spec
    );
}

#[test]
fn open_rejects_wrong_magic() {
    // 8 bytes wrong magic + 4 bytes schema
    let mut bytes = b"WRONGMAG".to_vec();
    bytes.extend_from_slice(&LEDGER_SCHEMA_VERSION.to_le_bytes());
    let r = LedgerReader::open(bytes.as_slice());
    assert!(matches!(r, Err(dl_ledger::LedgerError::BadMagic)));
}

#[test]
fn open_rejects_wrong_schema() {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(LEDGER_MAGIC);
    bytes.extend_from_slice(&1u32.to_le_bytes()); // schema 1, not 2
    let r = LedgerReader::open(bytes.as_slice());
    match r {
        Err(dl_ledger::LedgerError::SchemaMismatch { found, expected }) => {
            assert_eq!(found, 1);
            assert_eq!(expected, 2);
        }
        other => panic!("expected SchemaMismatch, got {:?}", other),
    }
}

#[test]
fn read_truncated_payload_returns_truncated() {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(LEDGER_MAGIC);
    bytes.extend_from_slice(&LEDGER_SCHEMA_VERSION.to_le_bytes());
    // Frame: length 100, but no payload
    bytes.extend_from_slice(&100u32.to_le_bytes());
    // intentionally omit the 100 payload bytes
    let mut r = LedgerReader::open(bytes.as_slice()).unwrap();
    match r.read_entry() {
        Err(dl_ledger::LedgerError::Truncated) => {}
        other => panic!("expected Truncated, got {:?}", other),
    }
}

#[test]
fn empty_ledger_round_trips_to_zero_entries() {
    let mut buf = Vec::new();
    {
        let _w = LedgerWriter::new(&mut buf).expect("writer new");
    }
    let mut r = LedgerReader::open(buf.as_slice()).expect("reader open");
    assert_eq!(r.read_entry().unwrap(), None);
    assert_eq!(r.entries_read(), 0);
}

#[test]
fn eval_outcome_constructor_integration() {
    // Exercise the full constructor path: build an EvalOutcome,
    // wrap it via LedgerEntry::from_evaluated, write+read it.
    use dl_state::cycle::{Cycle, Direction, Leg};

    let leg = |byte: u8, dir: Direction| Leg {
        pool: dl_state::Pubkey(
            [0u8; 31]
                .into_iter()
                .chain([byte])
                .collect::<Vec<u8>>()
                .try_into()
                .unwrap(),
        ),
        direction: dir,
        weight: 0,
    };
    let cycle = Cycle::new(vec![
        leg(1, Direction::BaseToQuote),
        leg(2, Direction::QuoteToBase),
    ]);
    let net = NetProfit {
        input_amount: 1_000_000,
        gross_output: 1_010_000,
        total_costs: empty_costs(),
        net_profit: 10_000,
        net_profit_bps: 100,
        profitable: true,
    };
    let outcome = EvalOutcome {
        optimistic: ev(10_000),
        conservative: ev(8_500),
    };
    let entry = LedgerEntry::from_evaluated(&cycle, net, &outcome, 0);
    assert_eq!(entry.decision, Decision::WouldTrade);
    assert_eq!(entry.optimistic.e_pnl, 10_000);
    assert_eq!(entry.conservative.e_pnl, 8_500);

    let mut buf = Vec::new();
    {
        let mut w = LedgerWriter::new(&mut buf).unwrap();
        w.write_entry(&entry).unwrap();
    }
    let mut r = LedgerReader::open(buf.as_slice()).unwrap();
    let got = r.read_entry().unwrap().unwrap();
    assert_eq!(got, entry);
}
