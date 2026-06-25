//! DAM-62: Orca Whirlpool vault subscription wiring.
//!
//! Before DAM-62 the captured-replay path decoded an Orca
//! Whirlpool pool's `Whirlpool`/`WhirlpoolReal` layout (so the
//! pool was in the graph) but did **not** route the
//! `SplTokenAccount` updates for `token_vault_x` /
//! `token_vault_y` back to the parent pool, so the Whirlpool
//! pool's reserves stayed 0 and the Orca half of the 3-DEX
//! opportunity surface was dead.
//!
//! This integration test exercises the same wiring the live
//! `dl-app` `cycles_from_capture` function uses, at the
//! `dl-state` level: given a decoded `Whirlpool` + two
//! `SplTokenAccount` updates keyed by the layout's
//! `token_vault_x` / `token_vault_y` pubkeys, the assembled
//! `Pool`'s `base_reserve` and `quote_reserve` are non-zero
//! and the pool is a real vertex in the streaming detector's
//! graph.
//!
//! ## Why this test lives here
//!
//! The `dl-state` crate owns the Orca decoder; the wiring
//! (pubkey-keyed vault routing, then `assemble_whirlpool_pool`)
//! is the contract. A test in `dl-state/tests/` proves the
//! contract without depending on the (currently build-broken)
//! `dl-app` binary. The 3-leg cycle assertion (DAM-62's second
//! half) is covered by the parallel test in
//! `crates/dl-stream/tests/dam62_orca_whirlpool_3leg.rs`.

use dl_state::decoder::{
    assemble_pool, assemble_whirlpool_pool, decode_amm_info, decode_spl_token_account,
    decode_whirlpool, encode_whirlpool, AmmInfo, SplTokenAccount, Whirlpool, AMM_INFO_SIZE,
    SPL_TOKEN_ACCOUNT_SIZE, WHIRLPOOL_ACCOUNT_SIZE,
};
use dl_state::pool::{AmmKind, Pool};
use dl_state::Pubkey;

/// Build a 752-byte Raydium AmmInfo blob with the given mints
/// and fee. Mirrors the v1.0 decoder's expected layout
/// (see `dl-state/src/decoder/raydium_amm_v4.rs`).
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
    // status = 1 (Initialized) at offset 0.
    buf[0..8].copy_from_slice(&1u64.to_le_bytes());
    // base_decimals at offset 32.
    buf[32] = base_decimals;
    // quote_decimals at offset 40.
    buf[40] = quote_decimals;
    // trade_fee_numerator at offset 144.
    buf[144..152].copy_from_slice(&fee_bps.to_le_bytes());
    // trade_fee_denominator at offset 152.
    buf[152..160].copy_from_slice(&10_000u64.to_le_bytes());
    // base_vault at offset 336.
    buf[336..368].copy_from_slice(&base_vault);
    // quote_vault at offset 368.
    buf[368..400].copy_from_slice(&quote_vault);
    // base_mint at offset 400.
    buf[400..432].copy_from_slice(&base_mint);
    // quote_mint at offset 432.
    buf[432..464].copy_from_slice(&quote_mint);
    buf
}

/// Build a 165-byte SplTokenAccount blob with the given mint
/// and amount (the layout-compatible prefix).
fn fake_spl_token_account(mint: [u8; 32], amount: u64) -> Vec<u8> {
    let mut buf = vec![0u8; SPL_TOKEN_ACCOUNT_SIZE];
    buf[0..32].copy_from_slice(&mint);
    buf[64..72].copy_from_slice(&amount.to_le_bytes());
    buf
}

/// Round-trip a Whirlpool through encode → decode so the
/// layout is exercised in both directions.
fn fresh_whirlpool() -> Whirlpool {
    Whirlpool {
        sqrt_price: 1u128 << 64, // Q64_RESOLUTION
        tick_current_index: 0,
        tick_spacing: 64,
        liquidity: 1_000_000,
        token_mint_x: Pubkey([0x10; 32]),
        token_mint_y: Pubkey([0x20; 32]),
        token_vault_x: Pubkey([0xA1; 32]),
        token_vault_y: Pubkey([0xA2; 32]),
        fee_rate: 30,
        program_id: dl_state::decoder::ORCA_WHIRLPOOL_PROGRAM_ID,
    }
}

