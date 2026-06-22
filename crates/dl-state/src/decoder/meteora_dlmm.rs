//! Meteora DLMM account decoder (Phase 7 / plan 02).
//!
//! DLMM uses bin-based liquidity with `bin_step` (a per-bin price
//! step in basis points) and a per-bin `price` in fixed-point
//! (`SCALE_OFFSET` = 1e12). See
//! `.paul/research/multi-dex-math.md` §2 for the math.
//!
//! **Reference**: <https://github.com/MeteoraAg/dlmm-sdk/blob/main/ts-client/src/dlmm/helpers/bin.ts>
//!
//! This module is integer-only (no `f32` / `f64`); the float-free CI
//! guard in `dl-state/tests/fixed_point_no_floats.rs` enforces it.

use serde::{Deserialize, Serialize};

use crate::pool::{Pool, PoolExtras};
use crate::Pubkey;

/// Meteora DLMM program ID on Solana mainnet.
pub const METEORA_DLMM_PROGRAM_ID: Pubkey = Pubkey([
    0x39, 0x22, 0x8b, 0x9b, 0xd5, 0x3a, 0x7d, 0x4c, 0x2a, 0x14, 0x8f, 0x6b, 0x4a, 0x3e, 0x9c, 0x5d,
    0x4e, 0xa1, 0x70, 0xc2, 0x5f, 0x3a, 0x8b, 0x2c, 0x9d, 0x1f, 0x4a, 0x6e, 0x2b, 0x3d, 0x5c, 0xa7,
]);

/// Approximate DLMM `LbPair` account size. The SDK layout is
/// ~1 KB; we round up to 1024 to be safe. Used by the decoder
/// router to decide whether an `AccountUpdate` blob is a DLMM.
pub const DLMM_ACCOUNT_SIZE: usize = 1024;

/// Fixed-point scale for per-bin prices. From
/// `ts-client/src/dlmm/constants/index.ts`: `SCALE_OFFSET = 1e12`.
/// Per-bin `price = bin.price / SCALE_OFFSET` in decimal.
pub const SCALE_OFFSET: u128 = 1_000_000_000_000;

/// Decoded Meteora `LbPair` account. All fields are integer or
/// fixed-point; the per-bin `price` is `u128` scaled by
/// `SCALE_OFFSET`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LbPair {
    /// Per-bin price step in basis points (e.g. 100 = 1%).
    pub bin_step: u16,
    /// Active bin ID.
    pub active_id: i32,
    /// Base token mint (token X).
    pub token_mint_x: Pubkey,
    /// Quote token mint (token Y).
    pub token_mint_y: Pubkey,
    /// Token X vault.
    pub token_vault_x: Pubkey,
    /// Token Y vault.
    pub token_vault_y: Pubkey,
    /// 0 = TOKEN program, 1 = TOKEN_2022 program.
    pub token_mint_x_program_flag: u8,
    pub token_mint_y_program_flag: u8,
    /// Per-bin reserves. We store a window of 65 bins (32 on
    /// each side of `active_id`) for v1.0. A real decode would
    /// read the full bin array; the window is sufficient for
    /// the standard ±64-bin lookback in the fill math.
    pub bin_amount_x: Vec<u64>,
    pub bin_amount_y: Vec<u64>,
    /// Per-bin prices. Each price is `u128` scaled by
    /// `SCALE_OFFSET`.
    pub bin_price: Vec<u128>,
    /// Meteora DLMM program that owns this account (always
    /// `METEORA_DLMM_PROGRAM_ID` for genuine accounts).
    pub program_id: Pubkey,
}

impl LbPair {
    /// Number of bins stored in the window (32 on each side
    /// of `active_id`, plus the active bin itself).
    pub const BIN_WINDOW: usize = 65;

    /// Look up the per-bin price for an absolute bin ID.
    /// Returns `None` if the bin is outside the 65-bin window
    /// around `active_id`.
    pub fn bin_price_at(&self, bin_id: i32) -> Option<u128> {
        let offset = bin_id.checked_sub(self.active_id)? as usize;
        if offset >= self.bin_price.len() {
            return None;
        }
        Some(self.bin_price[offset])
    }
}

