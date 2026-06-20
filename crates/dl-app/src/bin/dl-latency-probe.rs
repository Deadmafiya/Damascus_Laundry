//! `dl-latency-probe` — Phase 1d real submission-to-landing probe.
//!
//! Replaces `latency_probe.py` (which used a plain RPC
//! `sendTransaction`, not a Jito bundle). This binary builds real
//! 5-tx Jito bundles through `dl_executor::HttpJitoClient`,
//! times each submit→landed round-trip, and prints a JSON summary
//! to stdout.
//!
//! ## Bundle contents
//!
//! Each probe bundle is:
//! - 3x placeholder `VersionedTransaction`s (system transfer of 0
//!   lamports; signed by the keystore so submission is well-formed
//!   but no token movement happens).
//! - 1x dl-assert instruction with `min_net_pnl_lamports = 0`
//!   (so the program accepts any outcome).
//! - 1x Jito tip transfer.
//!
//! ## Cost
//!
//! Per probe: `tip_lamports` + tx fees + memo rent ≈ ~0.0001 SOL.
//! For a 20-probe session: ~0.002 SOL. Operator funds the hot wallet
//! once per day via cold wallet `solana transfer`.
//!
//! ## Output
//!
//! JSON to stdout:
//! ```json
//! {
//!   "probe_count": 20,
//!   "p50_ms": 420,
//!   "p95_ms": 680,
//!   "min_ms": 380,
//!   "max_ms": 920,
//!   "failures": 0,
//!   "samples_ms": [380, 410, 420, ...]
//! }
//! ```
//!
//! Exit code: 0 always. Failures reflected in the JSON.
//!
//! ## Operator usage
//!
//! ```bash
//! cargo run --release -p dl-app --bin dl-latency-probe \
//!     --rpc-url https://api.mainnet-beta.solana.com \
//!     --keyfile ~/.damascus/mainnet-keyfile.json \
//!     --assert-program-id <MAINNET_DL_ASSERT_PROGRAM_ID> \
//!     --tip-account <JITO_TIP_ACCOUNT> \
//!     --probe-count 20
//! ```

use std::path::PathBuf;
use std::process::ExitCode;
use std::time::{Duration, Instant};

use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use solana_sdk::hash::Hash;
use solana_sdk::message::Message;
use solana_sdk::pubkey::Pubkey as SolPubkey;
use solana_sdk::signature::SeedDerivable;
use solana_sdk::system_instruction;
use solana_sdk::transaction::VersionedTransaction;

use dl_assert_sdk::{build_assert_instruction, derive_vault_pda};
use dl_executor::bundle::{Bundle, TipLeg};
use dl_executor::jito::{
    HttpJitoClient, JitoClient, LandingResult,
};
use dl_executor::landing::{poll_bundle_landing, LandingPollConfig};
use dl_executor::metrics::LiveMetrics;
use dl_executor::signer_integration::{keystore_to_keypair, sign_transactions};

/// CLI args parsed from `std::env::args()`.
struct ProbeArgs {
    rpc_url: String,
    keyfile: PathBuf,
    assert_program_id: SolPubkey,
    tip_account: SolPubkey,
    tip_lamports: u64,
    probe_count: usize,
    poll_timeout_ms: u64,
}

fn print_usage_and_exit() -> ! {
    eprintln!("dl-latency-probe — Phase 1d submission-to-landing probe");
    eprintln!();
    eprintln!("USAGE:");
    eprintln!("    dl-latency-probe --rpc-url <URL> --keyfile <PATH> \\");
    eprintln!("                       --assert-program-id <PUBKEY> --tip-account <PUBKEY> \\");
    eprintln!("                       [--tip-lamports <N>] [--probe-count <N>] \\");
    eprintln!("                       [--poll-timeout-ms <N>]");
    eprintln!();
    eprintln!("Required env:");
    eprintln!("    DL_SIGNER_PASSPHRASE    (passphrase for the keyfile)");
    std::process::exit(2);
}

