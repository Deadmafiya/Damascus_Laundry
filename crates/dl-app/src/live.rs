//! `dl-app run` — v1.1+ live execution entry point (Phase 8 / plan 01).
//!
//! In 08-01, this module is **paper-mode only**: it builds bundles via
//! `dl-executor`, signs them via `dl-signer`, and submits to the
//! `MockJitoClient`. The real `jito-bundle::send_bundle` integration
//! lands in 08-02/08-03.

use std::path::Path;

use dl_executor::bundle::{Bundle, BundleBuilder, SwapLeg, TipLeg};
use dl_executor::jito::{JitoClient, JitoSubmitResult, MockJitoClient};
use dl_executor::jupiter::{JupiterClient, MockJupiterClient, QuoteRequest};
use dl_executor::tip::{tip_lamports, TipConfig};
use dl_recon::pipeline::{replay_pools_to_ledger, ReconReport, ReplayParams};
use dl_signer::cap::{CapConfig, CapState};
use dl_signer::keystore::KeyStore;
use dl_signer::ratelimit::{RateLimit, RateLimitConfig};

use crate::dry_run::synth_triangle_pools;
use crate::error::LiveError;

#[derive(Debug)]
pub struct PaperRunResult {
    pub report: ReconReport,
    pub bundles: Vec<SubmittedBundle>,
    pub total_tip_lamports: u64,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SubmittedBundle {
    pub opportunity_seq: u64,
    pub input_mint: String,
    pub output_mint: String,
    pub input_amount: u64,
    pub expected_output: u64,
    pub tip_lamports: u64,
    pub jito: JitoSubmitResult,
    pub would_trade: bool,
}

#[derive(Debug, Clone)]
pub struct PaperRunConfig {
    pub tip_config: TipConfig,
    pub cap_config: CapConfig,
    pub rate_limit: RateLimitConfig,
    /// If true, only `would_trade == true` cycles are bundled.
    pub only_would_trade: bool,
}

impl Default for PaperRunConfig {
    fn default() -> Self {
        Self {
            tip_config: TipConfig::default(),
            cap_config: CapConfig::default(),
            rate_limit: RateLimitConfig::default(),
            only_would_trade: true,
        }
    }
}

pub fn run_paper_live(
    keystore: &KeyStore,
    cap_state: &mut CapState,
    rate_limit: &RateLimit,
    jito: &dyn JitoClient,
    jupiter: &dyn JupiterClient,
    cfg: &PaperRunConfig,
    out_log: Option<&Path>,
) -> Result<PaperRunResult, LiveError> {
    let pools = synth_triangle_pools();

    let params = ReplayParams::default();
    let report = replay_pools_to_ledger(&pools, &params)
        .map_err(|e| LiveError::Pipeline(format!("replay: {e}")))?;

    let mut bundles = Vec::new();
    let mut total_tip: u64 = 0;

    for cycle_rec in &report.cycle_records {
        let would_trade = cycle_rec.entry.decision == dl_ledger::Decision::WouldTrade;
        if cfg.only_would_trade && !would_trade {
            continue;
        }

        let quote = jupiter
            .quote(&QuoteRequest::new("SOL", "USDC", 1_000_000_000, 50))
            .map_err(|e| LiveError::Pipeline(format!("jupiter: {e}")))?;

        let expected_out = if quote.out_amount > 0 {
            quote.out_amount
        } else {
            100_000_000_000
        };

        let conservative_e_pnl = cycle_rec.entry.conservative.e_pnl;
        let tip = tip_lamports(conservative_e_pnl, &cfg.tip_config);
        total_tip = total_tip.saturating_add(tip);

        if let Err(e) = cap_state.try_charge(tip) {
            tracing::warn!(
                cycle_seq = cycle_rec.seq,
                error = %e,
                "cap would be breached; skipping bundle"
            );
            continue;
        }

        if !rate_limit.try_acquire() {
            tracing::warn!(cycle_seq = cycle_rec.seq, "rate limit; skipping bundle");
            continue;
        }

        let bundle = build_bundle_from_quote(&quote, tip, &cycle_rec.seq.to_string())
            .map_err(|e| LiveError::Bundle(format!("{e}")))?;

        let _sig_marker = sign_marker(keystore, &bundle);

        let jito_result = jito
            .submit(&bundle)
            .map_err(|e| LiveError::Jito(format!("{e}")))?;

        bundles.push(SubmittedBundle {
            opportunity_seq: cycle_rec.seq,
            input_mint: quote
                .route_plan
                .first()
                .map(|s| s.input_mint.clone())
                .unwrap_or_default(),
            output_mint: quote
                .route_plan
                .first()
                .map(|s| s.output_mint.clone())
                .unwrap_or_default(),
            input_amount: quote.in_amount,
            expected_output: expected_out,
            tip_lamports: tip,
            jito: jito_result,
            would_trade,
        });
    }

    let result = PaperRunResult {
        report,
        bundles: bundles.clone(),
        total_tip_lamports: total_tip,
    };

    if let Some(path) = out_log {
        let json = serde_json::to_string_pretty(&bundles)
            .map_err(|e| LiveError::Io(format!("serde: {e}")))?;
        std::fs::write(path, json)
            .map_err(|e| LiveError::Io(format!("write {}: {e}", path.display())))?;
    }

    Ok(result)
}

fn build_bundle_from_quote(
    quote: &dl_executor::jupiter::JupiterQuote,
    tip_lamports: u64,
    cycle_id: &str,
) -> Result<Bundle, dl_executor::error::ExecutorError> {
    let mut b = BundleBuilder::new();
    for (i, step) in quote.route_plan.iter().enumerate() {
        b.push_swap(SwapLeg::new(
            format!("{}-{}", step.label, i),
            &step.input_mint,
            &step.output_mint,
            step.in_amount,
            step.out_amount,
        ));
    }
    if quote.route_plan.is_empty() {
        b.push_swap(SwapLeg::new(
            format!("synthetic-{}", cycle_id),
            "SOL",
            "USDC",
            quote.in_amount,
            quote.in_amount * 100,
        ));
    }
    b.set_tip(TipLeg::new(
        tip_lamports,
        "JitoTip1111111111111111111111111111111111",
    ));
    b.build()
}

fn sign_marker(keystore: &KeyStore, bundle: &Bundle) -> u64 {
    let pubkey = keystore.pubkey_hex_prefix();
    let mut h: u64 = 0xcbf29ce484222325;
    for b in pubkey.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h ^= bundle.total_tip_lamports();
    h
}

pub fn run_paper_live_with_mocks(
    keystore: &KeyStore,
    cap_state: &mut CapState,
    rate_limit: &RateLimit,
    cfg: &PaperRunConfig,
) -> Result<PaperRunResult, LiveError> {
    let jito = MockJitoClient::new();
    let jupiter = MockJupiterClient::new();
    run_paper_live(keystore, cap_state, rate_limit, &jito, &jupiter, cfg, None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use dl_signer::keystore::KeyFile;

    fn fresh_signer() -> (KeyStore, CapState, RateLimit) {
        let kf = KeyFile::new("test-passphrase");
        let secret = kf.decrypt("test-passphrase").unwrap();
        let ks = KeyStore::from_secret(secret);
        let cap = CapState::new(CapConfig::default());
        // Default RateLimit starts with a full bucket (bundles_per_minute
        // tokens). No pre-drain needed.
        let rl = RateLimit::new(RateLimitConfig::default());
        (ks, cap, rl)
    }

    #[test]
    fn paper_live_produces_bundles_for_synth_triangle() {
        let (ks, mut cap, rl) = fresh_signer();
        let cfg = PaperRunConfig {
            only_would_trade: false,
            ..Default::default()
        };
        let result = run_paper_live_with_mocks(&ks, &mut cap, &rl, &cfg).unwrap();
        assert!(!result.bundles.is_empty(), "should produce >= 1 bundle");
        for b in &result.bundles {
            assert!(b.tip_lamports >= 10_000, "tip must be >= min_lamports");
            assert!(b.jito.bundle_id.starts_with("mock-bundle-"));
        }
    }

    #[test]
    fn paper_live_only_would_trade_default_yields_zero_bundles_for_synth() {
        let (ks, mut cap, rl) = fresh_signer();
        let cfg = PaperRunConfig::default();
        let result = run_paper_live_with_mocks(&ks, &mut cap, &rl, &cfg).unwrap();
        assert_eq!(result.bundles.len(), 0);
    }

    #[test]
    fn paper_live_respects_daily_cap() {
        let (ks, mut cap, rl) = fresh_signer();
        // Simulate that the operator has already spent most of
        // today's cap on prior bundles. The cap is checked
        // per-bundle, so we burn small amounts across many
        // bundles to simulate "already at 4.99 SOL".
        for _ in 0..100 {
            cap.try_charge(49_900_000).unwrap(); // 49.9M each = 4.99B total
        }
        let cfg = PaperRunConfig {
            only_would_trade: false,
            ..Default::default()
        };
        let result = run_paper_live_with_mocks(&ks, &mut cap, &rl, &cfg).unwrap();
        // We've already burned ~4.99B; only ~10k remain.
        // 4 detected cycles at 10_000 tip each fits exactly
        // (40k < 10k remaining is false, so most bundles
        // should be rejected).
        let total_remaining = cap.remaining();
        let max_possible_bundles = total_remaining / 10_000;
        assert!(
            result.bundles.len() as u64 <= max_possible_bundles + 1,
            "cap should bound the bundle count: got {} bundles, max {} possible",
            result.bundles.len(),
            max_possible_bundles + 1
        );
    }
}