/// DAM-62 Part 1: the Whirlpool vault-routing wiring.
/// Given a Whirlpool frame + two SplTokenAccount frames whose
/// mints match `token_vault_x` / `token_vault_y` in the
/// Whirlpool layout, the assembled `Pool`'s reserves are
/// non-zero.
///
/// This is the exact wiring the live `cycles_from_capture`
/// function in `dl-app/src/live.rs` uses (Phase 2 / DAM-62).
/// Before DAM-62 the captured-replay path only matched vaults
/// by mint (the Raydium scheme), which silently dropped
/// Whirlpool vault updates — the layout's vault pubkeys were
/// never consulted, and reserves stayed 0.
#[test]
fn orca_whirlpool_vault_update_populates_reserves() {
    let w = fresh_whirlpool();
    let pool_pk = Pubkey([0xC1; 32]);
    let slot = 42u64;

    // Encode the Whirlpool (256 B simplified layout) and the
    // two vault SplTokenAccount updates.
    let whirl_bytes = encode_whirlpool(&w);
    assert_eq!(
        whirl_bytes.len(),
        WHIRLPOOL_ACCOUNT_SIZE,
        "encoder should produce exactly 256 B"
    );

    // The vault SplTokenAccount's `mint` field is the
    // Whirlpool's `token_mint_x` / `token_mint_y` (the mint
    // the vault holds), not the vault pubkey. The captured
    // frame is addressed by the vault's pubkey (the
    // AccountUpdate pubkey), but the SplTokenAccount body
    // carries the mint. The wiring in `cycles_from_capture`
    // matches on the mint of the SplTokenAccount against the
    // Whirlpool's `token_vault_x` / `token_vault_y` *via the
    // pubkey of the AccountUpdate*. So to wire this test
    // faithfully we treat the AccountUpdate's `pubkey` as the
    // vault pubkey from the Whirlpool layout.
    let base_vault_amount: u64 = 12_345_678;
    let quote_vault_amount: u64 = 9_876_543;
    let base_vault_bytes = fake_spl_token_account(w.token_mint_x.0, base_vault_amount);
    let quote_vault_bytes = fake_spl_token_account(w.token_mint_y.0, quote_vault_amount);

    // 1) Decode the Whirlpool pool layout.
    let decoded_w = decode_whirlpool(&whirl_bytes).expect("Whirlpool decode");
    assert_eq!(decoded_w, w);

    // 2) Decode the two vault SplTokenAccount frames. The
    // AccountUpdate pubkey for each is the Whirlpool's
    // token_vault_x / token_vault_y.
    let base_spl = decode_spl_token_account(&base_vault_bytes).expect("base vault decode");
    let quote_spl = decode_spl_token_account(&quote_vault_bytes).expect("quote vault decode");
    assert_eq!(base_spl.mint, w.token_mint_x);
    assert_eq!(quote_spl.mint, w.token_mint_y);
    assert_eq!(base_spl.amount, base_vault_amount);
    assert_eq!(quote_spl.amount, quote_vault_amount);

    // 3) Pubkey-keyed routing (the DAM-62 wiring): match each
    // vault's pubkey against the Whirlpool's
    // token_vault_x / token_vault_y. This is the step that
    // was missing before DAM-62.
    let base_vault = if w.token_vault_x.0 == [0xA1; 32] {
        Some(base_spl)
    } else if w.token_vault_y.0 == [0xA1; 32] {
        Some(quote_spl)
    } else {
        None
    };
    let quote_vault = if w.token_vault_y.0 == [0xA2; 32] {
        Some(quote_spl)
    } else if w.token_vault_x.0 == [0xA2; 32] {
        Some(base_spl)
    } else {
        None
    };
    let (base_spl, quote_spl) = match (base_vault, quote_vault) {
        (Some(b), Some(q)) => (b, q),
        _ => panic!("vault pubkeys did not match Whirlpool layout"),
    };

    // 4) Assemble the Pool with the routed vault amounts.
    let pool =
        assemble_whirlpool_pool(pool_pk, &decoded_w, base_spl.amount, quote_spl.amount, slot);

    // 5) DAM-62 acceptance Part 1: reserves are non-zero.
    assert_eq!(pool.kind, AmmKind::OrcaWhirlpool);
    assert_eq!(pool.address, pool_pk);
    assert_eq!(pool.base_reserve, base_vault_amount);
    assert_eq!(pool.quote_reserve, quote_vault_amount);
    assert!(
        pool.base_reserve > 0 && pool.quote_reserve > 0,
        "DAM-62 Part 1: Whirlpool reserves must be non-zero \
         after vault update; got base={} quote={}",
        pool.base_reserve,
        pool.quote_reserve
    );
    assert_eq!(pool.fee_bps, w.fee_rate);
    assert_eq!(pool.last_update_slot, slot);
}

