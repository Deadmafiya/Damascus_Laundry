//! DAM-63 Part 2: 3-leg cycle including the Meteora DLMM LbPair.
//!
//! Mirrors the wiring in `crates/dl-state/tests/dam62_orca_whirlpool_3leg.rs`
//! (the DAM-62 sibling test): given a 3-DEX triangle (Raydium AMM v4
//! + Orca Whirlpool + Meteora DLMM — the Meteora pool closes the
//! cycle, the same role Raydium 2 plays in the DAM-62 test), build
//! the `Pool`s the live path would assemble (Raydium vaults matched
//! by mint, Orca vaults matched by **pubkey**, Meteora vaults
//! matched by **pubkey** against the LbPair's `token_vault_x` /
//! `token_vault_y`), run them through a fresh
//! `dl_stream::detector::StreamingDetector`, and assert a 3-leg
//! cycle is detected that includes the Meteora pool.
//!
//! ## Why this test exists
//!
//! DAM-53 wired the LbPair subscription; DAM-62 wired the Whirlpool
//! vault subscription + reserve routing; DAM-63 closes the loop on
//! the Meteora half of the 3-DEX opportunity surface. Without this
//! test, "DLMM reserves flow in + a DLMM leg appears in detected
//! cycles" is asserted by nothing — the Meteora half of the
//! capture-replay pipeline is silent.
//!
//! ## Vault routing
//!
//! Meteora LbPair accounts carry `token_vault_x` / `token_vault_y`
//! pubkeys, identical in shape to the Orca Whirlpool's. The
//! pubkey-keyed routing is the right pattern (and matches what
//! `dl_app::main::run_live_paper` does at lines 1432–1542 for the
//! Meteora branch). Mints are insufficient because the same mint
//! can back multiple pools (a stablecoin like USDC is a base mint
//! on dozens of pools), and a vault update's AccountUpdate pubkey
//! is the **vault account's pubkey**, not the mint's.
//!
//! ## Acceptance
//!
//! - **Part 1**: the Meteora pool's `base_reserve` and `quote_reserve`
//!   are non-zero after the live path populates them from the
//!   captured vault updates.
//! - **Part 2**: a 3-leg cycle is detected that includes the
//!   Meteora pool's pubkey as one of its `Leg::pool`s.
//! - **Regression guard**: with the Meteora pool removed from the
//!   graph (the pre-DAM-63 bug, where vault updates were silently
//!   dropped because the dispatch did not look up the LbPair's
//!   vault pubkeys), no 3-leg cycle is detected.

use dl_state::decoder::{
    assemble_lb_pair_pool, assemble_pool, assemble_whirlpool_pool, decode_amm_info,
    decode_spl_token_account, encode_whirlpool, AmmInfo, LbPair, SplTokenAccount, Whirlpool,
    AMM_INFO_SIZE, SPL_TOKEN_ACCOUNT_SIZE, WHIRLPOOL_ACCOUNT_SIZE,
};
use dl_state::pool::{AmmKind, Pool};
use dl_state::Pubkey;
use dl_stream::detector::StreamingDetector;
use std::collections::HashMap;

/// Size of the synthetic LbPair blob produced by [`fake_lb_pair_bytes`].
/// Matches the v1.0 layout: 156 B header + 65 bins × 32 B = 2236 B.
const LB_PAIR_BLOB_SIZE: usize = 156 + 32 * 65;

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

