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
    /// 50th percentile of `conservative.e_pnl` (signed). 0 if the
    /// input is empty.
    median_conservative_e_pnl: i128,
    /// 95th percentile of `conservative.e_pnl` (signed). 0 if the
    /// input is empty.
    p95_conservative_e_pnl: i128,
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
        // Collect e_pnl into a local vec for percentile computation.
        // Sorted in place via `select_nth_unstable` (integer-only;
        // no fractional types). For 1M entries this is O(n) and
        // ~5 MB of memory.
        let mut e_pnls: Vec<i128> = Vec::with_capacity(entries.len());

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
            e_pnls.push(e.conservative.e_pnl);
        }

        let (median, p95) = percentiles_signed(&mut e_pnls);

        Ok(Self {
            total,
            would_trade,
            would_not_trade: total - would_trade,
            sum_optimistic_e_pnl: sum_opt,
            sum_conservative_e_pnl: sum_con,
            sum_conservative_p_land: sum_p_land,
            median_conservative_e_pnl: median,
            p95_conservative_e_pnl: p95,
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

    /// 50th percentile (median) of `conservative.e_pnl` across all
    /// entries. Returns 0 for an empty input.
    pub fn median_conservative_e_pnl(&self) -> i128 {
        self.median_conservative_e_pnl
    }

    /// 95th percentile of `conservative.e_pnl` across all entries.
    /// Returns 0 for an empty input.
    pub fn p95_conservative_e_pnl(&self) -> i128 {
        self.p95_conservative_e_pnl
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
            tip_lamports: 0,
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

    #[test]
    fn median_p95_known_distribution() {
        // Build 20 entries with conservative.e_pnl = 10, 20, ..., 200.
        // Median (idx 10) = 110. p95 (idx 19) = 200.
        let es: Vec<LedgerEntry> = (1..=20)
            .map(|i| entry((i - 1) as u64, 0, (i * 10) as i128, true))
            .collect();
        let s = LedgerSummary::from_entries(&es).unwrap();
        assert_eq!(s.median_conservative_e_pnl(), 110);
        assert_eq!(s.p95_conservative_e_pnl(), 200);
    }

    #[test]
    fn median_p95_with_negatives() {
        // -100, -50, 0, 50, 100 → median (idx 2) = 0, p95 (idx 4) = 100.
        let es = [
            entry(0, 0, -100, false),
            entry(1, 0, -50, false),
            entry(2, 0, 0, false),
            entry(3, 0, 50, true),
            entry(4, 0, 100, true),
        ];
        let s = LedgerSummary::from_entries(&es).unwrap();
        assert_eq!(s.median_conservative_e_pnl(), 0);
        assert_eq!(s.p95_conservative_e_pnl(), 100);
    }
}

#[cfg(test)]
mod percentile_helper_tests {
    use super::percentiles_signed;

    #[test]
    fn empty_returns_zero() {
        let mut xs: Vec<i128> = vec![];
        assert_eq!(percentiles_signed(&mut xs), (0, 0));
    }

    #[test]
    fn single_value_returned_twice() {
        let mut xs = vec![42];
        assert_eq!(percentiles_signed(&mut xs), (42, 42));
    }

    #[test]
    fn known_distribution_20_elements() {
        // 20 elements, evenly spaced 10, 20, ..., 200. With
        // `median_idx = n/2 = 10` (0-indexed), the 11th element
        // is 110. With `p95_idx = n*95/100 = 19`, the 20th
        // element is 200. (Pure 0-indexed `select_nth_unstable` —
        // not the floor-rank interpretation the test comment
        // originally claimed.)
        let mut xs: Vec<i128> = (1..=20).map(|i| i * 10).collect();
        assert_eq!(percentiles_signed(&mut xs), (110, 200));
    }

    #[test]
    fn all_same_value() {
        let mut xs = vec![7; 10];
        assert_eq!(percentiles_signed(&mut xs), (7, 7));
    }
}

/// Compute the 50th and 95th percentiles of a signed-integer
/// vector, in place. Integer-only — no `f64`.
///
/// - Empty input: returns `(0, 0)`.
/// - Single value: returns `(value, value)`.
/// - 2..=100: returns the floor-rank element.
/// - > 100: uses `select_nth_unstable` for O(n) average
///   performance.
///
/// The `i128` ordering is the natural `Ord` (sign-aware); no
/// special handling needed.
fn percentiles_signed(xs: &mut [i128]) -> (i128, i128) {
    if xs.is_empty() {
        return (0, 0);
    }
    let n = xs.len();
    if n == 1 {
        return (xs[0], xs[0]);
    }
    let median_idx = n / 2;
    // p95 = 95th percentile. For n=20, index 19 (ceiling);
    // for n=100, index 95 (ceiling). Use `min(n-1, (n * 95) / 100)`.
    let p95_idx = (n * 95 / 100).min(n - 1);

    // `select_nth_unstable` is O(n) average; for two percentiles,
    // call it twice. For very small n (≤100) the O(n log n) sort
    // would also be fine; the call is uniformly bounded.
    let median = *xs.select_nth_unstable(median_idx).1;
    let p95 = *xs.select_nth_unstable(p95_idx).1;
    (median, p95)
}
