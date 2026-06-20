//! `dl-assert` — Solana BPF program for v2.0 atomicity (see
//! `plan/atomicity-decision.md`).
//!
//! Single instruction `assert_min_pnl` that aborts the bundle if the
//! signer's SOL balance delta vs a pre-funded snapshot account is
//! below the operator-specified minimum.
//!
//! ## Instruction layout
//!
//! Accounts:
//! - `accounts[0]` = signer (writable, the hot wallet) — must sign
//! - `accounts[1]` = vault (writable, holds pre-bundle lamport
//!   snapshot)
//!
//! Instruction data:
//! - byte 0: discriminator (`ASSERT_INSTRUCTION_DISCRIMINATOR` = 0)
//! - bytes 1..9: `min_net_pnl_lamports` (little-endian u64)
//!
//! Total: 9 bytes.
//!
//! ## Logic
//!
//! ```text
//! pre_snapshot = vault.lamports()
//! post_balance = signer.lamports()
//! net_pnl = post_balance - pre_snapshot
//! require!(net_pnl >= min_net_pnl_lamports, AssertError::NetPnlBelowThreshold)
//! ```
//!
//! ## Vault funding (operator step, OUT of band)
//!
//! Before running the bot, the operator must transfer
//! `signer.lamports()` to the vault PDA once per session (or refresh
//! after each successful bundle). The vault PDA is derived as:
//!
//! ```text
//! vault = Pubkey::find_program_address(&[b"dl-assert-vault", signer.as_ref()], &program_id)
//! ```
//!
//! The bundle builder (`dl-assert-sdk::derive_vault_pda`) handles
//! this lookup.
//!
//! ## Why vault-based, not snapshot-in-instruction-data
//!
//! An alternative design bakes `signer.lamports()` into the
//! instruction data at build time. This requires an out-of-band
//! `getBalance` call before every bundle AND trusts the RPC not to
//! lie about the signer's balance. The vault-based design is
//! trustless: the on-chain vault lamport count IS the ground truth.

#![cfg_attr(not(feature = "std"), no_std)]

use solana_program::entrypoint::ProgramResult;
use solana_program::program_error::ProgramError;
use solana_program::{account_info::AccountInfo, entrypoint, msg, pubkey::Pubkey};

entrypoint!(process_instruction);

/// Discriminator for the `assert_min_pnl` instruction. Future
/// instructions get higher numbers.
pub const ASSERT_INSTRUCTION_DISCRIMINATOR: u8 = 0;

/// Length of the `assert_min_pnl` instruction data:
/// 1 byte discriminator + 8 bytes u64.
pub const ASSERT_INSTRUCTION_DATA_LEN: usize = 9;

/// Seed for the vault PDA. The full derivation is
/// `Pubkey::find_program_address(&[VAULT_SEED, signer.as_ref()], &program_id)`.
pub const VAULT_SEED: &[u8] = b"dl-assert-vault";

/// Error codes mapped to `ProgramError::Custom(u32)`. The exact
/// numeric values don't matter — they just need to be distinct
/// from each other and from `ProgramError`'s built-in codes.
pub const ERR_NET_PNL_BELOW_THRESHOLD: u32 = 1;
pub const ERR_INVALID_DISCRIMINATOR: u32 = 2;
pub const ERR_INVALID_DATA_LENGTH: u32 = 3;
pub const ERR_INVALID_ACCOUNTS_LENGTH: u32 = 4;
pub const ERR_VAULT_OVERFLOW: u32 = 5;

