//! Meteora DLMM subscription + decode glue for `dl-feed` (DAM-89).
//!
//! Bridges the `dl-state` Meteora DLMM decoder into the `dl-feed`
//! subscription layer. Each `LbPair` `AccountUpdate` is decoded into
//! a normalized `FeedEvent::Pool { amm: METEORA_DLMM, ... }` and
//! forwarded to the consumer.
//!
//! ## Why this module
//!
//! The Whirlpool sibling module is the template: the capture stream
//! is the value path, and centralizing decode glue here means
//! downstream consumers (detector, sim, paper trader) never have to
//! reach for the raw `AccountUpdate` bytes.
//!
//! ## Integer-only invariant
//!
//! Per-bin `price` is `u128` scaled by Meteora's `SCALE_OFFSET`
//! (1e12). No `f32` / `f64` appears in this module; the
//! `dl-feed` no-floats CI guard enforces it.
//!
//! ## Bin-array caveat
//!
//! The full DLMM `LbPair` account carries a pointer to a separate
//! `BinArray` account that holds the per-bin reserves. For DAM-89
//! v1.0 we read the simplified 65-bin window from the
//! `LbPair` blob itself and ignore the bin-array pointer. A
//! follow-up will subscribe the `BinArray` accounts for full
//! per-bin liquidity.

use dl_core::feed::PoolExtrasWire;
use dl_core::AmmTag;
use dl_core::FeedEvent;
use dl_state::decoder::meteora_dlmm::{
    decode_lb_pair, LbPair, DLMM_ACCOUNT_SIZE, METEORA_DLMM_PROGRAM_ID,
};
use dl_state::Pubkey;

/// Meteora DLMM program id, copied as a `[u8; 32]`. Source of truth
/// lives in `dl_state::decoder::meteora_dlmm`.
pub const METEORA_DLMM_PROGRAM_ID_BYTES: [u8; 32] = METEORA_DLMM_PROGRAM_ID.0;

/// Outcome of decoding one DLMM `AccountUpdate`. Mirrors
/// [`crate::whirlpool::WhirlpoolDecodeOutcome`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DlmmDecodeOutcome {
    /// Decoded a pool state update.
    Decoded(FeedEvent),
    /// The raw blob is not an `LbPair` (wrong size). Caller
    /// falls back to the raw `AccountUpdate`.
    NotALbPair,
    /// The blob is an `LbPair` but decoding failed (e.g. too
    /// short). Caller forwards the raw `AccountUpdate`.
    DecodeFailed,
}

/// Decode a Meteora `LbPair` `AccountUpdate` into a
/// `FeedEvent::Pool`.
///
/// `pubkey` is the LbPair account's pubkey. `slot` is the slot at
/// which the update was observed. The returned event's
/// `last_update_slot` equals `slot` (in-order subscription).
///
/// Vault amounts (`base_reserve` / `quote_reserve`) are reported
/// as `0` for DAM-89 v1.0 — the per-bin reserves are still
/// available via `extras`. The integer-only invariant still
/// holds: zero is the right answer when the vault amount has not
/// been observed.
pub fn decode_account_update(pubkey: [u8; 32], slot: u64, data: &[u8]) -> DlmmDecodeOutcome {
    // The simplified v1.0 layout fits in 1024 B but our decoder
    // reads 65 bins * 32 B + 156 B header = 2236 B. The decoder
    // itself rejects anything shorter with `TooShort`; we treat
    // the rejection as `NotALbPair` so the WS feed forwards the
    // raw `AccountUpdate` (the consumer may know better).
    if data.len() < DLMM_ACCOUNT_SIZE {
        return DlmmDecodeOutcome::NotALbPair;
    }
    match decode_lb_pair(data) {
        Ok(lb) => {
            // Active bin index is the bin at `active_id`. Our
            // window has 65 bins, with the active bin centred
            // at index 32.
            let active_idx = (LbPair::BIN_WINDOW / 2) as i32;
            let active_offset = (active_idx).max(0) as usize;
            let active_offset = active_offset.min(lb.bin_amount_x.len().saturating_sub(1));
            let active_amount_x = lb.bin_amount_x.get(active_offset).copied().unwrap_or(0);
            let active_amount_y = lb.bin_amount_y.get(active_offset).copied().unwrap_or(0);
            let active_price_scaled = lb.bin_price.get(active_offset).copied().unwrap_or(0);

            let event = FeedEvent::Pool {
                slot,
                amm: AmmTag::METEORA_DLMM,
                pool: pubkey,
                base_mint: lb.token_mint_x.0,
                quote_mint: lb.token_mint_y.0,
                fee_bps: lb.bin_step,
                base_reserve: 0,
                quote_reserve: 0,
                extras: PoolExtrasWire::Dlmm {
                    bin_step: lb.bin_step,
                    active_amount_x,
                    active_amount_y,
                    active_price_scaled,
                },
                last_update_slot: slot,
            };
            DlmmDecodeOutcome::Decoded(event)
        }
        Err(_) => DlmmDecodeOutcome::DecodeFailed,
    }
}

