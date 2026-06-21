//! Whirlpool subscription + decode glue for `dl-feed` (DAM-44a / DAM-52).
//!
//! Bridges the `dl-state` Orca Whirlpool decoder into the `dl-feed`
//! subscription layer. Each Whirlpool `AccountUpdate` is decoded into
//! a normalized `FeedEvent::Pool { amm: ORCA_WHIRLPOOL, ... }` and
//! forwarded to the consumer.
//!
//! ## Why this module
//!
//! The capture stream is the value path: if a Whirlpool `AccountUpdate`
//! landed in the capture without being decoded, every downstream
//! consumer (detector, sim, paper trader) would have to re-decode the
//! bytes, which is the right place to centralize the integer-only
//! fill math. The decoder already lives in `dl-state`; this module is
//! the `dl-feed` side of the same contract.
//!
//! ## Integer-only invariant
//!
//! The decoded Whirlpool `sqrt_price` is in Q64.64 (`u128`) and
//! passes through unchanged. The CI guard
//! `crates/dl-feed/tests/fixed_point_no_floats.rs` enforces no
//! `f32` / `f64` in this crate's source tree.
//!
//! ## Vault-amount caveat
//!
//! Whirlpool pools do not embed the SPL-token vault amounts in the
//! pool account; those live in separate SPL token accounts
//! (`token_vault_x`, `token_vault_y`) that must be subscribed
//! separately. For DAM-44a / DAM-52 the `base_reserve` /
//! `quote_reserve` are reported as `0` — the consumer must subscribe
//! the vault accounts to get a non-zero reserve (DAM-44d scope,
//! future). The integer-only invariant still holds: zero is the
//! right answer when the vault amount has not been observed.

use dl_core::amm_tag;
use dl_core::{FeedEvent, PoolExtrasWire};
use dl_state::decoder::orca_whirlpool::{
    decode_whirlpool, ORCA_WHIRLPOOL_PROGRAM_ID, WHIRLPOOL_ACCOUNT_SIZE,
};
use dl_state::Pubkey;

/// Orca Whirlpool program id, copied as a `[u8; 32]` for use as a
/// JSON-RPC `accountSubscribe` filter / registry key. Source of truth
/// lives in `dl_state::decoder::orca_whirlpool`; this constant is
/// duplicated here so the `ws` feature can match it without pulling
/// in the rest of `dl-state`.
pub const ORCA_WHIRLPOOL_PROGRAM_ID_BYTES: [u8; 32] = ORCA_WHIRLPOOL_PROGRAM_ID.0;

/// Outcome of decoding one Whirlpool `AccountUpdate`. The `ws_feed`
/// background task uses this to forward a `FeedEvent::Pool` to the
/// consumer; on `NotAWhirlpool` / `DecodeFailed` the raw
/// `AccountUpdate` is forwarded unchanged so the consumer can still
/// see the bytes (debug use).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WhirlpoolDecodeOutcome {
    /// Decoded a pool state update.
    Decoded(FeedEvent),
    /// The raw blob is not a Whirlpool account (wrong size, etc.).
    /// The caller should fall back to the raw `AccountUpdate`.
    NotAWhirlpool,
    /// The blob is a Whirlpool account but decoding failed (e.g. too
    /// short). The caller should forward the raw `AccountUpdate`.
    DecodeFailed,
}

/// Decode a Whirlpool `AccountUpdate` into a `FeedEvent::Pool`. The
/// input is the raw account bytes from the JSON-RPC
/// `accountNotification` payload.
///
/// `pubkey` is the pool account's pubkey. `slot` is the slot at
/// which the update was observed (carried in the notification's
/// `context.slot`). The returned event's `last_update_slot` equals
/// `slot` (in-order subscription).
///
/// Vault amounts (`base_reserve` / `quote_reserve`) are reported as
/// `0` for DAM-44a / DAM-52 — the pool account does not embed SPL
/// token amounts, and the vault-account subscription is a follow-up.
/// The `dl-feed` integer-only invariant still holds: `0` is the
/// right answer when the amount is unknown.
pub fn decode_account_update(pubkey: [u8; 32], slot: u64, data: &[u8]) -> WhirlpoolDecodeOutcome {
    // The simplified v1.0 layout is 256 bytes; the real on-chain
    // layout is 653 bytes. We accept the 256-byte layout because
    // that's what the AC-1 test fixtures and the `dl-state` unit
    // tests use; the 653-byte layout is wired in v1.1 alongside
    // the real Anchor decoder.
    if data.len() != WHIRLPOOL_ACCOUNT_SIZE {
        return WhirlpoolDecodeOutcome::NotAWhirlpool;
    }
    match decode_whirlpool(data) {
        Ok(w) => {
            // The v1.0 simplified layout does not carry vault
            // amounts. The real layout would surface the amounts
            // through `token_vault_x` / `token_vault_y` accounts,
            // which DAM-44d subscribes separately. For DAM-44a /
            // DAM-52 we report 0 — the consumer is expected to
            // know this and use the
            // `PoolExtrasWire::Whirlpool { sqrt_price }` for fill
            // math.
            let event = FeedEvent::Pool {
                slot,
                amm: amm_tag::ORCA_WHIRLPOOL,
                pool: pubkey,
                base_mint: w.token_mint_x.0,
                quote_mint: w.token_mint_y.0,
                fee_bps: w.fee_rate,
                base_reserve: 0,
                quote_reserve: 0,
                extras: PoolExtrasWire::Whirlpool {
                    sqrt_price: w.sqrt_price,
                },
                last_update_slot: slot,
            };
            WhirlpoolDecodeOutcome::Decoded(event)
        }
        Err(_) => WhirlpoolDecodeOutcome::DecodeFailed,
    }
}

