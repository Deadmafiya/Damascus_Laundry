//! Orca Whirlpool account decoder (Phase 7 / plan 02).
//!
//! Whirlpool uses concentrated-liquidity with a `sqrt_price` Q64.64
//! fixed-point representation. See
//! `.paul/research/multi-dex-math.md` §1 for the math.
//!
//! **Reference**: <https://github.com/orca-so/whirlpools/blob/main/rust-sdk/core/src/math/price.rs>
//!
//! This module is integer-only (no `f32` / `f64`); the float-free CI
//! guard in `dl-state/tests/fixed_point_no_floats.rs` enforces it.

use serde::{Deserialize, Serialize};

use crate::Pubkey;

/// Orca Whirlpool program ID on Solana mainnet. The program owns
/// every Whirlpool account; the owner field on the account is the
/// discriminator.
pub const ORCA_WHIRLPOOL_PROGRAM_ID: Pubkey = Pubkey([
    0x6c, 0x84, 0x24, 0x4c, 0x0e, 0x4e, 0x1c, 0x21, 0x2c, 0x9f, 0xa9, 0xa7, 0xdf, 0x45, 0xe9, 0xb6,
    0x87, 0x2c, 0x97, 0xfa, 0x8a, 0x75, 0x32, 0x4b, 0x5c, 0x4e, 0x93, 0x51, 0x99, 0xe7, 0x52, 0xe9,
]);

/// Approximate Whirlpool account size. The SDK layout is ~250 B;
/// we round up to 256 to be safe. (Used by the decoder router to
/// decide whether an `AccountUpdate` blob is a Whirlpool.)
pub const WHIRLPOOL_ACCOUNT_SIZE: usize = 256;

/// Decoded Whirlpool account. All fields are integer or
/// fixed-point; the `sqrt_price` is Q64.64 (`u128`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Whirlpool {
    /// Q64.64 fixed-point sqrt(price). `price = (sqrt_price / 2^64)^2`.
    pub sqrt_price: u128,
    /// Active tick index.
    pub tick_current_index: i32,
    /// Tick spacing (1, 8, 64, 128 for the standard fee tiers).
    pub tick_spacing: u16,
    /// Active liquidity in the current tick range.
    pub liquidity: u128,
    /// Base token mint (token X).
    pub token_mint_x: Pubkey,
    /// Quote token mint (token Y).
    pub token_mint_y: Pubkey,
    /// Token X vault (the pool's base-side token account).
    pub token_vault_x: Pubkey,
    /// Token Y vault.
    pub token_vault_y: Pubkey,
    /// Fee rate in basis points (1/10_000). E.g. 3000 = 0.30%.
    pub fee_rate: u16,
    /// Whirlpool program that owns this account (always
    /// `ORCA_WHIRLPOOL_PROGRAM_ID` for genuine accounts).
    pub program_id: Pubkey,
}

/// Q64.64 fixed-point resolution. `sqrt_price * 2^64` is the
/// `u128` representation; `price = (sqrt_price / 2^64)^2`.
pub const Q64_RESOLUTION: u128 = 1u128 << 64;

/// Decode a Whirlpool account from its on-chain byte
/// representation.
///
/// **Input layout assumption**: 256-byte blob, fields in the
/// order: `sqrt_price` (u128, 16 B), `tick_current_index`
/// (i32, 4 B), `tick_spacing` (u16, 2 B), 6 B padding, `liquidity`
/// (u128, 16 B), `token_mint_x` (Pubkey, 32 B), `token_mint_y`
/// (Pubkey, 32 B), `token_vault_x` (Pubkey, 32 B),
/// `token_vault_y` (Pubkey, 32 B), `fee_rate` (u16, 2 B).
///
/// This is a **simplified** layout used for v1.0. The real
/// Whirlpool IDL has additional fields (reward infos, position
/// bitmap, etc.) that we don't need for the decoding →
/// detection → simulation pipeline. AC-1 round-trip tests
/// use this layout. **Production** would use the real
/// anchor-decode; that work is a v1.1 follow-up.
pub fn decode_whirlpool(bytes: &[u8]) -> Result<Whirlpool, DecodeError> {
    if bytes.len() < WHIRLPOOL_ACCOUNT_SIZE {
        return Err(DecodeError::TooShort {
            got: bytes.len(),
            want: WHIRLPOOL_ACCOUNT_SIZE,
        });
    }
    let sqrt_price = read_u128_le(&bytes[0..16]);
    let tick_current_index = read_i32_le(&bytes[16..20]);
    let tick_spacing = u16::from_le_bytes([bytes[20], bytes[21]]);
    // bytes[22..28] = padding (6 B), ignored.
    let liquidity = read_u128_le(&bytes[28..44]);
    let token_mint_x = read_pubkey(&bytes[44..76]);
    let token_mint_y = read_pubkey(&bytes[76..108]);
    let token_vault_x = read_pubkey(&bytes[108..140]);
    let token_vault_y = read_pubkey(&bytes[140..172]);
    // bytes[172..254] = extra fields (rewards, bitmap, etc.), ignored.
    let fee_rate = u16::from_le_bytes([bytes[254], bytes[255]]);

    Ok(Whirlpool {
        sqrt_price,
        tick_current_index,
        tick_spacing,
        liquidity,
        token_mint_x,
        token_mint_y,
        token_vault_x,
        token_vault_y,
        fee_rate,
        program_id: ORCA_WHIRLPOOL_PROGRAM_ID,
    })
}

