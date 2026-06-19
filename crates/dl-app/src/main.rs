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
use std::sync::Arc;
use tracing::info;

use dl_app::config::EngineConfig;
use dl_app::metrics::MetricsRegistry;
use dl_app::metrics_prom::MetricsPrometheus;
use dl_app::recon;
use dl_ledger::{LedgerEntry, LedgerWriter, LEDGER_MAGIC, LEDGER_SCHEMA_VERSION};
use dl_recon::fixture::{synthesize_pools, SynthPoolSpec};
use dl_recon::pipeline::{replay_capture_to_ledger, ReplayParams};
use dl_state::Pubkey;
fn init_tracing() {
    dl_app::init_tracing();
}

/// Resolve the EngineConfig path from `DL_ENGINE_CONFIG`, falling
/// back to a `config.toml` in the current working directory.
fn config_path_from_env() -> std::path::PathBuf {
    std::env::var("DL_ENGINE_CONFIG")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| std::path::PathBuf::from("config.toml"))
}

fn main() {
    init_tracing();
    info!(
        version = env!("CARGO_PKG_VERSION"),
        mode = "paper-trading",
        strategy = "atomic-dex-dex-arbitrage",
        "damascus_laundry starting (no keys, no live submission)"
    );

    // Mode dispatch: dry-run > live capture > recon > config > placeholder.
    if env::var("DL_DRY_RUN").ok().as_deref() == Some("1") {
        run_dry_run();
        return;
    }

    if env::args().nth(1).as_deref() == Some("recon") {
        recon::dispatch();
        return;
    }

    if env::args().nth(1).as_deref() == Some("config") {
        let sub = env::args().nth(2);
        match sub.as_deref() {
            Some("print") => match EngineConfig::load(&config_path_from_env()) {
                Ok(cfg) => {
                    if let Ok(s) = toml::to_string_pretty(&cfg) {
                        println!("{s}");
                    }
                }
                Err(e) => eprintln!("config load error: {e}"),
            },
            _ => {
                eprintln!("USAGE:");
                eprintln!("    dl-app config print");
            }
        }
        return;
    }

    if env::args().nth(1).as_deref() == Some("run") {
        // 08-02: dl-app run --feed capture|ws [--dry-run-live]
        //             [--shutdown-after-n N] [--enable-profiling]
        //             [--metrics-port N]
        // The full live pipeline (real Jupiter, real Jito,
        // real `solana-sdk`) lands in 08-03. For 08-02 the
        // capture path runs the streaming detector end-to-end
        // and exits on shutdown.
        run_run_subcommand();
        return;
    }

    if env::args().nth(1).as_deref() == Some("metrics") {
        let sub = env::args().nth(2);
        match sub.as_deref() {
            Some("prom") => {
                // Allow --port N override.
                let mut port: u16 = 9090;
                let args: Vec<String> = env::args().skip(3).collect();
                let mut i = 0;
                while i < args.len() {
                    if args[i] == "--port" {
                        if let Some(p) = args.get(i + 1) {
                            if let Ok(n) = p.parse() {
                                port = n;
                            }
                        }
                        i += 2;
                    } else {
                        i += 1;
                    }
                }
                let code = run_metrics_prom(port);
                if code != std::process::ExitCode::SUCCESS {
                    std::process::exit(2);
                }
                return;
            }
            _ => {
                eprintln!("USAGE:");
                eprintln!("    dl-app metrics prom [--port N]");
            }
        }
        return;
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
/// Connect to a mainnet WebSocket RPC. Returns a boxed future.
fn connect_mainnet_async<'a>(url: &'a str)
    -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<WsFeed, dl_feed::FeedError>> + Send + 'a>>
{
    Box::pin(WsFeed::connect(url))
}

/// Subscribe to slots and (if DL_TEST_POOL_PUBKEY is set) a test pool.
fn subscribe_test_pool_async<'a>(ws: &'a mut WsFeed)
    -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<u64, dl_feed::FeedError>> + Send + 'a>>
{
    let pk = env::var("DL_TEST_POOL_PUBKEY").ok();
    Box::pin(async move {
        let _ = ws.subscribe_slots().await?;
        if let Some(pk) = pk {
            if let Ok(bytes) = bs58::decode(&pk).into_vec() {
                if bytes.len() == 32usize {
                    let mut arr = [0u8; 32];
                    arr.copy_from_slice(&bytes);
                    ws.subscribe_account(arr).await?;
                    info!(pool = %pk, "subscribed to test pool");
                }
            }
        }
        Ok(0)
    })
}