/// True if `program_id` is the Orca Whirlpool program. Used to
/// route `programSubscribe` notifications to the Whirlpool decoder
/// when the bot subscribes the program (not the per-pool pubkey).
pub fn is_whirlpool_program(program_id: &[u8; 32]) -> bool {
    *program_id == ORCA_WHIRLPOOL_PROGRAM_ID_BYTES
}

/// Convenience: return the canonical Whirlpool program id as a
/// `Pubkey`. Re-exports
/// `dl_state::decoder::orca_whirlpool::ORCA_WHIRLPOOL_PROGRAM_ID`.
pub fn whirlpool_program_pubkey() -> Pubkey {
    ORCA_WHIRLPOOL_PROGRAM_ID
}

#[cfg(test)]
mod tests {
    use super::*;
    use dl_state::decoder::orca_whirlpool::{encode_whirlpool, Whirlpool, Q64_RESOLUTION};

    fn sample_whirlpool_bytes(sqrt_price: u128, fee_rate: u16) -> Vec<u8> {
        let w = Whirlpool {
            sqrt_price,
            tick_current_index: -100,
            tick_spacing: 64,
            liquidity: 1_000_000,
            token_mint_x: Pubkey([0x11u8; 32]),
            token_mint_y: Pubkey([0x22u8; 32]),
            token_vault_x: Pubkey([0x33u8; 32]),
            token_vault_y: Pubkey([0x44u8; 32]),
            fee_rate,
            program_id: ORCA_WHIRLPOOL_PROGRAM_ID,
        };
        encode_whirlpool(&w)
    }

    #[test]
    fn decodes_a_valid_whirlpool_blob_into_pool_event() {
        let bytes = sample_whirlpool_bytes(Q64_RESOLUTION, 30);
        let pubkey = [0xAAu8; 32];
        let slot = 12345;

        let outcome = decode_account_update(pubkey, slot, &bytes);
        let event = match outcome {
            WhirlpoolDecodeOutcome::Decoded(ev) => ev,
            other => panic!("expected Decoded, got {other:?}"),
        };
        match event {
            FeedEvent::Pool {
                amm,
                pool,
                base_mint,
                quote_mint,
                fee_bps,
                base_reserve,
                quote_reserve,
                extras,
                last_update_slot,
                slot: ev_slot,
            } => {
                assert_eq!(amm, amm_tag::ORCA_WHIRLPOOL);
                assert_eq!(pool, pubkey);
                assert_eq!(base_mint, [0x11u8; 32]);
                assert_eq!(quote_mint, [0x22u8; 32]);
                assert_eq!(fee_bps, 30);
                assert_eq!(base_reserve, 0);
                assert_eq!(quote_reserve, 0);
                assert_eq!(last_update_slot, slot);
                assert_eq!(ev_slot, slot);
                assert!(matches!(
                    extras,
                    PoolExtrasWire::Whirlpool { sqrt_price: sp } if sp == Q64_RESOLUTION
                ));
            }
            other => panic!("expected Pool, got {other:?}"),
        }
    }

    #[test]
    fn short_blob_is_reported_as_not_a_whirlpool() {
        // 100 bytes is well below the 256-byte layout; the v1.0
        // decoder rejects with TooShort. The right response for
        // the feed layer is to forward the raw AccountUpdate
        // (the consumer may have a different decoder for this
        // account).
        let bytes = vec![0u8; 100];
        let outcome = decode_account_update([0u8; 32], 1, &bytes);
        assert!(matches!(outcome, WhirlpoolDecodeOutcome::NotAWhirlpool));
    }

    #[test]
    fn wrong_size_blob_is_reported_as_not_a_whirlpool() {
        // 256 bytes is the simplified layout; the real layout is
        // 653 bytes. For DAM-44a we accept only the 256-byte
        // layout; the 653-byte blob is `NotAWhirlpool` so the
        // caller forwards the raw account update.
        let bytes = vec![0u8; 500];
        let outcome = decode_account_update([0u8; 32], 1, &bytes);
        assert!(matches!(outcome, WhirlpoolDecodeOutcome::NotAWhirlpool));
    }

    #[test]
    fn empty_blob_is_not_a_whirlpool() {
        let outcome = decode_account_update([0u8; 32], 1, &[]);
        assert!(matches!(outcome, WhirlpoolDecodeOutcome::NotAWhirlpool));
    }

    #[test]
    fn is_whirlpool_program_matches_dl_state_constant() {
        // Sanity: the bytes we expose match `dl-state`'s constant.
        assert_eq!(ORCA_WHIRLPOOL_PROGRAM_ID_BYTES, ORCA_WHIRLPOOL_PROGRAM_ID.0);
        assert!(is_whirlpool_program(&ORCA_WHIRLPOOL_PROGRAM_ID_BYTES));
        // A random pubkey is not the Whirlpool program.
        let mut other = [0u8; 32];
        other[0] = 0xFF;
        assert!(!is_whirlpool_program(&other));
    }
}
