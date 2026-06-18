//! Aggregate counts over a sequence of `LedgerEntry`s.
//!
//! Used for "what did the paper ledger do?" reports (Phase 6 builds
//! the human-readable `Display` impl). All sums are integer-only; no
//! `f32`/`f64`. On overflow the summary returns `LedgerError::Math`
use crate::entry::{Decision, LedgerEntry};
use crate::error::LedgerError;

/// Aggregate statistics over a sequence of `LedgerEntry`s.
///
/// Plain accessor methods (`total`, `would_trade`, ...) expose the
/// fields. The fields are private; the struct is constructed only via
/// [`LedgerSummary::from_entries`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LedgerSummary {
    total: u64,
    would_trade: u64,
    would_not_trade: u64,
    sum_optimistic_e_pnl: i128,
    sum_conservative_e_pnl: i128,
    sum_conservative_p_land: u128,
}

impl LedgerSummary {
    /// Build a summary from a sequence of entries.
    ///
    /// Returns `LedgerError::Math` on integer overflow in any sum
    /// (unreachable for v1.0 magnitudes; kept as a hard error so a
    /// future run doesn't silently wrap).
    pub fn from_entries(entries: &[LedgerEntry]) -> Result<Self, LedgerError> {
        let total = entries.len() as u64;
        let mut would_trade: u64 = 0;
        let mut sum_opt: i128 = 0;
        let mut sum_con: i128 = 0;
        let mut sum_p_land: u128 = 0;

        for e in entries {
            if e.decision == Decision::WouldTrade {
                would_trade = would_trade.checked_add(1).ok_or(LedgerError::Math)?;
            }
            sum_opt = sum_opt
                .checked_add(e.optimistic.e_pnl)
                .ok_or(LedgerError::Math)?;
            sum_con = sum_con
                .checked_add(e.conservative.e_pnl)
                .ok_or(LedgerError::Math)?;
            sum_p_land = sum_p_land
                .checked_add(e.conservative.p_land.scaled())
                .ok_or(LedgerError::Math)?;
        }

        Ok(Self {
            total,
            would_trade,
            would_not_trade: total - would_trade,
            sum_optimistic_e_pnl: sum_opt,
            sum_conservative_e_pnl: sum_con,
            sum_conservative_p_land: sum_p_land,
        })
    }

    /// Number of entries in the input.
    pub fn total(&self) -> u64 {
        self.total
    }

    /// Number of entries with `decision == WouldTrade`.
    pub fn would_trade(&self) -> u64 {
        self.would_trade
    }

    /// Number of entries with `decision == WouldNotTrade`.
    pub fn would_not_trade(&self) -> u64 {
        self.would_not_trade
    }

    /// Sum of `optimistic.e_pnl` across all entries.
    pub fn sum_optimistic_e_pnl(&self) -> i128 {
        self.sum_optimistic_e_pnl
    }

    /// Sum of `conservative.e_pnl` across all entries.
    pub fn sum_conservative_e_pnl(&self) -> i128 {
        self.sum_conservative_e_pnl
    }

    /// Sum of `conservative.p_land` (ppm, 1e18 scale) across all entries.
    pub fn sum_conservative_p_land(&self) -> u128 {
        self.sum_conservative_p_land
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hash::LedgerHash;
    use dl_core::prob::PROB_SCALE_1E18;
    use dl_sim::cost::CostBreakdown;
    use dl_sim::ev::{ExpectedValue, Prob};

    fn empty_costs() -> CostBreakdown {
        CostBreakdown {
            base_sig_fee_lamports: 0,
            priority_fee_lamports: 0,
            jito_tip_lamports: 0,
            jito_tip_fee_lamports: 0,
            total_lamports: 0,
        }
    }

    fn ev(e_pnl: i128) -> ExpectedValue {
        ExpectedValue {
            e_pnl,
            p_detect: Prob::from_scaled_clamped(PROB_SCALE_1E18),
            p_win: Prob::from_scaled_clamped(PROB_SCALE_1E18),
            p_land: Prob::from_scaled_clamped(PROB_SCALE_1E18),
            expected_failed_cost: 0,
        }
    }

    fn entry(seq: u64, opt: i128, con: i128, trade: bool) -> LedgerEntry {
        LedgerEntry {
            seq,
            entry_id: seq,
            cycle_hash: LedgerHash(seq),
            net: dl_sim::net_profit::NetProfit {
                input_amount: 0,
                gross_output: 0,
                total_costs: empty_costs(),
                net_profit: 0,
                net_profit_bps: 0,
                profitable: false,
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
    fn empty_input() {
        let s = LedgerSummary::from_entries(&[]).unwrap();
        assert_eq!(s.total(), 0);
        assert_eq!(s.would_trade(), 0);
        assert_eq!(s.would_not_trade(), 0);
        assert_eq!(s.sum_optimistic_e_pnl(), 0);
        assert_eq!(s.sum_conservative_e_pnl(), 0);
        assert_eq!(s.sum_conservative_p_land(), 0);
    }

    #[test]
    fn single_would_trade() {
        let s = LedgerSummary::from_entries(&[entry(0, 1_000, 1_000, true)]).unwrap();
        assert_eq!(s.total(), 1);
        assert_eq!(s.would_trade(), 1);
        assert_eq!(s.would_not_trade(), 0);
        assert_eq!(s.sum_optimistic_e_pnl(), 1_000);
        assert_eq!(s.sum_conservative_e_pnl(), 1_000);
    }

    #[test]
    fn mixed_trade_and_not_trade() {
        let es = [entry(0, 100, 50, true), entry(1, -200, -300, false)];
        let s = LedgerSummary::from_entries(&es).unwrap();
        assert_eq!(s.total(), 2);
        assert_eq!(s.would_trade(), 1);
        assert_eq!(s.would_not_trade(), 1);
        assert_eq!(s.sum_optimistic_e_pnl(), 100 - 200);
        assert_eq!(s.sum_conservative_e_pnl(), 50 - 300);
    }
}
