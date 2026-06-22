//! `dl-app` v2.0 live submit pipeline (Phase 1).
//!
//! The 5-tx Jito bundle per cycle:
//!
//! ```text
//! tx0..tx2  = Jupiter swap legs (3, one per cycle leg)
//! tx3       = dl-assert instruction (asserts net_pnl ≥ threshold)
//! tx4       = Jito tip transfer
//! ```
//!
//! All safety gates live in this module: cap check, rate-limit,
//! kill switch circuit breaker, simulateTransaction RPC gate.

use std::path::Path;

use solana_sdk::hash::Hash;
use solana_sdk::instruction::Instruction;
use solana_sdk::message::Message;
use solana_sdk::signature::Signature;
use solana_sdk::system_instruction;
use solana_sdk::transaction::VersionedTransaction;

use dl_detect::cycle::Cycle;
use dl_executor::bundle::{BundleBuilder, TipLeg};
use dl_executor::jito::{JitoClient, LandingResult};
use dl_executor::jupiter::JupiterClient;
use dl_executor::killswitch::{BundleOutcome, KillSwitch};
use dl_executor::metrics::LiveMetrics;
use dl_executor::signer_integration::sign_with_keystore;
use dl_executor::tip::TipConfig;
use dl_signer::cap::CapState;
use dl_signer::keystore::KeyStore;
use dl_signer::ratelimit::RateLimit;
use dl_state::pool::Pool as DlPool;
use dl_state::Pubkey as DlPubkey;
use crate::opportunity::dl_to_solana_pubkey;
use crate::live_status::{LastLandedSnapshot, WriterInputs};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

/// Wrapped SOL mint pubkey (base58). Used to look up the SOL/USD
/// Pyth feed in `LiveConfig::pyth_feeds` so the writer can render
/// `sol_usd` when a Pyth oracle is configured.
const WRAPPED_SOL_MINT: &str = "So11111111111111111111111111111111111111112";

/// Wall-clock milliseconds since the Unix epoch. Mirror of
/// `crate::live_status::unix_ts_ms`; kept private to this module
/// so the writer-side field is populated with the same time base
/// the dashboard's `ts_unix_ms` will report on the next tick.
fn unix_ts_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Configuration for the v2.0 live submit path.
#[derive(Clone)]
pub struct LiveConfig {
    /// Assert program ID (devnet or mainnet deployment).
    pub assert_program_id: solana_sdk::pubkey::Pubkey,
    /// Tip-lamports sizing config (reused from `dl-executor::tip`).
    pub tip_config: TipConfig,
    /// Simulate-gate RPC URL. If `None`, the simulate gate is skipped.
    pub simulate_rpc_url: Option<String>,
    /// Jito tip account (one of 8 rotated accounts, picked at
    /// startup via `HttpJitoClient::next_tip_account`).
    pub tip_account: solana_sdk::pubkey::Pubkey,
    /// Recent blockhash (fetched per cycle from the RPC). For tests,
    /// callers pass a deterministic `Hash::new_unique()`.
    pub recent_blockhash: Hash,
    /// Pyth oracle client (Phase 2b). If `None`, the price gate is
    /// skipped — equivalent to the v1.x placeholder `1.0` oracle.
    pub pyth: Option<std::sync::Arc<dyn dl_oracle::PythClient>>,
    /// Pyth price feeds for the input/output mints. Map mint →
    /// Pyth feed pubkey. Missing entries = mint has no feed and is
    /// rejected (`OpportunityOutcome::NotSubmitted("pyth: no-feed")`).
    pub pyth_feeds:
        std::collections::HashMap<solana_sdk::pubkey::Pubkey, solana_sdk::pubkey::Pubkey>,
    /// Maximum age of a Pyth price (seconds). Default
    /// `dl_oracle::MAX_PYTH_AGE_SECS` (60). Override via env
    /// `DL_PYTH_MAX_AGE_SECS`.
    pub pyth_max_age_secs: u64,
    /// Niche config (Phase 2 H3). `None` = no filtering (paper-mode
    /// / cold-start). The trader consults `NicheConfig::is_enabled`
    /// per cycle and rejects the cycle if the resolved niche is
    /// disabled.
    pub niche_config: Option<dl_calibration::NicheConfig>,
}
/// Outcome of one `submit_opportunity` call.
#[derive(Debug, Clone)]
pub enum OpportunityOutcome {
    /// Bundle landed; realized PnL + slot returned.
    Landed {
        slot: u64,
        realized_pnl_lamports: i64,
    },
    /// Bundle was submitted but did not land within the timeout.
    Lost,
    /// Simulate gate rejected the bundle (net negative or RPC error).
    Rejected(String),
    /// Bundle was not submitted because the kill switch tripped,
    /// the cap would be breached, the rate limit was hit, or the
    /// build failed.
    NotSubmitted(String),
}

impl OpportunityOutcome {
    pub fn landed(&self) -> bool {
        matches!(self, Self::Landed { .. })
    }
}

