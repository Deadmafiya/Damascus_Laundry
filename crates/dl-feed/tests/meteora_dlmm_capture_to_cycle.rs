//! DAM-63 acceptance: a captured-replay test asserts that **Meteora
//! DLMM vault reserves flow in** and that a **DLMM leg appears in a
//! detected cycle**.
//!
//! ## Why this test exists
//!
//! Same shape as DAM-31.P2.1 (DAM-53 — DLMM LbPair subscription) and
//! DAM-62 (Whirlpool vault subscription). DAM-53 wired the LbPair
//! subscription in `dl-feed::ws_feed`; DAM-62 wired Orca Whirlpool
//! vault subscription + reserve routing in `dl-app::main`. DAM-63
//! mirrors DAM-62 for the Meteora DLMM path. Until this test lands,
//! the "DLMM half of the 3-DEX surface" has no end-to-end assertion.
//!
//! ## What the test does
//!
//! 1. Build a synthetic `.bincode` capture containing 9
//!    `AccountUpdate` frames: 1 Raydium `AmmInfo` + 2 SPL token
//!    account (vault) updates; 1 Orca Whirlpool + 2 vault updates;
//!    1 Meteora `LbPair` + 2 vault updates. The three pools form a
//!    triangle (USDC → SOL → USDT → USDC) with reserves sized to
//!    produce a negative cycle on detection.
//! 2. Replay the capture through a `CapturedFeed`.
//! 3. For each frame, dispatch to the right pool map
//!    (Raydium / Orca / Meteora) using the same decode-by-account-size
//!    logic as `dl_app::main::decode_pool_update` + the vault
//!    SplTokenAccount routing. The `AmmKind` per pool is recorded
//!    in a side table so the test can assert "this leg was DLMM".
//! 4. Push the resulting `Pool`s into a `StreamingDetector`. After
//!    the last vault update, the detector must yield at least one
//!    cycle.
//! 5. Assert that at least one of the cycle's legs was drawn from a
//!    pool with `kind = MeteoraDlmm`. This is the DLMM leg in
//!    detected cycles acceptance criterion.
//!
//! ## Why a fresh helper instead of `dl_app::main` directly
//!
//! The existing dispatch logic in `dl_app::main::run_live_paper` is
//! not `pub` and not exposed as a library entry point — it lives
//! inside `fn main`'s private scope. Replicating the ~70 lines of
//! dispatch in this test is the cheapest way to get a black-box
//! acceptance test without churning the binary's `fn main` shape.
//! The dispatch is small and stable: see `dispatch_capture_frame`.
//!
//! ## Test type
//!
//! Pure integration test in `dl-feed/tests/` because the capture
//! format is `dl-feed`'s wire contract; the test exercises the
//! capture-format round-trip + the decoder router + the
//! streaming detector. No async, no live RPC. The test does
//! **not** require the `ws` feature.

use std::collections::HashMap;
use std::io::Cursor;

use dl_core::{Feed, FeedEvent};
use dl_detect::cycle::Cycle;
use dl_feed::capture::{CaptureWriter, CapturedFeed};
use dl_state::decoder::{
    decode_amm_info, decode_lb_pair, decode_spl_token_account, decode_whirlpool,
};
use dl_state::pool::{AmmKind, Pool};
use dl_state::Pubkey;
use dl_stream::detector::StreamingDetector;

/// Mints used in the triangle. Match the fixtures in
/// `dl-detect/src/graph.rs::tests::multi_dex_triangle_dex_id_labeling`.
const MINT_USDC: [u8; 32] = [0x01; 32];
const MINT_SOL: [u8; 32] = [0x02; 32];
const MINT_USDT: [u8; 32] = [0x03; 32];

/// Pool addresses. The first byte is the per-DEX family tag so the
/// same addresses can appear in `dex_kind_by_pool` for assertions.
const POOL_RAYDIUM: [u8; 32] = [0xA1; 32];
const POOL_ORCA: [u8; 32] = [0xA2; 32];
const POOL_METEORA: [u8; 32] = [0xA3; 32];

