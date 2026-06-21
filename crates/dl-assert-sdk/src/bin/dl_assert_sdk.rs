//! `dl-assert-sdk` CLI — operator-facing helpers for the v2.0
//! on-chain profit-assert program (see `plan/atomicity-decision.md`).
//!
//! ## Subcommands
//!
//! - `dl-assert-sdk derive-vault-pda <SIGNER> <ASSERT_PROGRAM>`
//!   — print the vault PDA pubkey to stdout. Designed to be
//!   captured in a shell variable exactly as the runbook shows:
//!
//!   ```bash
//!   VAULT=$(dl-assert-sdk derive-vault-pda <SIGNER> <ASSERT_PROGRAM>)
//!   solana transfer --from <KEYPAIR> $VAULT <LAMPORTS>
//!   ```
//!
//!   The PDA is derived from
//!   `Pubkey::find_program_address(&[b"dl-assert-vault", signer.as_ref()], program_id)`.
//!
//! - `dl-assert-sdk info` — print the discriminator and the
//!   encoded instruction length. Useful for sanity-checking the
//!   SDK matches the deployed BPF program after `cargo build-sbf`
//!   and `solana program deploy`.
//!
//! - `dl-assert-sdk encode <min_net_pnl_lamports>` — print the
//!   9-byte instruction data layout (1 byte discriminator + 8
//!   byte little-endian u64) as a hex string. Useful for offline
//!   verification of the threshold encoding before the operator
//!   goes on-chain.
//!
//! ## Why this binary exists
//!
//! The runbook at `docs/v2.0-operator-runbook.md` references
//! `dl-assert-sdk derive-vault-pda` and `dl-assert-sdk derive-vault-pda
//! <SIGNER> <ASSERT_PROGRAM>` as part of the operator's
//! pre-flight. Before this CLI existed, the operator had to
//! write a small Rust program to call the lib's
//! `derive_vault_pda` function. This binary closes that gap.

use std::process::ExitCode;

use dl_assert_sdk::{
    assert_min_net_pnl_threshold_reasonable, derive_vault_pda, AssertInstructionData,
    ASSERT_INSTRUCTION_DISCRIMINATOR,
};
use solana_sdk::pubkey::Pubkey;

fn usage() {
    eprintln!("USAGE:");
    eprintln!("    dl-assert-sdk derive-vault-pda <SIGNER> <ASSERT_PROGRAM>");
    eprintln!("    dl-assert-sdk info");
    eprintln!("    dl-assert-sdk encode <min_net_pnl_lamports>");
    eprintln!();
    eprintln!("ENV VARS:");
    eprintln!("    (none required — read-only CLI)");
}

fn parse_pubkey(label: &str, s: &str) -> Result<Pubkey, String> {
    Pubkey::try_from(s).map_err(|e| format!("invalid {label} pubkey {s:?}: {e}"))
}

fn cmd_derive_vault_pda(signer: &str, program: &str) -> Result<(), String> {
    let signer_pk = parse_pubkey("signer", signer)?;
    let program_pk = parse_pubkey("assert-program", program)?;
    let (pda, bump) = derive_vault_pda(&signer_pk, &program_pk);
    // stdout: the PDA only (the runbook captures this in a shell var).
    println!("{}", pda);
    // The bump is part of the canonical PDA; surface it on stderr
    // so the operator can audit the derivation if needed.
    eprintln!("# bump: {bump}");
    Ok(())
}

fn cmd_info() -> Result<(), String> {
    println!("dl-assert-sdk v{}", env!("CARGO_PKG_VERSION"));
    println!("discriminator (assert_min_net_pnl): {ASSERT_INSTRUCTION_DISCRIMINATOR}");
    println!("instruction data length: {} bytes", AssertInstructionData::DATA_LEN);
    println!("layout: 1 byte discriminator + 8 byte little-endian u64 min_net_pnl_lamports");
    Ok(())
}

fn cmd_encode(min_pnl: &str) -> Result<(), String> {
    let lamports: u64 = min_pnl
        .parse()
        .map_err(|e| format!("invalid min_net_pnl_lamports {min_pnl:?}: {e}"))?;
    assert_min_net_pnl_threshold_reasonable(lamports)
        .map_err(|e| format!("threshold rejected: {e}"))?;
    let bytes = AssertInstructionData::new(lamports).to_bytes();
    let hex: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    println!("# lamports: {lamports}");
    println!("# hex: {hex}");
    Ok(())
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 2 {
        usage();
        return ExitCode::from(1);
    }
    let result: Result<(), String> = match args[1].as_str() {
        "derive-vault-pda" => {
            if args.len() < 4 {
                eprintln!("derive-vault-pda requires <SIGNER> <ASSERT_PROGRAM> as args 2 and 3");
                usage();
                std::process::exit(1);
            }
            cmd_derive_vault_pda(&args[2], &args[3])
        }
        "info" => cmd_info(),
        "encode" => {
            if args.len() < 3 {
                eprintln!("encode requires <min_net_pnl_lamports> as arg 2");
                usage();
                std::process::exit(1);
            }
            cmd_encode(&args[2])
        }
        "help" | "--help" | "-h" => {
            usage();
            Ok(())
        }
        other => {
            eprintln!("unknown subcommand: {other}");
            usage();
            std::process::exit(1);
        }
    };
    match result {
        Ok(()) => ExitCode::from(0),
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::from(1)
        }
    }
}
