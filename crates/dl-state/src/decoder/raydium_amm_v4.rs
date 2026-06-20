//! Raydium AMM v4 layout decoder.
//!
//! Translates raw on-chain account bytes into the normalized types used
//! by the rest of the engine. Two related but distinct inputs:
//!
//! 1. **`AmmInfo`** — a 752-byte blob owned by the Raydium AMM v4
//!    program. Contains mints, vault addresses, decimals, fee fraction.
//!    *Does not* contain reserves.
//! 2. **SPL token account** — a 165-byte account owned by the SPL Token
//!    program. Contains the on-hand token balance of a vault. This is
//!    where the reserves actually live.
//!
//! See `crates/dl-state/docs/RESEARCH.md` for the byte offsets and
//! upstream source. If the upstream struct changes, that file must be
//! updated before the decoder is.

use crate::error::DecodeError;
use crate::pool::{AmmKind, Pool, Pubkey};

/// Size of a serialized `AmmInfo` (per `#[repr(C, packed)]` layout,
/// `#[derive(Pod)]` on Raydium's struct — see `docs/raydium_state.rs`).
pub const AMM_INFO_SIZE: usize = 752;

/// Raydium AMM v4 program ID, mainnet-beta. Confirmed by
/// `getAccountInfo → executable: true` on 2026-06-17.
pub const RAYDIUM_AMM_V4_PROGRAM_ID: Pubkey = Pubkey([
    0x67, 0x5b, 0x96, 0x4d, 0xec, 0x0c, 0x1f, 0x45, 0x66, 0x2b, 0x65, 0x86, 0x3c, 0x24, 0x53, 0x1d,
    0x2a, 0xb0, 0x66, 0x4c, 0xf6, 0x8a, 0xc4, 0x5b, 0x39, 0x3c, 0x40, 0x0e, 0x1f, 0xa1, 0x3e, 0xa1,
]);

/// Parsed subset of `AmmInfo`. Only the fields the engine actually reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AmmInfo {
    /// `status` field at offset 0. Must be in 1..=7 (Initialized..WaitingTrade).
    pub status: u64,
    /// `coin_decimals` at offset 32.
    pub base_decimals: u8,
    /// `pc_decimals` at offset 40.
    pub quote_decimals: u8,
    /// `fees.trade_fee_numerator` at offset 144.
    pub trade_fee_numerator: u64,
    /// `fees.trade_fee_denominator` at offset 152.
    pub trade_fee_denominator: u64,
    /// `coin_vault` Pubkey at offset 336.
    pub base_vault: Pubkey,
    /// `pc_vault` Pubkey at offset 368.
    pub quote_vault: Pubkey,
    /// `coin_vault_mint` Pubkey at offset 400.
    pub base_mint: Pubkey,
    /// `pc_vault_mint` Pubkey at offset 432.
    pub quote_mint: Pubkey,
}

impl AmmInfo {
    /// Trade fee in basis points (1/10000). Returns `None` if the
    /// denominator is zero (a malformed pool).
    pub fn fee_bps(&self) -> Option<u16> {
        if self.trade_fee_denominator == 0 {
            return None;
        }
        // bps = num * 10000 / denom
        let bps = (self.trade_fee_numerator as u128)
            .checked_mul(10_000)?
            .checked_div(self.trade_fee_denominator as u128)?;
        // Saturate at u16::MAX — anything beyond is a 6.5%+ fee, weird.
        Some(bps.min(u16::MAX as u128) as u16)
    }
}