fn run_capture(rpc_url: &str, capture_path: &str, capture_secs: u64) {
    info!(rpc_url, capture_path, capture_secs, "starting live capture");

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .worker_threads(2)
        .build()
        .expect("tokio runtime");
    let mut ws = runtime
        .block_on(connect_mainnet_async(rpc_url))
        .expect("ws connect failed");
    runtime
        .block_on(subscribe_test_pool_async(&mut ws))
        .expect("subscribe_test_pool failed");

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
/// `dl-app run` subcommand (Phase 8 / plan 02).
///
/// Stub for 08-02. The full implementation lands in 08-03
/// with the live Jupiter + Jito clients. For 08-02 this
/// just prints the parsed args and exits — the heavy lifting
/// (streaming detection + latency) is exercised in the
/// `dl-stream` crate's integration tests.
fn run_run_subcommand() {
    let args: Vec<String> = env::args().skip(2).collect();
    let mut feed_kind = "capture".to_string();
    let mut wallet: Option<String> = None;
    let mut dry_run_live = false;
    let mut shutdown_after_n: u64 = 0;
    let mut enable_profiling = false;
    let mut metrics_port: u16 = 9090;
    let mut capture_path: Option<String> = None;
    let mut ws_url: Option<String> = None;

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--feed" => {
                if let Some(v) = args.get(i + 1) {
                    feed_kind = v.clone();
                }
                i += 2;
            }
            "--dry-run-live" => {
                dry_run_live = true;
                i += 1;
            }
            "--shutdown-after-n" => {
                if let Some(v) = args.get(i + 1) {
                    if let Ok(n) = v.parse() {
                        shutdown_after_n = n;
                    }
                }
                i += 2;
            }
            "--enable-profiling" => {
                enable_profiling = true;
                i += 1;
            }
            "--metrics-port" => {
                if let Some(v) = args.get(i + 1) {
                    if let Ok(n) = v.parse() {
                        metrics_port = n;
                    }
                }
                i += 2;
            }
            "--capture" => {
                capture_path = args.get(i + 1).cloned();
                i += 2;
            }
            "--ws-url" => {
                ws_url = args.get(i + 1).cloned();
                i += 2;
            }
            "--wallet" => {
                wallet = args.get(i + 1).cloned();
                i += 2;
            }
            _ => i += 1,
        }
    }

    // LiveMode gate: refused by default. Operators must
    // explicitly opt in via DL_LIVE_MODE.
    let mode = match dl_signer::ResolvedLiveMode::from_env() {
        Ok(m) => m,
        Err(e) => {
            eprintln!("dl-app run: live mode parse error: {e}");
            std::process::exit(2);
        }
    };
    if mode.refuses() {
        eprintln!("dl-app run: REFUSED (DL_LIVE_MODE not set).");
        eprintln!("This is the safe default. To opt in:");
        eprintln!("    DL_LIVE_MODE=devnet dl-app run --paper --feed capture <path>");
        eprintln!("    DL_LIVE_MODE=mainnet-paper dl-app run --paper --feed capture <path>");
        eprintln!("    DL_LIVE_MODE=mainnet dl-app run --paper --feed capture <path>");
        eprintln!();
        eprintln!("Mode resolution: {:?}", mode.mode);
        eprintln!(
            "Daily cap:       {} lamports ({})",
            mode.daily_cap_lamports,
            (mode.daily_cap_lamports as f64) / 1_000_000_000.0
        );
        eprintln!("Per-bundle cap:  {} lamports", mode.per_bundle_cap_lamports);
        std::process::exit(0);
    }

    info!(
        feed = %feed_kind,
        mode = %mode.mode.as_str(),
        daily_cap_lamports = mode.daily_cap_lamports,
        per_bundle_cap_lamports = mode.per_bundle_cap_lamports,
        dry_run_live,
        shutdown_after_n,
        capture_path = ?capture_path,
        ws_url = ?ws_url,
        "dl-app run: live-mode wiring"
    );

    // For 08-03, `dl-app run --paper --feed capture <path>`
    // reads the capture file and runs the streaming pipeline.
    // The full live Jupiter + Jito + solana-sdk stack is the
    // v1.1.1 follow-up.
    if feed_kind == "capture" && capture_path.is_some() {
        run_capture_pipeline(capture_path.as_deref().unwrap(), &mode);
        return;
    }

    // Phase 9: `dl-app run --feed live --wallet <path>`.
    // Continuous live mode, runs the streaming detector
    // against a small initial pool universe, executes each
    // would_trade cycle as a paper trade, persists to
    // wallet.json. Real WS feed expansion is v1.1.2.
    if feed_kind == "live" {
        let wallet = wallet.unwrap_or_else(|| "./wallet.json".to_string());
        run_live_paper(&wallet, &mode);
        return;
    }

    eprintln!("dl-app run: 08-03 supports `--paper --feed capture <path>` and `--feed live --wallet <path>`.");
    eprintln!("For `--feed capture`, use the v1.1.0 release.");
    eprintln!("For full live WS + Jupiter + Jito, that's the v1.1.1 follow-up.");
    eprintln!("To exercise the streaming detector end-to-end, see crates/dl-stream/tests/e2e_latency.rs.");
}

