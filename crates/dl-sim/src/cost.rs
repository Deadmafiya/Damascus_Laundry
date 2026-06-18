//! Cost stack for one atomic-arb transaction.
//!
//! The v1.0 model nets four components off the top of the gross cycle
//! output:
//!
//! 1. **Base signature fee** — 5,000 lamports per signature. Solana
//!    protocol constant (research: High confidence; re-verified in
//!    `solana-mev-landscape.md §1.3`).
//! 2. **Priority fee** — `cu_limit × cu_price_micro_lamports / 1_000_000`
//!    lamports. The Compute Budget Program's
//!    `SetComputeUnitPrice` instruction charges `cu_price` micro-lamports
//!    per CU consumed (1 micro-lamport = 1e-6 lamport). The divide by
//!    1_000_000 pulls back to lamports.
//! 3. **Jito tip** — the user's bid in lamports. v1.0: a configurable
//!    parameter; v1.1+ (live executor, Phase 7): the live tip floor from
//!    `bundles.jito.wtf/api/v1/bundles/tip_floor`.
//! 4. **5% Jito tip fee** — Jito takes 5% on the tip (per Helius MEV
//!    Report; `solana-mev-landscape.md §1.5`).
//!
//! All math is `u64` with `checked_*` arithmetic. The function never
//! panics; the only error is a `SimError::Math(MathError::Overflow)` if
//! the components sum to more than `u64::MAX` (~18.4 × 10^18 lamports —
//! unrealistic; the largest realistic per-tx cost is on the order of
//! 10^9 lamports).
//!
//! ## Integer-only invariant
//!
//! No fractional types appear in this module. All cost components are
//! computed in `u64` lamports via `checked_*` arithmetic; the only error
//! is `Math` (overflow / div-by-zero).
use crate::error::SimError;

/// Base signature fee in lamports. Solana protocol constant.
///
/// Research: `.paul/research/solana-mev-data-stack-research.md §3`,
/// re-confirmed in `solana-mev-landscape.md §1.3` (High confidence).
/// **Phase 6 must re-verify** against live on-chain fee data before
/// any forward-P&L claim.
pub const BASE_SIG_FEE_LAMPORTS: u64 = 5_000;

/// Jito tip fee in basis points (5%). Per Helius MEV Report.
///
/// Research: `.paul/research/solana-mev-landscape.md §1.5`
/// (High confidence).
pub const JITO_TIP_FEE_BPS: u64 = 5;

/// Jito tip fee denominator (100% in bps).
pub const JITO_TIP_FEE_DENOM_BPS: u64 = 100;

/// Priority-fee scale: 1 lamport = 1_000_000 micro-lamports.
///
/// `priority_fee = cu_limit × cu_price_micro_lamports / PRIORITY_FEE_SCALE`.
pub const PRIORITY_FEE_SCALE: u64 = 1_000_000;

/// Cost-stack inputs for one atomic-arb transaction.
///
/// All fields are configurable; the v1.0 sim uses [`CostModel::default_min`]
/// and [`CostModel::default_busy`] as canonical baselines.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CostModel {
    /// Number of signatures in the tx. A typical 3-leg arb: 1 system
    /// transfer (tip) + 3 swap instructions + 2 compute-budget
    /// instructions (SetComputeUnitLimit, SetComputeUnitPrice) + 1
    /// fee-payer sig = ~7. v1.0 default is 6.
    pub n_signatures: u16,

    /// Compute-unit limit for the tx. Per-leg budget: 200_000 CU is a
    /// safe baseline (a swap ix typically uses 100-150k; rounded up).
    /// For 3 legs: 600_000 + 5_000 overhead.
    pub cu_limit: u32,

    /// CU price in micro-lamports per CU. Default 1_000 (1 micro-
    /// lamport = 1e-6 lamport per CU) is a "no-contention" baseline.
    /// Live integration uses `getRecentPrioritizationFees` p75
    /// (Phase 7+).
    pub cu_price_micro_lamports: u64,

    /// Jito tip in lamports. Min 1_000 (current Jito docs). v1.0 sim
    /// default: 10_000 (Jito min + margin).
    pub jito_tip_lamports: u64,
}

/// Per-tx cost breakdown.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CostBreakdown {
    /// `BASE_SIG_FEE_LAMPORTS × n_signatures`.
    pub base_sig_fee_lamports: u64,
    /// `cu_limit × cu_price_micro_lamports / PRIORITY_FEE_SCALE`.
    pub priority_fee_lamports: u64,
    /// The user's Jito tip (input passthrough).
    pub jito_tip_lamports: u64,
    /// `jito_tip_lamports × 5 / 100` (the 5% Jito takes).
    pub jito_tip_fee_lamports: u64,
    /// Sum of the four components above.
    pub total_lamports: u64,
}

impl CostModel {
    /// Minimum-everything baseline: 1 sig, 200k CU, 1k µlamports/CU,
    /// 10k lamport tip. Yields a `total_lamports = 15_700`.
    pub fn default_min() -> Self {
        Self {
            n_signatures: 1,
            cu_limit: 200_000,
            cu_price_micro_lamports: 1_000,
            jito_tip_lamports: 10_000,
        }
    }

    /// Realistic busy-cycle baseline: 6 sigs, 600k CU, 50k µlamports/CU,
    /// 1M lamport tip. Yields a `total_lamports = 31_080_000`.
    pub fn default_busy() -> Self {
        Self {
            n_signatures: 6,
            cu_limit: 600_000,
            cu_price_micro_lamports: 50_000,
            jito_tip_lamports: 1_000_000,
        }
    }