/// Free-function submit pipeline for one detected cycle.
///
/// Flow per cycle:
/// 1. Resolve pool addresses → mints via `pool_lookup`.
/// 2. Build 3 Jupiter swap txs + 1 dl-assert tx + 1 Jito tip tx.
/// 3. (If `cfg.simulate_rpc_url` is set) call `simulate_bundle`
///    and reject on `NonPositive` verdict — fail-closed. The
///    dl-assert tx is skipped (its on-chain revert IS the gate;
///    simulating it would falsely flag profitable bundles as
///    non-positive).
/// 4. Sign all 5 txs via `sign_with_keystore`.
/// 5. Cap + rate-limit + kill switch check BEFORE submit.
/// 6. Submit via `jito.submit`. Record the outcome on the
///    kill switch.
///
/// DAM-99: the `live_status_inputs` parameter is the writer's
/// `Arc<Mutex<WriterInputs>>`. The function sets `running = true`
/// at entry, populates `pyth` and `pyth_sol_feed` from `cfg`
/// when a `PythClient` is configured, and updates `last_landed`
/// and `realized_pnl_today_lamports` when a Jito bundle lands.
/// The writer side task reads from this and serializes to
/// `live_status.json` at 1 Hz. The Mutex is held for microseconds
/// inside this function; the cycle path never blocks on it.
pub fn submit_opportunity(
    cycle: &Cycle,
    pool_lookup: &dyn Fn(&DlPubkey) -> Option<DlPool>,
    jupiter: &dyn JupiterClient,
    jito: &dyn JitoClient,
    keystore: &KeyStore,
    cfg: &LiveConfig,
    cap_state: &mut CapState,
    rate_limit: &RateLimit,
    killswitch: &mut KillSwitch,
    metrics: &LiveMetrics,
    live_status_inputs: &Arc<Mutex<WriterInputs>>,
) -> OpportunityOutcome {
    use crate::opportunity::build_unsigned_bundle;
    use dl_assert_sdk::assert_min_net_pnl_threshold_reasonable;
    // DAM-99: signal the writer that the live cycle is alive
    // (running=true). The dashboard renders a "degraded" state
    // when running is false.
    {
        let mut w = live_status_inputs.lock().expect("WriterInputs mutex poisoned");
        w.running = true;
        w.pyth = cfg.pyth.clone();
        w.pyth_sol_feed = cfg.pyth_feeds.get(
            &solana_sdk::pubkey::Pubkey::from_str_const(WRAPPED_SOL_MINT),
        ).copied();
    }
    let signer_sol = solana_sdk::pubkey::Pubkey::from(<[u8; 32]>::from(keystore.public_key_for_print()));
    let tip_account = cfg.tip_account;
    // Phase 2 C2: assign a stable `seq` to the cycle so calibration
    // captures and reconciliation rows can join on it.
    let cycle_seq = metrics.next_cycle_seq();
    let mut cycle_owned = cycle.clone();
    cycle_owned.seq = cycle_seq;
    let cycle = &cycle_owned;
    let min_net_pnl_lamports: u64 = 50_000;
    if assert_min_net_pnl_threshold_reasonable(min_net_pnl_lamports).is_err() {
        return OpportunityOutcome::NotSubmitted(
            "min_net_pnl_lamports out of reasonable range".into(),
        );
    }
    let input_amount = crate::opportunity::default_input_amount_lamports(cycle);
    let tip = dl_executor::tip::tip_lamports(input_amount as i128, &cfg.tip_config);

    // ─── M1: gates at the top, BEFORE cap/rate-limit/killswitch work ──
    // The niche + Pyth gates only need pool_lookup + the cycle, so
    // running them first means a disabled niche or a stale Pyth
    // price fails cheaply without burning cap / rate-limit quota.

    // Resolve the legs once — shared by the niche gate, the Pyth
    // gate, and `build_unsigned_bundle` later. This is L3's fix
    // (resolve_leg_mints no longer drops the metrics counter;
    // the metric is incremented via `resolve_cycle_legs` inside).
    let resolved_legs = match crate::opportunity::resolve_leg_mints(cycle, pool_lookup) {
        Ok(rs) => rs,
        Err(e) => {
            return OpportunityOutcome::NotSubmitted(format!("resolve: {e}"));
        }
    };
    let first_leg = resolved_legs.first();
    let (input_mint_str, output_mint_str) = match first_leg {
        Some(r) => (
            bs58::encode(r.input_mint.0).into_string(),
            bs58::encode(r.output_mint.0).into_string(),
        ),
        None => ("unknown".into(), "unknown".into()),
    };
    let input_amount_val = input_amount;

    // Phase 2 H3: niche filter.
    if let (Some(niche_cfg), Some(first)) = (cfg.niche_config.as_ref(), first_leg) {
        let class = dl_calibration::NicheClass {
            // M1: niche gate is only meaningful for the input mint's
            // DEX (classify uses input_mint as the primary DEX
            // discriminator).
            dex: dex_kind_from_mint(&dl_to_solana_pubkey(&first.input_mint).to_string()),
            pool_age: dl_calibration::PoolAge::Mature, // cold-start default
            time_of_day: dl_calibration::TimeOfDay::Normal, // cold-start default
            input_size: input_size_bucket(input_amount),
        };
        if !niche_cfg.is_enabled(&class) {
            return OpportunityOutcome::NotSubmitted(format!(
                "niche: disabled class={:?}",
                class
            ));
        }
    }

    // Phase 2b: Pyth price gate. Reject the cycle if either mint
    // lacks a feed or the price is stale. The gate is fail-closed:
    // any Pyth error → NotSubmitted("pyth: <reason>"). When
    // `cfg.pyth` is None, the gate is skipped (paper-mode /
    // cold-start).
    if let Some(pyth) = cfg.pyth.as_ref() {
        for r in &resolved_legs {
            for mint in [r.input_mint, r.output_mint] {
                let feed = match cfg.pyth_feeds.get(&dl_to_solana_pubkey(&mint)) {
                    Some(f) => *f,
                    None => {
                        return OpportunityOutcome::NotSubmitted(format!(
                            "pyth: no-feed mint={}",
                            dl_to_solana_pubkey(&mint)
                        ));
                    }
                };
                if let Err(e) =
                    dl_oracle::fetch_fresh(pyth.as_ref(), &feed, cfg.pyth_max_age_secs)
                {
                    return OpportunityOutcome::NotSubmitted(format!("pyth: {e}"));
                }
            }
        }
    }

    // ─── Cap / rate-limit / kill switch (after the cheap gates) ────────
    // Cap check BEFORE signing (fail-closed: refuse to spend
    // signature fees on a bundle we can't afford).
    if let Err(e) = cap_state.try_charge(tip) {
        metrics.bundles_failed.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        return OpportunityOutcome::NotSubmitted(format!("cap: {e}"));
    }

    // Rate-limit check.
    if !rate_limit.try_acquire() {
        // Refund the cap charge — we didn't actually submit.
        cap_state.refund(tip);
        return OpportunityOutcome::NotSubmitted("rate-limit".into());
    }

    // Kill switch — refuse if circuit is open.
    if killswitch.check().is_err() {
        cap_state.refund(tip);
        return OpportunityOutcome::NotSubmitted("killswitch open".into());
    }

    let output = match build_unsigned_bundle(
        cycle,
        pool_lookup,
        jupiter,
        metrics,
        cfg.assert_program_id,
        min_net_pnl_lamports,
        tip,
        tip_account,
        signer_sol,
        cfg.recent_blockhash,
    ) {
        Ok(o) => o,
        Err(e) => {
            cap_state.refund(tip);
            metrics.bundles_failed.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            return OpportunityOutcome::NotSubmitted(format!("build: {e}"));
        }
    };
    let expected_out_per_leg: Vec<u64> = output.legs.iter().map(|l| l.expected_out).collect();

    // If a simulate RPC URL is configured, call simulateTransaction on
    // every tx in the bundle EXCEPT the dl-assert tx (the assert's
    // on-chain revert IS the gate; simulating it would falsely flag
    // profitable bundles). Reject on `NonPositive` verdict — fail-closed.
    if let Some(rpc_url) = cfg.simulate_rpc_url.as_deref() {
        let sim_rpc = solana_client::rpc_client::RpcClient::new(rpc_url.to_string());
        let (report, verdict) = match dl_executor::simulate_and_classify(
            &sim_rpc,
            &output.signed_transactions,
            Some(&cfg.assert_program_id),
        ) {
            Ok((r, v)) => (r, v),
            Err(e) => {
                cap_state.refund(tip);
                return OpportunityOutcome::Rejected(format!("simulate rpc: {e}"));
            }
        };
        if matches!(
            verdict,
            dl_executor::SimulateVerdict::NonPositive | dl_executor::SimulateVerdict::Error
        ) {
            tracing::warn!(
                verdict = ?verdict,
                all_txs_ok = report.all_txs_ok,
                logs_len = report.logs.len(),
                "simulate gate rejected bundle"
            );
            cap_state.refund(tip);
            return OpportunityOutcome::Rejected(format!(
                "simulate verdict {:?} (all_txs_ok={})",
                verdict, report.all_txs_ok
            ));
        }
    }

    let mut txs = output.signed_transactions;
    if let Err(e) = sign_with_keystore(keystore, &mut txs, cfg.recent_blockhash) {
        cap_state.refund(tip);
        return OpportunityOutcome::NotSubmitted(format!("sign: {e}"));
    }

    let tip_leg = TipLeg::new(tip, tip_account.to_string());
    let mut builder = BundleBuilder::new();
    for leg in output.legs {
        builder.push_swap(leg);
    }
    let bundle = match builder
        .set_tip(tip_leg)
        .set_signed_transactions(txs)
        .build(Some(&cfg.assert_program_id))
    {
        Ok(b) => b,
        Err(e) => {
            cap_state.refund(tip);
            return OpportunityOutcome::NotSubmitted(format!("bundle: {e}"));
        }
    };

    let jito_result = match jito.submit(&bundle) {
        Ok(r) => r,
        Err(e) => {
            cap_state.refund(tip);
            metrics.bundles_failed.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            return OpportunityOutcome::NotSubmitted(format!("submit: {e}"));
        }
    };

    let outcome = match jito.poll_landing(&jito_result.bundle_id) {
        Ok(LandingResult::Landed { slot }) => {
            metrics.bundles_landed.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let _ = killswitch.record(BundleOutcome::Landed);
            // Phase 2 C2: persist a calibration capture to the JSONL
            // log so dl-calibrate can fit p_detect/p_win/p_land later.
            // Real mints, real cycle.seq, real tip_lamports. The
            // on-chain realized PnL is the dl-assert tx's emit (Phase
            // 3 work to parse the tx logs); for v1.0 we record the
            // tip as a lower bound on realized impact and let the
            // dashboard flag negative tip contributions as losses.
            let realized_proxy = jito_result.tip_lamports as i64;
            let _ = persist_calibration_capture(
                cycle,
                slot,
                &jito_result,
                true,
                realized_proxy,
                &input_mint_str,
                &output_mint_str,
                input_amount_val,
                &expected_out_per_leg,
            );
            // DAM-99: populate the writer fields on Landed.
            // 1) Roll the cap over if UTC midnight has passed
            //    (matches `CapState::reset_if_new_day`). 2) Roll
            //    the writer's PnL ledger over to 0 at UTC midnight
            //    (same semantics, separate field). 3) Add the
            //    realized proxy to today's running total.
            //    4) Overwrite `last_landed` (new bundle wins;
            //    the dashboard does not track a history here).
            cap_state.reset_if_new_day();
            {
                let mut w = live_status_inputs
                    .lock()
                    .expect("WriterInputs mutex poisoned");
                w.reset_if_new_day();
                w.realized_pnl_today_lamports =
                    w.realized_pnl_today_lamports.saturating_add(realized_proxy);
                w.last_landed = LastLandedSnapshot {
                    bundle_id: Some(jito_result.bundle_id.clone()),
                    ts_unix_ms: Some(unix_ts_ms()),
                    profit_lamports: Some(realized_proxy),
                };
            }
            OpportunityOutcome::Landed {
                slot,
                realized_pnl_lamports: realized_proxy,
            }
        }
        Ok(other) => {
            metrics
                .bundles_failed
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let _ = killswitch.record(BundleOutcome::Lost);
            tracing::warn!(?other, bundle_id = %jito_result.bundle_id, "bundle not landed");
            OpportunityOutcome::Lost
        }
        Err(e) => {
            metrics
                .bundles_failed
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let _ = killswitch.record(BundleOutcome::Lost);
            OpportunityOutcome::NotSubmitted(format!("poll: {e}"))
        }
    };
    outcome
}

