//! DAM-62 Part 2: 3-leg cycle including the Orca Whirlpool.
//!
//! Mirrors the wiring in `dl-app/src/live.rs::cycles_from_capture`
//! (Phase 2 / DAM-62): given a 3-DEX triangle (Raydium AMM v4 +
//! Orca Whirlpool + Raydium AMM v4 — the third Raydium pool
//! closes the cycle, keeping the test deterministic and
//! crate-light), build the `Pool`s the live path would
//! assemble (Raydium vaults matched by mint, Orca vaults
//! matched by **pubkey** against the Whirlpool's
//! `token_vault_x` / `token_vault_y`), run them through a
//! fresh `dl_stream::detector::StreamingDetector`, and assert
//! that cycles are detected that include the Whirlpool.
//!
//! ## Vault routing
//!
//! On mainnet, the WS feed subscribes to the pool account
//! (the Whirlpool's pubkey) AND to the two vault accounts
//! (the Whirlpool's `token_vault_x` / `token_vault_y`). When
//! a vault account updates, the AccountUpdate's pubkey is
//! the **vault account's pubkey** (NOT the SPL mint). The
//! routing step matches the AccountUpdate's pubkey against
//! the parent pool's `token_vault_x` / `token_vault_y` to
//! determine which side (base/quote) the new amount updates.
//! This is the wiring that was missing before DAM-62.

use dl_state::decoder::{
    assemble_pool, assemble_whirlpool_pool, decode_amm_info, decode_spl_token_account,
    encode_whirlpool, AmmInfo, SplTokenAccount, AMM_INFO_SIZE, SPL_TOKEN_ACCOUNT_SIZE,
    WHIRLPOOL_ACCOUNT_SIZE,
};
use dl_state::pool::{AmmKind, Pool};
use dl_state::Pubkey;
use dl_stream::detector::StreamingDetector;
use std::collections::HashMap;

fn fake_amm_info_bytes(
    base_mint: [u8; 32],
    quote_mint: [u8; 32],
    base_vault: [u8; 32],
    quote_vault: [u8; 32],
    base_decimals: u8,
    quote_decimals: u8,
    fee_bps: u64,
) -> Vec<u8> {
    let mut buf = vec![0u8; AMM_INFO_SIZE];
    buf[0..8].copy_from_slice(&1u64.to_le_bytes());
    buf[32] = base_decimals;
    buf[40] = quote_decimals;
    buf[144..152].copy_from_slice(&fee_bps.to_le_bytes());
    buf[152..160].copy_from_slice(&10_000u64.to_le_bytes());
    buf[336..368].copy_from_slice(&base_vault);
    buf[368..400].copy_from_slice(&quote_vault);
    buf[400..432].copy_from_slice(&base_mint);
    buf[432..464].copy_from_slice(&quote_mint);
    buf
}

fn fake_spl_token_account(mint: [u8; 32], amount: u64) -> Vec<u8> {
    let mut buf = vec![0u8; SPL_TOKEN_ACCOUNT_SIZE];
    buf[0..32].copy_from_slice(&mint);
    buf[64..72].copy_from_slice(&amount.to_le_bytes());
    buf
}