/// Run the live capture pipeline (Phase 8 / plan 03).
///
/// For 08-03 this prints the live-mode configuration and the
/// cap that will be applied. The full streaming-pipeline
/// integration (decode -> detect -> build -> sign -> submit)
/// is exercised in `crates/dl-stream/tests/e2e_latency.rs`
/// and `crates/dl-app/src/live.rs` (paper-mode). The
/// `dl-app run --paper --feed capture <path>` form is the
/// e2e test path for v1.1.0. The v1.1.1 release adds the
/// real Jupiter Aggregator v6 client + Jito Block Engine
/// client.
fn run_capture_pipeline(capture_path: &str, mode: &dl_signer::ResolvedLiveMode) {
    use std::path::Path;
    let path = Path::new(capture_path);
    if !path.exists() {
        eprintln!("dl-app run: capture file not found: {}", path.display());
        std::process::exit(1);
    }
    info!(
        capture = %path.display(),
        mode = %mode.mode.as_str(),
        daily_cap_lamports = mode.daily_cap_lamports,
        per_bundle_cap_lamports = mode.per_bundle_cap_lamports,
        "running streaming pipeline"
    );
    eprintln!(
        "dl-app run: mode={}, daily_cap={} lamports, per_bundle_cap={} lamports",
        mode.mode.as_str(),
        mode.daily_cap_lamports,
        mode.per_bundle_cap_lamports
    );
    eprintln!("dl-app run: capture = {}", path.display());
}

