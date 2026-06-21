//! DAM-44a / DAM-52 acceptance test: Whirlpool account-subscription
//! path in `dl-feed`.
//!
//! ## What this test proves
//!
//! 1. A scripted (mock) feed producing a Whirlpool `AccountUpdate`
//!    is decoded by `dl_feed::whirlpool::decode_account_update`
//!    into a `FeedEvent::Pool { amm: ORCA_WHIRLPOOL, ... }`.
//! 2. The decoded event flows through the `CapturingFeed` tee
//!    and lands in the capture stream.
//! 3. A round-trip read via `CapturedFeed` reproduces the decoded
//!    `Pool` event byte-for-byte.
//!
//! This is the acceptance path the issue calls out: a mock feed
//! in, a Whirlpool `AccountUpdate` in the middle, a
//! `FeedEvent::Pool` in the capture stream out.
//!
//! ## Why this is an integration test
//!
//! The unit tests in `dl_feed::whirlpool::tests` cover the decoder
//! in isolation. This test wires the decoder to the capture/tee
//! layer — the same wiring the live `ws_feed` background task
//! uses — and asserts the end-to-end shape of the capture
//! stream. If the capture format or the tee changes, this test
//! catches it.
//!
//! ## Integer-only invariant
//!
//! The decoded `sqrt_price` is `Q64_RESOLUTION` (`u128`); the
//! capture stream carries it as an integer. The CI guard
//! `tests/fixed_point_no_floats.rs` enforces no `f32` / `f64` in
//! `dl-feed`'s source tree.

use std::io::Cursor;

use dl_core::amm_tag;
use dl_core::ScriptedFeed;
use dl_core::{Feed, FeedEvent, PoolExtrasWire};
use dl_feed::capture::CapturedFeed;
use dl_feed::capturing::CapturingFeed;
use dl_feed::whirlpool::{
    decode_account_update, WhirlpoolDecodeOutcome, ORCA_WHIRLPOOL_PROGRAM_ID_BYTES,
};
use dl_state::decoder::orca_whirlpool::{encode_whirlpool, Whirlpool, Q64_RESOLUTION};
use dl_state::Pubkey;

/// Build a synthetic 256-byte Whirlpool blob. Same layout the
/// `dl-state` unit tests use (`encode_whirlpool`).
fn synthetic_whirlpool_bytes(sqrt_price: u128, fee_rate: u16) -> Vec<u8> {
    let w = Whirlpool {
        sqrt_price,
        tick_current_index: -42,
        tick_spacing: 64,
        liquidity: 1_234_567,
        token_mint_x: Pubkey([0x11u8; 32]),
        token_mint_y: Pubkey([0x22u8; 32]),
        token_vault_x: Pubkey([0x33u8; 32]),
        token_vault_y: Pubkey([0x44u8; 32]),
        fee_rate,
        program_id: Pubkey(ORCA_WHIRLPOOL_PROGRAM_ID_BYTES),
    };
    encode_whirlpool(&w)
}

#[test]
fn whirlpool_account_update_decodes_into_pool_event() {
    // 1. Build a synthetic Whirlpool blob.
    let sqrt_price = Q64_RESOLUTION * 2; // price = 4.0
    let bytes = synthetic_whirlpool_bytes(sqrt_price, 30);
    let pool_pubkey = [0xAAu8; 32];
    let slot = 100;

    // 2. Decode through the `dl-feed` glue.
    let outcome = decode_account_update(pool_pubkey, slot, &bytes);
    let event = match outcome {
        WhirlpoolDecodeOutcome::Decoded(ev) => ev,
        WhirlpoolDecodeOutcome::NotAWhirlpool => {
            panic!("expected Decoded, got NotAWhirlpool")
        }
        WhirlpoolDecodeOutcome::DecodeFailed => {
            panic!("expected Decoded, got DecodeFailed")
        }
    };

    // 3. The decoded event is a Pool with the right AMM tag,
    //    mint pair, fee, and Q64.64 sqrt_price.
    match event {
        FeedEvent::Pool {
            amm,
            pool,
            base_mint,
            quote_mint,
            fee_bps,
            extras,
            last_update_slot,
            slot: ev_slot,
            ..
        } => {
            assert_eq!(amm, amm_tag::ORCA_WHIRLPOOL);
            assert_eq!(pool, pool_pubkey);
            assert_eq!(base_mint, [0x11u8; 32]);
            assert_eq!(quote_mint, [0x22u8; 32]);
            assert_eq!(fee_bps, 30);
            assert_eq!(last_update_slot, slot);
            assert_eq!(ev_slot, slot);
            assert!(matches!(
                extras,
                PoolExtrasWire::Whirlpool { sqrt_price: sp } if sp == sqrt_price
            ));
        }
        other => panic!("expected FeedEvent::Pool, got {other:?}"),
    }
}

