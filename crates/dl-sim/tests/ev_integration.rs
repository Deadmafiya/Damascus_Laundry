//! End-to-end integration tests for `dl_sim::ev::evaluate`.
//!
//! Drives `evaluate` against constructed [`NetProfit`]s (no sizer required),
//! verifying the dual-bound contract: optimistic >= conservative, conservative
//! strictly below the raw net for a profitable cycle, and EV never positive
//! for a losing cycle.
//!
//! Reuses the same cycle/registry/pool fixtures as `simulate_integration.rs`
//! and `net_profit_unit.rs` so the test cases are recognizable.

use dl_core::prob::PROB_SCALE_1E18;
use dl_sim::cost::CostBreakdown;
use dl_sim::ev::{
    evaluate, CompetitionParams, EvalParams, FailedCostModel, LandingParams, LatencyBudget, Prob,
    SubmitPath, JITO_AUCTION_MS, JITO_TICK_MS, PPM_ONE,
};
use dl_sim::net_profit::NetProfit;

fn make_net_profit(net: i128, bps: i32, input: u128) -> NetProfit {
    NetProfit {
        input_amount: input,
        gross_output: 0,
        total_costs: CostBreakdown {
            base_sig_fee_lamports: 0,
            priority_fee_lamports: 0,
            jito_tip_lamports: 0,
            jito_tip_fee_lamports: 0,
            total_lamports: 0,
        },
        net_profit: net,
        net_profit_bps: bps,
        profitable: net > 0,
    }
}

#[test]
fn profitable_cycle_conservative_below_raw_and_below_optimistic() {
    // +1_000_000 base units net, 5 bps richness (below the conservative 10-bp
    // threshold so winner's-curse doesn't kick in).
    let net = make_net_profit(1_000_000, 5, 200_000_000);
    let out = evaluate(
        &net,
        &EvalParams::optimistic(),
        &EvalParams::conservative_default(),
    );

    assert_eq!(out.optimistic.e_pnl, net.net_profit);
    assert!(out.conservative.e_pnl < net.net_profit);
    assert!(out.conservative.e_pnl <= out.optimistic.e_pnl);
    // Conservative should still be positive for a meaningful profitable cycle.
    assert!(out.conservative.e_pnl > 0);
}

#[test]
fn rich_cycle_loses_to_winners_curse() {
    // Lean profitable cycle: 5 bps, p_win == base (30%).
    let lean = make_net_profit(1_000_000, 5, 200_000_000);
    // Rich profitable cycle: 100 bps, decay reduces p_win sharply.
    let rich = make_net_profit(1_000_000, 100, 200_000_000);

    let lean_out = evaluate(
        &lean,
        &EvalParams::optimistic(),
        &EvalParams::conservative_default(),
    );
    let rich_out = evaluate(
        &rich,
        &EvalParams::optimistic(),
        &EvalParams::conservative_default(),
    );

    // p_win reported on the conservative side is lower for the rich cycle.
    assert!(rich_out.conservative.p_win < lean_out.conservative.p_win);
    // Therefore conservative EV is strictly lower (or equal if the rich
    // cycle's gross also happened to land the same; here the bps differ so EV
    // differs).
    assert!(rich_out.conservative.e_pnl < lean_out.conservative.e_pnl);
}

#[test]
fn losing_cycle_never_positive_under_either_bound() {
    let net = make_net_profit(-50_000, -3, 200_000_000);
    let out = evaluate(
        &net,
        &EvalParams::optimistic(),
        &EvalParams::conservative_default(),
    );
    assert!(
        out.optimistic.e_pnl <= 0,
        "optimistic ev: {}",
        out.optimistic.e_pnl
    );
    assert!(
        out.conservative.e_pnl <= 0,
        "conservative ev: {}",
        out.conservative.e_pnl
    );
    // Conservative is at most the optimistic (more negative or equal).
    assert!(out.conservative.e_pnl <= out.optimistic.e_pnl);
}

#[test]
fn spam_failed_cost_lowers_conservative_ev() {
    // A profitable cycle evaluated under spam failed-cost vs Jito failed-cost.
    // Jito failed-cost is zero, so conservative EV under Jito should be >= the
    // conservative EV under spam (for the same NetProfit).
    let net = make_net_profit(1_000_000_000, 5, 200_000_000);

    let spam = EvalParams {
        p_detect: Prob::from_ppm(700_000).unwrap(),
        competition: CompetitionParams::conservative_default(),
        latency: LatencyBudget::conservative_default(),
        landing: LandingParams::conservative_default(),
        failed: FailedCostModel {
            attempts_per_win: 24,
            per_attempt_lamports: 5_000,
            path: SubmitPath::Spam,
        },
        tip_lamports: 0,
    };
    let jito = EvalParams {
        failed: FailedCostModel {
            attempts_per_win: 0,
            per_attempt_lamports: 0,
            path: SubmitPath::JitoBundle,
        },
        ..spam
    };

    let spam_out = evaluate(&net, &EvalParams::optimistic(), &spam);
    let jito_out = evaluate(&net, &EvalParams::optimistic(), &jito);

    // Jito (zero failed cost) >= spam (nonzero failed cost).
    assert!(jito_out.conservative.e_pnl > spam_out.conservative.e_pnl);
    assert_eq!(spam_out.conservative.expected_failed_cost, 24 * 5_000);
    assert_eq!(jito_out.conservative.expected_failed_cost, 0);
}

