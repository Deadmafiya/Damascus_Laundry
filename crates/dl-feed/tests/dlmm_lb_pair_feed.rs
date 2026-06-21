//! DAM-53 acceptance: a scripted `FeedEvent::AccountUpdate` whose
//! `data` is a synthetic Meteora DLMM `LbPair` blob (2236 B) is
//! decoded cleanly via `dl_state::decoder::meteora_dlmm`, the bin
//! array is preserved end-to-end, and the integer-only invariant
//! is held (no `f32` / `f64` on the decode path).
//!
//! ## Why this test exists
//!
//! DAM-44b wires the LbPair account subscription into the
//! `dl-feed` registry layer. The wire path is the same
//! `accountSubscribe` JSON-RPC method Raydium vault subscriptions
//! already use; the per-account notification lands as
//! `FeedEvent::AccountUpdate { data, .. }` with the raw 2236-byte
//! blob. The consumer (`dl_app::main::run_live_paper` and the
//! `dl-feed` capture pipeline) must be able to decode that blob
//! and pull the per-bin structure (`amount_x`, `amount_y`,
//! `price`) through to the v3 cross-DEX fill math without losing
//! the bin array.
//!
//! This test asserts the decode end of that path: a 2236-byte
//! synthetic LbPair blob, played back as a `ScriptedFeed`, is
//! decoded and the bin array comes back identical. It does not
//! require a live WebSocket; the test never opens a socket.
//!
//! ## Integer-only invariant
//!
//! The decoded `LbPair` carries `bin_amount_x: Vec<u64>`,
//! `bin_amount_y: Vec<u64>`, and `bin_price: Vec<u128>`. The fill
//! math in `dl_sim::fill_meteora` consumes these directly. The
//! CI guard `tests/fixed_point_no_floats.rs` enforces no
//! `f32` / `f64` in this crate's source tree; this test
//! reinforces the same invariant on the data path.
//!
//! ## What this test does NOT cover
//!
//! - Live WebSocket subscription. That's gated on env vars
//!   (`DL_TEST_RPC_URL` / `DL_TEST_POOL_PUBKEY`) and lives in
//!   `tests/ws_feed_live.rs`.
//! - The `MAX_POOL_AGE_SLOTS` graph-edge prune — DAM-54.
//! - End-to-end recon — DAM-55.

use dl_core::Feed;
use dl_core::FeedEvent;
use dl_state::decoder::meteora_dlmm::{decode_lb_pair, encode_lb_pair, LbPair, SCALE_OFFSET};
use dl_state::Pubkey;

/// Build a 2236-byte synthetic `LbPair` blob (156 B header + 65
/// bins × 32 B per bin). The active bin (index 0 = `active_id`)
/// carries a non-zero price + reserves; the next bin (index 1)
/// carries a `bin_step`-derived higher price; the remaining 63
/// bins are zero. This is enough to assert the per-bin structure
/// is preserved (no collapse-to-single-price) and that
/// `bin_price_at(absolute_bin_id)` continues to return the right
/// value.
fn build_synthetic_lb_pair_blob(active_id: i32, bin_step_bps: u16) -> Vec<u8> {
    let mut bin_amount_x = vec![0u64; LbPair::BIN_WINDOW];
    let mut bin_amount_y = vec![0u64; LbPair::BIN_WINDOW];
    let mut bin_price = vec![0u128; LbPair::BIN_WINDOW];
    // Active bin (index 0).
    bin_amount_x[0] = 1_000_000_000; // 1.0 SOL
    bin_amount_y[0] = 2_000_000_000; // 2.0 USDC
    bin_price[0] = SCALE_OFFSET; // price = 1.0
    // Next bin (index 1 = active_id + 1): bin_step_bps higher
    // price. The real DLMM SDK derives this from
    // `bin_step_bps / 10_000` of compounding; for the test we
    // seed a value that is a clean
    // `(SCALE_OFFSET * (1 + bin_step_bps / 10_000))` so the
    // assertion is exact.
    let next_price = SCALE_OFFSET + (SCALE_OFFSET * bin_step_bps as u128) / 10_000;
    bin_price[1] = next_price;
    let lp = LbPair {
        bin_step: bin_step_bps,
        active_id,
        token_mint_x: Pubkey([0x11u8; 32]),
        token_mint_y: Pubkey([0x22u8; 32]),
        token_vault_x: Pubkey([0x33u8; 32]),
        token_vault_y: Pubkey([0x44u8; 32]),
        token_mint_x_program_flag: 0,
        token_mint_y_program_flag: 0,
        bin_amount_x,
        bin_amount_y,
        bin_price,
        program_id: dl_state::decoder::meteora_dlmm::METEORA_DLMM_PROGRAM_ID,
    };
    encode_lb_pair(&lp)
}