#[test]
fn whirlpool_pool_event_lands_in_capture_stream() {
    // 1. Build a synthetic Whirlpool `AccountUpdate` event.
    let sqrt_price = Q64_RESOLUTION;
    let bytes = synthetic_whirlpool_bytes(sqrt_price, 30);
    let pool_pubkey = [0xAAu8; 32];
    let slot = 100u64;

    let raw_update = FeedEvent::AccountUpdate {
        slot,
        pubkey: pool_pubkey,
        data: bytes,
    };

    // 2. Script a small feed: a slot, the raw account update, and
    //    a follow-up slot. The detector (or a real `ws_feed`
    //    background task) would decode the raw `AccountUpdate`
    //    and emit a `FeedEvent::Pool`; this test does that
    //    transformation explicitly so the capture stream we
    //    inspect is the consumer-shaped one.
    let decoded = match decode_account_update(
        pool_pubkey,
        slot,
        &match raw_update.clone() {
            FeedEvent::AccountUpdate { data, .. } => data,
            _ => unreachable!(),
        },
    ) {
        WhirlpoolDecodeOutcome::Decoded(ev) => ev,
        other => panic!("expected Decoded, got {other:?}"),
    };

    let script = vec![
        FeedEvent::Slot { slot },
        raw_update,
        decoded,
        FeedEvent::Slot { slot: slot + 1 },
    ];

    // 3. Pipe the script through a tee wrapper that mirrors every
    //    event into a capture sink.
    let feed = ScriptedFeed::new(script.clone());
    let mut tee = CapturingFeed::new(feed, Vec::new()).expect("tee init");

    let mut passed = Vec::new();
    while let Some(ev) = tee.next_event() {
        passed.push(ev);
    }
    assert_eq!(passed, script);
    assert_eq!(tee.frames_written(), 4);
    assert_eq!(tee.write_failures(), 0);

    // 4. Round-trip the capture bytes: a `CapturedFeed` reading
    //    the same sink must yield the same events in the same
    //    order, including the decoded `Pool` event.
    let (_inner, bytes) = tee.into_parts().expect("into_parts");
    let mut replay = CapturedFeed::open(Cursor::new(bytes)).expect("open");
    let mut replayed = Vec::new();
    while let Some(ev) = replay.next_event() {
        replayed.push(ev);
    }
    assert_eq!(replayed, script);

    // 5. The capture stream contains exactly one `Pool` event
    //    and it is the Whirlpool kind.
    let pool_events: Vec<_> = replayed
        .iter()
        .filter(|e| matches!(e, FeedEvent::Pool { .. }))
        .collect();
    assert_eq!(pool_events.len(), 1);
    let pool = pool_events[0];
    if let FeedEvent::Pool { amm, .. } = pool {
        assert_eq!(*amm, amm_tag::ORCA_WHIRLPOOL);
    } else {
        unreachable!()
    }
}

#[test]
fn non_whirlpool_account_update_passes_through_unchanged() {
    // 100 bytes is well below the 256-byte Whirlpool layout; the
    // decoder reports `NotAWhirlpool` and the caller is expected
    // to forward the raw `AccountUpdate` unchanged. This test
    // pins that contract.
    let bytes = vec![0u8; 100];
    let outcome = decode_account_update([0u8; 32], 1, &bytes);
    assert_eq!(outcome, WhirlpoolDecodeOutcome::NotAWhirlpool);
}