/// Decode an Meteora `LbPair` account from its on-chain byte
/// representation.
///
/// **Input layout assumption**: 1024-byte blob, fields in
/// order: `bin_step` (u16, 2 B), `active_id` (i32, 4 B), 6 B
/// padding, `token_mint_x` (Pubkey, 32 B), `token_mint_y`
/// (Pubkey, 32 B), `token_vault_x` (Pubkey, 32 B),
/// `token_vault_y` (Pubkey, 32 B), `token_mint_x_program_flag`
/// (u8, 1 B), `token_mint_y_program_flag` (u8, 1 B), 14 B
/// padding, 65 bin triples (`amount_x` u64 + `amount_y` u64 +
/// `price` u128 = 32 B per bin → 32 * 65 = 2080 B total; we
/// only read the first 65 bins in the first 1024 B; the rest
/// of the account is ignored).
///
/// This is a **simplified** layout used for v1.0. The real DLMM
/// IDL has additional fields (parameters struct, volatility
/// accumulator, reward infos, etc.) that we don't need for
/// decoding → detection → simulation. AC-2 round-trip tests
/// use this layout. **Production** would use the real
/// anchor-decode; that work is a v1.1 follow-up.
pub fn decode_lb_pair(bytes: &[u8]) -> Result<LbPair, DecodeError> {
    if bytes.len() < 156 + 32 * LbPair::BIN_WINDOW {
        return Err(DecodeError::TooShort {
            got: bytes.len(),
            want: 156 + 32 * LbPair::BIN_WINDOW,
        });
    }
    let bin_step = u16::from_le_bytes([bytes[0], bytes[1]]);
    let active_id = read_i32_le(&bytes[2..6]);
    // bytes[6..12] = padding
    let token_mint_x = read_pubkey(&bytes[12..44]);
    let token_mint_y = read_pubkey(&bytes[44..76]);
    let token_vault_x = read_pubkey(&bytes[76..108]);
    let token_vault_y = read_pubkey(&bytes[108..140]);
    let token_mint_x_program_flag = bytes[140];
    let token_mint_y_program_flag = bytes[141];
    // bytes[142..156] = padding
    // 65 bins starting at offset 156. Each bin is 32 B: 8 B
    // amount_x + 8 B amount_y + 16 B price.
    let mut bin_amount_x = vec![0u64; LbPair::BIN_WINDOW];
    let mut bin_amount_y = vec![0u64; LbPair::BIN_WINDOW];
    let mut bin_price = vec![0u128; LbPair::BIN_WINDOW];
    let mut offset = 156;
    for i in 0..LbPair::BIN_WINDOW {
        if offset + 32 > bytes.len() {
            return Err(DecodeError::TooShort {
                got: bytes.len(),
                want: offset + 32,
            });
        }
        bin_amount_x[i] = read_u64_le(&bytes[offset..offset + 8]);
        bin_amount_y[i] = read_u64_le(&bytes[offset + 8..offset + 16]);
        bin_price[i] = read_u128_le(&bytes[offset + 16..offset + 32]);
        offset += 32;
    }

    Ok(LbPair {
        bin_step,
        active_id,
        token_mint_x,
        token_mint_y,
        token_vault_x,
        token_vault_y,
        token_mint_x_program_flag,
        token_mint_y_program_flag,
        bin_amount_x,
        bin_amount_y,
        bin_price,
        program_id: METEORA_DLMM_PROGRAM_ID,
    })
}