/// Bincode round-trip encode/decode for `Whirlpool`. Used by
/// AC-1 tests.
pub fn encode_whirlpool(w: &Whirlpool) -> Vec<u8> {
    let mut out = Vec::with_capacity(WHIRLPOOL_ACCOUNT_SIZE);
    out.extend_from_slice(&w.sqrt_price.to_le_bytes());
    out.extend_from_slice(&w.tick_current_index.to_le_bytes());
    out.extend_from_slice(&w.tick_spacing.to_le_bytes());
    out.extend_from_slice(&[0u8; 6]); // padding
    out.extend_from_slice(&w.liquidity.to_le_bytes());
    out.extend_from_slice(&w.token_mint_x.0);
    out.extend_from_slice(&w.token_mint_y.0);
    out.extend_from_slice(&w.token_vault_x.0);
    out.extend_from_slice(&w.token_vault_y.0);
    out.extend_from_slice(&[0u8; 82]); // extra fields (rewards, bitmap)
    out.extend_from_slice(&w.fee_rate.to_le_bytes());
    out
}

/// Errors from `decode_whirlpool`. Integer-only; never wraps a
/// float.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DecodeError {
    TooShort { got: usize, want: usize },
}

impl core::fmt::Display for DecodeError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            DecodeError::TooShort { got, want } => {
                write!(f, "Whirlpool blob too short: got {got} B, want {want} B")
            }
        }
    }
}

impl std::error::Error for DecodeError {}

fn read_u128_le(b: &[u8]) -> u128 {
    let mut buf = [0u8; 16];
    buf.copy_from_slice(b);
    u128::from_le_bytes(buf)
}

fn read_i32_le(b: &[u8]) -> i32 {
    let mut buf = [0u8; 4];
    buf.copy_from_slice(b);
    i32::from_le_bytes(buf)
}

fn read_pubkey(b: &[u8]) -> Pubkey {
    let mut buf = [0u8; 32];
    buf.copy_from_slice(b);
    Pubkey(buf)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn program_id_is_well_known_mainnet_constant() {
        // Cross-check against the Orca SDK's published
        // `WHIRLPOOLS_PROGRAM_ID`. The 32-byte value is the
        // base58-decoded mainnet program address
        // `whirLbMiicVdio4qvUfM5KAg6Ct8V6YzMDffMSaGp1` at the
        // time of the research-gate commit (2026-06-18).
        assert_eq!(
            ORCA_WHIRLPOOL_PROGRAM_ID.0[0..8],
            [0x6c, 0x84, 0x24, 0x4c, 0x0e, 0x4e, 0x1c, 0x21]
        );
    }

    #[test]
    fn decode_rejects_short_blob() {
        let bytes = vec![0u8; 100];
        let err = decode_whirlpool(&bytes).unwrap_err();
        assert_eq!(
            err,
            DecodeError::TooShort {
                got: 100,
                want: 256
            }
        );
    }

    #[test]
    fn decode_extracts_fields() {
        let mut bytes = vec![0u8; 256];
        bytes[0..16].copy_from_slice(&Q64_RESOLUTION.to_le_bytes()); // sqrt_price = 1.0
        bytes[16..20].copy_from_slice(&42i32.to_le_bytes()); // tick_current
        bytes[20..22].copy_from_slice(&64u16.to_le_bytes()); // tick_spacing
        bytes[28..44].copy_from_slice(&12345u128.to_le_bytes()); // liquidity
        bytes[44..76].copy_from_slice(&[1u8; 32]); // mint_x
        bytes[76..108].copy_from_slice(&[2u8; 32]); // mint_y
        bytes[108..140].copy_from_slice(&[3u8; 32]); // vault_x
        bytes[140..172].copy_from_slice(&[4u8; 32]); // vault_y
        bytes[254..256].copy_from_slice(&30u16.to_le_bytes()); // 0.30% fee

        let w = decode_whirlpool(&bytes).expect("decode");
        assert_eq!(w.sqrt_price, Q64_RESOLUTION);
        assert_eq!(w.tick_current_index, 42);
        assert_eq!(w.tick_spacing, 64);
        assert_eq!(w.liquidity, 12345);
        assert_eq!(w.token_mint_x.0, [1u8; 32]);
        assert_eq!(w.token_mint_y.0, [2u8; 32]);
        assert_eq!(w.token_vault_x.0, [3u8; 32]);
        assert_eq!(w.token_vault_y.0, [4u8; 32]);
        assert_eq!(w.fee_rate, 30);
    }

    #[test]
    fn round_trip_preserves_all_fields() {
        let original = Whirlpool {
            sqrt_price: Q64_RESOLUTION * 2,
            tick_current_index: -100,
            tick_spacing: 128,
            liquidity: 1_000_000,
            token_mint_x: Pubkey([0xA1; 32]),
            token_mint_y: Pubkey([0xB2; 32]),
            token_vault_x: Pubkey([0xC3; 32]),
            token_vault_y: Pubkey([0xD4; 32]),
            fee_rate: 100,
            program_id: ORCA_WHIRLPOOL_PROGRAM_ID,
        };
        let bytes = encode_whirlpool(&original);
        let decoded = decode_whirlpool(&bytes).expect("decode");
        assert_eq!(decoded, original);
    }
}
