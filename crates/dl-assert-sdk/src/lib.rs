//! `dl-assert-sdk` — Rust helpers for the dl-assert Solana
//! on-chain program (see `plan/atomicity-decision.md`).
//!
//! The dl-assert program is a tiny BPF program with one
//! instruction: `assert_min_net_pnl`. It takes:
//!
//!   accounts[0] = signer (writable, the hot wallet)
//!   accounts[1] = vault (writable, holds pre-bundle lamports snapshot)
//!
//!   instruction_data = `min_net_pnl_lamports` as little-endian u64
//!
//! On execution, the program requires
//! `signer.lamports() - vault.lamports() >= min_net_pnl_lamports`,
//! otherwise the tx errors and the bundle reverts.
//!
//! This crate provides:
//!
//! - `ASSERT_INSTRUCTION_DISCRIMINATOR` — the first byte of
//!   instruction data (the instruction tag). Currently `0` for
//!   `assert_min_net_pnl`. Future instructions get higher numbers.
//! - `AssertInstructionData` — typed wrapper for the instruction
//!   payload.
//! - `build_assert_instruction_data(min_net_pnl_lamports)` —
//!   serialize the instruction data.
//! - `assert_min_net_pnl_threshold_reasonable(...)` — sanity
//!   check the threshold (must be positive, must not exceed a
//!   sane ceiling).
//!
//! The actual BPF program source lives in
//! `crates/dl-assert-program/` (built via `cargo build-bpf` and
//! deployed with `solana program deploy`). It is **not** built by
//! the workspace's default `cargo build` because it targets
//! `bpfel-unknown-unknown` rather than `x86_64-unknown-linux-gnu`.
//!
//! ## Deployment (operator steps, also in docs/live-runbook.md)
//!
//! ```bash
//! # 1. Build the BPF program. Requires the Solana SDK.
//! cd crates/dl-assert-program
//! cargo build-sbf --release
//!
//! # 2. Deploy to devnet first.
//! solana program deploy target/deploy/dl_assert_program.so \
//!     --url devnet --keypair ~/.config/solana/id.json
//!
//! # 3. Record the deployed program ID in dl-app config.
//! # 4. Repeat for mainnet-beta when ready.
//! ```

use solana_sdk::instruction::{AccountMeta, Instruction};
use solana_sdk::pubkey::Pubkey;
use thiserror::Error;

/// Discriminator for the `assert_min_net_pnl` instruction.
pub const ASSERT_INSTRUCTION_DISCRIMINATOR: u8 = 0;

/// Errors for the dl-assert SDK helpers.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum AssertSdkError {
    #[error("min_net_pnl_lamports must be > 0 (got {0})")]
    ThresholdNotPositive(u64),
    #[error("min_net_pnl_lamports exceeds sane ceiling of 100 SOL; got {got} lamports")]
    ThresholdExceedsCeiling { got: u64 },
}

/// Sanity-check a `min_net_pnl_lamports` value. Returns `Ok(())` if
/// the threshold is in a plausible range.
///
/// Rules:
/// - Must be > 0 (a 0 threshold means "always pass" — operator
///   mistake, refuse).
/// - Must be ≤ 100 SOL (10^11 lamports). Anything larger is almost
///   certainly a unit confusion (e.g. SOL-vs-lamports).
pub fn assert_min_net_pnl_threshold_reasonable(min_net_pnl_lamports: u64) -> Result<(), AssertSdkError> {
    const MAX_LAMPORTS: u64 = 100 * 1_000_000_000; // 100 SOL
    if min_net_pnl_lamports == 0 {
        return Err(AssertSdkError::ThresholdNotPositive(0));
    }
    if min_net_pnl_lamports > MAX_LAMPORTS {
        return Err(AssertSdkError::ThresholdExceedsCeiling {
            got: min_net_pnl_lamports,
        });
    }
    Ok(())
}

/// The on-chain instruction data layout for `assert_min_pnl`.
///
/// Layout (after the discriminator byte):
///   - 8 bytes: `min_net_pnl_lamports` (little-endian u64)
///
/// Total: 9 bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AssertInstructionData {
    pub min_net_pnl_lamports: u64,
}

impl AssertInstructionData {
    pub const DATA_LEN: usize = 9; // 1 byte discriminator + 8 byte u64

    pub fn new(min_net_pnl_lamports: u64) -> Self {
        Self { min_net_pnl_lamports }
    }

    /// Serialize to the 9-byte instruction data buffer.
    pub fn to_bytes(&self) -> [u8; Self::DATA_LEN] {
        let mut out = [0u8; Self::DATA_LEN];
        out[0] = ASSERT_INSTRUCTION_DISCRIMINATOR;
        out[1..9].copy_from_slice(&self.min_net_pnl_lamports.to_le_bytes());
        out
    }

    /// Deserialize from the 9-byte instruction data buffer.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, AssertSdkError> {
        if bytes.len() != Self::DATA_LEN {
            return Err(AssertSdkError::ThresholdNotPositive(bytes.len() as u64));
        }
        if bytes[0] != ASSERT_INSTRUCTION_DISCRIMINATOR {
            return Err(AssertSdkError::ThresholdNotPositive(bytes[0] as u64));
        }
        let mut lamports_bytes = [0u8; 8];
        lamports_bytes.copy_from_slice(&bytes[1..9]);
        Ok(Self {
            min_net_pnl_lamports: u64::from_le_bytes(lamports_bytes),
        })
    }
}