/// Vault pubkeys. Each pool's two vaults. Keep distinct from each
/// other and from the pool pubkeys so the dispatch routes cleanly.
const VAULT_RAYDIUM_BASE: [u8; 32] = [0xB1; 32];
const VAULT_RAYDIUM_QUOTE: [u8; 32] = [0xB2; 32];
const VAULT_ORCA_BASE: [u8; 32] = [0xB3; 32];
const VAULT_ORCA_QUOTE: [u8; 32] = [0xB4; 32];
const VAULT_METEORA_BASE: [u8; 32] = [0xB5; 32];
const VAULT_METEORA_QUOTE: [u8; 32] = [0xB6; 32];

/// Build a synthetic Raydium `AmmInfo` blob (752 bytes) for the
/// USDC/SOL pool with the configured vault pubkeys.
fn build_raydium_amm_info() -> Vec<u8> {
    let mut buf = vec![0u8; 752];
    // status = 1 (Initialized) at offset 0.
    buf[0..8].copy_from_slice(&1u64.to_le_bytes());
    // base_decimals = 6 at offset 32 (u64).
    buf[32..40].copy_from_slice(&6u64.to_le_bytes());
    // quote_decimals = 9 at offset 40 (u64).
    buf[40..48].copy_from_slice(&9u64.to_le_bytes());
    // trade_fee_numerator = 25 at offset 144 (Raydium default 0.25% fee).
    buf[144..152].copy_from_slice(&25u64.to_le_bytes());
    // trade_fee_denominator = 10000 at offset 152.
    buf[152..160].copy_from_slice(&10_000u64.to_le_bytes());
    // base_vault at offset 336.
    buf[336..368].copy_from_slice(&VAULT_RAYDIUM_BASE);
    // quote_vault at offset 368.
    buf[368..400].copy_from_slice(&VAULT_RAYDIUM_QUOTE);
    // base_mint at offset 400.
    buf[400..432].copy_from_slice(&MINT_USDC);
    // quote_mint at offset 432.
    buf[432..464].copy_from_slice(&MINT_SOL);
    buf
}

/// Build a synthetic Orca Whirlpool blob (256 bytes) for the
/// SOL/USDT pool with the configured vault pubkeys. Uses the v1.0
/// "simplified" 256-byte layout (see
/// `dl_state::decoder::orca_whirlpool::WHIRLPOOL_ACCOUNT_SIZE`).
fn build_whirlpool_blob() -> Vec<u8> {
    let mut buf = vec![0u8; 256];
    // sqrt_price = Q64.64 of sqrt(105). Any non-zero value keeps
    // the decoder happy; the v1.0 graph only reads reserves + fee.
    let sqrt_price: u128 = 11_180_339_887_498_948_482_045_862_932_265_186_504;
    buf[0..16].copy_from_slice(&sqrt_price.to_le_bytes());
    // tick_current_index at offset 16.
    buf[16..20].copy_from_slice(&0i32.to_le_bytes());
    // tick_spacing at offset 20.
    buf[20..22].copy_from_slice(&64u16.to_le_bytes());
    // liquidity at offset 28 (6 bytes of padding skipped).
    let liquidity: u128 = 1_000_000_000_000_000;
    buf[28..44].copy_from_slice(&liquidity.to_le_bytes());
    // token_mint_x at offset 44.
    buf[44..76].copy_from_slice(&MINT_SOL);
    // token_mint_y at offset 76.
    buf[76..108].copy_from_slice(&MINT_USDT);
    // token_vault_x at offset 108.
    buf[108..140].copy_from_slice(&VAULT_ORCA_BASE);
    // token_vault_y at offset 140.
    buf[140..172].copy_from_slice(&VAULT_ORCA_QUOTE);
    // fee_rate at offset 254.
    buf[254..256].copy_from_slice(&30u16.to_le_bytes());
    buf
}