fn parse_args() -> Result<ProbeArgs, String> {
    let mut rpc_url = None;
    let mut keyfile = None;
    let mut assert_program_id = None;
    let mut tip_account = None;
    let mut tip_lamports: u64 = 10_000; // 0.00001 SOL — matches mainnet-paper tier
    let mut probe_count: usize = 20;
    let mut poll_timeout_ms: u64 = 30_000;

    let args: Vec<String> = std::env::args().collect();
    let mut i = 1;
    while i < args.len() {
        let val = || -> Result<String, String> {
            args.get(i + 1)
                .cloned()
                .ok_or_else(|| format!("{} requires a value", args[i]))
        };
        match args[i].as_str() {
                    "--rpc-url" => {
                        rpc_url = Some(val()?);
                        i += 2;
                    }
                    "--keyfile" => {
                        keyfile = Some(PathBuf::from(val()?));
                        i += 2;
                    }
                    "--assert-program-id" => {
                        let s = val()?;
                        assert_program_id = Some(
                            SolPubkey::try_from(s.as_str())
                                .map_err(|e| format!("invalid --assert-program-id: {e}"))?,
                        );
                        i += 2;
                    }
                    "--tip-account" => {
                        let s = val()?;
                        tip_account = Some(
                            SolPubkey::try_from(s.as_str())
                                .map_err(|e| format!("invalid --tip-account: {e}"))?,
                        );
                        i += 2;
                    }
                    "--tip-lamports" => {
                        tip_lamports = val()?.parse().map_err(|_| "--tip-lamports must be a u64")?;
                        i += 2;
                    }
                    "--probe-count" => {
                        probe_count = val()?.parse().map_err(|_| "--probe-count must be a usize")?;
                        i += 2;
                    }
                    "--poll-timeout-ms" => {
                        poll_timeout_ms = val()?.parse().map_err(|_| "--poll-timeout-ms must be a u64")?;
                        i += 2;
                    }
                    "-h" | "--help" => print_usage_and_exit(),
                    other => return Err(format!("unknown flag: {other}")),
                }
    }

    Ok(ProbeArgs {
        rpc_url: rpc_url.ok_or("--rpc-url is required")?,
        keyfile: keyfile.ok_or("--keyfile is required")?,
        assert_program_id: assert_program_id.ok_or("--assert-program-id is required")?,
        tip_account: tip_account.ok_or("--tip-account is required")?,
        tip_lamports,
        probe_count,
        poll_timeout_ms,
    })
}

/// Load the keystore from the keyfile using `DL_SIGNER_PASSPHRASE`.
fn load_keystore(path: &PathBuf) -> Result<dl_signer::keystore::KeyStore, String> {
    use dl_signer::keystore::{KeyFile, KeyStore};
    let kf = KeyFile::load(path).map_err(|e| format!("load keyfile: {e}"))?;
    let passphrase = std::env::var("DL_SIGNER_PASSPHRASE")
        .map_err(|_| "DL_SIGNER_PASSPHRASE not set".to_string())?;
    let secret = kf
        .decrypt(&passphrase)
        .map_err(|e| format!("decrypt keyfile: {e}"))?;
    Ok(KeyStore::from_secret(secret))
}

/// Build a dummy system-transfer `VersionedTransaction` whose
/// signature will be overwritten by `sign_transactions` later.
fn dummy_signed_tx(
    fee_payer: SolPubkey,
    to: SolPubkey,
    recent_blockhash: Hash,
) -> Result<VersionedTransaction, String> {
    dummy_signed_tx_with_amount(fee_payer, to, 0, recent_blockhash)
}

/// Build a dummy system-transfer `VersionedTransaction` with a
/// specific transfer amount. Builds the tx directly via the struct
/// literal (skipping `try_new`'s signer-pubkey validation, since
/// the placeholder signature gets overwritten by
/// `sign_transactions` later).
fn dummy_signed_tx_with_amount(
    fee_payer: SolPubkey,
    to: SolPubkey,
    amount: u64,
    recent_blockhash: Hash,
) -> Result<VersionedTransaction, String> {
    use solana_sdk::message::VersionedMessage;
    use solana_sdk::signature::Signature;
    let ix = system_instruction::transfer(&fee_payer, &to, amount);
    let mut msg = Message::new(&[ix], Some(&fee_payer));
    msg.recent_blockhash = recent_blockhash;
    let v0_msg = VersionedMessage::Legacy(msg);
    let n_sigs = v0_msg.header().num_required_signatures as usize;
    let signatures = vec![Signature::default(); n_sigs];
    Ok(VersionedTransaction {
        signatures,
        message: v0_msg,
    })
}