#[test]
fn evaluate_is_byte_identical_across_calls() {
    let net = make_net_profit(1_000_000, 5, 200_000_000);
    let a = evaluate(
        &net,
        &EvalParams::optimistic(),
        &EvalParams::conservative_default(),
    );
    let b = evaluate(
        &net,
        &EvalParams::optimistic(),
        &EvalParams::conservative_default(),
    );
    assert_eq!(a, b);
}

#[test]
fn p_land_under_conservative_latency_is_below_one() {
    // The conservative default latency is ~250ms. Conservative landing params
    // give a Jito-tick grace + 0.2% decay per ms. This should land at < 1.0.
    let latency = LatencyBudget::conservative_default();
    let landing = LandingParams::conservative_default();
    let pl = dl_sim::ev::p_land(&latency, &landing);
    assert!(pl < Prob::ONE, "expected p_land < 1.0, got {}", pl.to_ppm());
    // Sanity: total latency is well above the Jito tick grace.
    assert!(latency.total_ms() > JITO_TICK_MS);
    assert!(latency.total_ms() >= JITO_AUCTION_MS);
}

#[test]
fn p_win_conservative_decreases_across_threshold() {
    let p = CompetitionParams::conservative_default();
    let at = dl_sim::ev::p_win(p.richness_threshold_bps, &p);
    let just_above = dl_sim::ev::p_win(p.richness_threshold_bps + 1, &p);
    let far_above = dl_sim::ev::p_win(p.richness_threshold_bps + 1_000, &p);
    assert_eq!(at, Prob::from_ppm(p.base_win_ppm).unwrap());
    assert!(just_above < at);
    assert!(far_above < just_above);
}

#[test]
fn jito_constants_match_research() {
    // Anchor: the Jito auction window is ~200 ms, tick cadence ~50 ms. If
    // these change the latency model needs to be re-tuned.
    assert_eq!(JITO_AUCTION_MS, 200);
    assert_eq!(JITO_TICK_MS, 50);
    assert_eq!(PPM_ONE, 1_000_000);
}

#[test]
fn optimistic_eval_is_identity_on_profitable() {
    // Under the *optimistic* param set, p_detect = p_win = p_land = 1.0 and
    // failed cost = 0, so EV should equal the raw net exactly.
    let net = make_net_profit(1_000_000, 5, 200_000_000);
    let out = evaluate(
        &net,
        &EvalParams::optimistic(),
        &EvalParams::conservative_default(),
    );
    assert_eq!(out.optimistic.e_pnl, net.net_profit);
    assert_eq!(out.optimistic.expected_failed_cost, 0);
    assert_eq!(out.optimistic.p_detect, Prob::ONE);
    assert_eq!(out.optimistic.p_win, Prob::ONE);
    assert_eq!(out.optimistic.p_land, Prob::ONE);
}

#[test]
fn no_trade_net_profit_yields_non_positive_ev() {
    // A NoTrade-style NetProfit: profitable = false, net_profit = 0 or
    // negative. Even if the bps is positive, EV should not be positive.
    let net = NetProfit {
        input_amount: 0,
        gross_output: 0,
        total_costs: CostBreakdown {
            base_sig_fee_lamports: 0,
            priority_fee_lamports: 0,
            jito_tip_lamports: 0,
            jito_tip_fee_lamports: 0,
            total_lamports: 0,
        },
        net_profit: 0,
        net_profit_bps: 0,
        profitable: false,
    };
    let out = evaluate(
        &net,
        &EvalParams::optimistic(),
        &EvalParams::conservative_default(),
    );
    assert_eq!(out.optimistic.e_pnl, 0);
    // Conservative still subtracts the failed-cost stack even for a zero-net
    // cycle (the failed cost is real, regardless of the input).
    assert_eq!(
        out.conservative.e_pnl,
        -(out.conservative.expected_failed_cost as i128)
    );
    assert!(out.conservative.e_pnl <= 0);
    // PROB_SCALE_1E18 sanity check.
    assert!(PROB_SCALE_1E18 > PPM_ONE as u128);
}
