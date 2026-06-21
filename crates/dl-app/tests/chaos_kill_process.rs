//! Chaos drill #2: kill the process mid-bundle (before the
//! landing poll completes) and assert no double-submit and
//! cap consistency on restart.
//!
//! In the real pipeline, the process is a single-threaded loop
//! in `dl-app/src/main.rs` that calls `submit_opportunity` per
//! cycle. `kill -9` between the Jito `submit` returning a
//! `bundle_id` and `poll_landing` returning would orphan the
//! bundle on the Block Engine side. The on-host safety
//! guarantee we DO have: there is no retry loop and no
//! background queue — the next process iteration is the only
//! thing that can submit again, and it starts with a fresh
//! in-memory `CapState`. So a kill -9 cannot cause
//! double-submission of the SAME bundle (no auto-retry, no
//! in-process queue), and the cap resets on restart.
//!
//! The drill stands in a stub Jito client that:
//!   - returns `Ok(bundle_id)` on `submit` (we DID send the
//!     bundle to Jito — the cap charge is correct), and
//!   - returns `Err(ExecutorError)` on `poll_landing` (the
//!     "process was killed before landing completed" analog),
//! and asserts:
//!   1. The cap was charged once (at line 310 of `live.rs`)
//!      and NOT refunded on the poll error (the bundle was
//!      actually sent to Jito — the cap correctly accounts
//!      for the tip we paid).
//!   2. The pipeline does NOT have an in-process retry path:
//!      the same process cannot re-submit the same bundle. We
//!      verify this by checking the call count on the stub
//!      Jito client.
//!   3. On "restart" (a fresh `CapState`), a new bundle
//!      submits cleanly and the cap is consistent.

use std::sync::atomic::{AtomicU64, Ordering};
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

/// Jito client that simulates a `kill -9` between `submit`
/// returning and `poll_landing` completing:
///   - `submit` returns Ok with the bundle_id (we DID send
///     the bundle — the tip was actually paid).
///   - `poll_landing` returns Err (the process was killed
///     before landing completed — we don't know if it landed).
///
/// Records call counts so we can assert the pipeline did
/// NOT auto-retry.
struct KilledMidBundleJito {
    submit_calls: Arc<AtomicU64>,
    poll_calls: Arc<AtomicU64>,
}

impl KilledMidBundleJito {
    fn new() -> Self {
        Self {
            submit_calls: Arc::new(AtomicU64::new(0)),
            poll_calls: Arc::new(AtomicU64::new(0)),
        }
    }
}

impl JitoClient for KilledMidBundleJito {
    fn health(&self) -> JitoHealth {
        JitoHealth::Up
    }
    fn submit(
        &self,
        bundle: &dl_executor::bundle::Bundle,
    ) -> Result<JitoSubmitResult, ExecutorError> {
        self.submit_calls.fetch_add(1, Ordering::SeqCst);
        Ok(JitoSubmitResult {
            bundle_id: "killed-mid-bundle".into(),
            tip_lamports: bundle.total_tip_lamports(),
            submitted_at: 0,
            tip_account: None,
        })
    }
    fn poll_landing(&self, _bundle_id: &str) -> Result<LandingResult, ExecutorError> {
        self.poll_calls.fetch_add(1, Ordering::SeqCst);
        // Simulate "process killed before landing completed":
        // the in-flight poll returns an error.
        Err(ExecutorError::JitoSubmit(
            "process killed before landing poll completed".into(),
        ))
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

fn always_positive_simulate(
    _txs: &[VersionedTransaction],
    _assert_pid: Option<&Pubkey>,
) -> Result<
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
}

/// Post-restart Jito: always lands (RPC + Jito are back).
struct LandingJito;
impl JitoClient for LandingJito {
    fn health(&self) -> JitoHealth {
        JitoHealth::Up
    }
    fn submit(
        &self,
        bundle: &dl_executor::bundle::Bundle,
    ) -> Result<JitoSubmitResult, ExecutorError> {
        Ok(JitoSubmitResult {
            bundle_id: "post-restart-bundle".into(),
            tip_lamports: bundle.total_tip_lamports(),
            submitted_at: 0,
            tip_account: None,
        })
    }
    fn poll_landing(&self, _bundle_id: &str) -> Result<LandingResult, ExecutorError> {
        Ok(LandingResult::Landed { slot: 0 })
    }
}

/// Drill: process is killed between `jito.submit` returning Ok
/// and `jito.poll_landing` returning. The pipeline must:
///   1. Charge the cap exactly once (at `try_charge`, before
///      `submit`). The cap is NOT refunded on the poll
///      error — the bundle was actually sent to Jito, the
///      tip was actually paid.
///   2. Surface `OpportunityOutcome::NotSubmitted("poll: ...")`
///      to the operator.
///   3. NOT auto-retry: `submit` is called exactly once for
///      the killed bundle. There is no in-process retry loop
///      that could cause a double-submit of the same bundle.
///
/// On "restart" (a fresh `CapState`), a new bundle submits
/// cleanly and the cap is consistent.
#[test]
fn chaos_kill_process_does_not_double_submit_and_cap_is_consistent_on_restart() {
    let keystore = fresh_keystore();
    let cycle = three_leg_cycle();
    let pool_lookup = |pk: &DlPubkey| Some(dummy_pool(pk.0));
    let cfg = default_live_config();
    let jupiter = make_bypass_jupiter();

    // ── Phase 1: pre-restart process (the one that gets killed) ──────
    let jito = KilledMidBundleJito::new();
    let submit_calls = jito.submit_calls.clone();
    let poll_calls = jito.poll_calls.clone();

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
        Arc::new(always_positive_simulate),
    );