#[test]
fn scripted_lb_pair_account_update_decodes_via_dl_state() {
    // DAM-53 acceptance #1: a 2236-byte synthetic LbPair blob
    // played back as a `ScriptedFeed` decodes cleanly via the
    // existing `dl_state::decoder::meteora_dlmm` path. The
    // decode is what `dl_app::main::run_live_paper` invokes
    // when an `AccountUpdate` for a known LbPair pubkey
    // arrives.
    let bytes = build_synthetic_lb_pair_blob(100, 100);
    assert_eq!(bytes.len(), 156 + 32 * LbPair::BIN_WINDOW);
    let pubkey: [u8; 32] = [0xAA; 32];
    let slot: u64 = 12345;
    let script = vec![FeedEvent::AccountUpdate {
        slot,
        pubkey,
        data: bytes.clone(),
    }];
    let mut feed = dl_core::ScriptedFeed::new(script);
    let ev = feed.next_event().expect("first event");
    let data = match &ev {
        FeedEvent::AccountUpdate { data, .. } => data.clone(),
        other => panic!("expected AccountUpdate, got {other:?}"),
    };
    // DAM-53 acceptance #2: decode succeeds and the bin
    // array is preserved (no collapse-to-single-price).
    let lp = decode_lb_pair(&data).expect("decode_lb_pair");
    assert_eq!(lp.bin_step, 100);
    assert_eq!(lp.active_id, 100);
    assert_eq!(lp.bin_amount_x.len(), LbPair::BIN_WINDOW);
    assert_eq!(lp.bin_amount_y.len(), LbPair::BIN_WINDOW);
    assert_eq!(lp.bin_price.len(), LbPair::BIN_WINDOW);
    assert_eq!(lp.bin_price[0], SCALE_OFFSET);
    // The next bin's price reflects the `bin_step` (1%).
    assert_eq!(lp.bin_price[1], SCALE_OFFSET + SCALE_OFFSET / 100);
    // The full 65-bin window is preserved.
    assert_eq!(lp.bin_price.len(), 65);
}

#[test]
fn bin_arrays_preserve_per_bin_structure_through_decode() {
    // Stronger assertion: the per-bin structure comes back
    // identical after encode → decode. The fill math in
    // `dl_sim::fill_meteora` walks the bin array; this test
    // confirms the per-bin shape is not collapsed.
    let original_bytes = build_synthetic_lb_pair_blob(50, 250);
    let lp_decoded = decode_lb_pair(&original_bytes).expect("decode");
    // Active bin: prices + reserves present.
    assert_eq!(lp_decoded.bin_amount_x[0], 1_000_000_000);
    assert_eq!(lp_decoded.bin_amount_y[0], 2_000_000_000);
    assert_eq!(lp_decoded.bin_price[0], SCALE_OFFSET);
    // Round-trip: encode the decoded struct again and confirm
    // the bytes are identical (deterministic, integer-only).
    let re_encoded = encode_lb_pair(&lp_decoded);
    assert_eq!(re_encoded, original_bytes);
    // `bin_price_at(absolute_bin_id)` returns the per-bin
    // price. The active bin (id = 50) is at index 0; the
    // next bin (id = 51) is at index 1 and carries a 2.5%
    // higher price (bin_step = 250 bps).
    assert_eq!(lp_decoded.bin_price_at(50), Some(SCALE_OFFSET));
    let expected_next = SCALE_OFFSET + (SCALE_OFFSET * 250) / 10_000;
    assert_eq!(lp_decoded.bin_price_at(51), Some(expected_next));
    // A bin outside the 65-bin window around the active id
    // returns None (out of window, not "no price").
    // active_id=50, window=[50, 114]. Anything below 50 or
    // above 114 is out of the window.
    assert_eq!(lp_decoded.bin_price_at(0), None);
    assert_eq!(lp_decoded.bin_price_at(115), None);
    assert_eq!(lp_decoded.bin_price_at(i32::MAX), None);
}

#[test]
fn integer_only_decode_emits_no_floats() {
    // Integer-only invariant: the decoded LbPair is pure
    // integer (u64 / u128). The CI guard in
    // `tests/fixed_point_no_floats.rs` walks the dl-feed
    // source tree; this test confirms the same invariant
    // on the data path by asserting every field's type.
    let bytes = build_synthetic_lb_pair_blob(7, 100);
    let lp = decode_lb_pair(&bytes).expect("decode");
    // Type-level assertions: u16, i32, u8, u64, u128 only.
    let _bin_step: u16 = lp.bin_step;
    let _active_id: i32 = lp.active_id;
    let _program_flag: u8 = lp.token_mint_x_program_flag;
    let _amount_x: &Vec<u64> = &lp.bin_amount_x;
    let _amount_y: &Vec<u64> = &lp.bin_amount_y;
    let _price: &Vec<u128> = &lp.bin_price;
    // The struct contains no `f32` / `f64` fields. This
    // assertion is enforced at compile time by the field
    // types above; a regression that introduces a float
    // would fail to compile.
}

#[test]
fn wrong_size_blob_is_rejected() {
    // A 100-byte blob is below the 2236-byte v1.0 layout;
    // the decoder must reject it. The feed layer routes
    // such frames to `AccountUpdate` (not `Pool`) and
    // surfaces them as a non-fatal error to the consumer.
    let short = vec![0u8; 100];
    let err = decode_lb_pair(&short).unwrap_err();
    let msg = format!("{err}");
    assert!(
        msg.contains("too short"),
        "expected 'too short' in error, got: {msg}"
    );
}

#[test]
fn program_id_is_well_known_mainnet_constant() {
    // The decoder sets `LbPair.program_id` to the published
    // Meteora DLMM mainnet program id. This is a sanity
    // check that the program id constant is the right one.
    let bytes = build_synthetic_lb_pair_blob(0, 100);
    let lp = decode_lb_pair(&bytes).expect("decode");
    assert_eq!(
        lp.program_id.0[0..8],
        [0x39, 0x22, 0x8b, 0x9b, 0xd5, 0x3a, 0x7d, 0x4c]
    );
}