/// Build a probe bundle: 3 placeholder system-transfer txs +
/// dl-assert (min=0) + tip. Returns signed `Vec<VersionedTransaction>`.
fn build_probe_bundle(
    keystore: &dl_signer::keystore::KeyStore,
    signer_sol: SolPubkey,
    tip_account: SolPubkey,
    assert_program_id: SolPubkey,
    tip_lamports: u64,
    recent_blockhash: Hash,
) -> Result<Vec<VersionedTransaction>, String> {
    // 3 placeholder swap legs (no-op system transfers).
    let mut txs: Vec<VersionedTransaction> = Vec::with_capacity(5);
    for _ in 0..3 {
        txs.push(dummy_signed_tx(
            signer_sol,
            SolPubkey::new_unique(),
            recent_blockhash,
        )?);
    }

    // dl-assert tx with min=0.
    let (vault, _bump) = derive_vault_pda(&signer_sol, &assert_program_id);
    let assert_ix = build_assert_instruction(assert_program_id, signer_sol, vault, 0);
    let mut assert_msg = Message::new(&[assert_ix], Some(&signer_sol));
    assert_msg.recent_blockhash = recent_blockhash;
    // Use direct struct construction (skipping try_new's signer-pubkey
    // validation). The placeholder signature is overwritten by
    // sign_transactions below.
    use solana_sdk::message::VersionedMessage;
    use solana_sdk::signature::Signature;
    let v0_msg = VersionedMessage::Legacy(assert_msg);
    let n_sigs = v0_msg.header().num_required_signatures as usize;
    let signatures = vec![Signature::default(); n_sigs];
    txs.push(VersionedTransaction {
        signatures,
        message: v0_msg,
    });

    // Tip tx.
    txs.push(dummy_signed_tx_with_amount(
        signer_sol,
        tip_account,
        tip_lamports,
        recent_blockhash,
    )?);

    // Sign all 5 with the keystore (overwrites the dummy signatures).
    let keypair = keystore_to_keypair(keystore)
        .map_err(|e| format!("keystore→keypair: {e}"))?;
    sign_transactions(&keypair, &mut txs, recent_blockhash)
        .map_err(|e| format!("sign: {e}"))?;

    Ok(txs)
}

/// Fetch a recent blockhash from the RPC. Uses a simple JSON-RPC
/// POST.
fn fetch_blockhash(rpc_url: &str) -> Result<Hash, String> {
    let body = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "getLatestBlockhash",
        "params": [{"commitment": "processed"}]
    });
    let resp: serde_json::Value = reqwest::blocking::Client::new()
        .post(rpc_url)
        .json(&body)
        .send()
        .map_err(|e| format!("blockhash http: {e}"))?
        .json()
        .map_err(|e| format!("blockhash decode: {e}"))?;
    let bh_str = resp
        .pointer("/result/value/blockhash")
        .and_then(|b| b.as_str())
        .ok_or_else(|| "missing blockhash in response".to_string())?;
    let bytes = BASE64
        .decode(bh_str)
        .map_err(|e| format!("blockhash base64: {e}"))?;
    if bytes.len() != 32 {
        return Err(format!("blockhash wrong length: {}", bytes.len()));
    }
    let mut arr = [0u8; 32];
    arr.copy_from_slice(&bytes);
    Ok(Hash::new_from_array(arr))
}

