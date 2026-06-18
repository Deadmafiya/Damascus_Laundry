//! Unit tests for `NetProfit::from_optimal`.
//!
//! Hand-computed expected values: no floats, no tolerance. Every assertion
//! is on a value the test computes by hand from the input/output/cost.
//!
//! Scenarios:
//! - Profitable: net > 0, bps > 0, profitable flag set.
//! - Loss: net < 0, bps < 0, profitable flag clear.
//! - Break-even (zero cost): net = gross − input, profitable iff gross > input.
//! - Zero input: net = −cost, profitable = false.
//! - Default-busy cost (1,080,000 lamports) on a tiny trade: net negative.

use dl_sim::cost::CostModel;
use dl_sim::net_profit::NetProfit;
use dl_sim::sizing::OptimalInput;

#[test]
fn profitable_net_yields_positive_bps() {
    let cost = CostModel::default_min(); // 15,700 lamports
    let input = 1_000_000u128;
    let gross = 1_500_000u128; // 50% rate edge, no fees
    let optimal = OptimalInput::Profitable {
        amount: input,
        net_profit: 0,
    };
    let np = NetProfit::from_optimal(optimal, input, gross, &cost).unwrap();
    // net = 1_500_000 - 1_000_000 - 15_700 = 484_300
    assert_eq!(np.input_amount, input);
    assert_eq!(np.gross_output, gross);
    assert_eq!(np.net_profit, 484_300);
    // bps = 484_300 * 10_000 / 1_000_000 = 4_843
    assert_eq!(np.net_profit_bps, 4_843);
    assert!(np.profitable);
    // The cost breakdown should match the model.
    assert_eq!(np.total_costs.total_lamports, 15_700);
}

#[test]
fn loss_net_yields_negative_bps() {
    let cost = CostModel::default_min();
    let input = 1_000_000u128;
    let gross = 900_000u128; // 10% loss, no fees
    let optimal = OptimalInput::NoTrade {
        best_negative_net: 0,
    };
    let np = NetProfit::from_optimal(optimal, input, gross, &cost).unwrap();
    // net = 900_000 - 1_000_000 - 15_700 = -115_700
    assert_eq!(np.net_profit, -115_700);
    // bps = -115_700 * 10_000 / 1_000_000 = -1_157
    assert_eq!(np.net_profit_bps, -1_157);
    assert!(!np.profitable);
}

#[test]
fn zero_input_yields_negative_net_equal_to_cost() {
    let cost = CostModel::default_min();
    let optimal = OptimalInput::NoTrade {
        best_negative_net: 0,
    };
    let np = NetProfit::from_optimal(optimal, 0, 0, &cost).unwrap();
    // net = 0 - 0 - 15_700 = -15_700
    assert_eq!(np.net_profit, -15_700);
    // bps saturated to 0 (input is 0; divide-by-zero guard)
    assert_eq!(np.net_profit_bps, 0);
    assert!(!np.profitable);
}

#[test]
fn default_busy_cost_on_tiny_trade_is_loss() {
    // default_busy: 1,080,000 lamports. With a 1k-lamport input, no
    // realistic trade recovers this cost.
    let cost = CostModel::default_busy();
    let input = 1_000u128;
    let gross = 1_100u128; // 10% edge
    let optimal = OptimalInput::Profitable {
        amount: input,
        net_profit: 0,
    };
    let np = NetProfit::from_optimal(optimal, input, gross, &cost).unwrap();
    // net = 1_100 - 1_000 - 1_110_000 = -1_109_900
    assert_eq!(np.net_profit, -1_109_900);
    // bps = -1_109_900 * 10_000 / 1_000 = -11_099_000 (fits in i32, no saturation)
    assert_eq!(np.net_profit_bps, -11_099_000);
    assert!(!np.profitable);
}

#[test]
fn break_even_with_zero_cost_profitable_when_gross_exceeds_input() {
    let cost = CostModel::default_min(); // 15,700 cost, can't be zero
    let input = 1_000_000u128;
    let gross = 1_000_000u128 + 15_700u128; // gross - input = cost, so net = 0
    let optimal = OptimalInput::NoTrade {
        best_negative_net: 0,
    };
    let np = NetProfit::from_optimal(optimal, input, gross, &cost).unwrap();
    assert_eq!(np.net_profit, 0);
    assert_eq!(np.net_profit_bps, 0);
    // Net 0 is NOT profitable (strictly positive required).
    assert!(!np.profitable);
}
