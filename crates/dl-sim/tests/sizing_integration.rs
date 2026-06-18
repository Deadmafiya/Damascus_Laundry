//! Integration tests for `find_optimal_input` (AC-3).
//!
//! These exercise the golden-section search against hand-built synthetic
//! pools. The math is exact integer arithmetic; every assertion is on a
//! hand-computed expected value or a structural property of the optimizer
//! (e.g. "the optimal input is not at a boundary").
//!
//! Scenarios:
//! - Profitable 3-cycle: optimizer returns `Profitable { amount, net_profit }`
//!   with `net_profit > 0` and `amount ∈ (0, max_input)` (interior, not at
//!   a boundary).
//! - Bound check: net at the returned `amount` is ≥ net at the bracket
//!   endpoints 0 and `max_input`.
//! - NoTrade case: a 2-cycle with no rate edge returns `NoTrade`.
//! - Determinism: two calls return byte-identical `OptimalInput` values.
//! - Edge case: `max_input == 0` returns `NoTrade { best_negative_net: 0 }`.

use dl_detect::cycle::{Cycle, Direction, Leg};
use dl_sim::cost::CostModel;
use dl_sim::error::SimError;
use dl_sim::simulate::simulate_cycle;
use dl_sim::sizing::{find_optimal_input, OptimalInput};
use dl_state::pool::{AmmKind, Pool, Pubkey};
use dl_state::PoolRegistry;

fn make_pool(addr: [u8; 32], base_res: u64, quote_res: u64, fee_bps: u16) -> Pool {
    Pool {
        address: Pubkey(addr),
        kind: AmmKind::RaydiumAmmV4,
        base_mint: Pubkey([1u8; 32]),
        quote_mint: Pubkey([2u8; 32]),
        base_decimals: 9,
        quote_decimals: 6,
        base_reserve: base_res,
        quote_reserve: quote_res,
        fee_bps,
        last_update_slot: 1,
    }
}

/// Profitable 3-cycle: optimizer finds a positive-net input in (0, max_input).
///
/// Pool design: 1e15 reserves (deep enough that slippage is negligible for
/// inputs ≤ 1e9), 0 bps fees (isolate the rate edge from the cost model),
/// pool3 has a 50% quote premium (1.5e15) — i.e. round-trip yield ≈ 1.5x.
/// With input = 1e9, gross ≈ 1.5e9, net ≈ 0.5e9 − 15_700 ≈ 500M, clearly
/// profitable.
#[test]
fn profitable_three_cycle_finds_positive_net() {
    let pool1 = make_pool([1u8; 32], 1_000_000_000_000_000, 1_000_000_000_000_000, 0);
    let pool2 = make_pool([2u8; 32], 1_000_000_000_000_000, 1_000_000_000_000_000, 0);
    let pool3 = make_pool([3u8; 32], 1_000_000_000_000_000, 1_500_000_000_000_000, 0);
    let mut reg = PoolRegistry::new();
    reg.insert(pool1.clone());
    reg.insert(pool2.clone());
    reg.insert(pool3.clone());
    let cycle = Cycle::new(vec![
        Leg {
            pool: pool1.address,
            direction: Direction::BaseToQuote,
            weight: 0,
        },
        Leg {
            pool: pool2.address,
            direction: Direction::BaseToQuote,
            weight: 0,
        },
        Leg {
            pool: pool3.address,
            direction: Direction::BaseToQuote,
            weight: 0,
        },
    ]);
    let max_input = 1_000_000_000u128; // 1 SOL-ish
    let cost = CostModel::default_min();
    let optimal = find_optimal_input(&cycle, &reg, &cost, max_input).unwrap();
    match optimal {
        OptimalInput::Profitable { amount, net_profit } => {
            assert!(
                net_profit > 0,
                "expected positive net_profit, got {net_profit}"
            );
            assert!(amount > 0, "amount should be > 0, got {amount}");
            assert!(
                amount < max_input,
                "amount should be < max_input, got {amount}"
            );
        }
        OptimalInput::NoTrade { best_negative_net } => {
            panic!("expected Profitable, got NoTrade({best_negative_net})");
        }
    }
}