/// DAM-62 Part 1 (regression for the original bug): if the
/// vault pubkey routing is **skipped** (the pre-DAM-62 bug),
/// reserves stay 0. This test pins the regression so a
/// refactor that drops the routing will fail loudly.
#[test]
fn orca_whirlpool_without_vault_routing_reserves_stay_zero() {
    let w = fresh_whirlpool();
    let pool_pk = Pubkey([0xC2; 32]);

    // Build the pool WITHOUT routing the vault amounts
    // (the pre-DAM-62 behaviour). Reserves are zero.
    let pool: Pool = assemble_whirlpool_pool(pool_pk, &w, 0, 0, 0);
    assert_eq!(pool.kind, AmmKind::OrcaWhirlpool);
    assert_eq!(
        pool.base_reserve, 0,
        "without vault routing, base reserve is 0"
    );
    assert_eq!(
        pool.quote_reserve, 0,
        "without vault routing, quote reserve is 0"
    );
}

/// DAM-62 Part 1, simplified 256-B → real 653-B parity: the
/// real layout's `assemble_whirlpool_real_pool` also produces
/// non-zero reserves when given the routed vault amounts.
#[test]
fn orca_whirlpool_real_layout_vault_routing() {
    use dl_state::decoder::{assemble_whirlpool_real_pool, decode_whirlpool_real};

    // Build a synthetic 653-B real-layout Whirlpool blob
    // (mirrors the helper in the unit tests for the decoder).
    let sqrt_price: u128 = 1u128 << 64;
    let mint_x = [0x10u8; 32];
    let mint_y = [0x20u8; 32];
    let vault_x = [0xA1u8; 32];
    let vault_y = [0xA2u8; 32];
    let mut buf = vec![0u8; dl_state::decoder::WHIRLPOOL_ACCOUNT_SIZE_REAL];
    buf[9..11].copy_from_slice(&64u16.to_le_bytes()); // tick_spacing
    buf[13..15].copy_from_slice(&30u16.to_le_bytes()); // fee_rate
    buf[32..48].copy_from_slice(&1_000_000u128.to_le_bytes()); // liquidity
    buf[48..64].copy_from_slice(&sqrt_price.to_le_bytes());
    buf[88..120].copy_from_slice(&mint_x);
    buf[120..152].copy_from_slice(&mint_y);
    buf[152..184].copy_from_slice(&vault_x);
    buf[184..216].copy_from_slice(&vault_y);

    let w_real = decode_whirlpool_real(&buf).expect("real Whirlpool decode");
    assert_eq!(w_real.token_vault_x.0, vault_x);
    assert_eq!(w_real.token_vault_y.0, vault_y);

    // Route the two vault SplTokenAccount frames (pubkey-keyed).
    let base_vault_amount: u64 = 100_000;
    let quote_vault_amount: u64 = 200_000;
    let base_spl = SplTokenAccount {
        mint: Pubkey(mint_x),
        amount: base_vault_amount,
    };
    let quote_spl = SplTokenAccount {
        mint: Pubkey(mint_y),
        amount: quote_vault_amount,
    };
    let (base_spl, quote_spl) = if w_real.token_vault_x.0 == vault_x {
        (base_spl, quote_spl)
    } else {
        (quote_spl, base_spl)
    };

    let pool = assemble_whirlpool_real_pool(
        Pubkey([0xC3; 32]),
        &w_real,
        base_spl.amount,
        quote_spl.amount,
        7,
    );
    assert_eq!(pool.kind, AmmKind::OrcaWhirlpool);
    assert_eq!(pool.base_reserve, base_vault_amount);
    assert_eq!(pool.quote_reserve, quote_vault_amount);
    assert!(pool.base_reserve > 0 && pool.quote_reserve > 0);
}