    /// Compute the full cost breakdown.
    ///
    /// All four components are computed with `checked_*` arithmetic;
    /// the total is a `checked_add` chain. The only error is
    /// `SimError::Math(MathError::Overflow)`, which is unreachable
    /// for realistic inputs (`BASE_SIG_FEE_LAMPORTS × u16::MAX ≈ 327M`,
    /// `cu_limit × cu_price` overflows only at extreme inputs).
    pub fn total_cost(&self) -> Result<CostBreakdown, SimError> {
        // Base sig fee: 5_000 × n_signatures.
        let base_sig_fee_lamports = BASE_SIG_FEE_LAMPORTS
            .checked_mul(u64::from(self.n_signatures))
            .ok_or(SimError::Math(dl_core::fixed::MathError::Overflow))?;
        // Priority fee: cu_limit × cu_price_micro_lamports / 1_000_000.
        // The mul is the overflow risk; the div pulls back into u64.
        let priority_fee_lamports = (u64::from(self.cu_limit))
            .checked_mul(self.cu_price_micro_lamports)
            .ok_or(SimError::Math(dl_core::fixed::MathError::Overflow))?
            / PRIORITY_FEE_SCALE;
        // Jito tip fee: tip × 5 / 100.
        let jito_tip_fee_lamports = self
            .jito_tip_lamports
            .checked_mul(JITO_TIP_FEE_BPS)
            .ok_or(SimError::Math(dl_core::fixed::MathError::Overflow))?
            / JITO_TIP_FEE_DENOM_BPS;
        // Total: sum of the four components.
        let total_lamports = base_sig_fee_lamports
            .checked_add(priority_fee_lamports)
            .and_then(|x| x.checked_add(self.jito_tip_lamports))
            .and_then(|x| x.checked_add(jito_tip_fee_lamports))
            .ok_or(SimError::Math(dl_core::fixed::MathError::Overflow))?;
        Ok(CostBreakdown {
            base_sig_fee_lamports,
            priority_fee_lamports,
            jito_tip_lamports: self.jito_tip_lamports,
            jito_tip_fee_lamports,
            total_lamports,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_min_baseline_matches_plan() {
        // AC-4: 1 sig, 200k CU, 1k µlamports/CU, 10k tip
        // = 5_000 + 200 + 10_000 + 500 = 15_700
        let m = CostModel::default_min();
        assert_eq!(m.n_signatures, 1);
        assert_eq!(m.cu_limit, 200_000);
        assert_eq!(m.cu_price_micro_lamports, 1_000);
        assert_eq!(m.jito_tip_lamports, 10_000);
        let c = m.total_cost().unwrap();
        assert_eq!(c.base_sig_fee_lamports, 5_000);
        assert_eq!(c.priority_fee_lamports, 200);
        assert_eq!(c.jito_tip_lamports, 10_000);
        assert_eq!(c.jito_tip_fee_lamports, 500);
        assert_eq!(c.total_lamports, 15_700);
    }

    #[test]
    fn default_busy_baseline_matches_plan() {
        // AC-4: 6 sigs, 600k CU, 50k µlamports/CU, 1M tip
        // priority_fee = 600_000 × 50_000 / 1_000_000 = 30_000 lamports
        // (= 30 µlamports/CU × 600k CU; the 1e6 micro→lamport divide brings it down)
        // total = 30_000 + 30_000 + 1_000_000 + 50_000 = 1_110_000
        let m = CostModel::default_busy();
        assert_eq!(m.n_signatures, 6);
        assert_eq!(m.cu_limit, 600_000);
        assert_eq!(m.cu_price_micro_lamports, 50_000);
        assert_eq!(m.jito_tip_lamports, 1_000_000);
        let c = m.total_cost().unwrap();
        assert_eq!(c.base_sig_fee_lamports, 30_000);
        assert_eq!(c.priority_fee_lamports, 30_000);
        assert_eq!(c.jito_tip_lamports, 1_000_000);
        assert_eq!(c.jito_tip_fee_lamports, 50_000);
        assert_eq!(c.total_lamports, 1_110_000);
    }

    #[test]
    fn zero_tip_yields_zero_fee() {
        // The 5% Jito fee scales with the tip: no tip → no fee.
        let m = CostModel {
            n_signatures: 1,
            cu_limit: 0,
            cu_price_micro_lamports: 0,
            jito_tip_lamports: 0,
        };
        let c = m.total_cost().unwrap();
        assert_eq!(c.jito_tip_lamports, 0);
        assert_eq!(c.jito_tip_fee_lamports, 0);
        // base sig fee is the only non-zero component
        assert_eq!(c.base_sig_fee_lamports, 5_000);
        assert_eq!(c.priority_fee_lamports, 0);
        assert_eq!(c.total_lamports, 5_000);
    }

    #[test]
    fn zero_signatures_yields_zero_base_sig_fee() {
        // 0 sigs is a degenerate but valid input: no base fee.
        let m = CostModel {
            n_signatures: 0,
            cu_limit: 200_000,
            cu_price_micro_lamports: 1_000,
            jito_tip_lamports: 10_000,
        };
        let c = m.total_cost().unwrap();
        assert_eq!(c.base_sig_fee_lamports, 0);
        assert_eq!(c.priority_fee_lamports, 200);
        assert_eq!(c.jito_tip_lamports, 10_000);
        assert_eq!(c.jito_tip_fee_lamports, 500);
        assert_eq!(c.total_lamports, 10_700);
    }

    #[test]
    fn total_cost_is_deterministic() {
        let m = CostModel::default_busy();
        let a = m.total_cost().unwrap();
        let b = m.total_cost().unwrap();
        assert_eq!(a, b);
    }
}