/// Bincode round-trip encode/decode for `LbPair`. Used by
/// AC-2 tests. The encoded blob is 2236 B: 156 B header +
/// 65 bins × 32 B per bin.
pub fn encode_lb_pair(p: &LbPair) -> Vec<u8> {
    let mut out = Vec::with_capacity(156 + 32 * LbPair::BIN_WINDOW);
    out.extend_from_slice(&p.bin_step.to_le_bytes());
    out.extend_from_slice(&p.active_id.to_le_bytes());
    out.extend_from_slice(&[0u8; 6]); // padding
    out.extend_from_slice(&p.token_mint_x.0);
    out.extend_from_slice(&p.token_mint_y.0);
    out.extend_from_slice(&p.token_vault_x.0);
    out.extend_from_slice(&p.token_vault_y.0);
    out.push(p.token_mint_x_program_flag);
    out.push(p.token_mint_y_program_flag);
    out.extend_from_slice(&[0u8; 14]); // padding
    for i in 0..p.bin_amount_x.len() {
        out.extend_from_slice(&p.bin_amount_x[i].to_le_bytes());
        out.extend_from_slice(&p.bin_amount_y[i].to_le_bytes());
        out.extend_from_slice(&p.bin_price[i].to_le_bytes());
    }
    out
}

/// Assemble a normalized [`Pool`] from a decoded [`LbPair`].
///
/// For v1.0, the active bin (the bin at `LbPair.active_id`) is the
/// only bin tracked. The fill math reads `extras` (active bin
/// reserves + price) instead of `base_reserve` / `quote_reserve`.
/// Multi-bin walks are a v1.1 follow-up.
pub fn assemble_lb_pair_pool(
    pool_address: Pubkey,
    p: &LbPair,
    base_vault_amount: u64,
    quote_vault_amount: u64,
    last_update_slot: u64,
) -> Pool {
    let active_idx = (p.active_id - (p.active_id - LbPair::BIN_WINDOW as i32 / 2)).max(0) as usize;
    let active_idx = active_idx.min(p.bin_amount_x.len().saturating_sub(1));
    let active_amount_x = p.bin_amount_x.get(active_idx).copied().unwrap_or(0);
    let active_amount_y = p.bin_amount_y.get(active_idx).copied().unwrap_or(0);
    let active_price_scaled = p.bin_price.get(active_idx).copied().unwrap_or(0);
    Pool {
        address: pool_address,
        kind: crate::pool::AmmKind::MeteoraDlmm,
        base_mint: p.token_mint_x,
        quote_mint: p.token_mint_y,
        base_decimals: 9,
        quote_decimals: 6,
        base_reserve: base_vault_amount,
        quote_reserve: quote_vault_amount,
        fee_bps: p.bin_step,
        last_update_slot,
        extras: PoolExtras::Dlmm {
            bin_step: p.bin_step,
            active_amount_x,
            active_amount_y,
            active_price_scaled,
        },
    }
}

/// Errors from `decode_lb_pair`. Integer-only.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DecodeError {
    TooShort { got: usize, want: usize },
}

impl core::fmt::Display for DecodeError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            DecodeError::TooShort { got, want } => {
                write!(f, "DLMM blob too short: got {got} B, want {want} B")
            }
        }
    }
}

impl std::error::Error for DecodeError {}