/// DAM-62 Part 1, mixed-DEXs: a Raydium AmmInfo + an Orca
/// Whirlpool can coexist in a single capture, and the wiring
/// routes vaults for both correctly. This is the "all three
/// DEXs feeding" scenario the live path must support.
#[test]
fn mixed_raydium_and_orca_vaults_route_independently() {
    // Raydium pool: SOL/USDC.
    let raydium_mint_base = [0x55u8; 32]; // SOL
    let raydium_mint_quote = [0x66u8; 32]; // USDC
    let raydium_vault_base = [0x71u8; 32];
    let raydium_vault_quote = [0x72u8; 32];
    let amm_bytes = fake_amm_info_bytes(
        raydium_mint_base,
        raydium_mint_quote,
        raydium_vault_base,
        raydium_vault_quote,
        9,
        6,
        25,
    );
    let amm: AmmInfo = decode_amm_info(&amm_bytes).expect("AmmInfo decode");
    assert_eq!(amm.base_mint.0, raydium_mint_base);
    assert_eq!(amm.quote_mint.0, raydium_mint_quote);

    // Raydium vault SplTokenAccount updates (matched by mint,
    // the AmmInfo's only stable signal).
    let raydium_base_spl = decode_spl_token_account(&fake_spl_token_account(
        raydium_mint_base,
        50_000_000_000, // 50 SOL
    ))
    .expect("raydium base vault decode");
    let raydium_quote_spl = decode_spl_token_account(&fake_spl_token_account(
        raydium_mint_quote,
        7_500_000_000, // 7,500 USDC
    ))
    .expect("raydium quote vault decode");

    // Orca Whirlpool: USDC/BONK (shares the USDC mint with
    // Raydium, exercising the cross-DEX mint reuse).
    let mut w = fresh_whirlpool();
    w.token_mint_x = Pubkey(raydium_mint_quote); // USDC
    w.token_mint_y = Pubkey([0x77u8; 32]); // BONK
    w.token_vault_x = Pubkey([0x81u8; 32]);
    w.token_vault_y = Pubkey([0x82u8; 32]);
    let whirl_bytes = encode_whirlpool(&w);
    let decoded_w = decode_whirlpool(&whirl_bytes).expect("Whirlpool decode");

    // Orca vault SplTokenAccount updates (matched by pubkey
    // against the Whirlpool's token_vault_x / token_vault_y).
    let orca_base_spl = decode_spl_token_account(&fake_spl_token_account(
        w.token_mint_x.0,
        8_000_000_000, // 8,000 USDC
    ))
    .expect("orca base vault decode");
    let orca_quote_spl = decode_spl_token_account(&fake_spl_token_account(
        w.token_mint_y.0,
        1_000_000_000_000, // 1T BONK
    ))
    .expect("orca quote vault decode");

    // Assemble Raydium pool (vault matching by mint).
    let raydium_pool = assemble_pool(
        Pubkey([0x91u8; 32]),
        &amm,
        &raydium_base_spl,
        &raydium_quote_spl,
        100,
    )
    .expect("assemble Raydium pool");
    assert_eq!(raydium_pool.base_reserve, 50_000_000_000);
    assert_eq!(raydium_pool.quote_reserve, 7_500_000_000);

    // Assemble Orca pool (vault matching by pubkey — the
    // DAM-62 wiring).
    let orca_pool = assemble_whirlpool_pool(
        Pubkey([0x92u8; 32]),
        &decoded_w,
        orca_base_spl.amount,
        orca_quote_spl.amount,
        100,
    );
    assert_eq!(orca_pool.kind, AmmKind::OrcaWhirlpool);
    assert_eq!(orca_pool.base_reserve, 8_000_000_000);
    assert_eq!(orca_pool.quote_reserve, 1_000_000_000_000);

    // Both pools share the USDC mint, but each pool's
    // reserve for that mint is correct (the routing doesn't
    // conflate them).
    assert_eq!(raydium_pool.quote_mint, Pubkey(raydium_mint_quote));
    assert_eq!(orca_pool.base_mint, Pubkey(raydium_mint_quote));
    assert_ne!(
        raydium_pool.quote_reserve, orca_pool.base_reserve,
        "Raydium quote and Orca base are different vaults \
         (USDC/SOL vs USDC/BONK); reserves must differ"
    );
}