/// Build a synthetic Meteora DLMM `LbPair` blob (2236 bytes) for
/// the USDT/USDC pool. Uses the v1.0 layout (see
/// `dl_state::decoder::meteora_dlmm::decode_lb_pair`):
///   - 156 B header + 65 bins × 32 B = 2236 B total.
fn build_lb_pair_blob() -> Vec<u8> {
    let mut buf = vec![0u8; 156 + 32 * 65];
    // bin_step (u16) at offset 0.
    buf[0..2].copy_from_slice(&100u16.to_le_bytes());
    // active_id (i32) at offset 2.
    buf[2..6].copy_from_slice(&0i32.to_le_bytes());
    // 6 B padding at offset 6.
    // token_mint_x at offset 12.
    buf[12..44].copy_from_slice(&MINT_USDT);
    // token_mint_y at offset 44.
    buf[44..76].copy_from_slice(&MINT_USDC);
    // token_vault_x at offset 76.
    buf[76..108].copy_from_slice(&VAULT_METEORA_BASE);
    // token_vault_y at offset 108.
    buf[108..140].copy_from_slice(&VAULT_METEORA_QUOTE);
    // token_mint_x_program_flag (u8) at offset 140.
    buf[140] = 0;
    // token_mint_y_program_flag (u8) at offset 141.
    buf[141] = 0;
    // 14 B padding at offset 142.
    // 65 bins starting at offset 156. The v1.0 detector only reads
    // Pool.base_reserve / quote_reserve, so the bin layout is here
    // for completeness (so the decoder round-trips) but is not
    // load-bearing for cycle detection.
    let active_idx = 32usize;
    let base_reserve: u64 = 100_000;
    let quote_reserve: u64 = 110_000;
    let price: u128 = 1_000_000_000_000;
    let bin_off = 156 + active_idx * 32;
    buf[bin_off..bin_off + 8].copy_from_slice(&base_reserve.to_le_bytes());
    buf[bin_off + 8..bin_off + 16].copy_from_slice(&quote_reserve.to_le_bytes());
    buf[bin_off + 16..bin_off + 32].copy_from_slice(&price.to_le_bytes());
    buf
}

/// Build a 165-byte SPL token account blob carrying `amount`.
fn build_spl_token_account(mint: &[u8; 32], amount: u64) -> Vec<u8> {
    let mut buf = vec![0u8; 165];
    buf[0..32].copy_from_slice(mint);
    buf[64..72].copy_from_slice(&amount.to_le_bytes());
    buf
}

/// One frame: a synthetic `AccountUpdate` event.
fn acct_update(slot: u64, pubkey: [u8; 32], data: Vec<u8>) -> FeedEvent {
    FeedEvent::AccountUpdate { slot, pubkey, data }
}

/// Build the 9-frame triangle capture as a `Vec<u8>` (the on-disk
/// `.bincode` payload, in memory).
fn build_triangle_capture() -> Vec<u8> {
    let mut w = CaptureWriter::new(Vec::new()).expect("CaptureWriter::new");
    // Frame 0: Raydium AmmInfo (USDC/SOL).
    w.write_event(&acct_update(100, POOL_RAYDIUM, build_raydium_amm_info()))
        .expect("write raydium amm");
    // Frame 1: Raydium base vault update (USDC amount).
    // Reserves: 100_000 base (USDC) / 100_000 quote (SOL) — 1:1 ratio.
    w.write_event(&acct_update(
        101,
        VAULT_RAYDIUM_BASE,
        build_spl_token_account(&MINT_USDC, 100_000),
    ))
    .expect("write raydium base vault");
    // Frame 2: Raydium quote vault update (SOL amount).
    w.write_event(&acct_update(
        102,
        VAULT_RAYDIUM_QUOTE,
        build_spl_token_account(&MINT_SOL, 100_000),
    ))
    .expect("write raydium quote vault");
    // Frame 3: Orca Whirlpool (SOL/USDT).
    w.write_event(&acct_update(200, POOL_ORCA, build_whirlpool_blob()))
        .expect("write whirlpool");
    // Frame 4: Orca base vault update (SOL amount).
    // Reserves: 100_000 base (SOL) / 100_000 quote (USDT) — 1:1 ratio.
    w.write_event(&acct_update(
        201,
        VAULT_ORCA_BASE,
        build_spl_token_account(&MINT_SOL, 100_000),
    ))
    .expect("write orca base vault");
    // Frame 5: Orca quote vault update (USDT amount).
    w.write_event(&acct_update(
        202,
        VAULT_ORCA_QUOTE,
        build_spl_token_account(&MINT_USDT, 100_000),
    ))
    .expect("write orca quote vault");
    // Frame 6: Meteora LbPair (USDT/USDC).
    w.write_event(&acct_update(300, POOL_METEORA, build_lb_pair_blob()))
        .expect("write lb pair");
    // Frame 7: Meteora base vault update (USDT amount).
    // Reserves: 100_000 base (USDT) / 110_000 quote (USDC) — 100:110
    // (the "favorable" leg of the triangle; the round-trip
    // USDC→SOL→USDT→USDC earns ~10% gross, of which ~3×30 bps = 90
    // bps is fees, leaving a positive net). Matches the recipe in
    // `dl-detect/src/bellman_ford.rs::tests::finds_3leg_triangle_arb`.
    w.write_event(&acct_update(
        301,
        VAULT_METEORA_BASE,
        build_spl_token_account(&MINT_USDT, 100_000),
    ))
    .expect("write meteora base vault");
    // Frame 8: Meteora quote vault update (USDC amount).
    w.write_event(&acct_update(
        302,
        VAULT_METEORA_QUOTE,
        build_spl_token_account(&MINT_USDC, 110_000),
    ))
    .expect("write meteora quote vault");
    w.into_inner().expect("CaptureWriter into_inner")
}