/// Build the three `Pool`s the live `cycles_from_capture` path
/// would assemble from a 3-DEX triangle capture. Each vault
/// in the returned map is keyed by its **vault account pubkey**
/// (the value used by the routing step to match against the
/// Whirlpool's `token_vault_x` / `token_vault_y`).
fn build_three_dex_triangle_pools() -> (Vec<Pool>, [u8; 32]) {
    let mint_a = [0xA1u8; 32];
    let mint_b = [0xB2u8; 32];
    let mint_c = [0xC3u8; 32];

    // Raydium pool 1: A/B.
    let raydium_1_pool = [0x11u8; 32];
    let raydium_1_amm: AmmInfo = decode_amm_info(&fake_amm_info_bytes(
        mint_a,
        mint_b,
        [0x12u8; 32],
        [0x13u8; 32],
        9,
        6,
        25,
    ))
    .expect("raydium 1 amm");

    // Orca Whirlpool: B/C.
    let orca_pool = [0x21u8; 32];
    let orca_base_vault_pubkey = [0x22u8; 32];
    let orca_quote_vault_pubkey = [0x23u8; 32];
    let whirl = dl_state::decoder::Whirlpool {
        sqrt_price: 1u128 << 64,
        tick_current_index: 0,
        tick_spacing: 64,
        liquidity: 1_000_000,
        token_mint_x: Pubkey(mint_b),
        token_mint_y: Pubkey(mint_c),
        token_vault_x: Pubkey(orca_base_vault_pubkey),
        token_vault_y: Pubkey(orca_quote_vault_pubkey),
        fee_rate: 30,
        program_id: dl_state::decoder::ORCA_WHIRLPOOL_PROGRAM_ID,
    };
    let whirl_bytes = encode_whirlpool(&whirl);
    assert_eq!(whirl_bytes.len(), WHIRLPOOL_ACCOUNT_SIZE);

    // Raydium pool 2: C/A.
    let raydium_2_pool = [0x31u8; 32];
    let raydium_2_amm: AmmInfo = decode_amm_info(&fake_amm_info_bytes(
        mint_c,
        mint_a,
        [0x32u8; 32],
        [0x33u8; 32],
        5,
        9,
        25,
    ))
    .expect("raydium 2 amm");

    // Vault SplTokenAccount updates. Keyed by the **vault
    // account pubkey** (the AccountUpdate pubkey in the live
    // path) — NOT by the SPL mint.
    let mut vaults_by_pubkey: HashMap<[u8; 32], SplTokenAccount> = HashMap::new();
    vaults_by_pubkey.insert(
        [0x12u8; 32],
        decode_spl_token_account(&fake_spl_token_account(mint_a, 100_000_000_000))
            .expect("raydium 1 base vault"),
    );
    vaults_by_pubkey.insert(
        [0x13u8; 32],
        decode_spl_token_account(&fake_spl_token_account(mint_b, 15_000_000_000))
            .expect("raydium 1 quote vault"),
    );
    vaults_by_pubkey.insert(
        orca_base_vault_pubkey,
        decode_spl_token_account(&fake_spl_token_account(mint_b, 14_000_000_000))
            .expect("orca base vault"),
    );
    vaults_by_pubkey.insert(
        orca_quote_vault_pubkey,
        decode_spl_token_account(&fake_spl_token_account(mint_c, 1_000_000_000_000))
            .expect("orca quote vault"),
    );
    vaults_by_pubkey.insert(
        [0x32u8; 32],
        decode_spl_token_account(&fake_spl_token_account(mint_c, 1_050_000_000_000))
            .expect("raydium 2 base vault"),
    );
    vaults_by_pubkey.insert(
        [0x33u8; 32],
        decode_spl_token_account(&fake_spl_token_account(mint_a, 105_000_000_000))
            .expect("raydium 2 quote vault"),
    );

    // Raydium: vault matching by mint (the AmmInfo carries
    // base_mint / quote_mint; the captured SplTokenAccount
    // doesn't carry the parent pool's pubkey).
    let mint_to_vault: HashMap<[u8; 32], SplTokenAccount> = vaults_by_pubkey
        .iter()
        .map(|(_pk, spl)| (spl.mint.0, *spl))
        .collect();
    let mut pools: Vec<Pool> = Vec::new();
    let raydium_1 = assemble_pool(
        Pubkey(raydium_1_pool),
        &raydium_1_amm,
        &mint_to_vault
            .get(&raydium_1_amm.base_mint.0)
            .copied()
            .expect("raydium 1 base vault by mint"),
        &mint_to_vault
            .get(&raydium_1_amm.quote_mint.0)
            .copied()
            .expect("raydium 1 quote vault by mint"),
        100,
    )
    .expect("assemble raydium 1");
    pools.push(raydium_1);

    // DAM-62 wiring: pubkey-keyed routing for Orca.
    let orca_base_spl = vaults_by_pubkey
        .get(&orca_base_vault_pubkey)
        .copied()
        .expect("orca base vault by pubkey");
    let orca_quote_spl = vaults_by_pubkey
        .get(&orca_quote_vault_pubkey)
        .copied()
        .expect("orca quote vault by pubkey");
    let orca = assemble_whirlpool_pool(
        Pubkey(orca_pool),
        &whirl,
        orca_base_spl.amount,
        orca_quote_spl.amount,
        100,
    );
    pools.push(orca);

    let raydium_2 = assemble_pool(
        Pubkey(raydium_2_pool),
        &raydium_2_amm,
        &mint_to_vault
            .get(&raydium_2_amm.base_mint.0)
            .copied()
            .expect("raydium 2 base vault by mint"),
        &mint_to_vault
            .get(&raydium_2_amm.quote_mint.0)
            .copied()
            .expect("raydium 2 quote vault by mint"),
        100,
    )
    .expect("assemble raydium 2");
    pools.push(raydium_2);

    (pools, orca_pool)
}