/// Parse a 752-byte `AmmInfo` blob.
pub fn decode_amm_info(bytes: &[u8]) -> Result<AmmInfo, DecodeError> {
    if bytes.len() < AMM_INFO_SIZE {
        return Err(DecodeError::TooShort {
            need: AMM_INFO_SIZE,
            got: bytes.len(),
        });
    }
    // Little-endian reads. We use `try_into` slices so this is safe even
    // for misaligned reads on architectures with alignment requirements.
    let read_u64 = |off: usize| -> u64 {
        let s: [u8; 8] = bytes[off..off + 8].try_into().expect("checked len");
        u64::from_le_bytes(s)
    };
    let read_pubkey = |off: usize| -> Pubkey {
        let mut out = [0u8; 32];
        out.copy_from_slice(&bytes[off..off + 32]);
        Pubkey(out)
    };

    let status = read_u64(0);
    if status == 0 {
        return Err(DecodeError::BadDiscriminator {
            expected: vec![1, 0, 0, 0, 0, 0, 0, 0],
            got: bytes[0..8].to_vec(),
        });
    }
    if !(1..=7).contains(&status) {
        return Err(DecodeError::BadDiscriminator {
            expected: vec![1, 0, 0, 0, 0, 0, 0, 0],
            got: bytes[0..8].to_vec(),
        });
    }

    Ok(AmmInfo {
        status,
        base_decimals: read_u64(32) as u8,
        quote_decimals: read_u64(40) as u8,
        trade_fee_numerator: read_u64(144),
        trade_fee_denominator: read_u64(152),
        base_vault: read_pubkey(336),
        quote_vault: read_pubkey(368),
        base_mint: read_pubkey(400),
        quote_mint: read_pubkey(432),
    })
}

/// Minimal subset of an SPL token account — only the fields we read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SplTokenAccount {
    /// `mint` at offset 0.
    pub mint: Pubkey,
    /// `amount` at offset 64. The on-hand balance, in the mint's base units.
    pub amount: u64,
}

/// Size of a serialized SPL token account (Token, not Token-2022).
/// Token-2022 accounts are at least 165 bytes; many pools (notably
/// Orca Whirlpool and Meteora DLMM) use Token-2022 vault accounts
/// which are larger (typically 234 B for non-frozen, more for accounts
/// with extensions). Use [`decode_spl_token_account`] on both — the
/// first 165 bytes are layout-compatible.
pub const SPL_TOKEN_ACCOUNT_SIZE: usize = 165;

/// Parse a 165-byte (or larger) SPL token / Token-2022 account.
///
/// Reads `mint` (offset 0) and `amount` (offset 64). Both fields are
/// within the first 72 bytes, so this decoder accepts:
/// - 165-byte SPL Token accounts (standard).
/// - 234-byte Token-2022 accounts (extensions appended after offset 165).
/// - Any larger size that has the mint + amount layout at offsets
///   0 and 64.
///
/// The size check is `>= SPL_TOKEN_ACCOUNT_SIZE` rather than `==`
/// because Token-2022 vaults are larger.
pub fn decode_spl_token_account(bytes: &[u8]) -> Result<SplTokenAccount, DecodeError> {
    if bytes.len() < SPL_TOKEN_ACCOUNT_SIZE {
        return Err(DecodeError::TooShort {
            need: SPL_TOKEN_ACCOUNT_SIZE,
            got: bytes.len(),
        });
    }
    let mut mint = [0u8; 32];
    mint.copy_from_slice(&bytes[0..32]);
    let amount: [u8; 8] = bytes[64..72].try_into().expect("checked len");
    Ok(SplTokenAccount {
        mint: Pubkey(mint),
        amount: u64::from_le_bytes(amount),
    })
}

