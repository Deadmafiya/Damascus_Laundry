//! Integration tests for `dl_detect::cycle::simulate_through_pools` (Task 6).
//!
//! Verifies the dl-detect → dl-sim boundary works end-to-end: pass a
//! `Cycle` and a `PoolRegistry`, get back a gross-output `u128`. The
//! `simulate_through_pools` free function is the post-refactor replacement
//! for the (now-removed) `Cycle::simulate_through_pools` method; the
//! method-vs-function shift was forced by the orphan rule once the
//! `Cycle` type was relocated to `dl-state`.
//!
//! Scenarios:
//! - Profitable 3-cycle: gross output > input (the cycle has a rate edge).
//! - Round-trip 2-cycle: gross output < input (fees eat it).
//! - Missing pool: `Err(PoolNotFound)`.
//! - Zero input: simulate returns 0 (no fill at zero input).
//! - Determinism: two calls return byte-identical outputs.

use dl_detect::cycle::{simulate_through_pools, Cycle, Direction, Leg};
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

#[test]
fn profitable_three_cycle_returns_gross_output_above_input() {
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
    let out = simulate_through_pools(&cycle, &reg, input).unwrap();
    assert!(
        out > input,
        "expected out > input, got out={out}, input={input}"
    );
}

#[test]
fn two_cycle_round_trip_returns_less_than_input() {
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
    let input = 1_000_000u128;
    let out = simulate_through_pools(&cycle, &reg, input).unwrap();
    assert!(
        out < input,
        "expected out < input, got out={out}, input={input}"
    );
}

#[test]
fn missing_pool_returns_pool_not_found() {
    let reg = PoolRegistry::new();
    let cycle = Cycle::new(vec![Leg {
        pool: Pubkey([42u8; 32]),
        direction: Direction::BaseToQuote,
        weight: 0,
    }]);
    let err = simulate_through_pools(&cycle, &reg, 1_000).unwrap_err();
    match err {
        dl_detect::error::DetectError::PoolNotFound(pk) => {
            assert_eq!(pk, Pubkey([42u8; 32]));
        }
        other => panic!("expected PoolNotFound, got {other:?}"),
    }
}

#[test]
fn zero_input_returns_zero_output() {
    let pool = make_pool([1u8; 32], 1_000_000, 1_000_000, 0);
    let mut reg = PoolRegistry::new();
    reg.insert(pool.clone());
    let cycle = Cycle::new(vec![Leg {
        pool: pool.address,
        direction: Direction::BaseToQuote,
        weight: 0,
    }]);
    let out = simulate_through_pools(&cycle, &reg, 0).unwrap();
    assert_eq!(out, 0);
}

#[test]
fn simulate_through_pools_is_deterministic() {
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
    let a = simulate_through_pools(&cycle, &reg, 1_000).unwrap();
    let b = simulate_through_pools(&cycle, &reg, 1_000).unwrap();
    assert_eq!(a, b);
}