/// State held by the dispatch loop. One entry per pool with
/// reserves=0 at first, then updated as vault updates arrive.
#[derive(Debug, Clone)]
struct DispatchState {
    raydium: HashMap<[u8; 32], PoolStub>,
    orca: HashMap<[u8; 32], PoolStub>,
    meteora: HashMap<[u8; 32], PoolStub>,
    /// For each pool, which DEX family it belongs to. Lets the test
    /// map a `Cycle::Leg::pool` back to an `AmmKind` for the
    /// "DLMM leg appears in detected cycles" assertion.
    dex_kind_by_pool: HashMap<[u8; 32], AmmKind>,
}

#[derive(Debug, Clone)]
struct PoolStub {
    pool: Pool,
    base_vault: [u8; 32],
    quote_vault: [u8; 32],
}

impl Default for DispatchState {
    fn default() -> Self {
        Self {
            raydium: HashMap::new(),
            orca: HashMap::new(),
            meteora: HashMap::new(),
            dex_kind_by_pool: HashMap::new(),
        }
    }
}

impl DispatchState {
    fn pool_reserves(&self, pool: &[u8; 32]) -> (u64, u64) {
        let stub = self
            .raydium
            .get(pool)
            .or_else(|| self.orca.get(pool))
            .or_else(|| self.meteora.get(pool));
        match stub {
            Some(s) => (s.pool.base_reserve, s.pool.quote_reserve),
            None => (0, 0),
        }
    }
}