/// Map a base58 mint to one of the three known DEX kinds. Used by
/// the niche gate (H3). Mirrors the mapping in
/// `dl-calibration::classify` but operates on a single mint string.
fn dex_kind_from_mint(mint: &str) -> dl_calibration::DexKind {
    if mint.starts_with("So11111") {
        dl_calibration::DexKind::Orca
    } else if mint.starts_with("EPjFWdd5") {
        dl_calibration::DexKind::Raydium
    } else if mint.starts_with("Es9vMFrz") {
        dl_calibration::DexKind::Meteora
    } else {
        dl_calibration::DexKind::Meteora
    }
}

/// Map an input-amount (lamports) to one of the three size buckets.
/// Used by the niche gate (H3).
fn input_size_bucket(amount: u64) -> dl_calibration::SizeBucket {
    if amount < 1_000_000_000 {
        dl_calibration::SizeBucket::Small
    } else if amount < 10_000_000_000 {
        dl_calibration::SizeBucket::Medium
    } else {
        dl_calibration::SizeBucket::Large
    }
}

/// Pre-fund the vault PDA with `signer.lamports()` lamports so
/// the dl-assert program has a valid pre-bundle snapshot to read.
/// Idempotent: if `vault.lamports() >= signer.lamports()`, this
/// function returns `Ok(VaultFunded::AlreadyFunded)` without
/// sending any tx.
///
/// Flow:
/// 1. Read `getBalance(vault)` and `getBalance(signer)` from the
///    RPC at `rpc_url`.
/// 2. If `vault >= signer`, return early.
/// 3. Otherwise build a `system_instruction::transfer` for the
///    delta, sign with `keystore`, send via
///    `RpcClient::send_and_confirm_transaction`, then return
///    `Ok(VaultFunded::Funded { lamports })`.
///
/// Returns `Err(String)` for any RPC or signing failure. Callers
/// should log and continue (per the operator runbook, the manual
/// `solana transfer` flow is the fallback).
pub fn pre_fund_vault_if_needed(
    rpc_url: &str,
    signer: &solana_sdk::pubkey::Pubkey,
    vault: &solana_sdk::pubkey::Pubkey,
    keystore: &KeyStore,
) -> Result<VaultFunded, String> {
    let rpc = solana_client::rpc_client::RpcClient::new(rpc_url.to_string());
    let vault_balance = rpc
        .get_balance(vault)
        .map_err(|e| format!("getBalance(vault) failed: {e}"))?;
    let signer_balance = rpc
        .get_balance(signer)
        .map_err(|e| format!("getBalance(signer) failed: {e}"))?;
    if vault_balance >= signer_balance {
        return Ok(VaultFunded::AlreadyFunded {
            vault_lamports: vault_balance,
            signer_lamports: signer_balance,
        });
    }
    let delta = signer_balance - vault_balance;
    // Refetch the blockhash close to send-time to avoid stale-
    // blockhash errors after long startup pauses.
    let recent_blockhash = rpc
        .get_latest_blockhash()
        .map_err(|e| format!("getLatestBlockhash failed: {e}"))?;
    let transfer_ix =
        system_instruction::transfer(signer, vault, delta);
    let mut msg = Message::new(&[transfer_ix], Some(signer));
    msg.recent_blockhash = recent_blockhash;
    let v0 = solana_sdk::message::VersionedMessage::Legacy(msg);
    let n_sigs = v0.header().num_required_signatures as usize;
    let signatures = vec![Signature::default(); n_sigs];
    let mut tx = VersionedTransaction {
        signatures,
        message: v0,
    };
    sign_with_keystore(keystore, std::slice::from_mut(&mut tx), recent_blockhash)
        .map_err(|e| format!("sign_with_keystore failed: {e}"))?;
    let sig = rpc
        .send_and_confirm_transaction(&tx)
        .map_err(|e| format!("send_and_confirm_transaction failed: {e}"))?;
    Ok(VaultFunded::Funded {
        lamports: delta,
        signature: sig.to_string(),
    })
}

