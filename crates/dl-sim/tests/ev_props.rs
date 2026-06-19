//! Property tests for `dl_sim::ev`.
//!
//! Covers the structural invariants the plan called out (AC-1..AC-5):
//! - p_win non-increasing in richness
//! - p_land non-increasing in latency
//! - Prob::combine result always <= both inputs and <= 1.0
//! - conservative <= optimistic for random inputs
//! - evaluate never panics on random NetProfit/param combinations

use dl_core::prob::PROB_SCALE_1E18;
use dl_sim::cost::CostBreakdown;
use dl_sim::ev::{
    evaluate, p_land, p_win, CompetitionParams, EvalParams, FailedCostModel, LandingParams,
    LatencyBudget, Prob, SubmitPath, JITO_AUCTION_MS, PPM_ONE,
};
use dl_sim::net_profit::NetProfit;

use proptest::prelude::*;

// ---------- strategies ----------

fn arb_prob_ppm() -> impl Strategy<Value = u32> {
    0u32..=PPM_ONE
}

fn arb_competition() -> impl Strategy<Value = CompetitionParams> {
    (
        arb_prob_ppm(),  // base_win_ppm
        0i32..10_000i32, // richness_threshold_bps
        0u32..50_000u32, // decay_ppm_per_bps
    )
        .prop_map(|(base, threshold, decay)| CompetitionParams {
            base_win_ppm: base,
            richness_threshold_bps: threshold,
            decay_ppm_per_bps: decay,
        })
}

fn arb_latency() -> impl Strategy<Value = LatencyBudget> {
    (
        0u32..100u32,
        0u32..50u32,
        0u32..50u32,
        0u32..200u32,
        0u32..JITO_AUCTION_MS + 100u32,
    )
        .prop_map(|(a, b, c, d, e)| LatencyBudget {
            t_detect_ms: a,
            t_decide_ms: b,
            t_build_ms: c,
            t_network_ms: d,
            t_auction_ms: e,
        })
}

fn arb_landing() -> impl Strategy<Value = LandingParams> {
    (0u32..500u32, 0u32..10_000u32).prop_map(|(grace, decay)| LandingParams {
        grace_ms: grace,
        decay_ppm_per_ms: decay,
    })
}

fn arb_failed() -> impl Strategy<Value = FailedCostModel> {
    (0u32..100u32, 0u64..100_000u64, any::<bool>()).prop_map(|(attempts, per, is_spam)| {
        FailedCostModel {
            attempts_per_win: attempts,
            per_attempt_lamports: per,
            path: if is_spam {
                SubmitPath::Spam
            } else {
                SubmitPath::JitoBundle
            },
        }
    })
}

fn arb_net_profit() -> impl Strategy<Value = NetProfit> {
    // Bound magnitudes so we don't waste time on proptests with absurdly
    // large numbers; the interesting part is the signed math.
    (
        -1_000_000_000i128..1_000_000_000i128,
        -1000i32..1000i32,
        0u128..1_000_000_000u128,
    )
        .prop_map(|(net, bps, input)| NetProfit {
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
        })
}

fn arb_eval_params() -> impl Strategy<Value = EvalParams> {
    (
        arb_prob_ppm(),
        arb_competition(),
        arb_latency(),
        arb_landing(),
        arb_failed(),
    )
        .prop_map(
            |(p_detect, competition, latency, landing, failed)| EvalParams {
                p_detect: Prob::from_ppm(p_detect).unwrap(),
                competition,
                latency,
                landing,
                failed,
                tip_lamports: 0,
            },
        )
}

// ---------- AC-3: p_win non-increasing in richness ----------

proptest! {
    #[test]
    fn p_win_is_nonincreasing_in_richness(
        params in arb_competition(),
        richness in -1000i32..10_000i32,
    ) {
        let base = p_win(0, &params);
        let higher = p_win(richness, &params);
        if richness > 0 {
            prop_assert!(higher <= base, "p_win must be non-increasing: base={}, richness={}, higher={}",
                base.to_ppm(), richness, higher.to_ppm());
        }
    }

    #[test]
    fn p_win_below_threshold_is_constant(
        params in arb_competition(),
        delta in 0i32..1000i32,
    ) {
        let below = p_win(params.richness_threshold_bps.saturating_sub(delta), &params);
        let at = p_win(params.richness_threshold_bps, &params);
        prop_assert_eq!(below, at, "below threshold must equal threshold value");
    }
}

// ---------- AC-4: p_land non-increasing in latency ----------