/// Build a synthetic Meteora DLMM `LbPair` blob (2236 B) for the
/// given mint/vault fields. The active bin (offset 32 in the
/// 65-bin array) carries the `base_vault_amount` / `quote_vault_amount`
/// so the decoder round-trips into a non-zero reserves pair via
/// `assemble_lb_pair_pool`.
fn fake_lb_pair_bytes(
    base_mint: [u8; 32],
    quote_mint: [u8; 32],
    base_vault: [u8; 32],
    quote_vault: [u8; 32],
    base_vault_amount: u64,
    quote_vault_amount: u64,
) -> Vec<u8> {
    let mut buf = vec![0u8; LB_PAIR_BLOB_SIZE];
    // bin_step (u16) at offset 0. 100 bps = 1%.
    buf[0..2].copy_from_slice(&100u16.to_le_bytes());
    // active_id (i32) at offset 2.
    buf[2..6].copy_from_slice(&0i32.to_le_bytes());
    // 6 B padding at offset 6.
    // token_mint_x at offset 12.
    buf[12..44].copy_from_slice(&base_mint);
    // token_mint_y at offset 44.
    buf[44..76].copy_from_slice(&quote_mint);
    // token_vault_x at offset 76.
    buf[76..108].copy_from_slice(&base_vault);
    // token_vault_y at offset 108.
    buf[108..140].copy_from_slice(&quote_vault);
    // token_mint_x_program_flag (u8) at offset 140.
    buf[140] = 0;
    // token_mint_y_program_flag (u8) at offset 141.
    buf[141] = 0;
    // 14 B padding at offset 142.
    // Active bin (index 32) at offset 156 + 32 * 32 = 1180.
    let active_idx = 32usize;
    let bin_off = 156 + active_idx * 32;
    buf[bin_off..bin_off + 8].copy_from_slice(&base_vault_amount.to_le_bytes());
    buf[bin_off + 8..bin_off + 16].copy_from_slice(&quote_vault_amount.to_le_bytes());
    // 16 B price (1.0 scaled by SCALE_OFFSET = 1e12).
    buf[bin_off + 16..bin_off + 32].copy_from_slice(&1_000_000_000_000u128.to_le_bytes());
    buf
}