/// Result of [`pre_fund_vault_if_needed`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VaultFunded {
    /// Vault was already funded; no tx was sent. Both lamport
    /// counts returned for operator visibility.
    AlreadyFunded {
        vault_lamports: u64,
        signer_lamports: u64,
    },
    /// Vault was under-funded; we transferred `lamports` from
    /// signer to vault. `signature` is the on-chain tx sig.
    Funded { lamports: u64, signature: String },
}

/// Append a `CalibrationCapture` to the JSONL log. Phase 2 C2
/// — uses real input/output mints, real cycle.seq, real input
/// amount, real per-leg `expected_out` from Jupiter, and the Jito
/// tip as a `realized_pnl_lamports` lower bound (full on-chain PnL
/// comes from parsing the dl-assert tx logs in Phase 3).
///
/// Path is `DL_CALIBRATION_PATH` env or `./dl-calibration/captures.jsonl`.
fn persist_calibration_capture(
    cycle: &Cycle,
    slot: u64,
    jito_result: &dl_executor::jito::JitoSubmitResult,
    won: bool,
    realized_pnl_lamports: i64,
    input_mint: &str,
    output_mint: &str,
    input_amount: u64,
    expected_out_per_leg: &[u64],
) -> Result<(), String> {
    use dl_calibration::{CalibrationCapture, JsonlCaptures};
    let cap = CalibrationCapture {
        ts: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0),
        cycle_seq: cycle.seq,
        slot,
        input_mint: input_mint.to_string(),
        output_mint: output_mint.to_string(),
        input_amount,
        expected_out_per_leg: expected_out_per_leg.to_vec(),
        jito_bundle_id: jito_result.bundle_id.clone(),
        realized_pnl_lamports,
        won,
    };
    let path = std::path::PathBuf::from(
        std::env::var("DL_CALIBRATION_PATH")
            .unwrap_or_else(|_| "./dl-calibration/captures.jsonl".into()),
    );
    let sink = JsonlCaptures::open_append(&path).map_err(|e| e.to_string())?;
    sink.record(&cap).map_err(|e| e.to_string())?;
    Ok(())
}

