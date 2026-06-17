//! `dl-app` — binary entry point wiring the damascus_laundry pipeline together.
//!
//! v1.0 is paper-trading only: no keys, no signing, no network submission.
//!
//! Phase 2 adds an optional live-capture path: when `DL_CAPTURE_PATH` and
//! `DL_RPC_URL` are set, the binary connects to the RPC, wraps `WsFeed` in a
//! `CapturingFeed<File, _>`, drains for `DL_CAPTURE_SECS` seconds, and prints
//! a summary. Without those env vars, it stays in Phase 1's placeholder
//! mode so the AC-4 contract (`cargo run -p dl-app` always exits 0) holds.

use std::env;
use std::fs::File;
use std::time::Duration;

use dl_core::{Feed, FeedEvent};
use dl_feed::capturing::CapturingFeed;
use dl_feed::ws_feed::WsFeed;
use tracing::info;
use tracing_subscriber::EnvFilter;

fn init_tracing() {
    // Structured logging, configurable via `RUST_LOG` (default `info`).
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(true)
        .init();
}

fn main() {
    init_tracing();
    info!(
        version = env!("CARGO_PKG_VERSION"),
        mode = "paper-trading",
        strategy = "atomic-dex-dex-arbitrage",
        "damascus_laundry starting (no keys, no live submission)"
    );

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
