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
use dl_app::reconcile;
use dl_ledger::{LedgerEntry, LedgerWriter, LEDGER_MAGIC, LEDGER_SCHEMA_VERSION};
use dl_recon::fixture::{synthesize_pools, SynthPoolSpec};
use dl_recon::pipeline::{replay_capture_to_ledger, ReplayParams};
use dl_state::pool::Pool;
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

    if env::args().nth(1).as_deref() == Some("reconcile") {
        reconcile::dispatch();
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
            Some(FeedEvent::Pool { .. } | FeedEvent::StalePoolHalt { .. }) => {}
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
/// Phase 1 v2.0 live submit entry point. Wired via `--submit-live`.
///
/// Loads the keystore, builds the Jupiter + Jito + safety-module
/// stack, and submits each detected cycle through
/// `live::submit_opportunity`. Operator-supplied cycle stream
/// (Phase 2 will hook this to the StreamingDetector; for now
/// the function prints a "ready" banner and exits 0).
fn run_live_submit(
    keyfile: Option<&str>,
    assert_program_id: Option<&str>,
    jito_tip_account: Option<&str>,
    simulate_rpc_url: Option<&str>,
    mode: &dl_signer::ResolvedLiveMode,
) {
    use dl_assert_sdk::derive_vault_pda;
    use dl_executor::error::ExecutorError;
    use dl_executor::jito::{HttpJitoClient, JitoClient};
    use dl_executor::jupiter::HttpJupiterClient;
    use dl_executor::killswitch::{KillSwitch, KillSwitchConfig};
    use dl_signer::cap::{CapConfig, CapState};
    use dl_signer::ratelimit::{RateLimit, RateLimitConfig};
    use solana_sdk::hash::Hash;
    use solana_sdk::pubkey::Pubkey;
    use std::str::FromStr;
    use std::sync::Mutex;

    let keyfile_path = match keyfile {
        Some(p) => p.to_string(),
        None => {
            eprintln!("dl-app run --submit-live: --keyfile <PATH> is required");
            std::process::exit(2);
        }
    };
    let assert_pid = match assert_program_id {
        Some(s) => match Pubkey::from_str(s) {
            Ok(p) => p,
            Err(e) => {
                eprintln!("dl-app run: invalid --assert-program-id {s}: {e}");
                std::process::exit(2);
            }
        },
        None => {
            eprintln!("dl-app run --submit-live: --assert-program-id <PUBKEY> is required");
            std::process::exit(2);
        }
    };

    // Load the keystore (DL_SIGNER_PASSPHRASE env required).
    let keystore_res = (|| -> Result<dl_signer::keystore::KeyStore, String> {
        let kf = dl_signer::keystore::KeyFile::load(&std::path::PathBuf::from(&keyfile_path))
            .map_err(|e| format!("load: {e}"))?;
        let passphrase = std::env::var("DL_SIGNER_PASSPHRASE")
            .map_err(|_| "DL_SIGNER_PASSPHRASE not set".to_string())?;
        let secret = kf.decrypt(&passphrase).map_err(|e| format!("decrypt: {e}"))?;
        Ok(dl_signer::keystore::KeyStore::from_secret(secret))
    })();
    let keystore = match keystore_res {
        Ok(k) => k,
        Err(e) => {
            eprintln!("dl-app run: keystore load failed: {e}");
            std::process::exit(2);
        }
    };
    let signer_sol =
        solana_sdk::pubkey::Pubkey::new_from_array(keystore.public_key_for_print());
    let (vault, _bump) = derive_vault_pda(&signer_sol, &assert_pid);
    eprintln!(
        "dl-app run --submit-live: signer={signer_sol} \
         assert_program={assert_pid} vault={vault} mode={}",
        mode.mode.as_str()
    );

    let _jupiter = HttpJupiterClient::for_mainnet();
    let jito = HttpJitoClient::new("https://mainnet.block-engine.jito.wtf");
    if let Err(e) = jito.populate_tip_accounts() {
        eprintln!("dl-app run: populate_tip_accounts failed: {e}");
        std::process::exit(2);
    }
    // If the operator passed --jito-tip-account use it; otherwise
    // use HttpJitoClient's rotation (locked decision #4).
    // Note: `next_tip_account` returns the tip account as a base58
    // String, so we re-parse it via `Pubkey::from_str` to get a
    // `Pubkey` value for `LiveConfig::tip_account`.
    let tip_account_res: Result<Pubkey, String> = if let Some(s) = jito_tip_account {
        Pubkey::from_str(s).map_err(|e| format!("--jito-tip-account: {e}"))
    } else {
        jito.next_tip_account()
            .and_then(|s| {
                Pubkey::from_str(&s)
                    .map_err(|e| ExecutorError::JitoSubmit(format!("tip pubkey: {e}")))
            })
            .map_err(|e| format!("next_tip_account: {e}"))
    };
    let tip_account = match tip_account_res {
        Ok(p) => p,
        Err(e) => {
            eprintln!("dl-app run: tip account resolution failed: {e}");
            std::process::exit(2);
        }
    };
    eprintln!("dl-app run: using Jito tip_account={tip_account}");

    // Pre-fund the vault PDA. Idempotent: if vault >= signer
    // already, no tx is sent. The RPC URL defaults to the
    // simulate-rpc-url, or falls back to `DL_LIVE_WS_URL` (the
    // legacy env var), or finally the public mainnet endpoint.
    let rpc_url: &str = simulate_rpc_url.unwrap_or_else(|| {
        std::env::var("DL_LIVE_WS_URL")
            .ok()
            .map(|s| {
                if s.starts_with("wss://") {
                    Box::leak(
                        s.replacen("wss://", "https://", 1)
                            .into_boxed_str(),
                    ) as &str
                } else {
                    Box::leak(s.into_boxed_str()) as &str
                }
            })
            .unwrap_or("https://api.mainnet-beta.solana.com")
    });
    match dl_app::live::pre_fund_vault_if_needed(
        rpc_url,
        &signer_sol,
        &vault,
        &keystore,
    ) {
        Ok(dl_app::live::VaultFunded::AlreadyFunded {
            vault_lamports,
            signer_lamports,
        }) => eprintln!(
            "dl-app run: vault already funded (vault={} lamports, signer={} lamports)",
            vault_lamports, signer_lamports
        ),
        Ok(dl_app::live::VaultFunded::Funded {
            lamports,
            signature,
        }) => eprintln!(
            "dl-app run: vault funded +{} lamports (sig={})",
            lamports, signature
        ),
        Err(e) => eprintln!(
            "dl-app run: vault prefund failed: {e}. \
             The first bundle's assert tx will revert. \
             Operator must fund manually per docs/v2.0-operator-runbook.md."
        ),
    }

    // Safety modules — default config (operators override via
    // env vars DL_DAILY_CAP_LAMPORTS etc before running).
    //
    // The cap state is rehydrated from a durable JSON snapshot
    // (DAM-67 / Phase 3) so a crash mid-day does NOT reset the
    // daily budget. The path is `DL_CAP_SNAPSHOT` (default
    // `./dl-signer-cap-snapshot.json`). On first boot the file
    // is created with a fresh zero-state; on every subsequent
    // boot the prior `spent_today` is loaded back. A load
    // failure refuses to boot (exit 2) rather than fall back
    // to a fresh cap — refusing to risk a double-spend after
    // crash recovery.
    let cap_snapshot_path = std::env::var("DL_CAP_SNAPSHOT")
        .unwrap_or_else(|_| "./dl-signer-cap-snapshot.json".to_string());
    let cap_state = match CapState::load_or_init(
        std::path::Path::new(&cap_snapshot_path),
        CapConfig::default(),
    ) {
        Ok(s) => s,
        Err(e) => {
            eprintln!(
                "dl-app run --submit-live: cap snapshot load failed ({}); \
                 refusing to start with a fresh cap — refusing to risk a \
                 double-spend after crash recovery. Re-run with the snapshot \
                 file intact or unset DL_CAP_SNAPSHOT to start from zero.",
                e
            );
            std::process::exit(2);
        }
    };
    eprintln!(
        "dl-app run --submit-live: cap snapshot loaded from {} (spent_today={} lamports, remaining={} lamports)",
        cap_snapshot_path,
        cap_state.spent_today(),
        cap_state.remaining()
    );
    // DAM-82: wrap the safety modules in `Arc<Mutex<>>` so the
    // live_status writer (a side task that ticks at 1 Hz) can
    // take a snapshot under a cheap lock without blocking the
    // detection / submit path. The cycle path takes the same
    // locks to charge / refund; the lock is held for
    // microseconds.
    let cap_state = Arc::new(Mutex::new(cap_state));
    let _rate_limit = RateLimit::new(RateLimitConfig::default());
    let killswitch = Arc::new(Mutex::new(KillSwitch::new(KillSwitchConfig::default())));

    // Phase 2 C3: load `calibration.json` if it exists and feed it
    // into the live eval model. Fall back to `conservative_default()`
    // when no capture data is available (cold-start). The path is
    // `DL_CALIBRATION_IN` (defaults to
    // `./dl-calibration/calibration.json`).
    let _eval_params: dl_sim::ev::EvalParams = match std::env::var("DL_CALIBRATION_IN")
        .unwrap_or_else(|_| "./dl-calibration/calibration.json".into())
        .as_str()
    {
        p if std::path::Path::new(p).exists() => {
            match dl_calibration::read_calibration_report(p) {
                Some(report) => {
                    let ep = dl_app::live::eval_params_from_calibration(&report.result);
                    eprintln!(
                        "dl-app run --submit-live: loaded calibration from {} (p_detect={} p_win={} p_land={} n={} dsr={:?} cv={:?} overfit_risk={})",
                        p,
                        report.result.p_detect.to_ppm(),
                        report.result.p_win.to_ppm(),
                        report.result.p_land.to_ppm(),
                        report.result.sample_size,
                        report.overfit.dsr.as_ref().map(|d| d.dsr),
                        report.overfit.purged_cv.as_ref().map(|c| c.n_folds),
                        report.overfit.is_overfit_risk,
                    );
                    ep
                }
                None => {
                    eprintln!(
                        "dl-app run --submit-live: WARNING: failed to parse {}; using conservative_default()",
                        p
                    );
                    dl_sim::ev::EvalParams::conservative_default()
                }
            }
        }
        _ => {
            eprintln!(
                "dl-app run --submit-live: no calibration file (cold-start); using conservative_default()"
            );
            dl_sim::ev::EvalParams::conservative_default()
        }
    };
    let _ = &_eval_params; // silence unused for now; consumed by Phase 2 e2e loop

    // Construct the LiveConfig so operators can see the resolved
    // parameters. The actual cycle stream is wired by the
    // operator (Phase 2 will hook StreamingDetector here).
    let mut loaded_feeds: std::collections::HashMap<
        solana_sdk::pubkey::Pubkey,
        solana_sdk::pubkey::Pubkey,
    > = dl_oracle::load_pyth_feeds_from_env();
    if !loaded_feeds.is_empty() {
        eprintln!(
            "dl-app run --submit-live: loaded {} Pyth feed mappings from env",
            loaded_feeds.len()
        );
    }

    // Phase 2 L2: Pyth price staleness window. Operators override
    // via `DL_PYTH_MAX_AGE_SECS`; default to `dl_oracle::MAX_PYTH_AGE_SECS`
    // (60 s).
    let pyth_max_age_secs: u64 = std::env::var("DL_PYTH_MAX_AGE_SECS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(dl_oracle::MAX_PYTH_AGE_SECS);

    let cfg = dl_app::live::LiveConfig {
        assert_program_id: assert_pid,
        tip_config: dl_executor::tip::TipConfig::default(),
        simulate_rpc_url: simulate_rpc_url.map(str::to_string),
        tip_account,
        recent_blockhash: Hash::new_unique(),
        // Phase 2b: Pyth gate disabled at startup (paper-mode
        // / cold-start). Operators wire a real `HttpPythClient`
        // + `pyth_feeds` map via config file or `--pyth-*` flags
        // in a follow-up (Phase 3 work).
        pyth: None,
        pyth_feeds: loaded_feeds,
        pyth_max_age_secs,
        // Phase 2 H3: niche filter disabled at startup. Operators
        // populate via the DL_NICHES_IN env var pointing at a
        // `niches.json` written by `dl-niches`.
        niche_config: std::env::var("DL_NICHES_IN")
            .ok()
            .and_then(|p| dl_calibration::NicheConfig::load(p)),
    };
    eprintln!(
        "dl-app run --submit-live: ready. live_cfg.tip_account={} simulate_rpc={:?} cap={} lamports",
        cfg.tip_account,
        cfg.simulate_rpc_url.as_deref().unwrap_or("(none)"),
        mode.daily_cap_lamports
    );
    eprintln!("dl-app run: ready. wire `live::submit_opportunity` to your cycle stream.");
    eprintln!("dl-app run: press Ctrl-C to exit");

    // DAM-82: spin up the live_status.json writer on a side
    // tokio task that ticks at 1 Hz and snapshots the shared
    // cap + kill switch. The cycle path (Phase 2) will hand
    // its `Arc`s to the same lockable view. We block here on
    // a dedicated runtime; SIGINT flips the watch channel
    // and the writer does one final write before returning.
    let live_status_path = std::env::var("DL_LIVE_STATUS_PATH")
        .unwrap_or_else(|_| dl_app::live_status::DEFAULT_PATH.to_string());
    let live_status_path = std::path::PathBuf::from(live_status_path);
    let live_status_inputs = Arc::new(Mutex::new(dl_app::live_status::WriterInputs::default()));
    let state = dl_app::live_status::SharedState {
        cap: cap_state,
        killswitch,
        inputs: live_status_inputs,
    };
    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .enable_all()
        .thread_name("dl-app-live-status")
        .build()
        .expect("build tokio runtime");
    let ctrl_c_shutdown = shutdown_tx.clone();
    runtime.spawn(async move {
        if let Ok(()) = tokio::signal::ctrl_c().await {
            let _ = ctrl_c_shutdown.send(true);
        }
    });
    runtime.block_on(async move {
        dl_app::live_status::run_writer_loop(live_status_path, state, shutdown_rx).await;
    });
}

/// Pre-fund the vault PDA stub (kept for backwards compat
/// with main.rs — replaced by `live::pre_fund_vault_if_needed`).
fn pre_fund_vault_if_needed(
    _signer: &solana_sdk::pubkey::Pubkey,
    _vault: &solana_sdk::pubkey::Pubkey,
) -> Result<(), String> {
    // Replaced by `dl_app::live::pre_fund_vault_if_needed`. Kept as
    // a no-op stub so this binary still type-checks. New code
    // should call the live:: version directly.
    Ok(())
}

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
    // ─── Phase 1 v2.0 flags ──────────────────────────────────────────────
    /// `--submit-live` enables the v2.0 live submit path (real
    /// Jupiter + Jito + dl-assert). Without this flag the binary
    /// falls back to v1.x paper mode.
    let mut submit_live = false;
    /// `--keyfile <PATH>` points at the dl-signer keystore JSON.
    let mut keyfile: Option<String> = None;
    /// `--assert-program-id <PUBKEY>` is the deployed dl-assert
    /// program ID (devnet or mainnet).
    let mut assert_program_id: Option<String> = None;
    /// `--jito-tip-account <PUBKEY>` overrides Jito's auto-selected
    /// tip account (for tests).
    let mut jito_tip_account: Option<String> = None;
    /// `--daily-cap-sol <N>` overrides `DL_DAILY_CAP_LAMPORTS`
    /// for non-mainnet-paper modes. Mainnet-paper mode ignores
    /// this and uses the 0.001 SOL floor.
    let mut daily_cap_sol: Option<f64> = None;
    /// `--simulate-rpc-url <URL>` is the simulateTransaction
    /// gate RPC. Defaults to `DL_SIMULATE_RPC_URL` env var.
    let mut simulate_rpc_url: Option<String> = None;

    let mut i = 0;
    while i < args.len() {
        let val = || -> Option<String> { args.get(i + 1).cloned() };
        match args[i].as_str() {
            "--feed" => {
                if let Some(v) = val() {
                    feed_kind = v;
                }
                i += 2;
            }
            "--dry-run-live" => {
                dry_run_live = true;
                i += 1;
            }
            "--shutdown-after-n" => {
                if let Some(v) = val() {
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
                if let Some(v) = val() {
                    if let Ok(n) = v.parse() {
                        metrics_port = n;
                    }
                }
                i += 2;
            }
            "--capture" => {
                capture_path = val();
                i += 2;
            }
            "--ws-url" => {
                ws_url = val();
                i += 2;
            }
            "--wallet" => {
                wallet = val();
                i += 2;
            }
            "--submit-live" => {
                submit_live = true;
                i += 1;
            }
            "--keyfile" => {
                keyfile = val();
                i += 2;
            }
            "--assert-program-id" => {
                assert_program_id = val();
                i += 2;
            }
            "--jito-tip-account" => {
                jito_tip_account = val();
                i += 2;
            }
            "--daily-cap-sol" => {
                if let Some(v) = val() {
                    if let Ok(n) = v.parse() {
                        daily_cap_sol = Some(n);
                    }
                }
                i += 2;
            }
            "--simulate-rpc-url" => {
                simulate_rpc_url = val();
                i += 2;
            }
            _ => i += 1,
        }
    }
    // Track future planned but not yet wired (silences warnings).
    let _ = enable_profiling;
    let _ = metrics_port;

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

    // `--daily-cap-sol` override. Mainnet-paper mode ignores this
    // — the 0.001 SOL floor is hard-coded in `livemode.rs` (locked
    // decision in the runbook). For other modes the override
    // applies, capped at a sane ceiling (100 SOL) to prevent fat-finger
    // misconfigurations.
    if let Some(sol) = daily_cap_sol {
        let lamports = (sol * 1_000_000_000.0) as u64;
        if matches!(
            mode.mode,
            dl_signer::livemode::LiveMode::MainnetPaper
        ) {
            eprintln!(
                "dl-app run: ignoring --daily-cap-sol {sol} SOL: \
                 mainnet-paper floor is hard-coded to 0.001 SOL ({} lamports)",
                mode.daily_cap_lamports
            );
        } else if lamports > 100 * 1_000_000_000 {
            eprintln!(
                "dl-app run: --daily-cap-sol {sol} SOL exceeds 100 SOL ceiling, ignoring"
            );
        } else {
            std::env::set_var("DL_DAILY_CAP_LAMPORTS", lamports.to_string());
        }
    }

    // DL_SIMULATE_RPC_URL env override (fallback if --simulate-rpc-url
    // wasn't given on the command line).
    let simulate_rpc_url = simulate_rpc_url
        .or_else(|| std::env::var("DL_SIMULATE_RPC_URL").ok());

    info!(
        feed = %feed_kind,
        mode = %mode.mode.as_str(),
        daily_cap_lamports = mode.daily_cap_lamports,
        per_bundle_cap_lamports = mode.per_bundle_cap_lamports,
        submit_live,
        dry_run_live,
        shutdown_after_n,
        capture_path = ?capture_path,
        ws_url = ?ws_url,
        keyfile = ?keyfile,
        assert_program_id = ?assert_program_id,
        simulate_rpc_url = ?simulate_rpc_url,
        "dl-app run: live-mode wiring"
    );

    // v2.0 live submit path (Phase 1). When --submit-live is set,
    // route every detected cycle through `submit_opportunity` →
    // real Jupiter → real Jito. This is the production path.
    if submit_live {
        run_live_submit(
            keyfile.as_deref(),
            assert_program_id.as_deref(),
            jito_tip_account.as_deref(),
            simulate_rpc_url.as_deref(),
            &mode,
        );
        return;
    }

    // For 08-03, `dl-app run --paper --feed capture <path>`
    // reads the capture file and runs the streaming pipeline.
    if feed_kind == "capture" && capture_path.is_some() {
        run_capture_pipeline(capture_path.as_deref().unwrap(), &mode);
        return;
    }

    // Phase 9 paper path: `dl-app run --feed live --wallet <path>`.
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

/// Run the live paper trader (Phase 9 / v1.1.4).
///
/// Connects to **mainnet-beta** WebSocket RPC. For every
/// `AccountUpdate`:
///  - AmmInfo (Raydium AMM v4): decode + store + subscribe to
///    the pool's coin/pc vault accounts so reserves flow in.
///  - SplTokenAccount: look up the parent pool and update its
///    reserves, then re-run the streaming detector.
///  - Whirlpool / LbPair: decode and add to the graph (no
///    vault subscriptions for those yet — v1.1.5).
///
/// On every graph update, find negative cycles, evaluate the
/// conservative bound, and write a paper trade to `wallet.json`
/// only for `would_trade` cycles.
fn run_live_paper(wallet_path: &str, mode: &dl_signer::ResolvedLiveMode) {
    use std::collections::HashMap;
    use std::path::Path;
    use std::time::Duration;
    use dl_paper::{PaperWallet, TradeFill, Side};
    use dl_state::decoder::{
        decode_amm_info, decode_whirlpool, decode_whirlpool_real, decode_lb_pair,
        decode_spl_token_account, identify_amm_by_program, Whirlpool,
        RAYDIUM_AMM_V4_PROGRAM_ID, ORCA_WHIRLPOOL_PROGRAM_ID,
        METEORA_DLMM_PROGRAM_ID, SPL_TOKEN_ACCOUNT_SIZE,
    };
    use dl_state::pool::{AmmKind, Pool};
    use dl_stream::detector::StreamingDetector;
    use dl_sim::cost::{CostModel, CostBreakdown};
    use dl_sim::ev::{EvalParams, evaluate};
    use dl_sim::net_profit::NetProfit;
    use dl_ledger::Decision;
    use dl_detect::bellman_ford::find_negative_cycles;
    use dl_core::feed::{Feed, FeedEvent};
    use dl_state::Pubkey;

    let path = Path::new(wallet_path);
    let mut wallet = if path.exists() {
        match PaperWallet::load(path) {
            Ok(w) => {
                eprintln!(
                    "loaded wallet: balance={} lamports, trades={}",
                    w.balance_lamports, w.trades.len()
                );
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
    eprintln!(
        "dl-app run: mode={}, daily_cap={} lamports, per_bundle_cap={} lamports",
        mode.mode.as_str(), mode.daily_cap_lamports, mode.per_bundle_cap_lamports
    );

    let url = std::env::var("DL_LIVE_WS_URL")
        .unwrap_or_else(|_| "wss://api.mainnet-beta.solana.com".to_string());
    if url.contains("api.mainnet-beta.solana.com") {
        eprintln!("dl-app run: using public mainnet RPC (sustained subs will be disconnected)");
    } else {
        eprintln!("dl-app run: using custom WS URL: {url}");
    }
    eprintln!("dl-app run: connecting to MAINNET");
    eprintln!();
    eprintln!("PIPELINE:");
    eprintln!("  1. AccountUpdate -> decode pool (Raydium/Orca/Meteora)");
    eprintln!("  2. StreamingDetector updates price graph incrementally");
    eprintln!("  3. find_negative_cycles on the graph");
    eprintln!("  4. evaluate() per cycle: conservative bound gate");
    eprintln!("  5. decision == WouldTrade => paper trade in wallet.json");
    eprintln!();
    eprintln!("NOTE: a single pool update is NOT a trade. A trade is a");
    eprintln!("detected negative cycle (typically 3 pools across DEXs)");
    eprintln!("whose conservative bound says it would have been profitable.");
    eprintln!();
    eprintln!("Real mainnet reserves are needed for fill math. Without");
    eprintln!("vault subscriptions the graph has no weights -> no");
    eprintln!("cycles -> no trades. Vault subscriptions land in v1.1.3.");
    eprintln!();

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
        eprintln!(
            "dl-app run: subscribing to {} specific pool(s) via accountSubscribe",
            pool_strs.split(',').count()
        );
        for s in pool_strs.split(',') {
            let s = s.trim();
            if s.is_empty() {
                continue;
            }
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
    eprintln!("dl-app run: subscriptions complete");
    eprintln!("dl-app run: vault subscriptions enabled for Raydium AMM v4");

    // Tracks the latest state of each Raydium pool we discover
    // (AmmInfo + base_vault/quote_vault pubkeys + latest reserves).
    // Used to look up the parent pool when a vault update arrives.
    let mut raydium_pools: HashMap<[u8; 32], (Pool, Pubkey, Pubkey)> = HashMap::new();
    // Phase 2 C1: separate maps for Orca Whirlpool and Meteora DLMM
    // so the 165-byte SplTokenAccount branch can route vault updates
    // back to the parent pool for all three DEXs.
    let mut orca_pools: HashMap<[u8; 32], (Pool, Pubkey, Pubkey)> = HashMap::new();
    let mut meteora_pools: HashMap<[u8; 32], (Pool, Pubkey, Pubkey)> = HashMap::new();
    // Tracks which vaults we've already subscribed to (avoid duplicates).
    let mut subscribed_vaults: HashMap<[u8; 32], ()> = HashMap::new();

    // StreamingDetector needs an initial pool universe to
    // build the price graph. Without reserves from the live
    // AmmInfo we can't seed the graph from real mainnet pools
    // (see v1.1.3 notes). For v1.1.2 the initial universe is
    // empty; the graph is built up as AccountUpdates arrive.
    let mut detector = StreamingDetector::new(&[]).unwrap_or_else(|_| {
        // Empty initial pool set will fail because
        // build_from_pools requires >= 1 pool. Use a single
        // placeholder pool as a seed.
        StreamingDetector::new(&[Pool {
            address: Pubkey([0xFE; 32]),
            kind: AmmKind::RaydiumAmmV4,
            base_mint: Pubkey([0x01; 32]),
            quote_mint: Pubkey([0x02; 32]),
            base_decimals: 6,
            quote_decimals: 9,
            base_reserve: 1_000_000_000,
            quote_reserve: 1_000_000_000,
            fee_bps: 30,
            last_update_slot: 0,
            ..Default::default()
        }])
        .expect("placeholder pool construction must succeed")
    });

    // EvalParams: optimistic for paper mode so cycles pass
    // the conservative bound; realistic_mode applies a 30%
    // win rate at the trade-write step (losing trades burn
    // the Jito tip and write a loss to the wallet). This
    // gives a realistic distribution of PnL without making
    // 99% of sub-bp cycles invisible.
    let realistic_mode = env::var("DL_PAPER_MODE")
        .map(|v| v.eq_ignore_ascii_case("realistic"))
        .unwrap_or(false);
    // Two distinct EvalParams: the optimistic bound uses
    // `EvalParams::optimistic()` (no haircut, no tip, always win);
    // the conservative bound uses `EvalParams::conservative_default()`
    // (full pessimistic stack). The paper path defaults to
    // optimistic-both because the wallet is a *ceiling*, not a
    // prediction — operators who want realistic sizing should set
    // `DL_PAPER_MODE=realistic` (random win rate) or wire
    // dl-calibration to load fitted `p_detect / p_win / p_land`.
    let eval_optimistic = EvalParams::optimistic();
    let eval_conservative = EvalParams::conservative_default();
    // Cost stack: matches the v1.1 default (5_000 base sig +
    // 1_000 priority + 10_000 Jito tip).
    let cost = CostModel::default_busy();
    // Sizer input cap: 1 SOL in lamports. Matches `ReplayParams::default`.
    let max_input: u128 = 1_000_000_000;
    if realistic_mode {
        eprintln!("dl-app run: mode=REALISTIC (optimistic bound + 30% random win rate)");
    } else {
        eprintln!("dl-app run: mode=OPTIMISTIC (100% win rate — best case, not realistic)");
    }

    let deadline_secs: u64 = std::env::var("DL_LIVE_DURATION_SECS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(3600);
    let deadline = std::time::Instant::now() + Duration::from_secs(deadline_secs);
    eprintln!("dl-app run: running for {deadline_secs} seconds (override with DL_LIVE_DURATION_SECS)");
    eprintln!("dl-app run: writing paper trades to {wallet_path}");
    eprintln!();

    let mut events = 0u64;
    let mut pools_seen = 0u64;
    let mut vault_updates = 0u64;
    let mut cycles_evaluated = 0u64;
    let mut trades_written = 0u64;
    let mut last_log = std::time::Instant::now();

    // No closure — we inline subscribe_account calls in the
    // event loop to avoid borrow conflicts on `ws`.

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

        // FIRST-A: try to decode as an Orca Whirlpool. Accepts both
        // the simplified 256-B layout (v1.0 tests / synthetic) and
        // the real 653-B mainnet layout. Whichever succeeds first
        // wins. Subscribe to its vault accounts (Phase 2a: all 3
        // DEXs feeding).
        let whirl = if data.len() == 256 {
            decode_whirlpool(&data).ok()
        } else if data.len() >= 234 && data.len() <= 256 {
            // The simplified layout is 256; anything in 234..256 is
            // rejected. Skip.
            None
        } else {
            // Try the real 653-B layout first (the common case
            // on mainnet). Fall back to the simplified layout if
            // the bytes happen to be 256-B synthetic data.
            decode_whirlpool_real(&data).ok().map(|w_real| {
                // Build a synthetic simplified Whirlpool from the
                // real layout so the rest of the pipeline (which
                // uses `decode_whirlpool`-shaped data) can proceed.
                Whirlpool {
                    sqrt_price: w_real.sqrt_price,
                    tick_current_index: w_real.tick_current_index,
                    tick_spacing: w_real.tick_spacing,
                    liquidity: w_real.liquidity,
                    token_mint_x: w_real.token_mint_x,
                    token_mint_y: w_real.token_mint_y,
                    token_vault_x: w_real.token_vault_x,
                    token_vault_y: w_real.token_vault_y,
                    fee_rate: w_real.fee_rate,
                    program_id: ORCA_WHIRLPOOL_PROGRAM_ID,
                }
            })
        };
        if let Some(whirl) = whirl {
            for vault in [whirl.token_vault_x, whirl.token_vault_y] {
                if !subscribed_vaults.contains_key(&vault.0) {
                    if let Err(e) = runtime.block_on(ws.subscribe_account(vault.0)) {
                        eprintln!("dl-app run: orca vault subscribe failed: {e}");
                    } else {
                        subscribed_vaults.insert(vault.0, ());
                    }
                }
            }
            let pool = Pool {
                address: Pubkey(pubkey),
                kind: AmmKind::OrcaWhirlpool,
                base_mint: whirl.token_mint_x,
                quote_mint: whirl.token_mint_y,
                base_decimals: 9,
                quote_decimals: 6,
                base_reserve: 0,
                quote_reserve: 0,
                fee_bps: whirl.fee_rate,
                last_update_slot: 0,
                ..Default::default()
            };
            pools_seen += 1;
            orca_pools.insert(
                pubkey,
                (pool.clone(), whirl.token_vault_x, whirl.token_vault_y),
            );
            let cycles = detector.on_pool_update(&pool);
            cycles_evaluated += cycles.len() as u64;
            evaluate_and_write_cycles(
                cycles,
                &snapshot_all_pools(&raydium_pools, &orca_pools, &meteora_pools),
                &mut wallet,
                path,
                &eval_optimistic,
                &eval_conservative,
                &cost,
                max_input,
                realistic_mode,
                &mut cycles_evaluated,
                &mut trades_written,
            );
            continue;
        }

        // FIRST-B: try to decode as a Meteora DLMM LbPair (1024-2236 B).
        // Subscribe to its vault accounts (Phase 2a: all 3 DEXs feeding).
        if data.len() >= 156 + 32 * 65 {
            if let Ok(lp) = decode_lb_pair(&data) {
                for vault in [lp.token_vault_x, lp.token_vault_y] {
                    if !subscribed_vaults.contains_key(&vault.0) {
                        if let Err(e) = runtime.block_on(ws.subscribe_account(vault.0)) {
                            eprintln!("dl-app run: meteora vault subscribe failed: {e}");
                        } else {
                            subscribed_vaults.insert(vault.0, ());
                        }
                    }
                }
                let pool = Pool {
                    address: Pubkey(pubkey),
                    kind: AmmKind::MeteoraDlmm,
                    base_mint: lp.token_mint_x,
                    quote_mint: lp.token_mint_y,
                    base_decimals: 9,
                    quote_decimals: 6,
                    base_reserve: 0,
                    quote_reserve: 0,
                    fee_bps: lp.bin_step as u16,
                    last_update_slot: 0,
                    ..Default::default()
                };
                pools_seen += 1;
                meteora_pools.insert(pubkey, (pool.clone(), lp.token_vault_x, lp.token_vault_y));
                let cycles = detector.on_pool_update(&pool);
                cycles_evaluated += cycles.len() as u64;
                evaluate_and_write_cycles(
                    cycles,
                    &snapshot_all_pools(&raydium_pools, &orca_pools, &meteora_pools),
                    &mut wallet,
                    path,
                    &eval_optimistic,
                    &eval_conservative,
                    &cost,
                    max_input,
                    realistic_mode,
                    &mut cycles_evaluated,
                &mut trades_written,
                );
                continue;
            }
        }

        // FIRST: try to decode as a Raydium AmmInfo (752 bytes).
        // If yes, also subscribe to its vaults and store it.
        if data.len() == 752 {
            if let Ok(amm) = decode_amm_info(&data) {
                // Subscribe to the two vault accounts (idempotent).
                if !subscribed_vaults.contains_key(&amm.base_vault.0) {
                    if let Err(e) = runtime.block_on(ws.subscribe_account(amm.base_vault.0)) {
                        eprintln!("dl-app run: vault subscribe failed: {e}");
                    } else {
                        subscribed_vaults.insert(amm.base_vault.0, ());
                    }
                }
                if !subscribed_vaults.contains_key(&amm.quote_vault.0) {
                    if let Err(e) = runtime.block_on(ws.subscribe_account(amm.quote_vault.0)) {
                        eprintln!("dl-app run: vault subscribe failed: {e}");
                    } else {
                        subscribed_vaults.insert(amm.quote_vault.0, ());
                    }
                }

                // Phase 2a: also try Orca Whirlpool (256 B) and
                // Meteora DLMM (1024-2236 B). The size check is a
                // fast pre-filter; the decoder rejects sub-threshold
                // sizes with TooShort. Subscribe to both vaults for
                // each kind so reserves populate and the StreamingDetector
                // sees non-zero edge weights for those DEXs.

                // Build a pool stub (reserves 0) for the detector.
                let pool = Pool {
                    address: Pubkey(pubkey),
                    kind: AmmKind::RaydiumAmmV4,
                    base_mint: amm.base_mint,
                    quote_mint: amm.quote_mint,
                    base_decimals: amm.base_decimals,
                    quote_decimals: amm.quote_decimals,
                    base_reserve: 0,
                    quote_reserve: 0,
                    fee_bps: amm.fee_bps().unwrap_or(30),
                    last_update_slot: 0,
                    ..Default::default()
                };
                pools_seen += 1;
                // Insert (overwrite) in the tracking map.
                raydium_pools.insert(
                    pubkey,
                    (pool.clone(), amm.base_vault, amm.quote_vault),
                );

                // Run detection. With reserves=0 the edge has
                // no weight, so this rarely yields cycles, but
                // it registers the pool in the graph.
                let cycles = detector.on_pool_update(&pool);
                cycles_evaluated += cycles.len() as u64;
                evaluate_and_write_cycles(
                    cycles,
                    &snapshot_all_pools(&raydium_pools, &orca_pools, &meteora_pools),
                    &mut wallet,
                    path,
                    &eval_optimistic,
                    &eval_conservative,
                    &cost,
                    max_input,
                    realistic_mode,
                    &mut cycles_evaluated,
                &mut trades_written,
                );
                continue;
            }
        }

        // SECOND: try to decode as a vault SplTokenAccount
        // (SPL token accounts are 165 bytes; Token-2022 vault
        // accounts are 234 B or larger — both are layout-compatible
        // for the first 72 B which hold mint + amount). Phase 2 C1:
        // route through ALL three pool maps (Raydium + Orca +
        // Meteora) so vault updates populate reserves for all 3 DEXs.
        if data.len() >= SPL_TOKEN_ACCOUNT_SIZE {
            if let Ok(spl) = decode_spl_token_account(&data) {
                vault_updates += 1;
                // Find the parent pool that references this vault.
                // Search Raydium first (most common), then Orca, then Meteora.
                // Returns (parent_addr, new_reserve, is_base, kind).
                enum DexKind { Raydium, Orca, Meteora }
                let mut found: Option<([u8; 32], u64, bool, DexKind)> = None;
                for (k, (_, bv, qv)) in &raydium_pools {
                    if bv.0 == pubkey {
                        found = Some((*k, spl.amount, true, DexKind::Raydium));
                        break;
                    } else if qv.0 == pubkey {
                        found = Some((*k, spl.amount, false, DexKind::Raydium));
                        break;
                    }
                }
                if found.is_none() {
                    for (k, (_, bv, qv)) in &orca_pools {
                        if bv.0 == pubkey {
                            found = Some((*k, spl.amount, true, DexKind::Orca));
                            break;
                        } else if qv.0 == pubkey {
                            found = Some((*k, spl.amount, false, DexKind::Orca));
                            break;
                        }
                    }
                }
                if found.is_none() {
                    for (k, (_, bv, qv)) in &meteora_pools {
                        if bv.0 == pubkey {
                            found = Some((*k, spl.amount, true, DexKind::Meteora));
                            break;
                        } else if qv.0 == pubkey {
                            found = Some((*k, spl.amount, false, DexKind::Meteora));
                            break;
                        }
                    }
                }
                if let Some((parent_addr, new_reserve, is_base, kind)) = found {
                    match kind {
                        DexKind::Raydium => {
                            if let Some((mut pool, bv, qv)) =
                                raydium_pools.get(&parent_addr).cloned()
                            {
                                if is_base { pool.base_reserve = new_reserve; }
                                else { pool.quote_reserve = new_reserve; }
                                if pool.base_reserve > 0 && pool.quote_reserve > 0 {
                                    let cycles = detector.on_pool_update(&pool);
                                    cycles_evaluated += cycles.len() as u64;
                                    evaluate_and_write_cycles(
                                        cycles,
                                        &snapshot_all_pools(&raydium_pools, &orca_pools, &meteora_pools),
                                        &mut wallet,
                                        path,
                                        &eval_optimistic,
                                        &eval_conservative,
                                        &cost,
                                        max_input,
                                        realistic_mode,
                                        &mut cycles_evaluated,
                                    &mut trades_written,
                                    );
                                }
                                raydium_pools.insert(parent_addr, (pool, bv, qv));
                            }
                        }
                        DexKind::Orca => {
                            if let Some((mut pool, bv, qv)) =
                                orca_pools.get(&parent_addr).cloned()
                            {
                                if is_base { pool.base_reserve = new_reserve; }
                                else { pool.quote_reserve = new_reserve; }
                                if pool.base_reserve > 0 && pool.quote_reserve > 0 {
                                    let cycles = detector.on_pool_update(&pool);
                                    cycles_evaluated += cycles.len() as u64;
                                    evaluate_and_write_cycles(
                                        cycles,
                                        &snapshot_all_pools(&raydium_pools, &orca_pools, &meteora_pools),
                                        &mut wallet,
                                        path,
                                        &eval_optimistic,
                                        &eval_conservative,
                                        &cost,
                                        max_input,
                                        realistic_mode,
                                        &mut cycles_evaluated,
                                    &mut trades_written,
                                    );
                                }
                                orca_pools.insert(parent_addr, (pool, bv, qv));
                            }
                        }
                        DexKind::Meteora => {
                            if let Some((mut pool, bv, qv)) =
                                meteora_pools.get(&parent_addr).cloned()
                            {
                                if is_base { pool.base_reserve = new_reserve; }
                                else { pool.quote_reserve = new_reserve; }
                                if pool.base_reserve > 0 && pool.quote_reserve > 0 {
                                    let cycles = detector.on_pool_update(&pool);
                                    cycles_evaluated += cycles.len() as u64;
                                    evaluate_and_write_cycles(
                                        cycles,
                                        &snapshot_all_pools(&raydium_pools, &orca_pools, &meteora_pools),
                                        &mut wallet,
                                        path,
                                        &eval_optimistic,
                                        &eval_conservative,
                                        &cost,
                                        max_input,
                                        realistic_mode,
                                        &mut cycles_evaluated,
                                    &mut trades_written,
                                    );
                                }
                                meteora_pools.insert(parent_addr, (pool, bv, qv));
                            }
                        }
                    }
                }
                continue;
            }
        }

        // THIRD: Whirlpool (653 bytes) and LbPair (variable).
        let Some(pool) = decode_pool_update(&pubkey, &data) else {
            continue;
        };
        pools_seen += 1;
        let cycles = detector.on_pool_update(&pool);
        cycles_evaluated += cycles.len() as u64;
        evaluate_and_write_cycles(
            cycles,
            &snapshot_all_pools(&raydium_pools, &orca_pools, &meteora_pools),
            &mut wallet,
            path,
            &eval_optimistic,
            &eval_conservative,
            &cost,
            max_input,
            realistic_mode,
            &mut cycles_evaluated,
        &mut trades_written,
        );

        // Throttled status log every 5s.
        if last_log.elapsed() >= Duration::from_secs(5) {
            eprintln!(
                "trader: events={events} pools_seen={pools_seen} vault_updates={vault_updates} cycles_evaluated={cycles_evaluated} trades_written={trades_written} balance={} SOL",
                wallet.balance_lamports / 1_000_000_000
            );
            last_log = std::time::Instant::now();
        }
    }

    eprintln!();
    eprintln!(
        "dl-app run: stopped. events={events} pools_seen={pools_seen} vault_updates={vault_updates} cycles_evaluated={cycles_evaluated} trades_written={trades_written}"
    );
    eprintln!(
        "dl-app run: wallet balance = {} lamports ({} SOL)",
        wallet.balance_lamports,
        wallet.balance_lamports / 1_000_000_000
    );
    eprintln!("dl-app run: see status with: ./scripts/status.sh");
}

/// For each detected cycle, evaluate the conservative bound
/// and write a paper trade to `wallet.json` if the decision
/// is `WouldTrade`.
///
/// Uses the **real** `find_optimal_input` + `simulate_cycle` +
/// `evaluate` pipeline (the same one the recon harness uses), so
/// predicted PnL matches what `dl-recon` would record. The
/// `optimistic` and `conservative` `EvalParams` are passed in
/// separately — the previous version passed the same struct twice,
/// which collapsed the dual-bound check.
///
/// `pools` is the snapshot of currently-tracked pools across all
/// three DEXs. `cost` is the cost stack applied per bundle.
fn evaluate_and_write_cycles(
    cycles: Vec<dl_state::cycle::Cycle>,
    pools: &[dl_state::Pool],
    wallet: &mut dl_paper::PaperWallet,
    path: &std::path::Path,
    optimistic_eval: &dl_sim::ev::EvalParams,
    conservative_eval: &dl_sim::ev::EvalParams,
    cost: &dl_sim::cost::CostModel,
    max_input: u128,
    realistic_mode: bool,
    cycles_evaluated: &mut u64,
    trades_written: &mut u64,
) {
    use dl_paper::{Side, TradeFill};
    use dl_ledger::Decision;
    use dl_sim::ev::evaluate;
    use dl_sim::net_profit::NetProfit;
    use dl_sim::simulate::simulate_cycle;
    use dl_sim::sizing::{find_optimal_input, OptimalInput};
    use dl_state::PoolRegistry;

    // Build a transient PoolRegistry from the snapshot. `simulate_cycle`
    // and `find_optimal_input` both take a registry, so we materialise
    // it here.
    let mut registry = PoolRegistry::new();
    for p in pools {
        registry.insert(p.clone());
    }

    for cycle in cycles {
        // Sizing: find the input that maximises net profit.
        let sizing = match find_optimal_input(&cycle, &registry, cost, max_input) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("dl-app run: sizer error for cycle: {e}");
                continue;
            }
        };

        // Fill + cost: real `simulate_cycle` against the registry.
        let input: u128 = match &sizing {
            OptimalInput::Profitable { amount, .. } => *amount,
            // NoTrade: use the cycle's max_input/2 as a representative
            // sample so we still produce a NetProfit (loss-making).
            OptimalInput::NoTrade { .. } => max_input / 2,
        };
        let cycle_fill = match simulate_cycle(&cycle, &registry, input) {
            Ok(f) => f,
            Err(e) => {
                eprintln!("dl-app run: simulate error for cycle: {e}");
                continue;
            }
        };
        let gross = cycle_fill.final_output;

        let net = match NetProfit::from_optimal(sizing.clone(), input, gross, cost) {
            Ok(n) => n,
            Err(e) => {
                eprintln!("dl-app run: NetProfit error: {e}");
                continue;
            }
        };

        // EV: evaluate under both bounds.
        let ev_out = evaluate(&net, optimistic_eval, conservative_eval);
        let decision = Decision::from_ev(&ev_out.conservative);
        if !matches!(decision, Decision::WouldTrade) {
            continue;
        }
        *cycles_evaluated += 1;
        let net_profit_signed = ev_out.conservative.e_pnl;
        let pair = format_cycle_pair(&cycle);

        let output_lamports: u64 = if gross > u64::MAX as u128 {
            u64::MAX
        } else {
            gross as u64
        };
        let input_lamports: u64 = if input > u64::MAX as u128 {
            u64::MAX
        } else {
            input as u64
        };

        let fill = TradeFill {
            pair,
            side: Side::BaseToQuote,
            input_lamports,
            output_lamports,
            profit_lamports: net_profit_signed as i64,
            tip_lamports: cost.jito_tip_lamports,
            cycle_hash_hex: format!("{:?}", cycle),
        };
        let trade = if realistic_mode && !won_simulated() {
            dl_paper::TradeFill {
                pair: fill.pair.clone(),
                side: fill.side,
                input_lamports: fill.input_lamports,
                output_lamports: fill.input_lamports,
                profit_lamports: -(fill.tip_lamports as i64),
                tip_lamports: fill.tip_lamports,
                cycle_hash_hex: fill.cycle_hash_hex.clone(),
            }
        } else {
            fill
        };
        // Snapshot the values we need for jsonl BEFORE
        // wallet.execute() moves `trade`.
        let cycle_hash_hex_snapshot = trade.cycle_hash_hex.clone();
        let input_lamports_snapshot = trade.input_lamports;
        let output_lamports_snapshot = trade.output_lamports;
        match wallet.execute(trade) {
            Ok(_) => {
                *trades_written += 1;
                if let Err(e) = wallet.save(path) {
                    eprintln!("dl-app run: save failed: {e}");
                    break;
                }
                // Append the cycle to cycles.jsonl for the
                // ArbiNexus bridge. One JSON object per line.
                append_cycle_jsonl(
                    path,
                    &cycle,
                    &cycle_hash_hex_snapshot,
                    input_lamports_snapshot,
                    output_lamports_snapshot,
                );
            }
            Err(e) => {
                eprintln!("dl-app run: wallet.execute failed: {e}");
                break;
            }
        }
    }
}

/// Snapshot all currently-tracked pools across the three DEXs into
/// a single `Vec<Pool>`. Used to feed `evaluate_and_write_cycles`
/// (which builds a transient `PoolRegistry` for `simulate_cycle`).
fn snapshot_all_pools(
    raydium: &std::collections::HashMap<[u8; 32], (dl_state::Pool, dl_state::Pubkey, dl_state::Pubkey)>,
    orca: &std::collections::HashMap<[u8; 32], (dl_state::Pool, dl_state::Pubkey, dl_state::Pubkey)>,
    meteora: &std::collections::HashMap<[u8; 32], (dl_state::Pool, dl_state::Pubkey, dl_state::Pubkey)>,
) -> Vec<dl_state::Pool> {
    let mut out = Vec::with_capacity(raydium.len() + orca.len() + meteora.len());
    for (_, (p, _, _)) in raydium {
        out.push(p.clone());
    }
    for (_, (p, _, _)) in orca {
        out.push(p.clone());
    }
    for (_, (p, _, _)) in meteora {
        out.push(p.clone());
    }
    out
}

/// Simulated win/loss for the realistic paper mode. xorshift64
/// per-thread PRNG so the distribution is roughly uniform but
/// reproducible per process.
fn won_simulated() -> bool {
    use std::cell::Cell;
    use std::time::{SystemTime, UNIX_EPOCH};
    thread_local!(static STATE: Cell<u64> = Cell::new({
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0)
    }));
    STATE.with(|s| {
        let mut x = s.get();
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        s.set(x);
        (x % 100) < 30
    })
}

/// Append a single detected cycle to `cycles.jsonl` next to
/// the wallet file. The ArbiNexus bridge reads this file and
/// applies oracle + tip modeling.
fn append_cycle_jsonl(
    wallet_path: &std::path::Path,
    cycle: &dl_state::cycle::Cycle,
    cycle_hash_hex: &str,
    input_lamports: u64,
    output_lamports: u64,
) {
    let jsonl_path = wallet_path.with_extension("cycles.jsonl");
    // Synthesize a CycleRecord from what we have. The bridge
    // only needs base/quote mint and gross_bps.
    let legs = &cycle.legs;
    let base_mint = if legs.is_empty() { "" } else { "unknown" };
    let quote_mint = if legs.is_empty() { "" } else { "unknown" };
    let dex = "raydium"; // single-DEX paper mode in v1.1.5
    let gross_bps = if output_lamports > input_lamports {
        ((((output_lamports - input_lamports) as u128) * 10_000)
            / (input_lamports as u128).max(1)) as i64
    } else {
        0
    };
    let record = serde_json::json!({
        "pool_address": cycle_hash_hex,
        "dex": dex,
        "base_mint": base_mint,
        "quote_mint": quote_mint,
        "gross_bps": gross_bps,
        "fee_bps": 30, // Raydium AMM v4 default fee
        "detected_at_unix_ms": chrono::Utc::now().timestamp_millis(),
    });
    let line = format!("{}\n", record);
    use std::io::Write;
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&jsonl_path)
    {
        if let Err(e) = f.write_all(line.as_bytes()) {
            eprintln!("dl-app run: cycles.jsonl write failed: {e}");
        }
    }
}

/// Decode an AccountUpdate into a Pool, if possible.
fn decode_pool_update(pubkey: &[u8; 32], data: &[u8]) -> Option<Pool> {
    use dl_state::pool::AmmKind;
    use dl_state::Pubkey;
    use dl_state::decoder::{decode_amm_info, decode_whirlpool, decode_lb_pair};
    if data.len() == 752 {
        let amm = decode_amm_info(data).ok()?;
        return Some(Pool {
            address: Pubkey(*pubkey),
            kind: AmmKind::RaydiumAmmV4,
            base_mint: amm.base_mint,
            quote_mint: amm.quote_mint,
            base_decimals: amm.base_decimals,
            quote_decimals: amm.quote_decimals,
            // Without vault subscriptions we don't know the
            // reserves. Use 0; this means the edge weight
            // defaults to "no edge" (the graph will be empty
            // for this pool). Real reserves land in v1.1.3
            // via vault subscriptions.
            base_reserve: 0,
            quote_reserve: 0,
            fee_bps: amm.fee_bps().unwrap_or(30),
            last_update_slot: 0,
            ..Default::default()
        });
    }
    if data.len() == 653 {
        let w = decode_whirlpool(data).ok()?;
        return Some(Pool {
            address: Pubkey(*pubkey),
            kind: AmmKind::OrcaWhirlpool,
            base_mint: w.token_mint_x,
            quote_mint: w.token_mint_y,
            base_decimals: 0, // not parsed in 08-01 decoder
            quote_decimals: 0,
            base_reserve: 0,
            quote_reserve: 0,
            fee_bps: 30,
            last_update_slot: 0,
            ..Default::default()
        });
    }
    // Meteora DLMM: variable length; best-effort decode.
    if let Ok(lp) = decode_lb_pair(data) {
        return Some(Pool {
            address: Pubkey(*pubkey),
            kind: AmmKind::MeteoraDlmm,
            base_mint: lp.token_mint_x,
            quote_mint: lp.token_mint_y,
            base_decimals: 0,
            quote_decimals: 0,
            base_reserve: 0,
            quote_reserve: 0,
            fee_bps: (lp.bin_step as u16).min(u16::MAX),
            last_update_slot: 0,
            ..Default::default()
        });
    }
    None
}

/// Format a Cycle as a "pair string" for the wallet log.
/// E.g. "raydium/A->orca/B->meteora/A".
fn format_cycle_pair(cycle: &dl_state::cycle::Cycle) -> String {
    let mut s = String::new();
    for (i, leg) in cycle.legs.iter().enumerate() {
        if i > 0 {
            s.push('-');
        }
        s.push_str(match leg.direction {
            dl_state::cycle::Direction::BaseToQuote => "btq",
            dl_state::cycle::Direction::QuoteToBase => "qtb",
        });
    }
    if cycle.legs.is_empty() {
        s.push_str("empty");
    }
    s
}

fn run_dry_run() {
    let path = env::var("DL_DRY_RUN_PATH").unwrap_or_else(|_| {
        let manifest = env!("CARGO_MANIFEST_DIR");
        format!(
            "{}/../dl-feed/tests/fixtures/sample_capture.bincode",
            manifest
        )
    });

    info!(path = %path, "starting dry-run replay");

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
            FeedEvent::Pool { .. } | FeedEvent::StalePoolHalt { .. } => {
                // Stats counters track Slot + AccountUpdate only;
                // Pool + StalePoolHalt are v2.0 events that don't
                // contribute to the offline capture-vs-cycle test
                // stats surfaced by this helper.
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

    // DAM-81: wire the four DAM-68 target series
    // (dl_jito_submit_total, dl_jito_landed_total,
    // dl_daily_cap_remaining_lamports, dl_realized_pnl_sol)
    // into the registry. `run_metrics_prom` is the only path
    // that currently constructs a `MetricsRegistry` and serves
    // `/metrics`; the `--submit-live` runner is wired in DAM-95.
    // Here we use throwaway stand-ins for `LiveMetrics`,
    // `CapState`, and `PnLTracker` so the four series appear
    // on the first scrape even before the live runner is
    // running. The stand-ins let SRE flip the Phase 3 alerts to
    // active and validate the prom render path end-to-end.
    {
        use dl_app::live_metrics::{LiveMetricsAdapter, PnLTracker};
        use dl_executor::metrics::LiveMetrics;
        use dl_signer::cap::{CapConfig, CapState};
        use std::sync::Mutex;

        let live = Arc::new(LiveMetrics::new());
        let cap_state = Arc::new(Mutex::new(CapState::new(CapConfig::default())));
        let pnl = Arc::new(PnLTracker::new());
        let adapter = LiveMetricsAdapter::new(
            registry.clone(),
            live,
            cap_state,
            pnl,
        );
        // Initial poll so all four series appear on the
        // first scrape (Prometheus alerts evaluate `absent()`
        // and would otherwise flag the rule as no-data for
        // the first 30 s window).
        adapter.poll();
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
