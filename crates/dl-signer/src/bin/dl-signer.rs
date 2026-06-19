//! `dl-signer` CLI (Phase 8 / plan 03).
//!
//! Operator commands for the hot-wallet keyfile. All commands
//! take a passphrase via `DL_SIGNER_PASSPHRASE` env var (NOT a
//! CLI argument, to keep it out of process listings).
//!
//! ## Subcommands
//!
//! - `dl-signer generate --out <path>`: create a new encrypted
//!   keyfile. Writes a `KFK1` blob to `<path>`. The pubkey is
//!   printed to stdout. **The passphrase is not stored** —
//!   the operator must remember it.
//!
//! - `dl-signer verify --keyfile <path>`: read a keyfile and
//!   print the pubkey. Useful for confirming a keyfile is
//!   well-formed before the first live run.
//!
//! - `dl-signer drain-to <cold_address> --keyfile <path>`:
//   print a `solana transfer` command that the operator can
//!   run manually to drain the hot wallet to a cold address.
//!   We don't sign and send the transfer ourselves in v1.1.0
//!   because the live `solana-sdk` deps aren't pulled in (this
//!   binary is intentionally dep-light). The operator uses
//!   the standard Solana CLI for the actual transfer.
//!
//! ## Passphrase handling
//!
//! All commands read `DL_SIGNER_PASSPHRASE` from the env. The
//! binary never accepts a passphrase via the CLI (visible in
//! `ps` output).

use std::path::PathBuf;
use std::process::ExitCode;

use dl_signer::keystore::{KeyFile, KeyStore};
use dl_signer::livemode::ResolvedLiveMode;

fn read_passphrase() -> Result<String, String> {
    std::env::var("DL_SIGNER_PASSPHRASE")
        .map_err(|_| "DL_SIGNER_PASSPHRASE env var not set".to_string())
}

fn cmd_generate(out: PathBuf) -> Result<(), String> {
    let passphrase = read_passphrase()?;
    let kf = KeyFile::new(&passphrase);
    kf.save(&out).map_err(|e| e.to_string())?;
    let secret = kf.decrypt(&passphrase).map_err(|e| e.to_string())?;
    let ks = KeyStore::from_secret(secret);
    println!("wrote keyfile: {}", out.display());
    println!("pubkey: {}", bs58::encode(ks.public_key_for_print()).into_string());
    println!("DRAIN-INSTRUCTION-KEY: drain-to 0xDEADBEEFCAFE0000000000000000000000000000000000000000000000000000 --keyfile {}", out.display());
    println!("KEEP THE PASSPHRASE SAFE. It is not stored anywhere.");
    Ok(())
}

fn cmd_verify(keyfile: PathBuf) -> Result<(), String> {
    let passphrase = read_passphrase()?;
    let kf = KeyFile::load(&keyfile).map_err(|e| e.to_string())?;
    let secret = kf.decrypt(&passphrase).map_err(|e| e.to_string())?;
    let ks = KeyStore::from_secret(secret);
    println!("keyfile: {} (OK)", keyfile.display());
    println!("pubkey: {}", bs58::encode(ks.public_key_for_print()).into_string());
    Ok(())
}

fn cmd_drain_to(cold_address: String, keyfile: PathBuf) -> Result<(), String> {
    let passphrase = read_passphrase()?;
    let kf = KeyFile::load(&keyfile).map_err(|e| e.to_string())?;
    let secret = kf.decrypt(&passphrase).map_err(|e| e.to_string())?;
    let ks = KeyStore::from_secret(secret);
    let pubkey = bs58::encode(ks.public_key_for_print()).into_string();
    println!("# Hot-wallet drain instructions");
    println!("# 1. Verify the keyfile (sanity check)");
    println!("DL_SIGNER_PASSPHRASE='$PASSPHRASE' dl-signer verify --keyfile {}", keyfile.display());
    println!();
    println!("# 2. Check the balance");
    println!("solana balance {}", pubkey);
    println!();
    println!("# 3. Drain the hot wallet to the cold address");
    println!("#    Adjust --amount to leave 0.001 SOL for rent + fees.");
    println!("solana transfer --from {} {} --amount 0.99", pubkey, cold_address);
    println!();
    println!("# 4. Confirm the hot wallet is now empty");
    println!("solana balance {}", pubkey);
    Ok(())
}

fn usage() {
    eprintln!("USAGE:");
    eprintln!("    dl-signer generate --out <path>");
    eprintln!("    dl-signer verify --keyfile <path>");
    eprintln!("    dl-signer drain-to <cold_address> --keyfile <path>");
    eprintln!();
    eprintln!("ENV VARS:");
    eprintln!("    DL_SIGNER_PASSPHRASE  (required; the keyfile passphrase)");
    eprintln!("    DL_LIVE_MODE          (gate; refused / devnet / mainnet-paper / mainnet)");
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 2 {
        usage();
        return ExitCode::from(1);
    }

    // Resolve live mode at boot. If refused, refuse to run.
    let mode = match ResolvedLiveMode::from_env() {
        Ok(m) => m,
        Err(e) => {
            eprintln!("error: {e}");
            return ExitCode::from(2);
        }
    };
    if mode.refuses() {
        eprintln!("dl-signer CLI: refused (DL_LIVE_MODE not set)");
        eprintln!("This binary is for operators preparing for live mode.");
        eprintln!("To verify a keyfile without going live, use:");
        eprintln!("    DL_LIVE_MODE=devnet dl-signer verify --keyfile <path>");
        return ExitCode::from(0); // Soft exit; not an error
    }

    let subcmd = &args[1];
    let result = match subcmd.as_str() {
        "generate" => {
            let out = parse_arg(&args, "--out")
                .map(PathBuf::from)
                .ok_or_else(|| "--out <path> required".to_string());
            match out {
                Ok(o) => cmd_generate(o),
                Err(e) => Err(e),
            }
        }
        "verify" => {
            let kf = parse_arg(&args, "--keyfile")
                .map(PathBuf::from)
                .ok_or_else(|| "--keyfile <path> required".to_string());
            match kf {
                Ok(k) => cmd_verify(k),
                Err(e) => Err(e),
            }
        }
        "drain-to" => {
            if args.len() < 3 {
                usage();
                std::process::exit(1);
            }
            let cold = args[2].clone();
            let kf = parse_arg(&args, "--keyfile")
                .map(PathBuf::from)
                .ok_or_else(|| "--keyfile <path> required".to_string());
            match kf {
                Ok(k) => cmd_drain_to(cold, k),
                Err(e) => Err(e),
            }
        }
        "help" | "--help" | "-h" => {
            usage();
            Ok(())
        }
        _ => {
            eprintln!("unknown subcommand: {subcmd}");
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

fn parse_arg(args: &[String], flag: &str) -> Option<String> {
    let mut i = 0;
    while i < args.len() {
        if args[i] == flag {
            return args.get(i + 1).cloned();
        }
        i += 1;
    }
    None
}