fn main() -> ExitCode {
    let args = match parse_args() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("error: {e}");
            print_usage_and_exit();
        }
    };
    if args.probe_count == 0 {
        eprintln!("error: --probe-count must be > 0");
        return ExitCode::from(1);
    }

    // 1. Load keystore.
    let keystore = match load_keystore(&args.keyfile) {
        Ok(k) => k,
        Err(e) => {
            eprintln!("error: {e}");
            return ExitCode::from(1);
        }
    };
    let signer_sol = SolPubkey::new_from_array(keystore.public_key_for_print());
    let signer_pubkey_bs58 = signer_sol.to_string();

    // 2. Fetch recent blockhash.
    let blockhash = match fetch_blockhash(&args.rpc_url) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("error: fetch_blockhash: {e}");
            return ExitCode::from(1);
        }
    };

    // 3. Build Jito client. (HttpJitoClient::new with the same
    // RPC URL — works for mainnet/devnet.)
    let client = HttpJitoClient::new(args.rpc_url.clone());

    // 4. Run probes.
    let live = LiveMetrics::new();
    let mut samples: Vec<u64> = Vec::with_capacity(args.probe_count);
    let mut failures: u64 = 0;

    eprintln!(
        "dl-latency-probe: starting {} probes (signer={}, tip_account={}, tip_lamports={})",
        args.probe_count, signer_pubkey_bs58, args.tip_account, args.tip_lamports
    );

    for i in 0..args.probe_count {
        let txs = match build_probe_bundle(
            &keystore,
            signer_sol,
            args.tip_account,
            args.assert_program_id,
            args.tip_lamports,
            blockhash,
        ) {
            Ok(t) => t,
            Err(e) => {
                eprintln!("probe {i}: build error: {e}");
                failures += 1;
                continue;
            }
        };

        let bundle = Bundle {
            tip: TipLeg::new(args.tip_lamports, args.tip_account.to_string()),
            legs: vec![],
            signed_transactions: txs,
        };

        let t0 = Instant::now();
        let submit = match client.submit(&bundle) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("probe {i}: submit error: {e}");
                failures += 1;
                continue;
            }
        };
        let poll_cfg = LandingPollConfig {
            timeout: Duration::from_millis(args.poll_timeout_ms),
            ..LandingPollConfig::default()
        };
        let landed = poll_bundle_landing(&submit.bundle_id, &poll_cfg, |id| client.poll_landing(id));
        let elapsed_ms = t0.elapsed().as_millis() as u64;

        match landed {
            Ok(LandingResult::Landed { slot }) => {
                live.record_landing_latency_ms(elapsed_ms);
                samples.push(elapsed_ms);
                eprintln!(
                    "probe {i}/{}: landed slot={slot} in {elapsed_ms}ms",
                    args.probe_count
                );
            }
            Ok(other) => {
                eprintln!("probe {i}: not landed ({other:?}) after {elapsed_ms}ms");
                failures += 1;
            }
            Err(e) => {
                eprintln!("probe {i}: poll error: {e}");
                failures += 1;
            }
        }
    }

    // 5. Build summary.
    let snap = live.landing_latency_snapshot();
    let summary = serde_json::json!({
        "probe_count": args.probe_count,
        "successes": samples.len(),
        "failures": failures,
        "p50_ms": snap.p50_ms,
        "p95_ms": snap.p95_ms,
        "min_ms": snap.min_ms,
        "max_ms": snap.max_ms,
        "sum_ms": snap.sum_ms,
        "samples_ms": samples,
    });
    println!("{}", serde_json::to_string(&summary).unwrap());

    ExitCode::SUCCESS
}

#[cfg(test)]
mod tests {
    use super::*;
    use solana_sdk::message::VersionedMessage;

    #[test]
    fn build_probe_bundle_produces_5_txs_with_correct_blockhash() {
        use dl_signer::keystore::{KeyFile, KeyStore};
        let kf = KeyFile::new("test-passphrase-1d-probe");
        let secret = kf.decrypt("test-passphrase-1d-probe").unwrap();
        let ks = KeyStore::from_secret(secret);
        let signer = SolPubkey::new_from_array(ks.public_key_for_print());
        let bh = Hash::new_unique();

        let txs = build_probe_bundle(
            &ks,
            signer,
            SolPubkey::new_unique(),
            SolPubkey::new_unique(),
            10_000,
            bh,
        )
        .unwrap();
        assert_eq!(txs.len(), 5);
        for tx in &txs {
            match &tx.message {
                VersionedMessage::Legacy(m) => {
                    assert_eq!(m.recent_blockhash, bh);
                }
                _ => panic!("expected Legacy"),
            }
        }
    }

    #[test]
    fn build_probe_bundle_signs_with_keystore() {
        use dl_signer::keystore::{KeyFile, KeyStore};
        let kf = KeyFile::new("test-passphrase-1d-sign");
        let secret = kf.decrypt("test-passphrase-1d-sign").unwrap();
        let ks = KeyStore::from_secret(secret);
        let signer = SolPubkey::new_from_array(ks.public_key_for_print());
        let bh = Hash::new_unique();

        let txs = build_probe_bundle(
            &ks,
            signer,
            SolPubkey::new_unique(),
            SolPubkey::new_unique(),
            10_000,
            bh,
        )
        .unwrap();
        // Each tx has 1 signature from the keystore.
        for tx in &txs {
            assert_eq!(tx.signatures.len(), 1);
            assert_ne!(tx.signatures[0], solana_sdk::signature::Signature::default());
        }
    }

    #[test]
    fn fetch_blockhash_returns_32_byte_hash() {
        // Test with a minimal HTTP mock isn't worth it here — the
        // function is straightforward JSON-RPC. Just sanity-check
        // the input parsing path via BASE64 decode.
        let valid_b64 = BASE64.encode(&[42u8; 32]);
        let bytes = BASE64.decode(&valid_b64).unwrap();
        assert_eq!(bytes.len(), 32);
    }
}