/// Phase 2 C3: convert a fitted `CalibrationResult` into the
/// `EvalParams` used by the live trader. `p_detect` and
/// `competition.base_win_ppm` come from the fit; the other
/// submodels keep their conservative defaults (per the Phase 2
/// plan, the calibration loop only adjusts the two `p_*`
/// probabilities).
///
/// Lives in `dl-app` (not `dl-sim`) to avoid a cyclic
/// `dl-calibration → dl-sim` + `dl-sim → dl-calibration`
/// dependency.
pub fn eval_params_from_calibration(
    cal: &dl_calibration::CalibrationResult,
) -> dl_sim::ev::EvalParams {
    use dl_sim::ev::{
        CompetitionParams, FailedCostModel, LandingParams, LatencyBudget,
    };
    dl_sim::ev::EvalParams {
        p_detect: cal.p_detect,
        competition: CompetitionParams {
            // Map fitted p_win onto base_win_ppm; threshold stays
            // at 10 bps (Phase 2 plan locked decision).
            base_win_ppm: cal.p_win.to_ppm(),
            richness_threshold_bps: 10,
            decay_ppm_per_bps: 10_000,
        },
        latency: LatencyBudget::conservative_default(),
        landing: LandingParams::conservative_default(),
        failed: FailedCostModel::jito_bundle(),
        tip_lamports: 0,
    }
}

/// Replay a `CapturedFeed` JSONL file through the streaming detector
/// and return every 3-leg cycle found. Used by
/// `dl-app run --feed capture <path>` to drive off-line replay
/// against the same code path the live WebSocket feed uses.
///
/// ## Status
///
/// **Stub.** The DAM-62 commit `3f04ee` added this function with
/// a body that referenced `FeedEvent::PoolSnapshot`,
/// `FeedEvent::WhirlpoolSnapshot`, `FeedEvent::WhirlpoolRealSnapshot`,
/// `FeedEvent::SplTokenUpdate`, `dl_state::decoder::Pool`,
/// `assemble_pool`, `assemble_whirlpool_pool`,
/// `assemble_whirlpool_real_pool`, `decode_amm_info`,
/// `decode_spl_token_account`, `decode_whirlpool`, and
/// `decode_whirlpool_real` — none of which exist on disk in the
/// state needed by this signature. The DAM-62 acceptance test
/// (`dl-state/tests/dam62_orca_whirlpool_3leg.rs`) passes because
/// it constructs types locally; the production path is unwired.
///
/// This stub preserves the public signature
/// (`Result<Vec<Cycle>, String>`) and returns an empty vector so
/// the build compiles. The function is **not a DAM-82 concern**;
/// a follow-up DAM-98 / DAM-44c ticket will wire the body to the
/// real `dl-feed::capture` JSONL format, the `dl_state::decoder`
/// API, and the `dl_core::feed::FeedEvent` snapshot variants once
/// those types land. Until then, callers that need cycles from a
/// capture should use the local helpers in the DAM-62 / DAM-63
/// test crates.
pub fn cycles_from_capture(_path: &Path) -> Result<Vec<Cycle>, String> {
    // Stub body — see module docs.
    Ok(Vec::new())
}

#[cfg(test)]
mod live_submit_tests {
    use super::*;
    use dl_detect::cycle::{Direction, Leg};
    use dl_executor::jito::MockJitoClient;
    use dl_executor::jupiter::{JupiterClient, JupiterQuote, JupiterRouteStep};
    use dl_executor::killswitch::KillSwitchConfig;
    use dl_signer::cap::CapConfig;
    use dl_signer::ratelimit::RateLimitConfig;
    use dl_state::pool::{AmmKind, Pool};
    use dl_state::Pubkey as DlPubkey;
    use solana_sdk::message::{Message, VersionedMessage};
    use solana_sdk::signer::keypair::Keypair;
    use solana_sdk::signer::Signer as SdkSigner;
    use solana_sdk::transaction::VersionedTransaction;

    fn fresh_keystore() -> KeyStore {
        let kf = dl_signer::keystore::KeyFile::new("test-passphrase-1b");
        let secret = kf.decrypt("test-passphrase-1b").unwrap();
        KeyStore::from_secret(secret)
    }

    fn dummy_pool(addr: [u8; 32]) -> Pool {
        Pool {
            address: DlPubkey(addr),
            kind: AmmKind::RaydiumAmmV4,
            base_mint: DlPubkey([10u8; 32]),
            quote_mint: DlPubkey([20u8; 32]),
            base_decimals: 9,
            quote_decimals: 6,
            base_reserve: 1_000_000_000,
            quote_reserve: 100_000_000_000,
            fee_bps: 30,
            last_update_slot: 0,
            ..Default::default()
        }
    }

    fn three_leg_cycle() -> Cycle {
        Cycle::new(vec![
            Leg {
                pool: DlPubkey([1u8; 32]),
                direction: Direction::BaseToQuote,
                weight: -100,
            },
            Leg {
                pool: DlPubkey([2u8; 32]),
                direction: Direction::BaseToQuote,
                weight: -100,
            },
            Leg {
                pool: DlPubkey([3u8; 32]),
                direction: Direction::BaseToQuote,
                weight: -100,
            },
        ])
    }