/// Assemble a complete `Pool` from a parsed `AmmInfo` and the two vault
/// token accounts. Performs the cross-checks a healthy pool satisfies:
///
/// 1. Each vault's `mint` matches the corresponding AmmInfo `*_vault_mint`.
/// 2. The resulting `fee_bps` is sane (the call to `fee_bps` succeeds).
/// 3. The status is `Initialized..WaitingTrade` (already enforced in
///    `decode_amm_info`).
pub fn assemble_pool(
    pool_address: Pubkey,
    amm: &AmmInfo,
    coin_vault: &SplTokenAccount,
    pc_vault: &SplTokenAccount,
    last_update_slot: u64,
) -> Result<Pool, DecodeError> {
    if coin_vault.mint != amm.base_mint {
        return Err(DecodeError::BadDiscriminator {
            expected: amm.base_mint.0.to_vec(),
            got: coin_vault.mint.0.to_vec(),
        });
    }
    if pc_vault.mint != amm.quote_mint {
        return Err(DecodeError::BadDiscriminator {
            expected: amm.quote_mint.0.to_vec(),
            got: pc_vault.mint.0.to_vec(),
        });
    }
    let fee_bps = amm.fee_bps().ok_or(DecodeError::BadDiscriminator {
        expected: b"non-zero fee denominator".to_vec(),
        got: amm.trade_fee_denominator.to_le_bytes().to_vec(),
    })?;
    Ok(Pool {
        address: pool_address,
        kind: AmmKind::RaydiumAmmV4,
        base_mint: amm.base_mint,
        quote_mint: amm.quote_mint,
        base_decimals: amm.base_decimals,
        quote_decimals: amm.quote_decimals,
        base_reserve: coin_vault.amount,
        quote_reserve: pc_vault.amount,
        fee_bps,
        last_update_slot,
        extras: crate::pool::PoolExtras::Raydium,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a fake but well-formed 752-byte AmmInfo blob for tests.
    fn fake_amm_info_bytes() -> Vec<u8> {
        let mut buf = vec![0u8; AMM_INFO_SIZE];
        // status = 1 (Initialized) at offset 0
        buf[0..8].copy_from_slice(&1u64.to_le_bytes());
        // coin_decimals = 9 (SOL) at offset 32
        buf[32..40].copy_from_slice(&9u64.to_le_bytes());
        // pc_decimals = 6 (USDC) at offset 40
        buf[40..48].copy_from_slice(&6u64.to_le_bytes());
        // trade_fee_numerator = 25 at offset 144
        buf[144..152].copy_from_slice(&25u64.to_le_bytes());
        // trade_fee_denominator = 10000 at offset 152
        buf[152..160].copy_from_slice(&10_000u64.to_le_bytes());
        // base_vault = [3;32] at offset 336
        buf[336..368].fill(3);
        // quote_vault = [4;32] at offset 368
        buf[368..400].fill(4);
        // base_mint = [1;32] at offset 400
        buf[400..432].fill(1);
        // quote_mint = [2;32] at offset 432
        buf[432..464].fill(2);
        buf
    }

    fn fake_token_account(mint_byte: u8, amount: u64) -> Vec<u8> {
        let mut buf = vec![0u8; SPL_TOKEN_ACCOUNT_SIZE];
        buf[0..32].fill(mint_byte);
        buf[64..72].copy_from_slice(&amount.to_le_bytes());
        buf
    }

    #[test]
    fn decode_amm_info_happy_path() {
        let bytes = fake_amm_info_bytes();
        let info = decode_amm_info(&bytes).unwrap();
        assert_eq!(info.status, 1);
        assert_eq!(info.base_decimals, 9);
        assert_eq!(info.quote_decimals, 6);
        assert_eq!(info.fee_bps(), Some(25)); // 25/10000 = 25 bps
        assert_eq!(info.base_mint.0, [1u8; 32]);
        assert_eq!(info.quote_mint.0, [2u8; 32]);
    }

    #[test]
    fn decode_amm_info_too_short() {
        let bytes = vec![0u8; 100];
        match decode_amm_info(&bytes) {
            Err(DecodeError::TooShort {
                need: 752,
                got: 100,
            }) => (),
            other => panic!("expected TooShort, got {:?}", other),
        }
    }

    #[test]
    fn decode_amm_info_uninitialized_rejected() {
        let mut bytes = fake_amm_info_bytes();
        bytes[0..8].copy_from_slice(&0u64.to_le_bytes());
        assert!(matches!(
            decode_amm_info(&bytes),
            Err(DecodeError::BadDiscriminator { .. })
        ));
    }

    #[test]
    fn decode_amm_info_garbage_status_rejected() {
        let mut bytes = fake_amm_info_bytes();
        bytes[0..8].copy_from_slice(&42u64.to_le_bytes());
        assert!(matches!(
            decode_amm_info(&bytes),
            Err(DecodeError::BadDiscriminator { .. })
        ));
    }

    #[test]
    fn decode_spl_token_account_happy_path() {
        let bytes = fake_token_account(7, 123_456_789);
        let acc = decode_spl_token_account(&bytes).unwrap();
        assert_eq!(acc.mint.0, [7u8; 32]);
        assert_eq!(acc.amount, 123_456_789);
    }

    #[test]
    fn decode_spl_token_account_too_short() {
        let bytes = vec![0u8; 50];
        assert!(matches!(
            decode_spl_token_account(&bytes),
            Err(DecodeError::TooShort { .. })
        ));
    }

    /// Token-2022 vault accounts are typically 234 bytes (or larger
    /// with extensions). The decoder reads only the first 165 bytes
    /// (mint + amount fields are within the first 72 B), so the
    /// larger sizes must round-trip identically to the standard 165 B.
    #[test]
    fn decode_token_2022_account_at_234_bytes() {
        let mut bytes = vec![0u8; 234];
        bytes[0..32].fill(0xAB); // mint
        bytes[64..72].copy_from_slice(&987_654_321u64.to_le_bytes());
        // TLV extension area (offsets 165..234) is ignored.
        let acc = decode_spl_token_account(&bytes).unwrap();
        assert_eq!(acc.mint.0, [0xAB; 32]);
        assert_eq!(acc.amount, 987_654_321);
    }

    #[test]
    fn decode_token_2022_account_at_exactly_165_bytes() {
        // 165 B is the boundary between SPL Token (standard) and
        // Token-2022 (with extensions). Both must decode identically.
        let bytes = fake_token_account(0xCD, 42_000);
        let acc = decode_spl_token_account(&bytes).unwrap();
        assert_eq!(acc.mint.0, [0xCD; 32]);
        assert_eq!(acc.amount, 42_000);
    }

    #[test]
    fn assemble_pool_happy_path() {
        let amm_bytes = fake_amm_info_bytes();
        let amm = decode_amm_info(&amm_bytes).unwrap();
        let coin_vault = decode_spl_token_account(&fake_token_account(1, 1_000_000_000)).unwrap();
        let pc_vault = decode_spl_token_account(&fake_token_account(2, 250_000_000)).unwrap();
        let pool = assemble_pool(Pubkey([9u8; 32]), &amm, &coin_vault, &pc_vault, 12345).unwrap();
        assert_eq!(pool.address.0, [9u8; 32]);
        assert_eq!(pool.base_reserve, 1_000_000_000);
        assert_eq!(pool.quote_reserve, 250_000_000);
        assert_eq!(pool.fee_bps, 25);
        assert_eq!(pool.last_update_slot, 12345);
    }

    #[test]
    fn assemble_pool_rejects_mint_mismatch() {
        let amm_bytes = fake_amm_info_bytes();
        let amm = decode_amm_info(&amm_bytes).unwrap();
        // base_vault claims mint=[9;32] but AmmInfo says [1;32]
        let coin_vault = decode_spl_token_account(&fake_token_account(9, 1_000)).unwrap();
        let pc_vault = decode_spl_token_account(&fake_token_account(2, 1_000)).unwrap();
        assert!(matches!(
            assemble_pool(Pubkey([0u8; 32]), &amm, &coin_vault, &pc_vault, 1),
            Err(DecodeError::BadDiscriminator { .. })
        ));
    }

    #[test]
    fn fee_bps_zero_denominator_returns_none() {
        let mut bytes = fake_amm_info_bytes();
        bytes[152..160].copy_from_slice(&0u64.to_le_bytes());
        let amm = decode_amm_info(&bytes).unwrap();
        assert_eq!(amm.fee_bps(), None);
    }
}
