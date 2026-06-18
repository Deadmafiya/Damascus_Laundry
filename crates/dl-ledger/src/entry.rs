//! Ledger entry: the per-opportunity audit record.
//!
//! One `LedgerEntry` is written to the ledger for every evaluated
//! opportunity. The `decision` field is derived from the conservative
//! bound only (the trade gate), but the optimistic bound is recorded
//! for reconciliation / re-scoring under different conservative
//! settings (Phase 6 calibration).
//!
//! All fields are integer-only — no floats, no `as f64`, no
//! `f64::from(...)`. The `dl-ledger` int-only CI guard
//! (`tests/int_only_no_fractional.rs`) enforces this at the crate
//! level.

use dl_sim::ev::{EvalOutcome, ExpectedValue};
use dl_sim::net_profit::NetProfit;
use dl_state::cycle::Cycle;
use serde::{Deserialize, Serialize};

use crate::hash::LedgerHash;

/// Per-opportunity decision. The trade gate is **conservative only**:
/// a cycle is `WouldTrade` iff `conservative.e_pnl > 0`. The
/// optimistic bound is recorded for reconciliation but never drives
/// the gate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Decision {
    /// `conservative.e_pnl > 0` — paper-trade the cycle.
    WouldTrade,
    /// `conservative.e_pnl <= 0` — skip the cycle.
    WouldNotTrade,
}

impl Decision {
    /// Derive the decision from the conservative `ExpectedValue` only.
    pub fn from_ev(conservative: &ExpectedValue) -> Self {
        if conservative.e_pnl > 0 {
            Decision::WouldTrade
        } else {
            Decision::WouldNotTrade
        }
    }
}

/// One row in the paper ledger. Bincode-encodable.
///
/// All fields are public so the round-trip test can assert exact
/// equality. `#[derive(Serialize, Deserialize)]` requires the
/// `serde::Serialize` / `serde::Deserialize` traits on every field
/// type — those are derived on the source structs in `dl-sim` /
/// `dl-state` (see commit 5...).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LedgerEntry {
    /// Monotonic sequence number assigned by the writer (the
    /// upstream pipeline owns this). Must be unique per ledger file.
    pub seq: u64,
    /// Canonical id for "this opportunity at this seq". Equal to
    /// `seq` for v1.0; reserved for future use (e.g. a paper-trade
    /// id distinct from a per-decision seq).
    pub entry_id: u64,
    /// Deterministic hash of the cycle's leg sequence
    /// (FNV-1a 64 in `LedgerHash::from_cycle`).
    pub cycle_hash: LedgerHash,
    /// The Phase-4 net-profit estimate for this opportunity.
    pub net: NetProfit,
    /// The optimistic bound (`p_detect = p_win = p_land = 1.0`, no
    /// failed cost). Recorded for reconciliation, not for the gate.
    pub optimistic: ExpectedValue,
    /// The conservative bound (full haircut stack + failed cost).
    /// This is the bound that drives `decision`.
    pub conservative: ExpectedValue,
    /// `WouldTrade` iff `conservative.e_pnl > 0`.
    pub decision: Decision,
}

