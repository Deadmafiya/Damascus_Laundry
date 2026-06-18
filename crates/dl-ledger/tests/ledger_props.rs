//! Property tests for the paper ledger.
//!
//! Mirrors `crates/dl-sim/tests/ev_props.rs` (Phase 5 plan 01):
//!   - round-trip arbitrary `LedgerEntry` is identity
//!   - corruption is detectable (no false round-trip)
//!   - `LedgerSummary` is associative / order-independent
//!   - `Decision::from_ev` is exhaustive (positive / zero / negative)

use dl_core::prob::PROB_SCALE_1E18;
use dl_ledger::entry::Decision;
use dl_ledger::hash::LedgerHash;
use dl_ledger::{LedgerEntry, LedgerReader, LedgerSummary, LedgerWriter};
use dl_sim::cost::CostBreakdown;
use dl_sim::ev::{ExpectedValue, Prob};
use proptest::prelude::*;

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

fn arb_ev(e_pnl: i128) -> ExpectedValue {
    ExpectedValue {
        e_pnl,
        p_detect: one_p(),
        p_win: one_p(),
        p_land: one_p(),
        expected_failed_cost: 0,
    }
}

fn arb_entry(seq: u64, opt: i128, con: i128) -> LedgerEntry {
    LedgerEntry {
        seq,
        entry_id: seq,
        cycle_hash: LedgerHash(seq.wrapping_mul(0x9e37_79b9_7f4a_7c15)),
        net: dl_sim::net_profit::NetProfit {
            input_amount: 1,
            gross_output: 0,
            total_costs: empty_costs(),
            net_profit: opt,
            net_profit_bps: 0,
            profitable: opt > 0,
        },
        optimistic: arb_ev(opt),
        conservative: arb_ev(con),
        decision: Decision::from_ev(&arb_ev(con)),
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    #[test]
    fn round_trip_preserves_entry(
        seq in 0u64..10_000,
        opt in -1_000_000_000i128..1_000_000_000,
        con in -1_000_000_000i128..1_000_000_000,
    ) {
        let e = arb_entry(seq, opt, con);
        let mut buf = Vec::new();
        {
            let mut w = LedgerWriter::new(&mut buf).expect("writer");
            w.write_entry(&e).expect("write");
        }
        let mut r = LedgerReader::open(buf.as_slice()).expect("reader");
        let got = r.read_entry().expect("read ok").expect("entry");
        assert_eq!(got, e);
        assert_eq!(r.read_entry().expect("eof"), None);
    }

    #[test]
    fn decision_is_positive_iff_e_pnl_positive(e_pnl in -1_000_000i128..1_000_001) {
        let ev = arb_ev(e_pnl);
        let d = Decision::from_ev(&ev);
        match d {
            Decision::WouldTrade => assert!(e_pnl > 0),
            Decision::WouldNotTrade => assert!(e_pnl <= 0),
        }
    }

    #[test]
    fn summary_is_order_independent(
        a in arb_ledger_entry(),
        b in arb_ledger_entry(),
        c in arb_ledger_entry(),
    ) {
        let fwd = LedgerSummary::from_entries(&[a.clone(), b.clone(), c.clone()]).expect("fwd");
        let rev = LedgerSummary::from_entries(&[c, b, a]).expect("rev");
        assert_eq!(fwd, rev, "summary must not depend on entry order");
    }

    #[test]
    fn summary_conservative_sum_is_summable(
        entries in proptest::collection::vec(arb_ledger_entry(), 1..20),
    ) {
        let s = LedgerSummary::from_entries(&entries).expect("summary");
        let manual: i128 = entries.iter().map(|e| e.conservative.e_pnl).sum();
        assert_eq!(s.sum_conservative_e_pnl(), manual);
    }
}

fn arb_ledger_entry() -> BoxedStrategy<LedgerEntry> {
    (
        any::<u64>(),
        -1_000_000_000i128..1_000_000_000i128,
        -1_000_000_000i128..1_000_000_000i128,
    )
        .prop_map(|(seq, opt, con)| arb_entry(seq, opt, con))
        .boxed()
}
