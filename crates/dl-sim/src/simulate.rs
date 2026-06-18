//! Multi-leg forward fill: simulate a `Cycle` through the live
//! `PoolRegistry`, applying the constant-product fill math leg-by-leg.
//!
//! This is the boundary between detection (Phase 3) and sizing (Phase 4/5).
//! The detector flags a *cycle*; the sim answers "*what would the round-trip
//! output be at this input size, against the real on-chain reserves*?".
//!
//! ## Algorithm
//!
//! For each `Leg` in `cycle.legs`:
//! 1. Look up the `Pool` in the registry (`Err(PoolNotFound)` if missing).
//! 2. Determine `(reserve_in, reserve_out)` from the leg's `Direction`.
//! 3. Call `fill_constant_product(...)` to get the leg's output.
//! 4. Compute the *effective* amount that hit the pool (post-fee),
//!    `dx_eff = amount_in * (10_000 - fee_bps) / 10_000`.
//! 5. Compute the *new* reserves for observability:
//!    `(reserve_in + dx_eff, reserve_out - amount_out)`, saturated to
//!    `u64::MAX` (defensive — realistic reserves never approach that).
//! 6. Feed `amount_out` into the next leg as its `amount_in`.
//!
//! ## Determinism
//!
//! Pure function of `(cycle, registry, input)`. No `Clock`, `Rng`, or
//! system calls. Two calls on the same inputs are bit-identical.

use dl_core::fixed::mul_div_floor;
use dl_detect::cycle::{Cycle, Direction, Leg};
use dl_state::{Pool, PoolRegistry, Pubkey};

use crate::error::SimError;
use crate::fill::fill_constant_product;

/// Defensive cap on the number of legs a single cycle can have. Real
/// arb cycles are 2-4 legs; anything beyond is either misconfigured or
/// a degenerate graph. The cap is a guard, not a correctness bound.
const MAX_CYCLE_LEGS: usize = 16;

/// Per-tx fee denominator (Raydium AMM v4 model). `fee_bps` is in
/// basis points (1/10_000); `10_000 - fee_bps` is the *non-fee*
/// fraction that hits the pool.
const FEE_DENOM_BPS: u128 = 10_000;

/// Result of filling one leg of a cycle.
///
/// `amount_in` and `amount_out` are in the pool's own base units
/// (lamports for SOL, micro-USDC for USDC). The cycle is a closed walk
/// on the token graph, so the *output* of leg `N` (in some token's base
/// units) is the *input* of leg `N+1` (in the same token's base units) —
/// **no decimal conversion at the leg boundary**.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LegFill {
    /// Pool this leg filled through.
    pub pool: Pubkey,
    /// Whether the leg was base -> quote or quote -> base.
    pub direction: Direction,
    /// Input amount in the input token's base units.
    pub amount_in: u128,
    /// Output amount in the output token's base units.
    pub amount_out: u128,
    /// `(reserve_in, reserve_out)` *after* this leg's fill, in the
    /// pool's own base units. Observed-and-recorded for dashboards
    /// and future extensions (multi-pool splitters, route
    /// combinations); the next leg's starting reserves are the
    /// *next pool's* reserves, not these.
    pub reserves_after: (u64, u64),
}

/// Result of simulating an entire cycle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CycleFill {
    /// Per-leg fills, in cycle order.
    pub per_leg: Vec<LegFill>,
    /// Final output in the *input* token's base units. Since the cycle
    /// is a closed walk, this is in the same unit scale as `input`.
    pub final_output: u128,
}