    // The pipeline surfaced the kill as a poll error, not as
    // a landing or a clean NotSubmitted.
    match &outcome {
        OpportunityOutcome::NotSubmitted(reason) => {
            assert!(
                reason.contains("poll"),
                "expected poll-error NotSubmitted, got {reason}",
            );
        }
        other => {
            panic!("expected NotSubmitted(\"poll: ...\") after kill mid-bundle, got {other:?}",)
        }
    }

    // The cap was charged once and NOT refunded: the bundle
    // was actually sent to Jito. The tip was paid. The cap
    // correctly accounts for it.
    let expected_tip = dl_executor::tip::tip_lamports(1_000_000_000, &cfg.tip_config);
    assert_eq!(
        cap.spent_today(),
        expected_tip,
        "cap should reflect the one tip we sent to Jito ({}), \
         got {} — either the cap was double-charged (refund \
         failure) or the cap was charged twice (no retry, but \
         the charge didn't happen at all)",
        expected_tip,
        cap.spent_today(),
    );

    // The pipeline did NOT auto-retry: submit was called
    // exactly once. This is the no-double-submit invariant.
    // (kill -9 cannot be tested in a unit test — we test
    // the post-kill observable: the Jito client saw exactly
    // one submit, so a kill followed by a no-op restart
    // cannot produce a double-submit of this bundle.)
    assert_eq!(
        submit_calls.load(Ordering::SeqCst),
        1,
        "jito.submit was called {} times, expected 1 — pipeline \
         auto-retried after a poll error, which violates the \
         no-double-submit invariant",
        submit_calls.load(Ordering::SeqCst),
    );
    assert_eq!(
        poll_calls.load(Ordering::SeqCst),
        1,
        "jito.poll_landing was called {} times, expected 1",
        poll_calls.load(Ordering::SeqCst),
    );

    // ── Phase 2: simulate "process restart" with a fresh CapState ──
    // The killed bundle is orphaned on the Block Engine side.
    // It MAY or MAY NOT land (we don't know — that's the whole
    // point of the orphan). The new process is the only thing
    // that can submit a NEW bundle, and it starts with a
    // fresh `CapState`.
    let mut cap2 = CapState::new(CapConfig::default());
    let rl2 = RateLimit::new(RateLimitConfig::default());
    let mut ks2 = KillSwitch::new(KillSwitchConfig::default());
    let metrics2 = LiveMetrics::new();
    let jito2 = LandingJito; // post-restart, RPC is back, Jito is healthy

    // Sanity: fresh process is at zero.
    assert_eq!(cap2.spent_today(), 0);

    let outcome2 = submit_opportunity_with_simulate(
        &cycle,
        &pool_lookup,
        &jupiter,
        &jito2,
        &keystore,
        &cfg,
        &mut cap2,
        &rl2,
        &mut ks2,
        &metrics2,
        Arc::new(always_positive_simulate),
    );

    assert!(
        outcome2.landed(),
        "post-restart bundle should land cleanly, got {outcome2:?}",
    );

    // The cap accounts ONLY for the post-restart bundle's tip.
    // The pre-restart orphan's tip is NOT carried over (the
    // in-memory `CapState` does not persist across processes
    // — see `docs/chaos/README.md` §"Known gaps").
    assert_eq!(
        cap2.spent_today(),
        expected_tip,
        "post-restart cap should reflect exactly one bundle's tip \
         ({}), got {} — indicates a leak from the pre-restart \
         process or a double-charge",
        expected_tip,
        cap2.spent_today(),
    );
}
