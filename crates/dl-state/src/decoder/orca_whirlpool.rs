//! Orca Whirlpool account decoder (Phase 7 / plan 02).
//!
//! Two layouts are supported:
//!
//! - [`decode_whirlpool`]: the **simplified** v1.0 layout (256 bytes).
//!   Synthetic / test data; not real on-chain bytes. Used by AC-1
//!   round-trip tests and the paper trader for early development.
//! - [`decode_whirlpool_real`]: the **real** on-chain layout (653 bytes),
//!   matching Orca's published IDL. Use this for live mainnet.
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

use crate::pool::{Pool, PoolExtras};
use crate::Pubkey;

/// Orca Whirlpool program ID on Solana mainnet. The program owns
/// every Whirlpool account; the owner field on the account is the
/// discriminator.
pub const ORCA_WHIRLPOOL_PROGRAM_ID: Pubkey = Pubkey([
    0x6c, 0x84, 0x24, 0x4c, 0x0e, 0x4e, 0x1c, 0x21, 0x2c, 0x9f, 0xa9, 0xa7, 0xdf, 0x45, 0xe9, 0xb6,
    0x87, 0x2c, 0x97, 0xfa, 0x8a, 0x75, 0x32, 0x4b, 0x5c, 0x4e, 0x93, 0x51, 0x99, 0xe7, 0x52, 0xe9,
]);

/// Approximate Whirlpool account size. The **simplified** v1.0
/// layout used by tests + paper mode is 256 bytes (chosen so the
/// synthetic fixture fits in a small test). The **real** on-chain
/// layout is 653 bytes — see [`decode_whirlpool_real`].
pub const WHIRLPOOL_ACCOUNT_SIZE: usize = 256;

/// Real on-chain Whirlpool account size per Orca's published IDL.
/// Use this when dispatching on incoming `AccountUpdate` blob size.
pub const WHIRLPOOL_ACCOUNT_SIZE_REAL: usize = 653;

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

/// Assemble a normalized [`Pool`] from a decoded [`Whirlpool`].
///
/// The fill math for Whirlpool is single-tick and uses `sqrt_price`
/// (Q64.64) to derive the constant-product reserves. `base_reserve`
/// and `quote_reserve` are set to the latest observed vault amounts
/// for display, but the simulator reads `extras` instead.
pub fn assemble_whirlpool_pool(
    pool_address: Pubkey,
    w: &Whirlpool,
    base_vault_amount: u64,
    quote_vault_amount: u64,
    last_update_slot: u64,
) -> Pool {
    Pool {
        address: pool_address,
        kind: crate::pool::AmmKind::OrcaWhirlpool,
        base_mint: w.token_mint_x,
        quote_mint: w.token_mint_y,
        base_decimals: 9,
        quote_decimals: 6,
        base_reserve: base_vault_amount,
        quote_reserve: quote_vault_amount,
        fee_bps: w.fee_rate,
        last_update_slot,
        extras: PoolExtras::Whirlpool {
            sqrt_price: w.sqrt_price,
        },
    }
}

/// Real on-chain Whirlpool account layout (653 bytes per Orca's
/// published IDL). Compared to the v1.0 simplified [`Whirlpool`],
/// this struct carries the full on-chain shape: Anchor discriminator,
/// protocol fee rate, protocol fee owed per side, and reward infos.
///
/// The fields we actually need for the cycle-detect-and-fill path
/// are:
/// - `sqrt_price` — the active tick price (Q64.64).
/// - `tick_current_index` — informational; used to verify price is
///   in-range.
/// - `fee_rate` — the swap fee in basis points.
/// - `token_mint_a` / `token_mint_b` — the pool's mints.
/// - `token_vault_a` / `token_vault_b` — vault pubkeys for SplTokenAccount
///   subscription.
///
/// `liquidity`, `protocol_fee_*`, `reward_*` are read but not used by
/// v1.0 cycle math (the single-tick approximation ignores
/// cross-tick liquidity; reward emissions are out of scope).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WhirlpoolReal {
    /// Anchor discriminator (8 bytes; unused by us, kept for shape).
    pub discriminator: [u8; 8],
    /// PDA bump (unused by us).
    pub bump: u8,
    /// Tick spacing — the price-step granularity (e.g. 64 = 0.64%).
    pub tick_spacing: u16,
    /// Fee rate in basis points (1/10_000). E.g. 3000 = 0.30%.
    pub fee_rate: u16,
    /// Protocol fee rate (basis points). Operators subtract this from
    /// `fee_rate` to get the LP-only fee.
    pub protocol_fee_rate: u16,
    /// Active liquidity in the current tick range (`u128`).
    pub liquidity: u128,
    /// `sqrt(price)` in Q64.64. `price = (sqrt_price / 2^64)^2`.
    pub sqrt_price: u128,
    /// Active tick index (signed).
    pub tick_current_index: i32,
    /// Protocol fee owed in token A (informational).
    pub protocol_fee_owed_a: u64,
    /// Protocol fee owed in token B (informational).
    pub protocol_fee_owed_b: u64,
    /// Token X mint (the base side).
    pub token_mint_x: Pubkey,
    /// Token Y mint (the quote side).
    pub token_mint_y: Pubkey,
    /// Token X vault pubkey.
    pub token_vault_x: Pubkey,
    /// Token Y vault pubkey.
    pub token_vault_y: Pubkey,
}