/// Simulate a `Cycle` forward through the live `PoolRegistry`.
///
/// Returns a [`CycleFill`] with per-leg fills and the round-trip
/// `final_output` in input-token base units.
///
/// ## Errors
///
/// - `SimError::PoolNotFound(pubkey)` — a leg's pool is not in the
///   registry (stale cycle, or registry evicted it).
/// - `SimError::Math(...)` — the fill math overflowed (extremely
///   unrealistic for any reserves a real pool would have).
/// - `SimError::ZeroReserve` — a pool has a zero reserve on the side
///   the leg needs (should be filtered upstream).
/// - `SimError::FeeTooHigh(fee_bps)` — pool has `fee_bps >= 10_000`
///   (should be filtered upstream).
/// - `SimError::CycleTooLong(n)` — cycle has more than
///   [`MAX_CYCLE_LEGS`] legs.
pub fn simulate_cycle(
    cycle: &Cycle,
    registry: &PoolRegistry,
    input: u128,
) -> Result<CycleFill, SimError> {
    if cycle.legs.len() > MAX_CYCLE_LEGS {
        return Err(SimError::CycleTooLong(cycle.legs.len()));
    }
    let mut per_leg: Vec<LegFill> = Vec::with_capacity(cycle.legs.len());
    let mut amount_in: u128 = input;
    for leg in cycle.legs.iter() {
        let fill = simulate_one_leg(leg, registry, amount_in)?;
        amount_in = fill.amount_out;
        per_leg.push(fill);
    }
    Ok(CycleFill {
        per_leg,
        final_output: amount_in,
    })
}

