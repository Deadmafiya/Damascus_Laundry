//! Integration tests for `simulate_cycle` (AC-2).
//!
//! These exercise the multi-leg forward fill against hand-built synthetic
//! pools. The math is exact integer arithmetic: every assertion is a
//! hand-computed expected value (no floats, no tolerance).
//!
//! Scenarios:
//! - No-edge 2-cycle: fees eat the round-trip; `final_output < input`.
//! - Profitable 3-cycle: a 10% rate edge produces `final_output > input`.
//! - Reserve-mutation: `reserves_after` matches hand-computed values.
//! - Determinism: two calls return byte-identical `CycleFill`s.
//! - Pool-not-found: missing pool returns `Err(PoolNotFound)`.
//! - Output-is-independent-of-input-magnitude (the slippage-eats-it
//!   property): doubling the input does *not* double the output.

use dl_sim::error::SimError;
use dl_sim::simulate::{simulate_cycle, LegFill};
use dl_state::cycle::{Cycle, Direction, Leg};
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

/// No-edge 2-cycle: round-trip through a single pool with 30 bps fee on
/// each side. The two fees compound; the output must be strictly less
/// than the input.
#[test]
fn no_edge_two_cycle_loses_to_fees() {
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
    let input = 1_000_000_000u128; // 1 SOL
    let fill = simulate_cycle(&cycle, &reg, input).unwrap();
    assert_eq!(fill.per_leg.len(), 2);
    // Per-leg outputs are positive (the pool isn't empty).
    for leg in &fill.per_leg {
        assert!(leg.amount_out > 0);
    }
    // Round-trip: fees eat it.
    assert!(
        fill.final_output < input,
        "expected final_output ({}) < input ({})",
        fill.final_output,
        input
    );
}

/// Profitable 3-cycle: pool1 (A/B) and pool2 (B/C) at 1:1, pool3 (C/A)
/// priced 10% off (C is cheap — 1 C = 1.1 A). The round-trip produces
/// more A than the input.
#[test]
fn profitable_three_cycle_with_rate_edge() {
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
    let input = 1_000u128;
    let fill = simulate_cycle(&cycle, &reg, input).unwrap();
    assert_eq!(fill.per_leg.len(), 3);
    for leg in &fill.per_leg {
        assert!(leg.amount_out > 0);
    }
    assert!(
        fill.final_output > input,
        "expected profitable cycle, got {} for input {}",
        fill.final_output,
        input
    );
}

/// `reserves_after` of each leg matches a hand-computed value.
#[test]
fn reserves_after_match_hand_computation() {
    let pool = make_pool([9u8; 32], 1_000_000, 1_000_000, 0);
    let mut reg = PoolRegistry::new();
    reg.insert(pool.clone());
    let cycle = Cycle::new(vec![Leg {
        pool: pool.address,
        direction: Direction::BaseToQuote,
        weight: 0,
    }]);
    let input = 100_000u128;
    let fill = simulate_cycle(&cycle, &reg, input).unwrap();
    // 0 bps fee: dx_eff = input; dy = y * dx / (x + dx)
    let expected_dy = 1_000_000u128 * input / (1_000_000 + input);
    let leg: &LegFill = &fill.per_leg[0];
    assert_eq!(leg.amount_out, expected_dy);
    // reserves_after = (x + dx_eff, y - dy)
    let expected_new_in = 1_000_000u128 + input;
    let expected_new_out = 1_000_000u128 - expected_dy;
    assert_eq!(leg.reserves_after.0 as u128, expected_new_in);
    assert_eq!(leg.reserves_after.1 as u128, expected_new_out);
    assert_eq!(fill.final_output, expected_dy);
}

/// Determinism: two calls on identical inputs return byte-equal
/// `CycleFill`s.
#[test]
fn simulate_cycle_is_deterministic() {
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
    let a = simulate_cycle(&cycle, &reg, 1_000).unwrap();
    let b = simulate_cycle(&cycle, &reg, 1_000).unwrap();
    assert_eq!(a, b);
}

/// Slippage: doubling the input does *not* double the output. Constant-
/// product math is concave: more in → more out, but sub-linearly.
#[test]
fn doubling_input_does_not_double_output() {
    let pool = make_pool([4u8; 32], 1_000_000, 1_000_000, 0);
    let mut reg = PoolRegistry::new();
    reg.insert(pool.clone());
    let cycle = Cycle::new(vec![Leg {
        pool: pool.address,
        direction: Direction::BaseToQuote,
        weight: 0,
    }]);
    let out1 = simulate_cycle(&cycle, &reg, 100_000).unwrap().final_output;
    let out2 = simulate_cycle(&cycle, &reg, 200_000).unwrap().final_output;
    // 2x input -> strictly less than 2x output (slippage eats it).
    assert!(
        out2 < 2 * out1,
        "slippage violated: out2={out2} >= 2*out1={}",
        2 * out1
    );
}

/// Missing pool returns `Err(PoolNotFound)`.
#[test]
fn missing_pool_returns_pool_not_found() {
    let reg = PoolRegistry::new();
    let cycle = Cycle::new(vec![Leg {
        pool: Pubkey([42u8; 32]),
        direction: Direction::BaseToQuote,
        weight: 0,
    }]);
    let err = simulate_cycle(&cycle, &reg, 1_000).unwrap_err();
    match err {
        SimError::PoolNotFound(pk) => assert_eq!(pk, Pubkey([42u8; 32])),
        other => panic!("expected PoolNotFound, got {other:?}"),
    }
}