    /// Jupiter mock that bypasses bincode decode (returns a
    /// pre-built `VersionedTransaction` directly).
    struct BypassJupiter {
        tx_template: VersionedTransaction,
    }
    impl dl_executor::jupiter::JupiterClient for BypassJupiter {
        fn quote(
            &self,
            _req: &dl_executor::jupiter::QuoteRequest,
        ) -> Result<JupiterQuote, dl_executor::error::ExecutorError> {
            Ok(JupiterQuote {
                route_plan: vec![JupiterRouteStep {
                    amm_id: "amm1".into(),
                    label: "Raydium".into(),
                    input_mint: "SOL".into(),
                    output_mint: "USDC".into(),
                    in_amount: 1,
                    out_amount: 2,
                    fee_amount: 1,
                }],
                in_amount: 1,
                out_amount: 2,
                other_amount_threshold: 1,
                swap_transaction_base64: String::new(),
            })
        }
        fn swap_tx_base64(
            &self,
            _quote: &JupiterQuote,
            _user_pubkey: &solana_sdk::pubkey::Pubkey,
        ) -> Result<String, dl_executor::error::ExecutorError> {
            Ok(String::new())
        }
        fn build_swap_tx(
            &self,
            req: &dl_executor::jupiter::QuoteRequest,
            _u: &solana_sdk::pubkey::Pubkey,
        ) -> Result<(JupiterQuote, VersionedTransaction), dl_executor::error::ExecutorError>
        {
            Ok((self.quote(req)?, self.tx_template.clone()))
        }
    }

    fn default_live_config(signer: &KeyStore) -> LiveConfig {
        let signer_sol = solana_sdk::pubkey::Pubkey::from(<[u8; 32]>::from(signer.public_key_for_print()));
        let _ = signer_sol;
        LiveConfig {
            assert_program_id: solana_sdk::pubkey::Pubkey::new_unique(),
            tip_config: TipConfig::default(),
            simulate_rpc_url: None,
            tip_account: solana_sdk::pubkey::Pubkey::new_unique(),
            recent_blockhash: Hash::new_unique(),
            pyth: None,
            pyth_feeds: std::collections::HashMap::new(),
            pyth_max_age_secs: 60,
            niche_config: None,
        }
    }

    fn fresh_safety() -> (CapState, RateLimit, KillSwitch) {
        (
            CapState::new(CapConfig::default()),
            RateLimit::new(RateLimitConfig::default()),
            KillSwitch::new(KillSwitchConfig::default()),
        )
    }

    #[test]
    fn submit_opportunity_returns_landed_for_valid_cycle() {
        let keystore = fresh_keystore();
        let jito = MockJitoClient::new();
        let pool_lookup = |pk: &DlPubkey| Some(dummy_pool(pk.0));
        let kp = Keypair::new();
        let ix = solana_sdk::system_instruction::transfer(&kp.pubkey(), &kp.pubkey(), 0);
        let msg = Message::new(&[ix], Some(&kp.pubkey()));
        let tx = VersionedTransaction {
            signatures: vec![],
            message: VersionedMessage::Legacy(msg),
        };
        let jupiter = BypassJupiter { tx_template: tx };

        let cycle = three_leg_cycle();
        let cfg = default_live_config(&keystore);
        let (mut cap, rl, mut ks) = fresh_safety();
        let metrics = LiveMetrics::new();

        let outcome = submit_opportunity(
            &cycle,
            &pool_lookup,
            &jupiter,
            &jito,
            &keystore,
            &cfg,
            &mut cap,
            &rl,
            &mut ks,
            &metrics,
            &Arc::new(Mutex::new(WriterInputs::default())),
        );
        assert!(outcome.landed(), "expected Landed, got {:?}", outcome);
    }

    /// A Jito client that always returns `Lost` on `poll_landing`.
    struct LostJito;
    impl JitoClient for LostJito {
        fn health(&self) -> dl_executor::jito::JitoHealth {
            dl_executor::jito::JitoHealth::Up
        }
        fn submit(
            &self,
            bundle: &dl_executor::bundle::Bundle,
        ) -> Result<dl_executor::jito::JitoSubmitResult, dl_executor::error::ExecutorError> {
            Ok(dl_executor::jito::JitoSubmitResult {
                bundle_id: "lost-bundle-1".into(),
                tip_lamports: bundle.total_tip_lamports(),
                submitted_at: 0,
                tip_account: None,
            })
        }
        fn poll_landing(
            &self,
            _bundle_id: &str,
        ) -> Result<LandingResult, dl_executor::error::ExecutorError> {
            Ok(LandingResult::Lost)
        }
    }

    #[test]
    fn submit_opportunity_lost_when_jito_returns_lost() {
        let keystore = fresh_keystore();
        let pool_lookup = |pk: &DlPubkey| Some(dummy_pool(pk.0));
        let kp = Keypair::new();
        let ix = solana_sdk::system_instruction::transfer(&kp.pubkey(), &kp.pubkey(), 0);
        let msg = Message::new(&[ix], Some(&kp.pubkey()));
        let tx = VersionedTransaction {
            signatures: vec![],
            message: VersionedMessage::Legacy(msg),
        };
        let jupiter = BypassJupiter { tx_template: tx };

        let cycle = three_leg_cycle();
        let cfg = default_live_config(&keystore);
        let (mut cap, rl, mut ks) = fresh_safety();
        let metrics = LiveMetrics::new();

        let outcome = submit_opportunity(
            &cycle,
            &pool_lookup,
            &jupiter,
            &LostJito,
            &keystore,
            &cfg,
            &mut cap,
            &rl,
            &mut ks,
            &metrics,
            &Arc::new(Mutex::new(WriterInputs::default())),
        );
        assert!(matches!(outcome, OpportunityOutcome::Lost));
    }

    #[test]
    fn vault_funded_already_funded_branch_handles_correctly() {
        // Both lamport fields are public — verify the struct
        // shape and pattern-matching work.
        let already = VaultFunded::AlreadyFunded {
            vault_lamports: 1_000_000,
            signer_lamports: 1_000_000,
        };
        let funded = VaultFunded::Funded {
            lamports: 500_000,
            signature: "abc123".to_string(),
        };
        // Equality test
        assert_eq!(
            already,
            VaultFunded::AlreadyFunded {
                vault_lamports: 1_000_000,
                signer_lamports: 1_000_000,
            }
        );
        assert_ne!(already, funded);
        assert_eq!(
            funded,
            VaultFunded::Funded {
                lamports: 500_000,
                signature: "abc123".to_string(),
            }
        );
    }

