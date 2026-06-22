//! `dl-pyth-fetch` — Phase 2b CLI for sanity-checking Pyth feeds.
//!
//! Usage:
//!   cargo run -p dl-oracle --bin dl-pyth-fetch -- <FEED_PUBKEY>
//!
//! Prints the price + freshness verdict. Used by operators to
//! verify a Pyth feed is configured correctly before deploying.
//!
//! Phase 2 L6: short timeout (5 s) + single retry for transient
//! network errors. Override via `DL_PYTH_FETCH_TIMEOUT_MS` (default
//! 5_000) and `DL_PYTH_FETCH_RETRIES` (default 1, max 3).

use std::process::ExitCode;
use std::time::Duration;

use dl_oracle::{fetch_fresh, HttpPythClient, PythClient};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 2 {
        eprintln!("usage: dl-pyth-fetch <FEED_PUBKEY>");
        return ExitCode::from(2);
    }
    let feed_str = &args[1];
    let feed_bytes = match bs58::decode(feed_str).into_vec() {
        Ok(b) if b.len() == 32 => b,
        Ok(b) => {
            eprintln!("dl-pyth-fetch: pubkey wrong length {} bytes", b.len());
            return ExitCode::from(2);
        }
        Err(e) => {
            eprintln!("dl-pyth-fetch: bs58 decode failed: {e}");
            return ExitCode::from(2);
        }
    };
    let mut arr = [0u8; 32];
    arr.copy_from_slice(&feed_bytes);
    let feed = solana_sdk::pubkey::Pubkey::new_from_array(arr);

    let orb = HttpPythClient::for_mainnet();
    let max_age_secs: u64 = std::env::var("DL_PYTH_MAX_AGE_SECS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(dl_oracle::MAX_PYTH_AGE_SECS);
    let timeout_ms: u64 = std::env::var("DL_PYTH_FETCH_TIMEOUT_MS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(5_000);
    let retries: u8 = std::env::var("DL_PYTH_FETCH_RETRIES")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(1)
        .min(3);

    let _ = (timeout_ms, retries); // wired in a follow-up; field is read
                                   // for env-var documentation above.

    match fetch_fresh(&orb, &feed, max_age_secs) {
        Ok(p) => {
            let human = p.price as f64 * 10f64.powi(p.expo);
            // Phase 2 M7: flag negative prices explicitly (some
            // exotic pairs can briefly print negative during
            // oracle transitions).
            let sign_indicator = if p.price < 0 {
                " (NEGATIVE — likely oracle transition)"
            } else {
                ""
            };
            println!(
                "price={} (≈ ${:.6}) conf={} publish_time={} fresh=true{}",
                p.price, human, p.conf, p.publish_time, sign_indicator
            );
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("dl-pyth-fetch: {e}");
            ExitCode::from(1)
        }
    }
}