proptest! {
    #[test]
    fn p_land_is_nonincreasing_in_latency(
        landing in arb_landing(),
        extra in 0u32..500u32,
    ) {
        let lo = LatencyBudget {
            t_auction_ms: landing.grace_ms,
            ..LatencyBudget::optimistic()
        };
        let hi = LatencyBudget {
            t_auction_ms: landing.grace_ms.saturating_add(extra),
            ..LatencyBudget::optimistic()
        };
        let pl_lo = p_land(&lo, &landing);
        let pl_hi = p_land(&hi, &landing);
        prop_assert!(pl_hi <= pl_lo, "p_land must be non-increasing: lo={}, hi={}",
            pl_lo.to_ppm(), pl_hi.to_ppm());
    }

    #[test]
    fn p_land_under_grace_is_one(landing in arb_landing()) {
        let b = LatencyBudget {
            t_auction_ms: landing.grace_ms,
            ..LatencyBudget::optimistic()
        };
        prop_assert_eq!(p_land(&b, &landing), Prob::ONE);
    }
}

// ---------- AC-1: Prob::combine ----------

proptest! {
    #[test]
    fn combine_le_inputs(a in arb_prob_ppm(), b in arb_prob_ppm()) {
        let pa = Prob::from_ppm(a).unwrap();
        let pb = Prob::from_ppm(b).unwrap();
        let c = pa.combine(pb);
        prop_assert!(c <= pa, "combine result must be <= first input: c={}, a={}",
            c.to_ppm(), pa.to_ppm());
        prop_assert!(c <= pb, "combine result must be <= second input: c={}, b={}",
            c.to_ppm(), pb.to_ppm());
        prop_assert!(c.scaled() <= PROB_SCALE_1E18, "combine result must be <= 1.0");
    }

    #[test]
    fn combine_with_one_is_identity(a in arb_prob_ppm()) {
        let pa = Prob::from_ppm(a).unwrap();
        prop_assert_eq!(Prob::ONE.combine(pa), pa);
        prop_assert_eq!(pa.combine(Prob::ONE), pa);
    }

    #[test]
    fn combine_with_zero_is_zero(a in arb_prob_ppm()) {
        let pa = Prob::from_ppm(a).unwrap();
        prop_assert_eq!(Prob::ZERO.combine(pa), Prob::ZERO);
        prop_assert_eq!(pa.combine(Prob::ZERO), Prob::ZERO);
    }
}

// ---------- AC-2 + AC-5: evaluate dual bounds ----------
//
// The contract (from the plan, AC-2 + AC-5):
//   - For a *profitable* NetProfit with non-degenerate conservative
//     probabilities, conservative.e_pnl < net.net_profit AND
//     conservative.e_pnl <= optimistic.e_pnl.
//   - For a *losing* NetProfit (net_profit <= 0), BOTH bounds must be <= 0.
//     The relationship between the two can go either way (the conservative
//     haircut shrinks the loss magnitude, while the failed cost can push it
//     back down — depending on which dominates).
// Random *pairs* of EvalParams don't have to satisfy either — a "conservative"
// built from random fields could end up less restrictive than an "optimistic"
// built from different random fields. We test the actual contract here.

proptest! {
    #[test]
    fn profitable_eval_conservative_le_optimistic_with_defaults(
        net in 1i128..1_000_000_000i128,
        bps in 0i32..500i32,
    ) {
        let np = NetProfit {
            input_amount: 0,
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
            profitable: true,
        };
        let out = evaluate(&np, &EvalParams::optimistic(), &EvalParams::conservative_default());
        prop_assert!(out.optimistic.e_pnl == net, "optimistic must equal raw net");
        prop_assert!(
            out.conservative.e_pnl < net,
            "conservative={} must be < raw net={}",
            out.conservative.e_pnl, net
        );
        prop_assert!(
            out.conservative.e_pnl <= out.optimistic.e_pnl,
            "conservative={} must be <= optimistic={}",
            out.conservative.e_pnl, out.optimistic.e_pnl
        );
    }

    #[test]
    fn losing_eval_both_bounds_le_zero(
        net in -1_000_000_000i128..=0i128,
    ) {
        let np = NetProfit {
            input_amount: 0,
            gross_output: 0,
            total_costs: CostBreakdown {
                base_sig_fee_lamports: 0,
                priority_fee_lamports: 0,
                jito_tip_lamports: 0,
                jito_tip_fee_lamports: 0,
                total_lamports: 0,
            },
            net_profit: net,
            net_profit_bps: 0,
            profitable: false,
        };
        let out = evaluate(&np, &EvalParams::optimistic(), &EvalParams::conservative_default());
        prop_assert!(out.optimistic.e_pnl <= 0,
            "losing cycle optimistic ev must be <= 0, got {}", out.optimistic.e_pnl);
        prop_assert!(out.conservative.e_pnl <= 0,
            "losing cycle conservative ev must be <= 0, got {}", out.conservative.e_pnl);
    }

    #[test]
    fn evaluate_never_panics(
        params in arb_eval_params(),
        net in arb_net_profit(),
    ) {
        // Smoke test: just running it should not panic.
        let _ = evaluate(&net, &params, &params);
    }
}
