//! Live WsFeed integration test. Gated on `DL_TEST_RPC_URL` and
//! `DL_TEST_POOL_PUBKEY` env vars; ignored in CI by default.
//!
//! Run locally with:
//!   DL_TEST_RPC_URL=wss://api.mainnet-beta.solana.com/ \
//!   DL_TEST_POOL_PUBKEY=<base58-pubkey> \
//!   cargo test -p dl-feed --features ws --test ws_feed_live -- --ignored --nocapture
//!
//! The `DL_TEST_POOL_PUBKEY` should be a USDC/SOL Raydium AMM v4 pool
//! (researched in 02-02-02 and written to `crates/dl-state/docs/RESEARCH.md`).

#![cfg(feature = "ws")]

use dl_core::{Feed, FeedEvent};
use dl_feed::ws_feed::WsFeed;
use std::time::Duration;

fn env_or_panic(name: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| panic!("{name} env var is required for live test"))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires DL_TEST_RPC_URL and DL_TEST_POOL_PUBKEY; run with --ignored"]
async fn ws_feed_subscribes_to_known_pool() {
    let url = env_or_panic("DL_TEST_RPC_URL");
    let pubkey_str = env_or_panic("DL_TEST_POOL_PUBKEY");
    let pubkey_bytes = bs58::decode(&pubkey_str)
        .into_vec()
        .expect("DL_TEST_POOL_PUBKEY is not valid base58");
    assert_eq!(pubkey_bytes.len(), 32, "pubkey must be 32 bytes");
    let mut pubkey = [0u8; 32];
    pubkey.copy_from_slice(&pubkey_bytes);

    let mut feed = WsFeed::connect(&url).await.expect("connect failed");
    feed.subscribe_account(pubkey)
        .await
        .expect("subscribe failed");
    eprintln!("subscribed; polling for events (15s timeout)…");

    // Sync polling: caller is async, but `next_event` is sync. Loop with
    // short sleeps to let the runtime drive the background task.
    let deadline = std::time::Instant::now() + Duration::from_secs(15);
    let mut got: Option<FeedEvent> = None;
    while std::time::Instant::now() < deadline {
        if let Some(ev) = feed.next_event() {
            got = Some(ev);
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let ev = got.expect("timed out waiting for first AccountUpdate");
    assert!(
        matches!(ev, FeedEvent::AccountUpdate { .. }),
        "expected AccountUpdate, got {ev:?}"
    );
}