/// Build the full Solana `Instruction` for `assert_min_pnl`.
///
/// `program_id` is the deployed dl-assert program ID (devnet or
/// mainnet). `signer` is the hot wallet (also the fee payer for
/// the assert tx). `vault` is the account whose lamport balance
/// holds the pre-bundle snapshot — in v2.0 we use a fresh
/// per-bundle PDA derived from the signer, so the program can read
/// the snapshot deterministically.
pub fn build_assert_instruction(
    program_id: Pubkey,
    signer: Pubkey,
    vault: Pubkey,
    min_net_pnl_lamports: u64,
) -> Instruction {
    let data = AssertInstructionData::new(min_net_pnl_lamports).to_bytes();
    Instruction {
        program_id,
        accounts: vec![
            AccountMeta::new(signer, true),      // signer, writable, signs
            AccountMeta::new(vault, false),      // writable, not a signer
        ],
        data: data.to_vec(),
    }
}

/// Derive the vault PDA from the signer. The seed is the signer's
/// pubkey + the literal b"dl-assert-vault" string. The PDA is the
/// canonical "pre-bundle lamports snapshot" account.
///
/// Real on-chain logic: the bundle-builder funds this PDA with
/// `signer.lamports()` BEFORE the bundle runs; the assert
/// instruction reads `vault.lamports()` as the snapshot.
pub fn derive_vault_pda(signer: &Pubkey, program_id: &Pubkey) -> (Pubkey, u8) {
    Pubkey::find_program_address(&[b"dl-assert-vault", signer.as_ref()], program_id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn threshold_rejects_zero() {
        let err = assert_min_net_pnl_threshold_reasonable(0).unwrap_err();
        assert_eq!(err, AssertSdkError::ThresholdNotPositive(0));
    }

    #[test]
    fn threshold_rejects_huge_value() {
        let err = assert_min_net_pnl_threshold_reasonable(200 * 1_000_000_000).unwrap_err();
        assert!(matches!(err, AssertSdkError::ThresholdExceedsCeiling { .. }));
    }

    #[test]
    fn threshold_accepts_normal_values() {
        assert!(assert_min_net_pnl_threshold_reasonable(10_000).is_ok());
        assert!(assert_min_net_pnl_threshold_reasonable(1_000_000).is_ok());
        assert!(assert_min_net_pnl_threshold_reasonable(1_000_000_000).is_ok());
        assert!(assert_min_net_pnl_threshold_reasonable(100 * 1_000_000_000).is_ok());
    }

    #[test]
    fn instruction_data_round_trips() {
        let original = AssertInstructionData::new(123_456);
        let bytes = original.to_bytes();
        assert_eq!(bytes.len(), AssertInstructionData::DATA_LEN);
        assert_eq!(bytes[0], ASSERT_INSTRUCTION_DISCRIMINATOR);
        let parsed = AssertInstructionData::from_bytes(&bytes).unwrap();
        assert_eq!(parsed, original);
    }

    #[test]
    fn instruction_data_from_bytes_rejects_wrong_length() {
        let err = AssertInstructionData::from_bytes(&[0u8; 5]).unwrap_err();
        assert!(matches!(err, AssertSdkError::ThresholdNotPositive(_)));
    }

    #[test]
    fn instruction_data_from_bytes_rejects_wrong_discriminator() {
        let mut bytes = [0u8; AssertInstructionData::DATA_LEN];
        bytes[0] = 99; // invalid discriminator
        let err = AssertInstructionData::from_bytes(&bytes).unwrap_err();
        assert!(matches!(err, AssertSdkError::ThresholdNotPositive(_)));
    }

    #[test]
    fn build_assert_instruction_has_correct_accounts_and_data() {
        let program = Pubkey::new_unique();
        let signer = Pubkey::new_unique();
        let vault = Pubkey::new_unique();
        let ix = build_assert_instruction(program, signer, vault, 50_000);
        assert_eq!(ix.program_id, program);
        assert_eq!(ix.accounts.len(), 2);
        assert_eq!(ix.accounts[0].pubkey, signer);
        assert!(ix.accounts[0].is_signer);
        assert!(ix.accounts[0].is_writable);
        assert_eq!(ix.accounts[1].pubkey, vault);
        assert!(!ix.accounts[1].is_signer);
        assert!(ix.accounts[1].is_writable);
        assert_eq!(ix.data.len(), AssertInstructionData::DATA_LEN);
        let parsed = AssertInstructionData::from_bytes(&ix.data).unwrap();
        assert_eq!(parsed.min_net_pnl_lamports, 50_000);
    }

    #[test]
    fn derive_vault_pda_is_deterministic() {
        let program = Pubkey::new_unique();
        let signer = Pubkey::new_unique();
        let (pda1, bump1) = derive_vault_pda(&signer, &program);
        let (pda2, bump2) = derive_vault_pda(&signer, &program);
        assert_eq!(pda1, pda2);
        assert_eq!(bump1, bump2);
    }

    #[test]
    fn derive_vault_pda_differs_per_signer() {
        let program = Pubkey::new_unique();
        let s1 = Pubkey::new_unique();
        let s2 = Pubkey::new_unique();
        let (pda1, _) = derive_vault_pda(&s1, &program);
        let (pda2, _) = derive_vault_pda(&s2, &program);
        assert_ne!(pda1, pda2);
    }
}