/// True if `program_id` is the Meteora DLMM program. Used by
/// the WS feed to route `programSubscribe` notifications to this
/// decoder.
pub fn is_meteora_dlmm_program(program_id: &[u8; 32]) -> bool {
    *program_id == METEORA_DLMM_PROGRAM_ID_BYTES
}

/// Convenience: return the canonical Meteora DLMM program id
/// as a `Pubkey`. Re-exports
/// `dl_state::decoder::meteora_dlmm::METEORA_DLMM_PROGRAM_ID`.
pub fn meteora_dlmm_program_pubkey() -> Pubkey {
    METEORA_DLMM_PROGRAM_ID
}

#[cfg(test)]
mod tests {
    use super::*;
    use dl_state::decoder::meteora_dlmm::encode_lb_pair;

    fn sample_lb_pair_bytes() -> Vec<u8> {
        let mut bin_amount_x = vec![0u64; LbPair::BIN_WINDOW];
        let mut bin_amount_y = vec![0u64; LbPair::BIN_WINDOW];
        let mut bin_price = vec![0u128; LbPair::BIN_WINDOW];
        // Active bin = middle of the window. Give it non-zero
        // reserves / price so the test can verify the bit-exact
        // values flow through.
        let active = LbPair::BIN_WINDOW / 2;
        bin_amount_x[active] = 1_000_000_000;
        bin_amount_y[active] = 25_000_000_000;
        bin_price[active] = 150_000_000_000_u128; // 150 * SCALE_OFFSET
        let p = LbPair {
            bin_step: 100,
            active_id: 0,
            token_mint_x: Pubkey([0x11u8; 32]),
            token_mint_y: Pubkey([0x22u8; 32]),
            token_vault_x: Pubkey([0x33u8; 32]),
            token_vault_y: Pubkey([0x44u8; 32]),
            token_mint_x_program_flag: 0,
            token_mint_y_program_flag: 0,
            bin_amount_x,
            bin_amount_y,
            bin_price,
            program_id: METEORA_DLMM_PROGRAM_ID,
        };
        encode_lb_pair(&p)
    }

    #[test]
    fn decodes_a_valid_lb_pair_blob_into_pool_event() {
        let bytes = sample_lb_pair_bytes();
        let pubkey = [0xBBu8; 32];
        let slot = 999;

        let outcome = decode_account_update(pubkey, slot, &bytes);
        let event = match outcome {
            DlmmDecodeOutcome::Decoded(ev) => ev,
            other => panic!("expected Decoded, got {other:?}"),
        };
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
                assert_eq!(amm, AmmTag::METEORA_DLMM);
                assert_eq!(pool, pubkey);
                assert_eq!(base_mint, [0x11u8; 32]);
                assert_eq!(quote_mint, [0x22u8; 32]);
                assert_eq!(fee_bps, 100);
                assert_eq!(last_update_slot, slot);
                assert_eq!(ev_slot, slot);
                match extras {
                    PoolExtrasWire::Dlmm {
                        bin_step,
                        active_amount_x,
                        active_amount_y,
                        active_price_scaled,
                    } => {
                        assert_eq!(bin_step, 100);
                        assert_eq!(active_amount_x, 1_000_000_000);
                        assert_eq!(active_amount_y, 25_000_000_000);
                        assert_eq!(active_price_scaled, 150_000_000_000);
                    }
                    other => panic!("expected Dlmm extras, got {other:?}"),
                }
            }
            other => panic!("expected Pool, got {other:?}"),
        }
    }

    #[test]
    fn short_blob_is_reported_as_not_a_lb_pair() {
        // < DLMM_ACCOUNT_SIZE: the decoder would reject with
        // TooShort anyway; we treat it as NotALbPair so the
        // caller forwards the raw AccountUpdate.
        let bytes = vec![0u8; 100];
        let outcome = decode_account_update([0u8; 32], 1, &bytes);
        assert!(matches!(outcome, DlmmDecodeOutcome::NotALbPair));
    }

    #[test]
    fn empty_blob_is_not_a_lb_pair() {
        let outcome = decode_account_update([0u8; 32], 1, &[]);
        assert!(matches!(outcome, DlmmDecodeOutcome::NotALbPair));
    }

    #[test]
    fn is_meteora_dlmm_program_matches_dl_state_constant() {
        assert_eq!(METEORA_DLMM_PROGRAM_ID_BYTES, METEORA_DLMM_PROGRAM_ID.0);
        assert!(is_meteora_dlmm_program(&METEORA_DLMM_PROGRAM_ID_BYTES));
        let mut other = [0u8; 32];
        other[0] = 0xEE;
        assert!(!is_meteora_dlmm_program(&other));
    }
}