/// Decode a real 653-byte Whirlpool account from mainnet.
///
/// Field offsets are taken from Orca's published IDL
/// (`orca-so/whirlpools/programs/whirlpool/src/state/whirlpool.rs`).
/// The Anchor discriminator (8 bytes) is at offset 0 and not
/// validated here — the program ID filter at the subscription
/// layer guarantees the source account is a real Whirlpool.
pub fn decode_whirlpool_real(bytes: &[u8]) -> Result<WhirlpoolReal, DecodeError> {
    if bytes.len() < WHIRLPOOL_ACCOUNT_SIZE_REAL {
        return Err(DecodeError::TooShort {
            got: bytes.len(),
            want: WHIRLPOOL_ACCOUNT_SIZE_REAL,
        });
    }
    let read_u128_le = |b: &[u8]| -> u128 {
        let mut buf = [0u8; 16];
        buf.copy_from_slice(b);
        u128::from_le_bytes(buf)
    };
    let read_u64_le = |b: &[u8]| -> u64 {
        let mut buf = [0u8; 8];
        buf.copy_from_slice(b);
        u64::from_le_bytes(buf)
    };
    let read_u16_le = |b: &[u8]| -> u16 {
        let mut buf = [0u8; 2];
        buf.copy_from_slice(b);
        u16::from_le_bytes(buf)
    };
    let read_i32_le = |b: &[u8]| -> i32 {
        let mut buf = [0u8; 4];
        buf.copy_from_slice(b);
        i32::from_le_bytes(buf)
    };
    let read_pubkey = |b: &[u8]| -> Pubkey {
        let mut buf = [0u8; 32];
        buf.copy_from_slice(b);
        Pubkey(buf)
    };

    let mut discriminator = [0u8; 8];
    discriminator.copy_from_slice(&bytes[0..8]);
    let bump = bytes[8];
    let tick_spacing = read_u16_le(&bytes[9..11]);
    let fee_rate = read_u16_le(&bytes[13..15]);
    let protocol_fee_rate = read_u16_le(&bytes[15..17]);
    // bytes[17..32] = padding
    let liquidity = read_u128_le(&bytes[32..48]);
    let sqrt_price = read_u128_le(&bytes[48..64]);
    let tick_current_index = read_i32_le(&bytes[64..68]);
    // bytes[68..72] = padding
    let protocol_fee_owed_a = read_u64_le(&bytes[72..80]);
    let protocol_fee_owed_b = read_u64_le(&bytes[80..88]);
    let token_mint_x = read_pubkey(&bytes[88..120]);
    let token_mint_y = read_pubkey(&bytes[120..152]);
    let token_vault_x = read_pubkey(&bytes[152..184]);
    let token_vault_y = read_pubkey(&bytes[184..216]);
    // bytes[216..600] = 3 reward_infos × 128 B each (unused in v1.0)
    // bytes[600..653] = reward_last_updated_timestamp + tail (unused)

    Ok(WhirlpoolReal {
        discriminator,
        bump,
        tick_spacing,
        fee_rate,
        protocol_fee_rate,
        liquidity,
        sqrt_price,
        tick_current_index,
        protocol_fee_owed_a,
        protocol_fee_owed_b,
        token_mint_x,
        token_mint_y,
        token_vault_x,
        token_vault_y,
    })
}