/// Bound check: the net at the returned optimal is ≥ net at the bracket
/// endpoints 0 and `max_input`. This is the sizer's correctness property:
/// the returned amount is the *max* of the function on the bracket.
#[test]
fn optimal_net_is_at_least_as_good_as_endpoints() {
    let pool1 = make_pool([1u8; 32], 1_000_000, 1_000_000, 30);
    let pool2 = make_pool([2u8; 32], 1_000_000, 1_000_000, 30);
    let pool3 = make_pool([3u8; 32], 1_000_000, 1_100_000, 30);
    let mut reg = PoolRegistry::new();
    reg.insert(pool1.clone());
    reg.insert(pool2.clone());
    reg.insert(pool3.clone());
    let cycle = Cycle::new(vec![
        Leg {
            pool: pool1.address,
            direction: Direction::BaseToQuote,
            weight: 0,
        },
        Leg {
            pool: pool2.address,
            direction: Direction::BaseToQuote,
            weight: 0,
        },
        Leg {
            pool: pool3.address,
            direction: Direction::BaseToQuote,
            weight: 0,
        },
    ]);
    let max_input = 100_000u128;
    let cost = CostModel::default_min();
    let breakdown = cost.total_cost().unwrap();
    let total_cost = breakdown.total_lamports as i128;

    let net_at = |input: u128| -> i128 {
        let gross = simulate_cycle(&cycle, &reg, input).unwrap().final_output;
        (gross as i128) - (input as i128) - total_cost
    };

    let n0 = net_at(0);
    let nmax = net_at(max_input);
    let optimal = find_optimal_input(&cycle, &reg, &cost, max_input).unwrap();
    if let OptimalInput::Profitable { amount, net_profit } = optimal {
        // The optimal net must be ≥ the net at the endpoints.
        // Note: the optimal net is computed internally, so we recompute it
        // at the returned amount to verify the bound.
        let n_optimal = net_at(amount);
        assert_eq!(
            net_profit, n_optimal,
            "internal net_profit should match net_at(amount)"
        );
        assert!(
            n_optimal >= n0,
            "optimal net ({n_optimal}) should be >= net at 0 ({n0})"
        );
        assert!(
            n_optimal >= nmax,
            "optimal net ({n_optimal}) should be >= net at max ({nmax})"
        );
    } else {
        // NoTrade: skip the bound check (the cycle is unprofitable at
        // every input; the bound is trivial).
    }
}

/// NoTrade case: a 2-cycle through a single pool with no rate edge and
/// 30 bps fees eats the round-trip. Combined with a positive cost, the
/// cycle is unprofitable at every input in [0, max_input].
#[test]
fn no_edge_two_cycle_returns_no_trade() {
    let pool = make_pool([7u8; 32], 1_000_000_000_000, 15_000_000_000_000, 30);
    let mut reg = PoolRegistry::new();
    reg.insert(pool.clone());
    let cycle = Cycle::new(vec![
        Leg {
            pool: pool.address,
            direction: Direction::BaseToQuote,
            weight: 0,
        },
        Leg {
            pool: pool.address,
            direction: Direction::QuoteToBase,
            weight: 0,
        },
    ]);
    let max_input = 1_000_000u128;
    let cost = CostModel::default_min();
    let optimal = find_optimal_input(&cycle, &reg, &cost, max_input).unwrap();
    match optimal {
        OptimalInput::NoTrade { best_negative_net } => {
            assert!(
                best_negative_net < 0,
                "expected negative best_negative_net, got {best_negative_net}"
            );
        }
        OptimalInput::Profitable { amount, net_profit } => {
            panic!("expected NoTrade, got Profitable(amount={amount}, net_profit={net_profit})");
        }
    }
}

/// Determinism: two calls on identical inputs return byte-equal
/// `OptimalInput` values.
#[test]
fn find_optimal_input_is_deterministic() {
    let pool1 = make_pool([1u8; 32], 1_000_000, 1_000_000, 30);
    let pool2 = make_pool([2u8; 32], 1_000_000, 1_000_000, 30);
    let pool3 = make_pool([3u8; 32], 1_000_000, 1_100_000, 30);
    let mut reg = PoolRegistry::new();
    reg.insert(pool1.clone());
    reg.insert(pool2.clone());
    reg.insert(pool3.clone());
    let cycle = Cycle::new(vec![
        Leg {
            pool: pool1.address,
            direction: Direction::BaseToQuote,
            weight: 0,
        },
        Leg {
            pool: pool2.address,
            direction: Direction::BaseToQuote,
            weight: 0,
        },
        Leg {
            pool: pool3.address,
            direction: Direction::BaseToQuote,
            weight: 0,
        },
    ]);
    let cost = CostModel::default_min();
    let a = find_optimal_input(&cycle, &reg, &cost, 100_000).unwrap();
    let b = find_optimal_input(&cycle, &reg, &cost, 100_000).unwrap();
    assert_eq!(a, b);
}

/// Edge case: `max_input == 0` returns `NoTrade { best_negative_net: 0 }`.
#[test]
fn zero_max_input_returns_no_trade() {
    let reg = PoolRegistry::new();
    let cycle = Cycle::new(vec![]);
    let cost = CostModel::default_min();
    let optimal = find_optimal_input(&cycle, &reg, &cost, 0).unwrap();
    assert_eq!(
        optimal,
        OptimalInput::NoTrade {
            best_negative_net: 0
        }
    );
}

/// Missing pool error propagates from `simulate_cycle` through
/// `find_optimal_input`.
#[test]
fn missing_pool_propagates_error() {
    let reg = PoolRegistry::new();
    let cycle = Cycle::new(vec![Leg {
        pool: Pubkey([99u8; 32]),
        direction: Direction::BaseToQuote,
        weight: 0,
    }]);
    let cost = CostModel::default_min();
    let err = find_optimal_input(&cycle, &reg, &cost, 1_000).unwrap_err();
    match err {
        SimError::PoolNotFound(pk) => assert_eq!(pk, Pubkey([99u8; 32])),
        other => panic!("expected PoolNotFound, got {other:?}"),
    }
}
