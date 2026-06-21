//! Chaos drill #1: drop the RPC connection mid-submit and assert
//! the cap is not double-charged and is consistent on restart.
//!
//! In the production pipeline (`dl-app/src/live.rs`), the simulate
//! gate constructs a fresh `RpcClient` on every call to
//! `submit_opportunity`. A dropped mid-trade RPC manifests as the
//! simulate-fn closure returning `Err(...)`, which the pipeline
//! converts to `OpportunityOutcome::Rejected("simulate: ...")`
//! and refunds the cap charge (see `live.rs:362`). The drill
//! substitutes a stub `simulate_fn` that returns that exact error
//! to exercise the cap-refund path, then restarts the pipeline
//! (a fresh `CapState`) and asserts that no tip leaked across
//! the "process boundary" — i.e. the cap is consistent.
//!
//! Note: `CapState` is currently in-memory only, so "consistent
//! on restart" today means "fresh cap, no leak from the prior
//! process" — this is by design for the v2.0 in-memory model
//! and is recorded as a known gap in `docs/chaos/README.md`.

use std::sync::Arc;

use dl_app::live::{submit_opportunity_with_simulate, LiveConfig, OpportunityOutcome};
use dl_detect::cycle::{Cycle, Direction, Leg};
use dl_executor::error::ExecutorError;
use dl_executor::jito::{JitoClient, JitoHealth, JitoSubmitResult, LandingResult};
use dl_executor::jupiter::{JupiterClient, JupiterQuote, JupiterRouteStep};
use dl_executor::killswitch::{KillSwitch, KillSwitchConfig};
use dl_executor::metrics::LiveMetrics;
use dl_executor::tip::TipConfig;
use dl_signer::cap::{CapConfig, CapState};
use dl_signer::keystore::{KeyFile, KeyStore};
use dl_signer::ratelimit::{RateLimit, RateLimitConfig};
use dl_state::pool::{AmmKind, Pool};
use dl_state::Pubkey as DlPubkey;
use solana_sdk::hash::Hash;
use solana_sdk::message::{Message, VersionedMessage};
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signer::keypair::Keypair;
use solana_sdk::signer::Signer as SdkSigner;
use solana_sdk::transaction::VersionedTransaction;

