//! Unit tests for `CostModel` + `CostBreakdown` (AC-4).
//!
//! Two baseline scenarios from the plan, plus three edge cases
//! (zero tip, zero sigs, determinism). All math is `u64` lamport
//! arithmetic; the expected totals are hand-computed.

use dl_sim::cost::{CostBreakdown, CostModel, JITO_TIP_FEE_BPS, PRIORITY_FEE_SCALE};

/// Default-min baseline (AC-4): n=1, cu=200_000, cu_price=1_000, tip=10_000.
/// Expected: total = 5_000 + 200 + 10_000 + 500 = 15_700.
#[test]
fn default_min_yields_15_700_lamports() {
    let m = CostModel::default_min();
    let c: CostBreakdown = m.total_cost().unwrap();
    assert_eq!(c.base_sig_fee_lamports, 5_000);
    assert_eq!(c.priority_fee_lamports, 200);
    assert_eq!(c.jito_tip_lamports, 10_000);
    assert_eq!(c.jito_tip_fee_lamports, 500);
    assert_eq!(c.total_lamports, 15_700);
}

/// Default-busy baseline (AC-4): n=6, cu=600_000, cu_price=50_000, tip=1_000_000.
/// Expected: priority_fee = 600_000 × 50_000 / 1_000_000 = 30_000 lamports
/// (= 30 µlamports/CU × 600k CU; the 1e6 micro→lamport divide brings it down).
/// Total = 30_000 + 30_000 + 1_000_000 + 50_000 = 1_110_000.
#[test]
fn default_busy_yields_1_110_000_lamports() {
    let m = CostModel::default_busy();
    let c = m.total_cost().unwrap();
    assert_eq!(c.base_sig_fee_lamports, 30_000);
    assert_eq!(c.priority_fee_lamports, 30_000);
    assert_eq!(c.jito_tip_lamports, 1_000_000);
    assert_eq!(c.jito_tip_fee_lamports, 50_000);
    assert_eq!(c.total_lamports, 1_110_000);
}

/// Zero tip → zero Jito tip fee. The 5% scales with the tip; no tip = no fee.
#[test]
fn zero_tip_yields_zero_tip_fee() {
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

/// Zero signatures → zero base sig fee. Other components still apply.
#[test]
fn zero_signatures_yields_zero_base_sig_fee() {
    let m = CostModel {
        n_signatures: 0,
        cu_limit: 200_000,
        cu_price_micro_lamports: 1_000,
        jito_tip_lamports: 10_000,
    };
    let c = m.total_cost().unwrap();
    assert_eq!(c.base_sig_fee_lamports, 0);
    // priority fee still applies: 200_000 × 1_000 / 1_000_000 = 200
    assert_eq!(c.priority_fee_lamports, 200);
    // tip and tip fee still apply
    assert_eq!(c.jito_tip_lamports, 10_000);
    assert_eq!(c.jito_tip_fee_lamports, 500);
    assert_eq!(c.total_lamports, 10_700);
}

/// Determinism: two calls on the same `CostModel` return byte-identical
/// `CostBreakdown`s.
#[test]
fn total_cost_is_deterministic() {
    let m = CostModel::default_busy();
    let a = m.total_cost().unwrap();
    let b = m.total_cost().unwrap();
    assert_eq!(a, b);
}

/// Verify the public constants are what the plan documents.
#[test]
fn public_constants_match_plan() {
    // Base sig fee: 5,000 lamports (Solana protocol constant)
    assert_eq!(dl_sim::cost::BASE_SIG_FEE_LAMPORTS, 5_000);
    // Jito tip fee: 5% (Helius MEV Report)
    assert_eq!(JITO_TIP_FEE_BPS, 5);
    assert_eq!(dl_sim::cost::JITO_TIP_FEE_DENOM_BPS, 100);
    // Priority fee scale: 1_000_000 micro-lamports per lamport
    assert_eq!(PRIORITY_FEE_SCALE, 1_000_000);
}