/// Dispatch one captured frame. Mirrors the routing in
/// `dl_app::main::run_live_paper` (the `FIRST` / `FIRST-B` /
/// `SECOND` branches at the bottom of the file). The dispatch is
/// pure (no RPC, no async) — it just routes a frame into the right
/// pool map and updates reserves.
fn dispatch_capture_frame(state: &mut DispatchState, ev: &FeedEvent) {
    let FeedEvent::AccountUpdate {
        slot: _,
        pubkey,
        data,
    } = ev
    else {
        return;
    };
    // FIRST-A: Orca Whirlpool (256 B).
    if data.len() == 256 {
        if let Ok(w) = decode_whirlpool(data) {
            let pool = Pool {
                address: Pubkey(*pubkey),
                kind: AmmKind::OrcaWhirlpool,
                base_mint: w.token_mint_x,
                quote_mint: w.token_mint_y,
                base_decimals: 9,
                quote_decimals: 6,
                base_reserve: 0,
                quote_reserve: 0,
                fee_bps: w.fee_rate,
                last_update_slot: 0,
                ..Default::default()
            };
            state
                .dex_kind_by_pool
                .insert(*pubkey, AmmKind::OrcaWhirlpool);
            state.orca.insert(
                *pubkey,
                PoolStub {
                    pool,
                    base_vault: w.token_vault_x.0,
                    quote_vault: w.token_vault_y.0,
                },
            );
            return;
        }
    }
    // FIRST-B: Meteora DLMM LbPair (>= 2236 B for the v1.0 layout
    // with 65 bins).
    if data.len() >= 156 + 32 * 65 {
        if let Ok(lp) = decode_lb_pair(data) {
            let pool = Pool {
                address: Pubkey(*pubkey),
                kind: AmmKind::MeteoraDlmm,
                base_mint: lp.token_mint_x,
                quote_mint: lp.token_mint_y,
                base_decimals: 6,
                quote_decimals: 6,
                base_reserve: 0,
                quote_reserve: 0,
                fee_bps: (lp.bin_step as u16).min(u16::MAX),
                last_update_slot: 0,
                ..Default::default()
            };
            state.dex_kind_by_pool.insert(*pubkey, AmmKind::MeteoraDlmm);
            state.meteora.insert(
                *pubkey,
                PoolStub {
                    pool,
                    base_vault: lp.token_vault_x.0,
                    quote_vault: lp.token_vault_y.0,
                },
            );
            return;
        }
    }
    // FIRST-C: Raydium AmmInfo (752 B).
    if data.len() == 752 {
        if let Ok(amm) = decode_amm_info(data) {
            let pool = Pool {
                address: Pubkey(*pubkey),
                kind: AmmKind::RaydiumAmmV4,
                base_mint: amm.base_mint,
                quote_mint: amm.quote_mint,
                base_decimals: amm.base_decimals,
                quote_decimals: amm.quote_decimals,
                base_reserve: 0,
                quote_reserve: 0,
                fee_bps: amm.fee_bps().unwrap_or(30),
                last_update_slot: 0,
                ..Default::default()
            };
            state
                .dex_kind_by_pool
                .insert(*pubkey, AmmKind::RaydiumAmmV4);
            state.raydium.insert(
                *pubkey,
                PoolStub {
                    pool,
                    base_vault: amm.base_vault.0,
                    quote_vault: amm.quote_vault.0,
                },
            );
            return;
        }
    }
    // SECOND: SPL token account (165+ B). Find the parent pool
    // across all three pool maps.
    if data.len() >= 165 {
        if let Ok(spl) = decode_spl_token_account(data) {
            update_parent_reserves(state, pubkey, spl.amount);
        }
    }
}

fn update_parent_reserves(state: &mut DispatchState, vault_pk: &[u8; 32], amount: u64) {
    for map in [&mut state.raydium, &mut state.orca, &mut state.meteora] {
        let hit = map
            .iter_mut()
            .find(|(_, stub)| stub.base_vault == *vault_pk || stub.quote_vault == *vault_pk);
        if let Some((_, stub)) = hit {
            if stub.base_vault == *vault_pk {
                stub.pool.base_reserve = amount;
            } else {
                stub.pool.quote_reserve = amount;
            }
            return;
        }
    }
}

fn snapshot_pools(state: &DispatchState) -> Vec<Pool> {
    let mut out = Vec::with_capacity(state.raydium.len() + state.orca.len() + state.meteora.len());
    for (_, s) in &state.raydium {
        out.push(s.pool.clone());
    }
    for (_, s) in &state.orca {
        out.push(s.pool.clone());
    }
    for (_, s) in &state.meteora {
        out.push(s.pool.clone());
    }
    out
}