    #[test]
    fn submit_opportunity_not_submitted_when_pool_missing() {
        let keystore = fresh_keystore();
        let jito = MockJitoClient::new();
        let pool_lookup = |_: &DlPubkey| None;
        let kp = Keypair::new();
        let ix = solana_sdk::system_instruction::transfer(&kp.pubkey(), &kp.pubkey(), 0);
        let msg = Message::new(&[ix], Some(&kp.pubkey()));
        let tx = VersionedTransaction {
            signatures: vec![],
            message: VersionedMessage::Legacy(msg),
        };
        let jupiter = BypassJupiter { tx_template: tx };

        let cycle = three_leg_cycle();
        let cfg = default_live_config(&keystore);
        let (mut cap, rl, mut ks) = fresh_safety();
        let metrics = LiveMetrics::new();

        let outcome = submit_opportunity(
            &cycle,
            &pool_lookup,
            &jupiter,
            &jito,
            &keystore,
            &cfg,
            &mut cap,
            &rl,
            &mut ks,
            &metrics,
            &Arc::new(Mutex::new(WriterInputs::default())),
        );
        assert!(matches!(outcome, OpportunityOutcome::NotSubmitted(_)));
    }

    // ─── DAM-99 acceptance tests ─────────────────────────────────
    // The three tests below cover the issue body's acceptance
    // criteria that are testable without a running network:
    //   (1) cap day-rollover resets `realized_pnl_today_lamports` to 0;
    //   (2) kill-switch open does NOT block the writer;
    //   (3) new bundle overwrites the previous `last_landed_bundle`.
    // The fourth (Pyth sol_usd) is already covered by the
    // `live_status_contract` integration test
    // (`sol_usd_renders_with_pyth_mock`).

    /// Test Jito client that returns a Landed result with a
    /// caller-specified `tip_lamports` and `bundle_id`, so the
    /// test can assert the writer picked up exactly those values.
    struct ConfigurableJito {
        bundle_id: String,
        tip_lamports: u64,
    }
    impl JitoClient for ConfigurableJito {
        fn health(&self) -> dl_executor::jito::JitoHealth {
            dl_executor::jito::JitoHealth::Up
        }
        fn submit(
            &self,
            _bundle: &dl_executor::bundle::Bundle,
        ) -> Result<dl_executor::jito::JitoSubmitResult, dl_executor::error::ExecutorError>
        {
            Ok(dl_executor::jito::JitoSubmitResult {
                bundle_id: self.bundle_id.clone(),
                tip_lamports: self.tip_lamports,
                submitted_at: 0,
                tip_account: None,
            })
        }
        fn poll_landing(
            &self,
            _bundle_id: &str,
        ) -> Result<LandingResult, dl_executor::error::ExecutorError> {
            Ok(LandingResult::Landed { slot: 42 })
        }
    }

    /// Build a fully-populated writer `Arc<Mutex<WriterInputs>>`
    /// with `running = true` (the live cycle is up). Tests
    /// observe the writer after `submit_opportunity` returns.
    fn running_writer() -> Arc<Mutex<WriterInputs>> {
        Arc::new(Mutex::new(WriterInputs {
            running: true,
            ..WriterInputs::default()
        }))
    }

    /// (1) Day-rollover resets `realized_pnl_today_lamports` to 0.
    ///
    /// The issue body says the writer's PnL field must reset at
    /// UTC midnight "to match `CapState::reset_if_new_day`." We
    /// pre-load the writer with a stale day index and a non-zero
    /// PnL; after one Landed bundle the day index rolls over and
    /// the PnL field drops to 0 + this bundle's realized_proxy
    /// only.
    #[test]
    fn cap_day_rollover_resets_realized_pnl_today_lamports() {
        let keystore = fresh_keystore();
        let pool_lookup = |pk: &DlPubkey| Some(dummy_pool(pk.0));
        let kp = Keypair::new();
        let ix = solana_sdk::system_instruction::transfer(&kp.pubkey(), &kp.pubkey(), 0);
        let msg = Message::new(&[ix], Some(&kp.pubkey()));
        let tx = VersionedTransaction {
            signatures: vec![],
            message: VersionedMessage::Legacy(msg),
        };
        let jupiter = BypassJupiter { tx_template: tx };
        // tip_lamports of 7_777 so the realized_proxy is
        // distinguishable from the stale PnL seed.
        let jito = ConfigurableJito {
            bundle_id: "rollover-bundle-1".into(),
            tip_lamports: 7_777,
        };
        let cycle = three_leg_cycle();
        let cfg = default_live_config(&keystore);
        let (mut cap, rl, mut ks) = fresh_safety();
        let metrics = LiveMetrics::new();
        let writer = running_writer();
        // Seed: yesterday's PnL is 999_999. The default
        // `last_pnl_reset_day` is 0 (year-0), so the first
        // `reset_if_new_day` call inside `submit_opportunity`
        // will roll over to today and zero the field.
        writer.lock().unwrap().realized_pnl_today_lamports = 999_999;
        let before_today = {
            use chrono::Datelike;
            let g = writer.lock().unwrap();
            chrono::Utc::now()
                .date_naive()
                .num_days_from_ce()
        };
        let _ = before_today; // referenced for the comment only

        let outcome = submit_opportunity(
            &cycle,
            &pool_lookup,
            &jupiter,
            &jito,
            &keystore,
            &cfg,
            &mut cap,
            &rl,
            &mut ks,
            &metrics,
            &writer,
        );
        assert!(outcome.landed(), "expected Landed, got {:?}", outcome);

        // The day-rollover fires (because `last_pnl_reset_day`
        // was 0, today is much later), zeroing the field
        // BEFORE the new bundle's 7_777 is added. Net result
        // is exactly 7_777, NOT 999_999 + 7_777.
        let w = writer.lock().unwrap();
        assert_eq!(
            w.realized_pnl_today_lamports, 7_777,
            "day-rollover did not zero the writer PnL before adding the new bundle"
        );
    }