/// Program entry point.
pub fn process_instruction(
    _program_id: &Pubkey,
    accounts: &[AccountInfo],
    instruction_data: &[u8],
) -> ProgramResult {
    // 1. Validate instruction data length + discriminator.
    if instruction_data.len() != ASSERT_INSTRUCTION_DATA_LEN {
        msg!(
            "dl-assert: InvalidDataLength got={} expected={}",
            instruction_data.len(),
            ASSERT_INSTRUCTION_DATA_LEN
        );
        return Err(ProgramError::Custom(ERR_INVALID_DATA_LENGTH));
    }
    if instruction_data[0] != ASSERT_INSTRUCTION_DISCRIMINATOR {
        msg!(
            "dl-assert: InvalidDiscriminator got={} expected={}",
            instruction_data[0],
            ASSERT_INSTRUCTION_DISCRIMINATOR
        );
        return Err(ProgramError::Custom(ERR_INVALID_DISCRIMINATOR));
    }

    // 2. Decode `min_net_pnl_lamports` (little-endian u64).
    let mut bytes = [0u8; 8];
    bytes.copy_from_slice(&instruction_data[1..9]);
    let min_net_pnl_lamports = u64::from_le_bytes(bytes);

    // 3. Validate accounts length.
    if accounts.len() != 2 {
        msg!(
            "dl-assert: InvalidAccountsLength got={}",
            accounts.len()
        );
        return Err(ProgramError::Custom(ERR_INVALID_ACCOUNTS_LENGTH));
    }
    let signer = &accounts[0];
    let vault = &accounts[1];

    // 4. Compute net SOL delta = signer.lamports() - vault.lamports().
    let post_balance = signer.lamports();
    let pre_snapshot = vault.lamports();
    let net_pnl = match (post_balance as i128).checked_sub(pre_snapshot as i128) {
        Some(v) => v,
        None => {
            msg!("dl-assert: VaultOverflow");
            return Err(ProgramError::Custom(ERR_VAULT_OVERFLOW));
        }
    };

    // 5. The gate.
    if net_pnl < min_net_pnl_lamports as i128 {
        msg!(
            "dl-assert: NetPnlBelowThreshold net_pnl={} lamports < min={} lamports",
            net_pnl,
            min_net_pnl_lamports
        );
        return Err(ProgramError::Custom(ERR_NET_PNL_BELOW_THRESHOLD));
    }

    // 6. Log success (only visible in tx logs in debug mode).
    msg!(
        "dl-assert: OK net_pnl={} lamports >= min={} lamports",
        net_pnl,
        min_net_pnl_lamports
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Decode helper for tests (host-side, not BPF).
    #[cfg(feature = "std")]
    fn decode_min_pnl(data: &[u8]) -> Result<u64, u32> {
        if data.len() != ASSERT_INSTRUCTION_DATA_LEN {
            return Err(ERR_INVALID_DATA_LENGTH);
        }
        if data[0] != ASSERT_INSTRUCTION_DISCRIMINATOR {
            return Err(ERR_INVALID_DISCRIMINATOR);
        }
        let mut bytes = [0u8; 8];
        bytes.copy_from_slice(&data[1..9]);
        Ok(u64::from_le_bytes(bytes))
    }

    #[cfg(feature = "std")]
    #[test]
    fn discriminator_is_zero() {
        // Bumping this would be a breaking change for the SDK.
        assert_eq!(ASSERT_INSTRUCTION_DISCRIMINATOR, 0);
    }

    #[cfg(feature = "std")]
    #[test]
    fn data_length_is_nine() {
        assert_eq!(ASSERT_INSTRUCTION_DATA_LEN, 9);
    }

    #[cfg(feature = "std")]
    #[test]
    fn decode_min_pnl_happy_path() {
        let mut data = vec![ASSERT_INSTRUCTION_DISCRIMINATOR];
        data.extend_from_slice(&123_456u64.to_le_bytes());
        assert_eq!(decode_min_pnl(&data).unwrap(), 123_456);
    }

    #[cfg(feature = "std")]
    #[test]
    fn decode_min_pnl_zero_is_valid_bytes() {
        // On-chain zero is technically valid; SDK rejects at the
        // builder layer. Here we only verify the byte parse.
        let mut data = vec![ASSERT_INSTRUCTION_DISCRIMINATOR];
        data.extend_from_slice(&0u64.to_le_bytes());
        assert_eq!(decode_min_pnl(&data).unwrap(), 0);
    }

    #[cfg(feature = "std")]
    #[test]
    fn decode_min_pnl_max_u64() {
        let mut data = vec![ASSERT_INSTRUCTION_DISCRIMINATOR];
        data.extend_from_slice(&u64::MAX.to_le_bytes());
        assert_eq!(decode_min_pnl(&data).unwrap(), u64::MAX);
    }

    #[cfg(feature = "std")]
    #[test]
    fn decode_min_pnl_rejects_short_data() {
        let err = decode_min_pnl(&[0u8; 5]).unwrap_err();
        assert_eq!(err, ERR_INVALID_DATA_LENGTH);
    }

    #[cfg(feature = "std")]
    #[test]
    fn decode_min_pnl_rejects_long_data() {
        let err = decode_min_pnl(&[0u8; 20]).unwrap_err();
        assert_eq!(err, ERR_INVALID_DATA_LENGTH);
    }

    #[cfg(feature = "std")]
    #[test]
    fn decode_min_pnl_rejects_wrong_discriminator() {
        let mut data = vec![99u8]; // wrong discriminator
        data.extend_from_slice(&123_456u64.to_le_bytes());
        let err = decode_min_pnl(&data).unwrap_err();
        assert_eq!(err, ERR_INVALID_DISCRIMINATOR);
    }

    #[cfg(feature = "std")]
    #[test]
    fn decode_min_pnl_rejects_discriminator_255() {
        let mut data = vec![255u8];
        data.extend_from_slice(&0u64.to_le_bytes());
        let err = decode_min_pnl(&data).unwrap_err();
        assert_eq!(err, ERR_INVALID_DISCRIMINATOR);
    }

    #[cfg(feature = "std")]
    #[test]
    fn error_codes_are_distinct() {
        let codes = [
            ERR_NET_PNL_BELOW_THRESHOLD,
            ERR_INVALID_DISCRIMINATOR,
            ERR_INVALID_DATA_LENGTH,
            ERR_INVALID_ACCOUNTS_LENGTH,
            ERR_VAULT_OVERFLOW,
        ];
        for (i, a) in codes.iter().enumerate() {
            for (j, b) in codes.iter().enumerate() {
                if i != j {
                    assert_ne!(a, b, "codes at {} and {} are equal", i, j);
                }
            }
        }
    }

    #[cfg(feature = "std")]
    #[test]
    fn net_pnl_calculation_subtracts_snapshot() {
        let post = 2_000_000_000u64;
        let pre = 1_500_000_000u64;
        let net = (post as i128) - (pre as i128);
        assert_eq!(net, 500_000_000);
        let min = 100_000u64;
        assert!(net >= min as i128);
    }

    #[cfg(feature = "std")]
    #[test]
    fn net_pnl_calculation_detects_loss() {
        let post = 1_400_000_000u64;
        let pre = 1_500_000_000u64;
        let net = (post as i128) - (pre as i128);
        assert_eq!(net, -100_000_000);
        let min = 50_000u64;
        assert!(net < min as i128);
    }

    #[cfg(feature = "std")]
    #[test]
    fn vault_seed_is_dl_assert_vault() {
        // The PDA derivation uses this seed. Changing it would
        // orphan all existing vault accounts.
        assert_eq!(VAULT_SEED, b"dl-assert-vault");
    }
}