fn fresh_keystore() -> KeyStore {
    let kf = KeyFile::new("chaos-test-passphrase");
    let secret = kf.decrypt("chaos-test-passphrase").unwrap();
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

struct BypassJupiter {
    tx_template: VersionedTransaction,
}

impl JupiterClient for BypassJupiter {
    fn quote(
        &self,
        _req: &dl_executor::jupiter::QuoteRequest,
    ) -> Result<JupiterQuote, ExecutorError> {
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
        _user_pubkey: &Pubkey,
    ) -> Result<String, ExecutorError> {
        Ok(String::new())
    }
    fn build_swap_tx(
        &self,
        req: &dl_executor::jupiter::QuoteRequest,
        _u: &Pubkey,
    ) -> Result<(JupiterQuote, VersionedTransaction), ExecutorError> {
        Ok((self.quote(req)?, self.tx_template.clone()))
    }
}

/// Jito client that always lands successfully. We don't expect
/// `submit` or `poll_landing` to fire during the RPC-drop drill
/// (the simulate gate rejects first), but they must still satisfy
/// the trait.
struct LandingJito;
impl JitoClient for LandingJito {
    fn health(&self) -> JitoHealth {
        JitoHealth::Up
    }
    fn submit(
        &self,
        _bundle: &dl_executor::bundle::Bundle,
    ) -> Result<JitoSubmitResult, ExecutorError> {
        Ok(JitoSubmitResult {
            bundle_id: "chaos-bundle".into(),
            tip_lamports: 0,
            submitted_at: 0,
            tip_account: None,
        })
    }
    fn poll_landing(&self, _bundle_id: &str) -> Result<LandingResult, ExecutorError> {
        Ok(LandingResult::Landed { slot: 0 })
    }
}

fn default_live_config() -> LiveConfig {
    LiveConfig {
        assert_program_id: Pubkey::new_unique(),
        tip_config: TipConfig::default(),
        simulate_rpc_url: None,
        tip_account: Pubkey::new_unique(),
        recent_blockhash: Hash::new_unique(),
        pyth: None,
        pyth_feeds: std::collections::HashMap::new(),
        pyth_max_age_secs: 60,
        niche_config: None,
    }
}

fn make_bypass_jupiter() -> BypassJupiter {
    let kp = Keypair::new();
    let ix = solana_sdk::system_instruction::transfer(&kp.pubkey(), &kp.pubkey(), 0);
    let msg = Message::new(&[ix], Some(&kp.pubkey()));
    let tx = VersionedTransaction {
        signatures: vec![],
        message: VersionedMessage::Legacy(msg),
    };
    BypassJupiter { tx_template: tx }
}

/// Simulate-fn that simulates a dropped RPC connection: returns
/// `Err(ExecutorError::SimulateFailed(...))` exactly as the real
/// `RpcClient` would on a TCP reset mid-`simulateTransaction` call.
fn dropped_rpc_simulate(
    _txs: &[VersionedTransaction],
    _assert_pid: Option<&Pubkey>,
) -> Result<
    (
        dl_executor::simulate::SimulationReport,
        dl_executor::simulate::SimulateVerdict,
    ),
    ExecutorError,
> {
    Err(ExecutorError::SimulateFailed(
        "connection reset by peer (simulated mid-trade RPC drop)".into(),
    ))
}

/// Drill: the RPC connection drops during the simulate gate. The
/// pipeline must:
///   1. NOT submit the bundle to Jito (the simulate gate is
///      fail-closed and precedes the Jito submit).
///   2. Refund the cap charge that was applied at line 310 of
///      `live.rs` (the only `try_charge` call site, before
///      simulate).
///   3. Surface `OpportunityOutcome::Rejected("simulate: ...")`
///      to the operator, not `Landed`.
///
/// On "restart" (a fresh `CapState`), a new bundle must be
/// able to charge cleanly — the cap is consistent.
#[test]
fn chaos_kill_rpc_does_not_double_charge_and_cap_is_consistent_on_restart() {
    let keystore = fresh_keystore();
    let jito = LandingJito;
    let jupiter = make_bypass_jupiter();
    let cycle = three_leg_cycle();
    let pool_lookup = |pk: &DlPubkey| Some(dummy_pool(pk.0));
    let cfg = default_live_config();

    // ── Phase 1: pre-restart process ────────────────────────────────────
    // 1 SOL input, default tip config → 1_000_000_000 * 50 / 10_000
    // = 5_000_000 lamports tip, floored at min_lamports=10_000.
    let mut cap = CapState::new(CapConfig::default());
    let rl = RateLimit::new(RateLimitConfig::default());
    let mut ks = KillSwitch::new(KillSwitchConfig::default());
    let metrics = LiveMetrics::new();

    let outcome = submit_opportunity_with_simulate(
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
        Arc::new(dropped_rpc_simulate),
    );

    // The bundle was NOT submitted. The simulate gate rejected
    // it before `jito.submit` was called. The `LandingJito` was
    // never reached, so it cannot have reported `Landed`.
    match &outcome {
        OpportunityOutcome::Rejected(reason) => {
            assert!(
                reason.contains("simulate"),
                "expected simulate-gate rejection, got {reason}",
            );
        }
        other => panic!("expected Rejected (simulate gate fail-closed), got {other:?}",),
    }

    // The cap was charged at line 310, then refunded at line 362
    // (the simulate-fn Err branch). Net: 0 lamports spent.
    assert_eq!(
        cap.spent_today(),
        0,
        "cap was not refunded after simulate gate error: spent_today = {}",
        cap.spent_today(),
    );
    assert_eq!(
        cap.remaining(),
        CapConfig::default().daily_lamports,
        "cap should be at full daily limit after refund",
    );

    // ── Phase 2: simulate "process restart" with a fresh CapState ──────
    // The pre-restart bundle is dead on the floor. There is no
    // idempotency key, no persistent queue, no in-process
    // background thread that could re-submit it. A new
    // process iteration is the only thing that can submit a
    // bundle, and it starts with a fresh `CapState` (this is
    // the current in-memory design — see
    // `docs/chaos/README.md` §"Known gaps" for the persistence
    // gap, and `docs/runbook.md` for the on-call implication).
    let mut cap2 = CapState::new(CapConfig::default());
    let rl2 = RateLimit::new(RateLimitConfig::default());
    let mut ks2 = KillSwitch::new(KillSwitchConfig::default());
    let metrics2 = LiveMetrics::new();

    // Sanity: a fresh process is at zero.
    assert_eq!(cap2.spent_today(), 0);

    // The fresh process submits a NEW bundle cleanly. The
    // pre-restart bundle's tip is NOT carried over.
    let outcome2 = submit_opportunity_with_simulate(
        &cycle,
        &pool_lookup,
        &jupiter,
        &jito,
        &keystore,
        &cfg,
        &mut cap2,
        &rl2,
        &mut ks2,
        &metrics2,
        Arc::new(
            |_txs: &[VersionedTransaction],
             _assert_pid: Option<&Pubkey>|
             -> Result<
                (
                    dl_executor::simulate::SimulationReport,
                    dl_executor::simulate::SimulateVerdict,
                ),
                ExecutorError,
            > {
                Ok((
                    dl_executor::simulate::SimulationReport {
                        net_pnl_lamports: None,
                        compute_units: 0,
                        logs: Vec::new(),
                        all_txs_ok: true,
                    },
                    dl_executor::simulate::SimulateVerdict::Positive,
                ))
            },
        ),
    );

    assert!(
        outcome2.landed(),
        "post-restart bundle should land cleanly, got {outcome2:?}",
    );

    // The cap accounts ONLY for the post-restart bundle's tip —
    // exactly one tip's worth, not two.
    let post_tip = dl_executor::tip::tip_lamports(1_000_000_000, &cfg.tip_config);
    assert_eq!(
        cap2.spent_today(),
        post_tip,
        "post-restart cap should reflect exactly one bundle's tip \
         ({}), got {} — indicates a leak from the pre-restart \
         process or a double-charge",
        post_tip,
        cap2.spent_today(),
    );
}