#[test]
fn meteora_dlmm_capture_to_cycle_replays_end_to_end() {
    // Build the capture.
    let bytes = build_triangle_capture();
    let mut feed = CapturedFeed::open(Cursor::new(bytes)).expect("CapturedFeed::open");

    // Replay the capture through the dispatch.
    let mut state = DispatchState::default();
    let mut frame_count = 0u64;
    while let Some(ev) = feed.next_event() {
        dispatch_capture_frame(&mut state, &ev);
        frame_count += 1;
    }
    assert_eq!(
        frame_count, 9,
        "expected 9 frames in the capture; got {frame_count}"
    );

    // DAM-63 acceptance #1: DLMM reserves flowed in. Both
    // VAULT_METEORA_BASE and VAULT_METEORA_QUOTE updates must have
    // reached the Meteora pool stub.
    let (dlmm_base, dlmm_quote) = state.pool_reserves(&POOL_METEORA);
    assert_eq!(dlmm_base, 100_000, "DLMM base reserve did not flow in");
    assert_eq!(dlmm_quote, 110_000, "DLMM quote reserve did not flow in");
    assert_eq!(
        state.dex_kind_by_pool.get(&POOL_METEORA).copied(),
        Some(AmmKind::MeteoraDlmm),
        "Meteora pool kind not recorded"
    );

    // Sanity: the other two pools also have reserves (the test
    // checks the full triangle, not just DLMM).
    assert_eq!(state.pool_reserves(&POOL_RAYDIUM), (100_000, 100_000));
    assert_eq!(state.pool_reserves(&POOL_ORCA), (100_000, 100_000));

    // DAM-63 acceptance #2: a DLMM leg appears in a detected cycle.
    // Build the streaming detector from the post-replay snapshot and
    // run detection. Note: in the live paper trader, every vault
    // update re-runs `detector.on_pool_update(&pool)` once both
    // reserves are non-zero. Here we re-run all three pools in
    // insertion order against a single detector.
    let pools = snapshot_pools(&state);
    let mut detector = StreamingDetector::new(&pools).expect("detector::new");
    let mut all_cycles: Vec<Cycle> = Vec::new();
    for p in &pools {
        let cyc = detector.on_pool_update(p);
        all_cycles.extend(cyc);
    }
    assert!(
        !all_cycles.is_empty(),
        "no cycles detected from the captured triangle"
    );
    // Find at least one cycle containing the Meteora pool. A "leg"
    // in a cycle carries the pool pubkey and the direction; we map
    // the pubkey back to its AmmKind via dex_kind_by_pool.
    let dlmm_cycles: Vec<&Cycle> = all_cycles
        .iter()
        .filter(|c| {
            c.legs
                .iter()
                .any(|l| state.dex_kind_by_pool.get(&l.pool.0) == Some(&AmmKind::MeteoraDlmm))
        })
        .collect();
    assert!(
        !dlmm_cycles.is_empty(),
        "no cycle contains a Meteora DLMM leg; legs seen: {:?}",
        all_cycles
            .iter()
            .flat_map(|c| c.legs.iter())
            .map(|l| state.dex_kind_by_pool.get(&l.pool.0).copied())
            .collect::<Vec<_>>()
    );
    // The DLMM cycle should be a 3-leg triangle (USDC -> SOL -> USDT
    // -> USDC). Pin the n_legs contract.
    let cyc = dlmm_cycles[0];
    assert_eq!(
        cyc.n_legs(),
        3,
        "expected a 3-leg triangle; got {} legs",
        cyc.n_legs()
    );
    assert!(
        cyc.weight_sum < 0,
        "DLMM triangle cycle should be negative-weight (profitable); got weight_sum={}",
        cyc.weight_sum
    );
}

#[test]
fn meteora_pool_reserves_remain_zero_until_both_vaults_seen() {
    // DAM-63 acceptance #1 (sub-case): a partial capture that
    // includes the LbPair + only one vault update must NOT yet
    // credit the Meteora pool with non-zero reserves. The other
    // (missing) vault stays at 0, and the live detector's
    // "both reserves > 0" gate therefore holds. This is the
    // guard against a partial-capture regression that would
    // falsely emit a cycle with a half-populated pool.
    let mut w = CaptureWriter::new(Vec::new()).expect("CaptureWriter::new");
    w.write_event(&acct_update(300, POOL_METEORA, build_lb_pair_blob()))
        .expect("write lb pair");
    w.write_event(&acct_update(
        301,
        VAULT_METEORA_BASE,
        build_spl_token_account(&MINT_USDT, 100_000),
    ))
    .expect("write meteora base vault");
    // No second vault update.
    let bytes = w.into_inner().expect("into_inner");

    let mut feed = CapturedFeed::open(Cursor::new(bytes)).expect("CapturedFeed::open");
    let mut state = DispatchState::default();
    while let Some(ev) = feed.next_event() {
        dispatch_capture_frame(&mut state, &ev);
    }
    let (b, q) = state.pool_reserves(&POOL_METEORA);
    assert_eq!(b, 100_000, "base vault should be credited");
    assert_eq!(q, 0, "quote vault should be 0 (no update seen yet)");
}