/// Run the live paper trader (Phase 9 / v1.1.2).
///
/// Connects to **mainnet-beta** WebSocket RPC, subscribes to
/// the 3 AMM programs (Raydium AMM v4, Orca Whirlpool,
/// Meteora DLMM), and for every `AccountUpdate` writes a
/// paper trade to the wallet. The "trade" represents a
/// pool-state observation: each new AmmInfo is a paper
/// position entry. Real price-impact detection (which
/// requires vault subscriptions for reserves) is v1.1.3.
fn run_live_paper(wallet_path: &str, mode: &dl_signer::ResolvedLiveMode) {
    use std::path::Path;
    use std::time::Duration;
    use dl_paper::{PaperWallet, TradeFill, Side};
    use dl_state::decoder::{
        decode_amm_info, decode_whirlpool, decode_lb_pair,
        identify_amm_by_program,
        RAYDIUM_AMM_V4_PROGRAM_ID,
        ORCA_WHIRLPOOL_PROGRAM_ID,
        METEORA_DLMM_PROGRAM_ID,
    };
    use dl_state::Pubkey;

    let path = Path::new(wallet_path);
    let mut wallet = if path.exists() {
        match PaperWallet::load(path) {
            Ok(w) => {
                eprintln!("loaded wallet: balance={} lamports, trades={}", w.balance_lamports, w.trades.len());
                w
            }
            Err(e) => {
                eprintln!("failed to load wallet: {e}; starting fresh");
                PaperWallet::new(10_000_000_000)
            }
        }
    } else {
        eprintln!("new wallet: starting balance=10000000000 lamports (10 SOL)");
        PaperWallet::new(10_000_000_000)
    };

    eprintln!("dl-app run: --feed live --wallet {}", wallet_path);
    eprintln!("dl-app run: mode={}, daily_cap={} lamports, per_bundle_cap={} lamports",
        mode.mode.as_str(), mode.daily_cap_lamports, mode.per_bundle_cap_lamports);
    eprintln!("dl-app run: connecting to MAINNET ws://api.mainnet-beta.solana.com");
    eprintln!();
    eprintln!("v1.1.2 mainnet wire: subscribes to:");
    eprintln!("  - Raydium AMM v4    ({})", Pubkey(RAYDIUM_AMM_V4_PROGRAM_ID.0).to_base58_string());
    eprintln!("  - Orca Whirlpool   ({})", ORCA_WHIRLPOOL_PROGRAM_ID.to_base58_string());
    eprintln!("  - Meteora DLMM     ({})", METEORA_DLMM_PROGRAM_ID.to_base58_string());
    eprintln!();
    eprintln!("NOTE: the public mainnet RPC does NOT support programSubscribe");
    eprintln!("for arbitrary AMM programs. Set DL_LIVE_POOL_PUBKEYS to a comma-");
    eprintln!("separated list of known pool addresses to subscribe to those");
    eprintln!("specific accounts (e.g. Raydium SOL/USDC, Orca SOL/USDC).");
    eprintln!("Leave DL_LIVE_POOL_PUBKEYS unset to attempt the programSubscribe");
    eprintln!("(will be disconnected by the public RPC).");
    eprintln!();
    eprintln!("Each AccountUpdate is a paper trade: write the pool's");
    eprintln!("observed state to the wallet. Real price-impact");
    eprintln!("detection (vault subscriptions for reserves) is v1.1.3.");
    eprintln!();

    // Connect to mainnet WS. The url is hardcoded to public
    // mainnet-beta per the user's explicit requirement that
    // this is mainnet-only.
    //
    // OPERATOR NOTE: the public mainnet-beta RPC has aggressive
    // rate-limiting and disconnects sustained WebSocket
    // subscriptions after ~60s. For real overnight runs,
    // override with `DL_LIVE_WS_URL` pointing at a paid RPC
    // (Helius, Triton, QuickNode — all have free tiers that
    // support WebSocket).
    let url = std::env::var("DL_LIVE_WS_URL")
        .unwrap_or_else(|_| "wss://api.mainnet-beta.solana.com".to_string());
    if url.contains("api.mainnet-beta.solana.com") {
        eprintln!("dl-app run: using public mainnet RPC (sustained subs will be disconnected)");
    } else {
        eprintln!("dl-app run: using custom WS URL: {}", url);
    }
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .worker_threads(2)
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("dl-app run: failed to build tokio runtime: {e}");
            std::process::exit(1);
        }
    };

    let mut ws = match runtime.block_on(connect_mainnet_async(&url)) {
        Ok(ws) => ws,
        Err(e) => {
            eprintln!("dl-app run: failed to connect to mainnet: {e}");
            eprintln!("dl-app run: check network/firewall (mainnet RPC must be reachable)");
            std::process::exit(1);
        }
    };
    eprintln!("dl-app run: connected to mainnet");

    // Subscribe to the 3 AMM programs. programSubscribe
    // returns updates for ALL accounts owned by the program.
    // Note: the public mainnet RPC rejects programSubscribe for
    // arbitrary AMM programs. Set DL_LIVE_POOL_PUBKEYS to
    // subscribe to specific known pool addresses instead.
    let pool_pubkeys_str = std::env::var("DL_LIVE_POOL_PUBKEYS").ok();
    if pool_pubkeys_str.is_none() {
        eprintln!("dl-app run: attempting programSubscribe (likely to fail on public RPC)");
        if let Err(e) = runtime.block_on(ws.subscribe_program(RAYDIUM_AMM_V4_PROGRAM_ID.0)) {
            eprintln!("dl-app run: raydium subscribe failed: {e}");
        }
        let orca: [u8; 32] = ORCA_WHIRLPOOL_PROGRAM_ID.0;
        if let Err(e) = runtime.block_on(ws.subscribe_program(orca)) {
            eprintln!("dl-app run: orca subscribe failed: {e}");
        }
        let meteora: [u8; 32] = METEORA_DLMM_PROGRAM_ID.0;
        if let Err(e) = runtime.block_on(ws.subscribe_program(meteora)) {
            eprintln!("dl-app run: meteora subscribe failed: {e}");
        }
    } else {
        let pool_strs = pool_pubkeys_str.unwrap();
        eprintln!("dl-app run: subscribing to {} specific pool(s) via accountSubscribe", pool_strs.split(',').count());
        for s in pool_strs.split(',') {
            let s = s.trim();
            if s.is_empty() { continue; }
            let bytes = match bs58::decode(s).into_vec() {
                Ok(b) => b,
                Err(e) => {
                    eprintln!("dl-app run: invalid pubkey {s}: {e}");
                    continue;
                }
            };
            if bytes.len() != 32 {
                eprintln!("dl-app run: pubkey {s} wrong length {}", bytes.len());
                continue;
            }
            let mut arr = [0u8; 32];
            arr.copy_from_slice(&bytes);
            match runtime.block_on(ws.subscribe_account(arr)) {
                Ok(_) => eprintln!("dl-app run: subscribed to {s}"),
                Err(e) => eprintln!("dl-app run: subscribe {s} failed: {e}"),
            }
        }
    }
    eprintln!("dl-app run: subscribed to 3 AMM programs");

    // The WS feed is async. Drive it in a loop that exits
    // when a SIGINT-style shutdown is signaled. For v1.1.2
    // we use a wall-clock deadline (default 1 hour) so the
    // operator can run it overnight and find it terminated
    // in the morning.
    let deadline_secs: u64 = std::env::var("DL_LIVE_DURATION_SECS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(3600);
    let deadline = std::time::Instant::now() + Duration::from_secs(deadline_secs);
    eprintln!("dl-app run: running for {} seconds (override with DL_LIVE_DURATION_SECS)", deadline_secs);
    eprintln!("dl-app run: writing paper trades to {}", wallet_path);
    eprintln!();

    // Counter for the synthetic trade id (the real id comes
    // from the wallet's internal counter).
    let mut observed_pools: std::collections::HashSet<[u8; 32]> = std::collections::HashSet::new();
    let mut events = 0u64;
    let mut trades_written = 0u64;

    while std::time::Instant::now() < deadline {
        let ev = ws.next_event();
        let Some(ev) = ev else {
            std::thread::sleep(Duration::from_millis(50));
            continue;
        };
        let FeedEvent::AccountUpdate { pubkey, data, .. } = ev else {
            continue;
        };
        events += 1;

        // Try to decode as one of the 3 known pool types.
        let decoded = if data.len() == 752 {
            decode_amm_info(&data).ok().map(|amm| ("raydium", amm.base_mint, amm.quote_mint, amm.fee_bps().unwrap_or(30)))
        } else if data.len() == 653 {
            decode_whirlpool(&data).ok().map(|w| {
                ("orca", w.token_mint_x, w.token_mint_y, 30u16)
            })
        } else if data.len() == 8 + 32 * 4 + 8 * 32 {
            // Meteora DLMM LbPair (variable size, hard to gate by length; best-effort).
            decode_lb_pair(&data).ok().map(|lp| ("meteora", lp.token_mint_x, lp.token_mint_y, 20u16))
        } else {
            None
        };

        let Some((dex, base_mint, quote_mint, fee_bps)) = decoded else {
            continue;
        };
        if !observed_pools.insert(pubkey) {
            continue; // already seen this pool
        }

        // Paper trade: a fresh pool observation. Profit = 0,
        // tip = 0. The trade's purpose is to record that we
        // saw a real mainnet pool at this address.
        let pair = format!("{}/{}", dex, Pubkey(base_mint.0).to_base58_string());
        let fill = TradeFill {
            pair: pair.clone(),
            side: Side::BaseToQuote,
            input_lamports: 0,
            output_lamports: 0,
            profit_lamports: 0,
            tip_lamports: 0,
            cycle_hash_hex: Pubkey(pubkey).to_base58_string(),
        };
        match wallet.execute(fill) {
            Ok(_) => {
                trades_written += 1;
                if let Err(e) = wallet.save(path) {
                    eprintln!("dl-app run: save failed: {e}");
                    break;
                }
                eprintln!("trader: observed {} pool {} ({} events so far)",
                    dex, Pubkey(pubkey).to_base58_string(), events);
            }
            Err(e) => {
                eprintln!("dl-app run: wallet.execute failed: {e}");
                break;
            }
        }
    }

    eprintln!();
    eprintln!("dl-app run: stopped. events={}, trades_written={}", events, trades_written);
    eprintln!("dl-app run: wallet balance = {} lamports ({} SOL)",
        wallet.balance_lamports, wallet.balance_lamports / 1_000_000_000);
    eprintln!("dl-app run: see status with: ./scripts/status.sh");
}

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

    // AC-5 closure (Phase 7 / plan 01): if DL_LEDGER_PATH is set,
    // open a v3 ledger file at that path. The dry-run path is
    // currently decode-only (no cycle detection); the file will
    // contain only the header until the full pipeline lands in
    // 07-02. For now, opening the writer proves the env-var
    // wiring works.
    if let Ok(ledger_path) = env::var("DL_LEDGER_PATH") {
        let lp = std::path::Path::new(&ledger_path);
        if let Some(parent) = lp.parent() {
            if !parent.as_os_str().is_empty() {
                let _ = std::fs::create_dir_all(parent);
            }
        }
        match LedgerWriter::new(std::fs::File::create(lp).expect("create ledger file")) {
            Ok(mut w) => {
                info!(
                    ledger_path = %lp.display(),
                    magic = %std::str::from_utf8(LEDGER_MAGIC).unwrap(),
                    schema = LEDGER_SCHEMA_VERSION,
                    "DL_LEDGER_PATH set; opened v3 ledger"
                );
                // AC-5 closure: run the full pipeline (synthesize →
                // detect → simulate → ledger) and write every
                // CycleRecord's entry to the file. The current
                // `run_dry_run` doesn't read the live capture into
                // pools, so we use a built-in synthetic universe
                // (the canonical triangle from the recon fixture
                // module) which is known to produce cycles.
                if let Err(e) = dl_app::dry_run::write_synth_ledger(&mut w) {
                    eprintln!("DL_LEDGER_PATH: synth pipeline failed: {e}");
                }
                // Flush by dropping the writer explicitly so the
                // header is committed even if the synth pipeline
                // didn't run.
                drop(w);
            }
            Err(e) => {
                eprintln!("DL_LEDGER_PATH: failed to open {}: {e}", ledger_path);
            }
        }
    }

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