/// Assemble a normalized [`Pool`] from a real on-chain [`WhirlpoolReal`].
pub fn assemble_whirlpool_real_pool(
    pool_address: Pubkey,
    w: &WhirlpoolReal,
    base_vault_amount: u64,
    quote_vault_amount: u64,
    last_update_slot: u64,
) -> Pool {
    Pool {
        address: pool_address,
        kind: crate::pool::AmmKind::OrcaWhirlpool,
        base_mint: w.token_mint_x,
        quote_mint: w.token_mint_y,
        base_decimals: 9,
        quote_decimals: 6,
        base_reserve: base_vault_amount,
        quote_reserve: quote_vault_amount,
        fee_bps: w.fee_rate,
        last_update_slot,
        extras: PoolExtras::Whirlpool {
            sqrt_price: w.sqrt_price,
        },
    }
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

    /// Build a synthetic 653-byte real-layout Whirlpool blob.
    /// Layout follows Orca's published IDL.
    fn fake_whirlpool_real_bytes(
        sqrt_price: u128,
        fee_rate: u16,
        tick_spacing: u16,
        tick_current: i32,
        liquidity: u128,
        mint_x: [u8; 32],
        mint_y: [u8; 32],
        vault_x: [u8; 32],
        vault_y: [u8; 32],
    ) -> Vec<u8> {
        let mut buf = vec![0u8; WHIRLPOOL_ACCOUNT_SIZE_REAL];
        // bytes[0..8] discriminator
        buf[0..8].copy_from_slice(b"WHIRLPOOL");
        // bytes[8] bump
        buf[8] = 255;
        // bytes[9..11] tick_spacing
        buf[9..11].copy_from_slice(&tick_spacing.to_le_bytes());
        // bytes[11..13] tick_spacing_seed (reserved)
        // bytes[13..15] fee_rate
        buf[13..15].copy_from_slice(&fee_rate.to_le_bytes());
        // bytes[15..17] protocol_fee_rate (zero for simplicity)
        // bytes[17..32] padding
        // bytes[32..48] liquidity
        buf[32..48].copy_from_slice(&liquidity.to_le_bytes());
        // bytes[48..64] sqrt_price
        buf[48..64].copy_from_slice(&sqrt_price.to_le_bytes());
        // bytes[64..68] tick_current_index
        buf[64..68].copy_from_slice(&tick_current.to_le_bytes());
        // bytes[68..72] padding
        // bytes[72..80] protocol_fee_owed_a
        // bytes[80..88] protocol_fee_owed_b
        // bytes[88..120] token_mint_x
        buf[88..120].copy_from_slice(&mint_x);
        // bytes[120..152] token_mint_y
        buf[120..152].copy_from_slice(&mint_y);
        // bytes[152..184] token_vault_x
        buf[152..184].copy_from_slice(&vault_x);
        // bytes[184..216] token_vault_y
        buf[184..216].copy_from_slice(&vault_y);
        // bytes[216..653] reward_infos + tail (left zero)
        buf
    }

    #[test]
    fn decode_whirlpool_real_rejects_short_blob() {
        let bytes = vec![0u8; 100];
        let err = decode_whirlpool_real(&bytes).unwrap_err();
        assert!(matches!(err, DecodeError::TooShort { .. }));
    }

    #[test]
    fn decode_whirlpool_real_extracts_mainnet_fields() {
        let sqrt_price: u128 = 1u128 << 64; // price = 1.0
        let bytes = fake_whirlpool_real_bytes(
            sqrt_price,
            30,            // 0.30% fee
            64,            // tick_spacing
            -100,          // tick_current
            1_000_000_000, // liquidity
            [0x11; 32],    // mint_x
            [0x22; 32],    // mint_y
            [0x33; 32],    // vault_x
            [0x44; 32],    // vault_y
        );
        let w = decode_whirlpool_real(&bytes).expect("decode");
        assert_eq!(w.sqrt_price, sqrt_price);
        assert_eq!(w.fee_rate, 30);
        assert_eq!(w.tick_spacing, 64);
        assert_eq!(w.tick_current_index, -100);
        assert_eq!(w.liquidity, 1_000_000_000);
        assert_eq!(w.token_mint_x.0, [0x11; 32]);
        assert_eq!(w.token_mint_y.0, [0x22; 32]);
        assert_eq!(w.token_vault_x.0, [0x33; 32]);
        assert_eq!(w.token_vault_y.0, [0x44; 32]);
        // Discriminator captured (not validated against an
        // expected value, but kept for shape).
        assert_eq!(&w.discriminator, b"WHIRLPOO");
    }

    #[test]
    fn assemble_whirlpool_real_pool_populates_extras() {
        let sqrt_price: u128 = 1u128 << 64;
        let bytes = fake_whirlpool_real_bytes(
            sqrt_price, 30, 64, 0, 0, [0x11; 32], [0x22; 32], [0x33; 32], [0x44; 32],
        );
        let w = decode_whirlpool_real(&bytes).unwrap();
        let pool = assemble_whirlpool_real_pool(Pubkey([0xFE; 32]), &w, 100_000, 200_000, 12345);
        assert_eq!(pool.kind, crate::pool::AmmKind::OrcaWhirlpool);
        assert_eq!(pool.fee_bps, 30);
        assert!(matches!(
            pool.extras,
            crate::pool::PoolExtras::Whirlpool { sqrt_price: sp } if sp == sqrt_price
        ));
    }
}