impl LedgerEntry {
    /// Build a `LedgerEntry` from a cycle, the net-profit estimate,
    /// the dual-bound EV outcome, and the writer-supplied sequence
    /// number.
    ///
    /// `cycle` is consumed only to compute the `cycle_hash` — the
    /// full leg list is *not* stored in the entry, to keep entries
    /// small and the format stable. The cycle can be re-derived
    /// from a capture replay (Phase 6 reconciliation).
    pub fn from_evaluated(cycle: &Cycle, net: NetProfit, outcome: &EvalOutcome, seq: u64) -> Self {
        let cycle_hash = LedgerHash::from_cycle(cycle);
        let decision = Decision::from_ev(&outcome.conservative);
        Self {
            seq,
            entry_id: seq,
            cycle_hash,
            net,
            optimistic: outcome.optimistic,
            conservative: outcome.conservative,
            decision,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dl_core::prob::PROB_SCALE_1E18;
    use dl_sim::cost::CostBreakdown;
    use dl_sim::ev::{
        CompetitionParams, EvalParams, FailedCostModel, LandingParams, LatencyBudget,
    };
    use dl_state::cycle::{Direction, Leg};

    fn leg(pool_byte: u8, dir: Direction) -> Leg {
        let mut pk = [0u8; 32];
        pk[31] = pool_byte;
        Leg {
            pool: dl_state::Pubkey(pk),
            direction: dir,
            weight: 0,
        }
    }

    fn zero_cost() -> CostBreakdown {
        CostBreakdown {
            base_sig_fee_lamports: 0,
            priority_fee_lamports: 0,
            jito_tip_lamports: 0,
            jito_tip_fee_lamports: 0,
            total_lamports: 0,
        }
    }

    fn zero_net(p: i128, bps: i32) -> NetProfit {
        NetProfit {
            input_amount: 1,
            gross_output: 0,
            total_costs: zero_cost(),
            net_profit: p,
            net_profit_bps: bps,
            profitable: p > 0,
        }
    }

    fn zero_ev(p: i128) -> ExpectedValue {
        ExpectedValue {
            e_pnl: p,
            p_detect: dl_sim::ev::Prob::from_scaled_clamped(PROB_SCALE_1E18),
            p_win: dl_sim::ev::Prob::from_scaled_clamped(PROB_SCALE_1E18),
            p_land: dl_sim::ev::Prob::from_scaled_clamped(PROB_SCALE_1E18),
            expected_failed_cost: 0,
        }
    }

    fn outcome(cons: i128) -> EvalOutcome {
        EvalOutcome {
            optimistic: zero_ev(0),
            conservative: zero_ev(cons),
        }
    }

    #[test]
    fn decision_from_e_pnl_positive_is_would_trade() {
        let ev = zero_ev(100);
        assert_eq!(Decision::from_ev(&ev), Decision::WouldTrade);
    }

    #[test]
    fn decision_from_e_pnl_zero_is_would_not_trade() {
        let ev = zero_ev(0);
        assert_eq!(Decision::from_ev(&ev), Decision::WouldNotTrade);
    }

    #[test]
    fn decision_from_e_pnl_negative_is_would_not_trade() {
        let ev = zero_ev(-1);
        assert_eq!(Decision::from_ev(&ev), Decision::WouldNotTrade);
    }

    #[test]
    fn from_evaluated_populates_cycle_hash_and_seq() {
        let cycle = Cycle::new(vec![
            leg(1, Direction::BaseToQuote),
            leg(2, Direction::QuoteToBase),
        ]);
        let net = zero_net(1000, 10);
        let out = outcome(500);
        let entry = LedgerEntry::from_evaluated(&cycle, net, &out, 42);
        assert_eq!(entry.seq, 42);
        assert_eq!(entry.entry_id, 42);
        assert_eq!(entry.cycle_hash, LedgerHash::from_cycle(&cycle));
        assert_eq!(entry.decision, Decision::WouldTrade);
    }

    #[test]
    fn from_evaluated_losing_cycle_records_would_not_trade() {
        let cycle = Cycle::new(vec![
            leg(1, Direction::BaseToQuote),
            leg(2, Direction::QuoteToBase),
        ]);
        let net = zero_net(-1000, -10);
        let out = outcome(-500);
        let entry = LedgerEntry::from_evaluated(&cycle, net, &out, 0);
        assert_eq!(entry.decision, Decision::WouldNotTrade);
    }

    // Reference to the type aliases so unused warnings don't fire
    #[allow(dead_code)]
    fn _typecheck(
        _p: CompetitionParams,
        _l: LatencyBudget,
        _lp: LandingParams,
        _f: FailedCostModel,
        _e: EvalParams,
    ) {
    }
}