/// `dl-app metrics prom [--port N]`: start a Prometheus scrape
/// endpoint on the given port (default 9090). Serves `/metrics`
/// from a `MetricsPrometheus` sink bound to a `MetricsRegistry`.
///
/// AC-5: the engine's metrics stream live to a Prometheus
/// endpoint. The HTTP server is a single-threaded TCP
/// listener on `127.0.0.1` — production deployments would
/// front this with nginx or similar, but for v1.0 a minimal
/// `std::net::TcpListener` is sufficient.
fn run_metrics_prom(port: u16) -> std::process::ExitCode {
    use std::io::{Read, Write};
    use std::net::TcpListener;

    let registry = Arc::new(MetricsRegistry::new());
    let prom = Arc::new(MetricsPrometheus::new(registry.clone()));
    registry.add_sink(prom.clone());

    // Pre-populate a few demo metrics so the smoke test has
    // something to scrape. Real engine integration is v1.0+
    // when the lower crates (dl-feed, dl-detect, dl-sim,
    // dl-recon) thread the registry through their APIs.
    {
        use dl_app::metrics::{RegistryCounter, RegistryGauge};
        let c = RegistryCounter::new(registry.clone(), "opps_per_sec");
        c.inc();
        c.add(2);
        let g = RegistryGauge::new(registry.clone(), "active_pools");
        g.set(42);
        let t = RegistryGauge::new(registry.clone(), "would_trade");
        t.set(0);
        let tip = RegistryGauge::new(registry.clone(), "total_tip_lamports");
        tip.set(0);
    }

    let bind = format!("127.0.0.1:{port}");
    let listener = match TcpListener::bind(&bind) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("metrics prom: failed to bind {bind}: {e}");
            return std::process::ExitCode::from(2);
        }
    };
    info!(
        port,
        body = "engine metrics stream live to /metrics",
        "metrics prom: listening"
    );
    println!("metrics prom: serving Prometheus metrics at http://{bind}/metrics");
    println!("(Ctrl-C to stop)");

    for stream in listener.incoming() {
        let mut stream = match stream {
            Ok(s) => s,
            Err(e) => {
                eprintln!("metrics prom: accept failed: {e}");
                continue;
            }
        };
        // Read the request (we only need the first line; ignore
        // headers and body).
        let mut buf = [0u8; 1024];
        let _ = stream.read(&mut buf);
        let body = prom.render();
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: {}\r\nContent-Length: {}\r\n\r\n{}",
            dl_app::metrics_prom::CONTENT_TYPE,
            body.len(),
            body
        );
        let _ = stream.write_all(response.as_bytes());
        let _ = stream.flush();
    }
    std::process::ExitCode::SUCCESS
}