    /// (2) Kill-switch open does NOT block the writer.
    ///
    /// The acceptance criterion in the DAM-99 body is: "kill-switch
    /// open does not block the writer." That refers to the WRITER
    /// SIDE TASK, not the cycle path. The cycle path is correct
    /// to refuse new bundles when the kill switch is open (the
    /// `ks.check()` gate in `submit_opportunity` short-circuits
    /// to `OpportunityOutcome::NotSubmitted("killswitch open")`).
    /// What must NOT happen is the writer suppressing its tick
    /// when ks is open — the operator console needs to SEE the
    /// open state in `live_status.json`, not a missing file.
    ///
    /// So this test does NOT call `submit_opportunity`. It
    /// pre-loads the writer with a populated Landed snapshot
    /// (as if a bundle had landed just before the kill switch
    /// tripped), trips the kill switch, and asserts that
    /// `tick_once` writes a v1 record with `kill_switch.open =
    /// true` and the previously-landed bundle still visible.
    #[test]
    fn kill_switch_open_does_not_block_writer() {
        let ks_stop = std::env::temp_dir().join(format!(
            "dl-ks-stop-test-{}-v2.flag",
            std::process::id()
        ));
        std::fs::write(&ks_stop, b"").expect("write ks stop file");
        let ks = KillSwitch::with_default_stop_file(&ks_stop);
        // Sanity: ks.check() now returns Err (open).
        assert!(ks.check().is_err(), "precondition: ks must be open");

        // Pre-load the writer as if a bundle had landed just
        // before the kill switch tripped.
        let writer = Arc::new(Mutex::new(WriterInputs {
            running: true,
            last_landed: LastLandedSnapshot {
                bundle_id: Some("pre-trip-bundle".into()),
                ts_unix_ms: Some(1_700_000_000_000),
                profit_lamports: Some(500),
            },
            realized_pnl_today_lamports: 500,
            ..WriterInputs::default()
        }));

        // Build a SharedState and run one tick.
        let cap = Arc::new(Mutex::new(CapState::new(CapConfig::default())));
        let state = crate::live_status::SharedState {
            cap,
            killswitch: Arc::new(Mutex::new(ks)),
            inputs: writer.clone(),
        };
        let path = std::env::temp_dir().join(format!(
            "dl-live-status-ks-test-{}.json",
            std::process::id()
        ));
        crate::live_status::tick_once(&path, &state);

        // The file must exist (the writer ticked despite the
        // open kill switch).
        assert!(path.exists(), "writer must tick even when kill switch is open");
        let body = std::fs::read_to_string(&path).expect("read live_status.json");
        let v: serde_json::Value = serde_json::from_str(&body).expect("parse json");
        assert_eq!(
            v["kill_switch"]["open"], true,
            "writer must surface kill_switch.open=true"
        );
        assert_eq!(
            v["last_landed_bundle"]["bundle_id"], "pre-trip-bundle",
            "previously-landed bundle must still be visible"
        );
        assert_eq!(v["running"], true);

        // Clean up the stop file and the status file so the
        // test does not pollute the host.
        let _ = std::fs::remove_file(&ks_stop);
        let _ = std::fs::remove_file(&path);
    }

    /// (3) New bundle overwrites the previous `last_landed_bundle`.
    ///
    /// Two consecutive Landed bundles with distinct bundle_ids
    /// and tip_lamports: the second one must replace the first
    /// in the writer (the dashboard shows only the most recent
    /// landing, not a history).
    #[test]
    fn new_bundle_overwrites_previous_last_landed() {
        let keystore = fresh_keystore();
        let pool_lookup = |pk: &DlPubkey| Some(dummy_pool(pk.0));
        let kp = Keypair::new();
        let ix = solana_sdk::system_instruction::transfer(&kp.pubkey(), &kp.pubkey(), 0);
        let msg = Message::new(&[ix], Some(&kp.pubkey()));
        let tx = VersionedTransaction {
            signatures: vec![],
            message: VersionedMessage::Legacy(msg),
        };
        let jupiter = BypassJupiter { tx_template: tx };
        let cycle = three_leg_cycle();
        let cfg = default_live_config(&keystore);
        let (mut cap, rl, mut ks) = fresh_safety();
        let metrics = LiveMetrics::new();
        let writer = running_writer();

        // First bundle.
        let jito1 = ConfigurableJito {
            bundle_id: "first-bundle".into(),
            tip_lamports: 100,
        };
        let outcome1 = submit_opportunity(
            &cycle,
            &pool_lookup,
            &jupiter,
            &jito1,
            &keystore,
            &cfg,
            &mut cap,
            &rl,
            &mut ks,
            &metrics,
            &writer,
        );
        assert!(outcome1.landed());
        {
            let w = writer.lock().unwrap();
            assert_eq!(w.last_landed.bundle_id.as_deref(), Some("first-bundle"));
            assert_eq!(w.last_landed.profit_lamports, Some(100));
            assert_eq!(w.realized_pnl_today_lamports, 100);
        }

        // Second bundle — different id, different tip.
        let jito2 = ConfigurableJito {
            bundle_id: "second-bundle".into(),
            tip_lamports: 250,
        };
        let outcome2 = submit_opportunity(
            &cycle,
            &pool_lookup,
            &jupiter,
            &jito2,
            &keystore,
            &cfg,
            &mut cap,
            &rl,
            &mut ks,
            &metrics,
            &writer,
        );
        assert!(outcome2.landed());
        let w = writer.lock().unwrap();
        // last_landed is overwritten (not appended).
        assert_eq!(w.last_landed.bundle_id.as_deref(), Some("second-bundle"));
        assert_eq!(w.last_landed.profit_lamports, Some(250));
        // realized_pnl_today_lamports is the SUM (100 + 250).
        assert_eq!(w.realized_pnl_today_lamports, 350);
    }
}