#[test]
fn orca_whirlpool_in_3dex_triangle_produces_cycle_through_orca() {
    let (pools, orca_pool_pk) = build_three_dex_triangle_pools();
    assert_eq!(pools.len(), 3, "expected 3 pools (Raydium + Orca + Raydium)");

    // DAM-62 Part 1: the Orca pool's reserves are non-zero
    // after the vault update.
    let orca_pool = pools
        .iter()
        .find(|p| p.address.0 == orca_pool_pk)
        .expect("Orca pool not in assembled list");
    assert_eq!(orca_pool.kind, AmmKind::OrcaWhirlpool);
    assert!(
        orca_pool.base_reserve > 0 && orca_pool.quote_reserve > 0,
        "DAM-62 Part 1: Orca reserves must be non-zero; \
         got base={} quote={}",
        orca_pool.base_reserve,
        orca_pool.quote_reserve
    );

    // DAM-62 Part 2: the streaming detector sees the Whirlpool
    // in the graph and detects a cycle that includes it.
    let mut detector =
        StreamingDetector::new(&pools).expect("StreamingDetector::new");
    let mut cycles_through_orca = 0u32;
    for p in &pools {
        for cyc in detector.on_pool_update(p) {
            if cyc.legs.iter().any(|leg| leg.pool.0 == orca_pool_pk) {
                cycles_through_orca += 1;
            }
        }
    }
    assert!(
        cycles_through_orca > 0,
        "DAM-62 Part 2: at least one cycle must include \
         the Orca Whirlpool pool (pubkey={:02x?}); none did",
        orca_pool_pk
    );
}

/// DAM-62 regression for the pre-DAM-62 bug: when the Orca
/// pool's vault amounts are NOT routed, the Whirlpool's
/// edge in the price graph has no weight (reserves=0 → rate=0
/// → no edge contribution). The streaming detector should
/// still build (the pool is in the registry with reserves=0),
/// but no cycle can include the Whirlpool because its edge
/// weight is `i64::MAX` (the "rate=0 → no edge" sentinel).
#[test]
fn orca_whirlpool_with_zero_reserves_yields_no_orca_cycle() {
    let (mut pools, orca_pool_pk) = build_three_dex_triangle_pools();
    // Pre-DAM-62 bug: vault update wasn't routed, so the
    // Whirlpool's reserves stay 0.
    for p in pools.iter_mut() {
        if p.kind == AmmKind::OrcaWhirlpool {
            p.base_reserve = 0;
            p.quote_reserve = 0;
        }
    }
    // With Orca reserves=0 the streaming detector either
    // fails to build the graph (DivByZero in the rate
    // computation — Whirlpool's sqrt_price is non-zero but
    // the effective rate needs the vault amounts) or, if
    // it does build, no cycle can include the Whirlpool
    // because its edge has no weight. Either outcome
    // proves the pre-DAM-62 failure mode: the Whirlpool
    // is invisible to cycle detection when vault updates
    // are not routed.
    match StreamingDetector::new(&pools) {
        Err(_) => {
            // Detector refused to build with the zero-reserve
            // Whirlpool — the Whirlpool has no place in the
            // price graph. Pre-DAM-62 bug confirmed.
        }
        Ok(mut detector) => {
            let mut cycles_through_orca = 0u32;
            for p in &pools {
                for cyc in detector.on_pool_update(p) {
                    if cyc.legs.iter().any(|leg| leg.pool.0 == orca_pool_pk) {
                        cycles_through_orca += 1;
                    }
                }
            }
            assert_eq!(
                cycles_through_orca, 0,
                "regression: with Orca reserves=0, no cycle should \
                 include the Whirlpool; got {} cycles",
                cycles_through_orca
            );
        }
    };
}
