//! `dl-app` — binary entry point wiring the damascus_laundry pipeline together.
//!
//! v1.0 is paper-trading only: no keys, no signing, no network submission.
//!
//! # Modes
//!
//! - **No env vars** — Phase 1 placeholder. Logs "foundations ready" and
//!   exits 0. (AC-4 contract.)
//! - **`DL_CAPTURE_PATH` + `DL_RPC_URL` set** — Live capture mode. Connects
//!   to the WS RPC, subscribes to slots (and an optional test pool), drains
//!   for `DL_CAPTURE_SECS` seconds, and prints a summary.
//! - **`DL_DRY_RUN=1`** — Dry-run mode. Opens the sample capture at
//!   `crates/dl-feed/tests/fixtures/sample_capture.bincode`, replays it
//!   through the Raydium AMM v4 decoder, and prints a summary of
//!   decoded/errored counts. No live network. No state mutation. Serves
//!   as the smoke test for the end-to-end Phase 2 pipeline and as a
//!   scaffold for Phase 3's detection harness.

use std::env;
use std::fs::File;
use std::time::Duration;

use dl_core::{Feed, FeedEvent};
use dl_feed::capturing::CapturingFeed;
use dl_feed::ws_feed::WsFeed;
use dl_state::decoder::decode_amm_info;
use tracing::info;

use dl_app::recon;

fn init_tracing() {
    dl_app::init_tracing();
}

fn main() {
    init_tracing();
    info!(
        version = env!("CARGO_PKG_VERSION"),
        mode = "paper-trading",
        strategy = "atomic-dex-dex-arbitrage",
        "damascus_laundry starting (no keys, no live submission)"
    );

    // Mode dispatch: dry-run > live capture > recon > placeholder.
    if env::var("DL_DRY_RUN").ok().as_deref() == Some("1") {
        run_dry_run();
        return;
    }

    if env::args().nth(1).as_deref() == Some("recon") {
        recon::dispatch();
    }

    match (env::var("DL_CAPTURE_PATH"), env::var("DL_RPC_URL")) {
        (Ok(capture_path), Ok(rpc_url)) => {
            let capture_secs: u64 = env::var("DL_CAPTURE_SECS")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(60);
            run_capture(&rpc_url, &capture_path, capture_secs);
        }
        _ => {
            info!("foundations ready; pipeline stages are placeholders until Phase 2+");
        }
    }
}

/// Live capture loop. Subscribes to slot updates and (if `DL_TEST_POOL_PUBKEY`
/// is set) a single pool, then drains for `capture_secs` and prints a summary.
fn run_capture(rpc_url: &str, capture_path: &str, capture_secs: u64) {
    info!(rpc_url, capture_path, capture_secs, "starting live capture");

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .worker_threads(2)
        .build()
        .expect("tokio runtime");
    let mut ws = runtime
        .block_on(async { WsFeed::connect(rpc_url).await })
        .expect("ws connect failed");
    runtime.block_on(async {
        ws.subscribe_slots().await.expect("slotSubscribe failed");
        if let Ok(pk) = env::var("DL_TEST_POOL_PUBKEY") {
            if let Ok(bytes) = bs58::decode(&pk).into_vec() {
                if bytes.len() == 32usize {
                    let mut arr = [0u8; 32];
                    arr.copy_from_slice(&bytes);
                    ws.subscribe_account(arr)
                        .await
                        .expect("accountSubscribe failed");
                    info!(pool = %pk, "subscribed to test pool");
                }
            }
        }
    });

    let file = File::create(capture_path).expect("create capture file");
    let mut tee = CapturingFeed::new(ws, file).expect("CapturingFeed::new failed");

    let deadline = std::time::Instant::now() + Duration::from_secs(capture_secs);
    let mut slots = 0u64;
    let mut accounts = 0u64;
    while std::time::Instant::now() < deadline {
        match tee.next_event() {
            Some(FeedEvent::Slot { .. }) => slots += 1,
            Some(FeedEvent::AccountUpdate { .. }) => accounts += 1,
            None => std::thread::sleep(Duration::from_millis(50)),
        }
    }

    let frames = tee.frames_written();
    let failures = tee.write_failures();
    info!(
        events = slots + accounts,
        slots,
        accounts,
        frames_written = frames,
        capture_write_failures = failures,
        to = capture_path,
        duration_secs = capture_secs,
        "captured events"
    );
}

/// Dry-run: replay a captured `.bincode` file and try to decode every
/// `AccountUpdate` as a Raydium AMM v4 `AmmInfo`. Phase 3+ will replace
/// the `eprintln!`-style summary with a `PoolRegistry` write path; for
/// now this proves the wire format, decoder, and main-loop plumbing
/// hang together end-to-end.
///
/// The sample path defaults to the in-repo `sample_capture.bincode`
/// fixture produced by task 02-01-07. Override with `DL_DRY_RUN_PATH`
/// to point at any other capture file.
fn run_dry_run() {
    let path = env::var("DL_DRY_RUN_PATH").unwrap_or_else(|_| {
        // CARGO_MANIFEST_DIR is the dl-app crate dir; the fixture sits
        // two crates over. This path works from `cargo run -p dl-app`
        // regardless of the current shell cwd.
        let manifest = env!("CARGO_MANIFEST_DIR");
        format!(
            "{}/../dl-feed/tests/fixtures/sample_capture.bincode",
            manifest
        )
    });

    info!(path = %path, "starting dry-run replay");

    let file = File::open(&path).unwrap_or_else(|e| {
        panic!(
            "failed to open capture file at {}: {}. Run `DL_CAPTURE_PATH={} DL_RPC_URL=wss://api.mainnet-beta.solana.com/ cargo run -p dl-app` to produce one.",
            path, e, path
        )
    });
    let mut feed = dl_feed::capture::CapturedFeed::open(file).expect("capture open failed");

    let mut slots = 0u64;
    let mut accounts_total = 0u64;
    let mut decoded_ok = 0u64;
    let mut decoded_err = 0u64;
    let mut min_slot: Option<u64> = None;
    let mut max_slot: Option<u64> = None;

    while let Some(ev) = feed.next_event() {
        match ev {
            FeedEvent::Slot { slot } => {
                slots += 1;
                min_slot = Some(min_slot.map_or(slot, |s| s.min(slot)));
                max_slot = Some(max_slot.map_or(slot, |s| s.max(slot)));
            }
            FeedEvent::AccountUpdate {
                slot,
                pubkey: _,
                data,
            } => {
                accounts_total += 1;
                min_slot = Some(min_slot.map_or(slot, |s| s.min(slot)));
                max_slot = Some(max_slot.map_or(slot, |s| s.max(slot)));
                match decode_amm_info(&data) {
                    Ok(_amm) => decoded_ok += 1,
                    Err(_e) => decoded_err += 1,
                }
            }
        }
    }

    let events_returned = feed.events_returned();
    let slot_range = match (min_slot, max_slot) {
        (Some(lo), Some(hi)) => format!("{}..={}", lo, hi),
        _ => "n/a".to_string(),
    };

    info!(
        events_returned,
        slots,
        accounts_total,
        decoded_ok,
        decoded_err,
        slot_range,
        from = %path,
        "dry-run replay complete"
    );
}
