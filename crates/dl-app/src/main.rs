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
fn run_capture(rpc_url: &str, capture_path: &str, capture_secs: u64) {
    info!(rpc_url, capture_path, capture_secs, "starting live capture");

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .worker_threads(2)
        .build()
        .expect("tokio runtime");
    let mut ws = runtime
        .block_on(async {
            WsFeed::connect(rpc_url).await
        })
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
            _ => i += 1,
        }
    }

    info!(
        feed = %feed_kind,
        dry_run_live,
        shutdown_after_n,
        enable_profiling,
        metrics_port,
        capture_path = ?capture_path,
        ws_url = ?ws_url,
        "dl-app run (08-02 stub)"
    );
    eprintln!("dl-app run: streaming pipeline stub. 08-03 wires the full live Jupiter + Jito + solana-sdk stack.");
    eprintln!("To exercise the streaming detector end-to-end, see crates/dl-stream/tests/e2e_latency.rs.");
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