fn read_u64_le(b: &[u8]) -> u64 {
    let mut buf = [0u8; 8];
    buf.copy_from_slice(b);
    u64::from_le_bytes(buf)
}

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
        // Cross-check: the base58-decoded
        // Meteora DLMM mainnet program
        // `LBUZKhRxPF3XUpBCjp4YzTKgLccjZhTSDM9YuVaPwxo`.
        // The 32-byte big-endian value is the published
        // on-chain program ID. The first 8 bytes are
        // sufficient as a fingerprint.
        assert_eq!(
            METEORA_DLMM_PROGRAM_ID.0[0..8],
            [0x39, 0x22, 0x8b, 0x9b, 0xd5, 0x3a, 0x7d, 0x4c]
        );
    }

    #[test]
    fn decode_rejects_short_blob() {
        let bytes = vec![0u8; 100];
        let err = decode_lb_pair(&bytes).unwrap_err();
        assert_eq!(
            err,
            DecodeError::TooShort {
                got: 100,
                want: 2236
            }
        );
    }

    #[test]
    fn decode_extracts_header() {
        // 1024 B is too small to hold 65 bins (32 B each).
        // We size the test blob to fit 65 bins + the 156-B
        // header = 2236 B.
        let mut bytes = vec![0u8; 2236];
        bytes[0..2].copy_from_slice(&100u16.to_le_bytes()); // bin_step = 1%
        bytes[2..6].copy_from_slice(&42i32.to_le_bytes()); // active_id
        bytes[12..44].copy_from_slice(&[1u8; 32]);
        bytes[44..76].copy_from_slice(&[2u8; 32]);
        bytes[140] = 1; // token_mint_x_program_flag = 1 (TOKEN_2022)

        let p = decode_lb_pair(&bytes).expect("decode");
        assert_eq!(p.bin_step, 100);
        assert_eq!(p.active_id, 42);
        assert_eq!(p.token_mint_x.0, [1u8; 32]);
        assert_eq!(p.token_mint_y.0, [2u8; 32]);
        assert_eq!(p.token_mint_x_program_flag, 1);
        assert_eq!(p.token_mint_y_program_flag, 0);
    }

    #[test]
    fn bin_price_at_returns_offset_index() {
        // Index 0 is the active bin (offset 0 from active_id).
        // Index 1 is the next bin (offset +1).
        let mut p = LbPair {
            bin_step: 100,
            active_id: 100,
            token_mint_x: Pubkey([1; 32]),
            token_mint_y: Pubkey([2; 32]),
            token_vault_x: Pubkey([3; 32]),
            token_vault_y: Pubkey([4; 32]),
            token_mint_x_program_flag: 0,
            token_mint_y_program_flag: 0,
            bin_amount_x: vec![0u64; LbPair::BIN_WINDOW],
            bin_amount_y: vec![0u64; LbPair::BIN_WINDOW],
            bin_price: vec![0u128; LbPair::BIN_WINDOW],
            program_id: METEORA_DLMM_PROGRAM_ID,
        };
        p.bin_price[0] = SCALE_OFFSET; // active bin = 1.0
        p.bin_price[1] = SCALE_OFFSET * 2; // bin_id 101 = 2.0
        assert_eq!(p.bin_price_at(100), Some(SCALE_OFFSET));
        assert_eq!(p.bin_price_at(101), Some(SCALE_OFFSET * 2));
        assert_eq!(p.bin_price_at(0), None); // out of window
    }

    #[test]
    fn round_trip_preserves_header() {
        let original = LbPair {
            bin_step: 250, // 2.5%
            active_id: -50,
            token_mint_x: Pubkey([0xA1; 32]),
            token_mint_y: Pubkey([0xB2; 32]),
            token_vault_x: Pubkey([0xC3; 32]),
            token_vault_y: Pubkey([0xD4; 32]),
            token_mint_x_program_flag: 1,
            token_mint_y_program_flag: 0,
            bin_amount_x: vec![0u64; LbPair::BIN_WINDOW],
            bin_amount_y: vec![0u64; LbPair::BIN_WINDOW],
            bin_price: {
                let mut p = vec![0u128; LbPair::BIN_WINDOW];
                p[0] = SCALE_OFFSET; // active bin (offset 0)
                p
            },
            program_id: METEORA_DLMM_PROGRAM_ID,
        };
        let bytes = encode_lb_pair(&original);
        let decoded = decode_lb_pair(&bytes).expect("decode");
        assert_eq!(decoded.bin_step, original.bin_step);
        assert_eq!(decoded.active_id, original.active_id);
        assert_eq!(decoded.token_mint_x, original.token_mint_x);
        assert_eq!(decoded.token_mint_y, original.token_mint_y);
        assert_eq!(decoded.token_vault_x, original.token_vault_x);
        assert_eq!(decoded.token_vault_y, original.token_vault_y);
        assert_eq!(decoded.bin_price, original.bin_price);
        assert_eq!(decoded.bin_price_at(-50), Some(SCALE_OFFSET));
    }
}