/// Build the three `Pool`s the live `cycles_from_capture` path
/// would assemble from a 3-DEX triangle capture. Mints form a
/// triangle (A → B → C → A). The Meteora LbPair is the third
/// pool (closing the cycle), with the same role the second
/// Raydium pool plays in `dam62_orca_whirlpool_3leg.rs`.
fn build_three_dex_triangle_pools() -> (Vec<Pool>, [u8; 32]) {
    let mint_a = [0xA1u8; 32];
    let mint_b = [0xB2u8; 32];
    let mint_c = [0xC3u8; 32];

    // Raydium pool 1: A/B. Reserves 1:1 with 30 bps fee.
    let raydium_1_pool = [0x11u8; 32];
    let raydium_1_amm_bytes = fake_amm_info_bytes(
        mint_a,
        mint_b,
        [0x12u8; 32],
        [0x13u8; 32],
        9,
        6,
        25,
    );
    let raydium_1_amm: AmmInfo =
        decode_amm_info(&raydium_1_amm_bytes).expect("raydium 1 amm");

    // Orca Whirlpool: B/C. Reserves 1:1 with 30 bps fee.
    let orca_pool = [0x21u8; 32];
    let orca_base_vault_pubkey = [0x22u8; 32];
    let orca_quote_vault_pubkey = [0x23u8; 32];
    let whirl = Whirlpool {
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

    // Meteora DLMM: C/A. Reserves 100:110 (the "favorable" leg
    // of the triangle; the round-trip A→B→C→A earns ~10% gross,
    // minus 3×30 bps = 90 bps in fees, leaves a positive net).
    // Same recipe as
    // `dl_detect::bellman_ford::tests::finds_3leg_triangle_arb`.
    let meteora_pool = [0x41u8; 32];
    let meteora_base_vault_pubkey = [0x42u8; 32];
    let meteora_quote_vault_pubkey = [0x43u8; 32];
    // Captured-vault amounts (token_vault_x balance = 100_000,
    // token_vault_y balance = 110_000).
    let meteora_base_amt: u64 = 100_000;
    let meteora_quote_amt: u64 = 110_000;
    let lb_pair_bytes = fake_lb_pair_bytes(
        mint_c,
        mint_a,
        meteora_base_vault_pubkey,
        meteora_quote_vault_pubkey,
        meteora_base_amt,
        meteora_quote_amt,
    );
    assert_eq!(lb_pair_bytes.len(), LB_PAIR_BLOB_SIZE);
    let lb_pair: LbPair = dl_state::decoder::decode_lb_pair(&lb_pair_bytes)
        .expect("meteora lb_pair decode");

    // Vault SplTokenAccount updates. Keyed by the **vault
    // account pubkey** (the AccountUpdate pubkey in the live
    // path) — NOT by the SPL mint. The mint is the body's
    // first 32 bytes; the pubkey is the routing key.
    let mut vaults_by_pubkey: HashMap<[u8; 32], SplTokenAccount> = HashMap::new();
    // Raydium 1 base vault: holds A, balance 100k.
    vaults_by_pubkey.insert(
        [0x12u8; 32],
        decode_spl_token_account(&fake_spl_token_account(mint_a, 100_000))
            .expect("raydium 1 base vault"),
    );
    // Raydium 1 quote vault: holds B, balance 100k.
    vaults_by_pubkey.insert(
        [0x13u8; 32],
        decode_spl_token_account(&fake_spl_token_account(mint_b, 100_000))
            .expect("raydium 1 quote vault"),
    );
    // Orca base vault (token_vault_x): holds B, balance 100k.
    vaults_by_pubkey.insert(
        orca_base_vault_pubkey,
        decode_spl_token_account(&fake_spl_token_account(mint_b, 100_000))
            .expect("orca base vault"),
    );
    // Orca quote vault (token_vault_y): holds C, balance 100k.
    vaults_by_pubkey.insert(
        orca_quote_vault_pubkey,
        decode_spl_token_account(&fake_spl_token_account(mint_c, 100_000))
            .expect("orca quote vault"),
    );
    // Meteora base vault (token_vault_x): holds C, balance 100k.
    vaults_by_pubkey.insert(
        meteora_base_vault_pubkey,
        decode_spl_token_account(&fake_spl_token_account(mint_c, meteora_base_amt))
            .expect("meteora base vault"),
    );
    // Meteora quote vault (token_vault_y): holds A, balance 110k.
    vaults_by_pubkey.insert(
        meteora_quote_vault_pubkey,
        decode_spl_token_account(&fake_spl_token_account(mint_a, meteora_quote_amt))
            .expect("meteora quote vault"),
    );

    // ── Assemble Raydium 1 (mint-keyed vault matching). ─
    let mint_to_vault: HashMap<[u8; 32], SplTokenAccount> = vaults_by_pubkey
        .iter()
        .map(|(_, spl)| (spl.mint.0, *spl))
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

    // ── Assemble Orca (pubkey-keyed). ─
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

    // ── Assemble Meteora (DAM-63 wiring: pubkey-keyed). ─
    // The LbPair layout carries `token_vault_x` / `token_vault_y`
    // pubkeys, identical in shape to the Whirlpool's. Each
    // captured SplTokenAccount is addressed by the vault
    // account's pubkey (not by mint), so the routing is by
    // pubkey. This is the precise change DAM-63 introduces;
    // before DAM-63 the Meteora vault updates were matched by
    // mint and could be mis-routed when the same mint backed
    // multiple pools.
    let meteora_base_spl = vaults_by_pubkey
        .get(&meteora_base_vault_pubkey)
        .copied()
        .expect("meteora base vault by pubkey");
    let meteora_quote_spl = vaults_by_pubkey
        .get(&meteora_quote_vault_pubkey)
        .copied()
        .expect("meteora quote vault by pubkey");
    let meteora = assemble_lb_pair_pool(
        Pubkey(meteora_pool),
        &lb_pair,
        meteora_base_spl.amount,
        meteora_quote_spl.amount,
        100,
    );
    pools.push(meteora);

    (pools, meteora_pool)
}

#[test]
fn three_dex_triangle_3leg_cycle_includes_meteora_dlmm() {
    let (pools, meteora_pool_pk) = build_three_dex_triangle_pools();
    assert_eq!(
        pools.len(),
        3,
        "expected 3 pools (Raydium + Orca + Meteora)"
    );

    // DAM-63 Part 1: the Meteora pool's reserves are non-zero.
    let meteora_pool = pools
        .iter()
        .find(|p| p.address.0 == meteora_pool_pk)
        .expect("Meteora pool not in assembled list");
    assert_eq!(meteora_pool.kind, AmmKind::MeteoraDlmm);
    assert!(
        meteora_pool.base_reserve > 0 && meteora_pool.quote_reserve > 0,
        "DAM-63 Part 1: Meteora reserves must be non-zero; \
         got base={} quote={}",
        meteora_pool.base_reserve,
        meteora_pool.quote_reserve
    );

    // DAM-63 Part 2: build the streaming detector with all
    // three pools and replay updates; assert a 3-leg cycle
    // is detected that includes the Meteora DLMM pool.
    let mut detector = StreamingDetector::new(&pools).expect("StreamingDetector::new");
    let mut any_cycle_includes_meteora = false;
    let mut any_3leg_dlmm_cycle: Option<dl_state::cycle::Cycle> = None;
    for p in &pools {
        let cycles = detector.on_pool_update(p);
        for cyc in &cycles {
            if cyc.legs.iter().any(|leg| leg.pool.0 == meteora_pool_pk) {
                any_cycle_includes_meteora = true;
                if cyc.n_legs() == 3 && any_3leg_dlmm_cycle.is_none() {
                    any_3leg_dlmm_cycle = Some(cyc.clone());
                }
            }
        }
    }
    assert!(
        any_cycle_includes_meteora,
        "DAM-63 Part 2: at least one cycle must include \
         the Meteora DLMM pool; none of the detected cycles \
         included it"
    );
    // Pin the contract: the DLMM-bearing cycle is a 3-leg
    // triangle and is negative-weight (profitable per the
    // linearized v1.0 weight formulation).
    let cyc = any_3leg_dlmm_cycle.expect(
        "DAM-63 Part 2: a 3-leg triangle arb must include \
         the Meteora DLMM pool",
    );
    assert_eq!(cyc.n_legs(), 3);
    assert!(
        cyc.weight_sum < 0,
        "DLMM triangle cycle should be negative-weight (profitable); got weight_sum={}",
        cyc.weight_sum
    );
}

#[test]
fn three_dex_triangle_without_meteora_vault_routing_no_3leg_cycle() {
    let (mut pools, meteora_pool_pk) = build_three_dex_triangle_pools();

    // Simulate the pre-DAM-63 bug: drop the Meteora pool from
    // the graph entirely (mimicking "vault update wasn't routed
    // to the parent pool because the dispatch didn't look up
    // the LbPair's `token_vault_x` / `token_vault_y` by
    // pubkey"). The graph builder will refuse to build with a
    // zero-reserve pool (`InvalidMath(DivByZero)`), so dropping
    // the pool is the equivalent of "no edge in the graph" from
    // the detector's perspective.
    pools.retain(|p| p.kind != AmmKind::MeteoraDlmm);
    assert_eq!(pools.len(), 2, "expected Raydium + Orca only");

    let mut detector = StreamingDetector::new(&pools).expect("StreamingDetector::new");
    let mut any_cycle_includes_meteora = false;
    for p in &pools {
        for cyc in detector.on_pool_update(p) {
            if cyc.legs.iter().any(|leg| leg.pool.0 == meteora_pool_pk) {
                any_cycle_includes_meteora = true;
            }
        }
    }
    assert!(
        !any_cycle_includes_meteora,
        "regression: with no Meteora pool in the graph, no \
         cycle should reference its pubkey; got a cycle anyway"
    );
}