/// Fill a single leg. The output's `amount_out` becomes the next
/// leg's `amount_in`.
fn simulate_one_leg(
    leg: &Leg,
    registry: &PoolRegistry,
    amount_in: u128,
) -> Result<LegFill, SimError> {
    let pool: &Pool = registry
        .get(&leg.pool.0)
        .ok_or(SimError::PoolNotFound(leg.pool))?;
    let (reserve_in, reserve_out) = match leg.direction {
        Direction::BaseToQuote => (pool.base_reserve, pool.quote_reserve),
        Direction::QuoteToBase => (pool.quote_reserve, pool.base_reserve),
    };
    let amount_out = fill_constant_product(
        reserve_in as u128,
        reserve_out as u128,
        pool.fee_bps,
        amount_in,
    )?;
    // dx_eff = amount_in * (10_000 - fee_bps) / 10_000 — the post-fee
    // amount that actually hit the pool. This is what the next-leg
    // reserves use when the *same* pool is revisited (not the case in
    // v1.0 — cycles are simple walks — but the design supports it).
    let fee_num: u128 = FEE_DENOM_BPS - pool.fee_bps as u128;
    let dx_eff = mul_div_floor(amount_in, fee_num, FEE_DENOM_BPS)?;
    // New reserves, saturated at u64::MAX. Realistic reserves never
    // approach that, but the saturate is defensive.
    let new_reserve_in = reserve_in.saturating_add(dx_eff.min(u64::MAX as u128) as u64);
    let new_reserve_out = reserve_out.saturating_sub(amount_out.min(u64::MAX as u128) as u64);
    Ok(LegFill {
        pool: leg.pool,
        direction: leg.direction,
        amount_in,
        amount_out,
        reserves_after: (new_reserve_in, new_reserve_out),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use dl_state::pool::{AmmKind, Pool, Pubkey};

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

    fn build_cycle(legs: Vec<Leg>) -> Cycle {
        Cycle::new(legs)
    }

    /// 2-cycle (single pool, base->quote then quote->base). 30 bps fee
    /// eats a small fraction of the round-trip; output must be strictly
    /// less than input.
    #[test]
    fn two_cycle_through_single_pool_loses_to_fees() {
        let pool = make_pool([7u8; 32], 1_000_000_000_000, 15_000_000_000_000, 30);
        let mut reg = PoolRegistry::new();
        reg.insert(pool.clone());
        let cycle = build_cycle(vec![
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
        let fill = simulate_cycle(&cycle, &reg, 1_000_000_000).unwrap();
        assert_eq!(fill.per_leg.len(), 2);
        assert!(
            fill.final_output < 1_000_000_000,
            "expected loss to fees, got {}",
            fill.final_output
        );
    }

    /// 3-cycle triangle: pool1 (A/B) at 1:1, pool2 (B/C) at 1:1,
    /// pool3 (C/A) priced 10% off (C is cheap — 1 C = 1.1 A). The
    /// round-trip should produce more A than we put in.
    #[test]
    fn three_cycle_with_rate_edge_is_profitable() {
        // Pool1: A/B 1:1
        let pool1 = make_pool([1u8; 32], 1_000_000, 1_000_000, 30);
        // Pool2: B/C 1:1
        let pool2 = make_pool([2u8; 32], 1_000_000, 1_000_000, 30);
        // Pool3: C/A 1:1.1 (C is cheap)
        let pool3 = make_pool([3u8; 32], 1_000_000, 1_100_000, 30);
        let mut reg = PoolRegistry::new();
        reg.insert(pool1.clone());
        reg.insert(pool2.clone());
        reg.insert(pool3.clone());
        let cycle = build_cycle(vec![
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
        assert!(
            fill.final_output > input,
            "expected profitable cycle, got {} for input {}",
            fill.final_output,
            input
        );
    }

    /// `reserves_after` matches hand-computed values.
    #[test]
    fn reserves_after_match_hand_computation() {
        let pool = make_pool([9u8; 32], 1_000_000, 1_000_000, 0);
        let mut reg = PoolRegistry::new();
        reg.insert(pool.clone());
        let cycle = build_cycle(vec![Leg {
            pool: pool.address,
            direction: Direction::BaseToQuote,
            weight: 0,
        }]);
        let input = 100_000u128;
        let fill = simulate_cycle(&cycle, &reg, input).unwrap();
        // 0 bps fee: dx_eff = input; dy = y * dx / (x + dx)
        let expected_dy = 1_000_000u128 * input / (1_000_000 + input);
        let leg = &fill.per_leg[0];
        assert_eq!(leg.amount_out, expected_dy);
        // reserves_after = (x + dx_eff, y - dy) = (x + input, y - dy)
        let expected_new_in = 1_000_000u128 + input;
        let expected_new_out = 1_000_000u128 - expected_dy;
        assert_eq!(leg.reserves_after.0 as u128, expected_new_in);
        assert_eq!(leg.reserves_after.1 as u128, expected_new_out);
        assert_eq!(fill.final_output, expected_dy);
    }

    /// Determinism: two calls on the same inputs return byte-identical
    /// `CycleFill`s.
    #[test]
    fn simulate_cycle_is_deterministic() {
        let pool = make_pool([5u8; 32], 1_000_000_000, 1_000_000_000, 25);
        let mut reg = PoolRegistry::new();
        reg.insert(pool.clone());
        let cycle = build_cycle(vec![Leg {
            pool: pool.address,
            direction: Direction::BaseToQuote,
            weight: 0,
        }]);
        let a = simulate_cycle(&cycle, &reg, 1_000_000).unwrap();
        let b = simulate_cycle(&cycle, &reg, 1_000_000).unwrap();
        assert_eq!(a, b);
    }

    /// `Err(PoolNotFound)` when a leg's pool isn't in the registry.
    #[test]
    fn missing_pool_returns_error() {
        let reg = PoolRegistry::new();
        let cycle = build_cycle(vec![Leg {
            pool: Pubkey([42u8; 32]),
            direction: Direction::BaseToQuote,
            weight: 0,
        }]);
        let err = simulate_cycle(&cycle, &reg, 1_000).unwrap_err();
        assert!(matches!(err, SimError::PoolNotFound(pk) if pk == Pubkey([42u8; 32])));
    }

    /// `Err(CycleTooLong)` when the cycle has more than
    /// `MAX_CYCLE_LEGS` legs.
    #[test]
    fn cycle_too_long_returns_error() {
        let reg = PoolRegistry::new();
        let legs: Vec<Leg> = (0..(MAX_CYCLE_LEGS + 1))
            .map(|i| Leg {
                pool: Pubkey([i as u8; 32]),
                direction: Direction::BaseToQuote,
                weight: 0,
            })
            .collect();
        let cycle = build_cycle(legs);
        let err = simulate_cycle(&cycle, &reg, 1_000).unwrap_err();
        assert!(matches!(err, SimError::CycleTooLong(n) if n == MAX_CYCLE_LEGS + 1));
    }
}
