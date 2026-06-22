//! Opportunity-to-bundle conversion (sub-plan 1b).
//!
//! Bridges `dl-detect::Cycle` (a 3-leg negative cycle) to a real
//! Jito Bundle with signed `VersionedTransaction`s. This is the
//! file where the v2.0 atomicity shape — `[3 swap legs + 1 assert
//! + 1 tip] = 5 txs` — gets assembled.
//!
//! ## Type boundaries
//!
//! - `dl_state::Pubkey` is the project's own `pub struct Pubkey(pub [u8;32])`.
//! - `solana_sdk::Pubkey` is the SDK's opaque Pubkey.
//! - We use `dl_state::Pubkey` for our internal types (`Pool`, `Leg`,
//!   `Cycle`) and convert to `solana_sdk::Pubkey` only at the
//!   Solana SDK boundary (instructions, transactions).
//!
//! ## Flow
//!
//! For a detected `Cycle` with 3 legs:
//! 1. Resolve each leg's pool address → `Pool` data (mints).
//! 2. For each leg, build a Jupiter `QuoteRequest` and call
//!    `JupiterClient::build_swap_tx` to get a `VersionedTransaction`.
//! 3. Build the dl-assert instruction and wrap it in a tx.
//! 4. Build the Jito tip transfer tx.
//! 5. Sign all 5 txs (live.rs does this via `sign_with_keystore`).
//!
//! The atomicity guard is the **4th tx** (dl-assert). It reverts
//! the bundle if `signer.lamports() - vault.lamports() <
//! min_net_pnl_lamports`. See `plan/atomicity-decision.md`.

use solana_sdk::hash::Hash;
use solana_sdk::instruction::Instruction;
use solana_sdk::signature::{SeedDerivable, Signer};
use solana_sdk::system_instruction;
use solana_sdk::transaction::VersionedTransaction;

use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;

use dl_assert_sdk::{build_assert_instruction, derive_vault_pda};
use dl_detect::cycle::{Cycle, Direction, Leg};
use dl_executor::bundle::{BundleBuilder, SwapLeg, TipLeg};
use dl_executor::error::ExecutorError;
use dl_executor::jupiter::{JupiterClient, QuoteRequest};
use dl_executor::metrics::LiveMetrics;
use dl_executor::ExecutorError as ExecErr;
use dl_state::pool::Pool;
use dl_state::Pubkey as DlPubkey;

/// Errors during opportunity → bundle conversion.
#[derive(Debug, thiserror::Error)]
pub enum OpportunityError {
    #[error("cycle must have exactly 3 legs for v2.0 (got {0})")]
    WrongLegCount(usize),
    #[error("pool {0:?} not found in registry")]
    PoolNotFound(DlPubkey),
    #[error("Jupiter error: {0}")]
    Jupiter(String),
    #[error("assert SDK error: {0}")]
    Assert(String),
    #[error("simulate gate rejected the bundle: {0}")]
    SimulateRejected(String),
    #[error("bundle assembly error: {0}")]
    Assembly(String),
}

impl From<OpportunityError> for ExecErr {
    fn from(e: OpportunityError) -> Self {
        ExecErr::BundleAssembly(format!("opportunity: {e}"))
    }
}

/// One leg of a cycle, fully resolved with mint addresses.
#[derive(Debug, Clone)]
pub struct ResolvedLeg {
    pub leg: Leg,
    pub pool: Pool,
    pub input_mint: DlPubkey,
    pub output_mint: DlPubkey,
}

/// Resolve a `Cycle`'s legs against a pool-lookup function. Returns
/// the per-leg `ResolvedLeg` with mint addresses resolved.
pub fn resolve_leg_mints<F>(
    cycle: &Cycle,
    pool_lookup: F,
) -> Result<Vec<ResolvedLeg>, OpportunityError>
where
    F: Fn(&DlPubkey) -> Option<Pool>,
{
    resolve_cycle_legs(cycle, pool_lookup, &dl_executor::metrics::LiveMetrics::new())
}

/// Resolve a `Cycle`'s legs against a pool-lookup function.
///
/// On `OpportunityError::WrongLegCount(n)`, the metric
/// `LiveMetrics::wrong_leg_count_total` is incremented so the
/// dashboard shows a breakdown of cycle lengths that the detector
/// emitted (useful when Phase 2 introduces 2-leg and 4-leg
/// cycles — operators can see at a glance which length dominates).
pub fn resolve_cycle_legs<F>(
    cycle: &Cycle,
    pool_lookup: F,
    metrics: &LiveMetrics,
) -> Result<Vec<ResolvedLeg>, OpportunityError>
where
    F: Fn(&DlPubkey) -> Option<Pool>,
{
    if cycle.legs.len() != 3 {
        metrics.inc_wrong_leg_count(cycle.legs.len());
        return Err(OpportunityError::WrongLegCount(cycle.legs.len()));
    }
    let mut resolved = Vec::with_capacity(3);
    for leg in &cycle.legs {
        let pool = pool_lookup(&leg.pool).ok_or(OpportunityError::PoolNotFound(leg.pool))?;
        let (input_mint, output_mint) = match leg.direction {
            Direction::BaseToQuote => (pool.base_mint, pool.quote_mint),
            Direction::QuoteToBase => (pool.quote_mint, pool.base_mint),
        };
        resolved.push(ResolvedLeg {
            leg: *leg,
            pool,
            input_mint,
            output_mint,
        });
    }
    Ok(resolved)
}

/// Conservative sizing for Phase 1: 0.1 SOL floor, 10 SOL ceiling,
/// scaled by `expected_profit_bps` (100 bps → 1 SOL, 1000 bps → 10 SOL).
pub fn default_input_amount_lamports(cycle: &Cycle) -> u64 {
    const MIN: u64 = 100_000_000; // 0.1 SOL
    const MAX: u64 = 10_000_000_000; // 10 SOL
    let bps = cycle.expected_profit_bps.max(0) as u64;
    let scaled = MIN.saturating_mul(bps.max(1)) / 100;
    scaled.clamp(MIN, MAX)
}

/// `dl_state::Pubkey` ↔ `solana_sdk::pubkey::Pubkey` conversion
/// (both are 32-byte arrays underneath). Phase 2 M5: a `From`
/// impl here is blocked by Rust's orphan rule (we can't impl
/// `From` for `solana_sdk::pubkey::Pubkey` in a downstream crate
/// without solana_sdk defining a marker trait for it). The
/// helper function is the idiomatic workaround; inlined at all
/// call sites in this crate. Conversion is O(1) and zero-cost.
pub fn dl_to_solana_pubkey(pk: &DlPubkey) -> solana_sdk::pubkey::Pubkey {
    solana_sdk::pubkey::Pubkey::new_from_array(pk.0)
}

/// Output of `build_unsigned_bundle`: the 5 signed `VersionedTransaction`s
/// + the per-leg metadata needed to satisfy `BundleBuilder`.
pub struct UnsignedBundleOutput {
    pub signed_transactions: Vec<VersionedTransaction>,
    pub legs: Vec<SwapLeg>,
}

/// Build the unsigned 5-tx bundle. Signing happens elsewhere
/// (in `live.rs::submit_opportunity` via `sign_with_keystore`).
///
/// Returns the 5 `VersionedTransaction`s in order:
/// `[swap_leg_0, swap_leg_1, swap_leg_2, assert, tip]`. The
/// `recent_blockhash` is set on all 5 messages; signatures are
/// placeholders that the caller overwrites.
///
/// `leg_metas` carries each leg's Jupiter `out_amount` so that
/// `dl-recon` can compare predicted vs realized against Jupiter's
/// actual quote (Phase 2 calibration data).
pub fn build_unsigned_bundle(
    cycle: &Cycle,
    pool_lookup: impl Fn(&DlPubkey) -> Option<Pool>,
    jupiter: &dyn JupiterClient,
    metrics: &LiveMetrics,
    assert_program_id: solana_sdk::pubkey::Pubkey,
    min_net_pnl_lamports: u64,
    tip_lamports: u64,
    tip_account: solana_sdk::pubkey::Pubkey,
    signer_pubkey: solana_sdk::pubkey::Pubkey,
    recent_blockhash: Hash,
) -> Result<UnsignedBundleOutput, OpportunityError> {
    let resolved = resolve_cycle_legs(cycle, pool_lookup, metrics)?;
    let input_amount = default_input_amount_lamports(cycle);

    // Build the 3 swap txs via Jupiter.
    let mut swap_txs: Vec<VersionedTransaction> = Vec::with_capacity(3);
    let mut leg_metas: Vec<SwapLeg> = Vec::with_capacity(3);
    for (i, leg) in resolved.iter().enumerate() {
        let req = QuoteRequest::new(
            dl_to_solana_pubkey(&leg.input_mint).to_string(),
            dl_to_solana_pubkey(&leg.output_mint).to_string(),
            input_amount,
            50, // 0.5% slippage
        );
        let (quote, swap_tx) = jupiter
            .build_swap_tx(&req, &signer_pubkey)
            .map_err(|e| OpportunityError::Jupiter(format!("{e}")))?;
        // Persist Jupiter's quoted `out_amount` + the slippage
        // threshold in the bundle metadata. `dl-recon` reads
        // these to compare predicted vs realized.
        let predicted_out = quote.out_amount;
        leg_metas.push(SwapLeg::new(
            format!("leg-{i}"),
            dl_to_solana_pubkey(&leg.input_mint).to_string(),
            dl_to_solana_pubkey(&leg.output_mint).to_string(),
            input_amount,
            predicted_out,
        ));
        swap_txs.push(swap_tx);
    }

    // Build the assert tx.
    let (vault, _bump) = derive_vault_pda(&signer_pubkey, &assert_program_id);
    let assert_ix = build_assert_instruction(
        assert_program_id,
        signer_pubkey,
        vault,
        min_net_pnl_lamports,
    );
    let assert_tx =
        build_single_instruction_tx(signer_pubkey, assert_ix, recent_blockhash)
            .map_err(|e| OpportunityError::Assert(format!("{e}")))?;

    // Build the tip tx (system transfer to Jito tip account).
    let tip_ix = system_instruction::transfer(&signer_pubkey, &tip_account, tip_lamports);
    let tip_tx = build_single_instruction_tx(signer_pubkey, tip_ix, recent_blockhash)
        .map_err(|e| OpportunityError::Assembly(format!("tip tx: {e}")))?;

    let mut all = swap_txs;
    all.push(assert_tx);
    all.push(tip_tx);

    Ok(UnsignedBundleOutput {
        signed_transactions: all,
        legs: leg_metas,
    })
}

/// Wrap a single instruction into a `VersionedTransaction` with
/// the given fee-payer + blockhash. Signs with a placeholder
/// signature (the caller overwrites via `sign_with_keystore`).
fn build_single_instruction_tx(
    fee_payer: solana_sdk::pubkey::Pubkey,
    ix: Instruction,
    recent_blockhash: Hash,
) -> Result<VersionedTransaction, ExecErr> {
    use solana_sdk::message::Message;
    use solana_sdk::signature::Signature;
    let mut msg = Message::new(&[ix], Some(&fee_payer));
    // Message::new leaves recent_blockhash at Hash::default().
    // We need to set it explicitly to the caller's value.
    msg.recent_blockhash = recent_blockhash;
    let v0_msg = solana_sdk::message::VersionedMessage::Legacy(msg);
    // Build the tx directly without going through try_new's
    // signer-pubkey validation. Placeholder signatures are
    // overwritten by sign_with_keystore in live.rs. The number of
    // signatures must match `msg.header.num_required_signatures`
    // (1 for a fee-payer-only message).
    let n_sigs = v0_msg.header().num_required_signatures as usize;
    let signatures = vec![Signature::default(); n_sigs];
    Ok(VersionedTransaction {
        signatures,
        message: v0_msg,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use dl_detect::cycle::{Direction, Leg};
    use dl_state::pool::Pool;
    use solana_sdk::hash::Hash;
    use solana_sdk::message::{Message, VersionedMessage};
    use solana_sdk::signer::keypair::Keypair;
    use solana_sdk::signer::Signer;

    fn dummy_pool(addr: [u8; 32], base: [u8; 32], quote: [u8; 32]) -> Pool {
        Pool {
            address: DlPubkey(addr),
            kind: dl_state::pool::AmmKind::RaydiumAmmV4,
            base_mint: DlPubkey(base),
            quote_mint: DlPubkey(quote),
            base_decimals: 9,
            quote_decimals: 6,
            base_reserve: 1_000_000_000,
            quote_reserve: 100_000_000_000,
            fee_bps: 30,
            last_update_slot: 0,
            ..Default::default()
        }
    }

    fn dummy_leg(pool: [u8; 32]) -> Leg {
        Leg {
            pool: DlPubkey(pool),
            direction: Direction::BaseToQuote,
            weight: -100,
        }
    }

    fn three_leg_cycle() -> Cycle {
        Cycle::new(vec![
            dummy_leg([1u8; 32]),
            dummy_leg([2u8; 32]),
            dummy_leg([3u8; 32]),
        ])
    }

    #[test]
    fn resolve_cycle_legs_rejects_non_three() {
        use dl_executor::metrics::LiveMetrics;
        let metrics = LiveMetrics::new();
        let cycle = Cycle::new(vec![dummy_leg([1u8; 32])]);
        let res = resolve_cycle_legs(&cycle, |_| None, &metrics);
        assert!(matches!(res, Err(OpportunityError::WrongLegCount(1))));
        assert_eq!(metrics.wrong_leg_count_total(1), 1);
    }

    #[test]
    fn resolve_cycle_legs_succeeds_with_pool_lookup() {
        use dl_executor::metrics::LiveMetrics;
        let metrics = LiveMetrics::new();
        let cycle = three_leg_cycle();
        let res = resolve_cycle_legs(&cycle, |pk| {
            Some(dummy_pool(pk.0, [10u8; 32], [20u8; 32]))
        }, &metrics)
        .unwrap();
        assert_eq!(res.len(), 3);
        for r in &res {
            assert_eq!(r.input_mint, DlPubkey([10u8; 32]));
            assert_eq!(r.output_mint, DlPubkey([20u8; 32]));
        }
    }

    #[test]
    fn resolve_cycle_legs_errors_when_pool_missing() {
        use dl_executor::metrics::LiveMetrics;
        let metrics = LiveMetrics::new();
        let cycle = three_leg_cycle();
        let res = resolve_cycle_legs(&cycle, |_| None, &metrics);
        assert!(matches!(res, Err(OpportunityError::PoolNotFound(_))));
    }

    #[test]
    fn default_input_amount_scales_with_profit_bps() {
        let mut cycle = three_leg_cycle();
        cycle.expected_profit_bps = 0;
        assert_eq!(default_input_amount_lamports(&cycle), 100_000_000);
        cycle.expected_profit_bps = 100;
        assert_eq!(default_input_amount_lamports(&cycle), 100_000_000);
        cycle.expected_profit_bps = 1_000;
        assert_eq!(default_input_amount_lamports(&cycle), 1_000_000_000);
        cycle.expected_profit_bps = 10_000;
        assert_eq!(default_input_amount_lamports(&cycle), 10_000_000_000);
        cycle.expected_profit_bps = -5_000;
        assert_eq!(default_input_amount_lamports(&cycle), 100_000_000);
    }

    #[test]
    fn build_single_instruction_tx_round_trips() {
        let kp = Keypair::new();
        let ix = system_instruction::transfer(
            &kp.pubkey(),
            &solana_sdk::pubkey::Pubkey::new_unique(),
            1000,
        );
        let bh = Hash::new_unique();
        let tx = build_single_instruction_tx(kp.pubkey(), ix, bh).unwrap();
        match &tx.message {
            VersionedMessage::Legacy(m) => {
                assert_eq!(m.recent_blockhash, bh);
                // Message::new puts the SystemProgram first, fee-payer
                // second. Check fee_payer is in account_keys.
                assert!(
                    m.account_keys.contains(&kp.pubkey()),
                    "fee_payer must be in account_keys"
                );
            }
            _ => panic!("expected Legacy message"),
        }
    }

    #[test]
    fn build_unsigned_bundle_produces_5_txs() {
        use dl_executor::jupiter::{JupiterQuote, JupiterRouteStep};
        // Custom JupiterClient that bypasses bincode decode so the
        // test focuses on the 5-tx assembly, not on bincode round-
        // tripping through Jupiter's wire format.
        struct BypassJupiter {
            tx_template: VersionedTransaction,
        }
        impl JupiterClient for BypassJupiter {
            fn quote(&self, _req: &QuoteRequest) -> Result<JupiterQuote, ExecErr> {
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
            ) -> Result<String, ExecErr> {
                Ok(String::new())
            }
            fn build_swap_tx(
                &self,
                req: &QuoteRequest,
                _user_pubkey: &solana_sdk::pubkey::Pubkey,
            ) -> Result<(JupiterQuote, VersionedTransaction), ExecErr> {
                Ok((self.quote(req)?, self.tx_template.clone()))
            }
        }

        let kp = Keypair::new();
        let ix = system_instruction::transfer(
            &kp.pubkey(),
            &solana_sdk::pubkey::Pubkey::new_unique(),
            0,
        );
        let msg = Message::new(&[ix], Some(&kp.pubkey()));
        let tx = VersionedTransaction {
            signatures: vec![],
            message: VersionedMessage::Legacy(msg),
        };
        let mock = BypassJupiter { tx_template: tx };

        let cycle = three_leg_cycle();
        let assert_program = solana_sdk::pubkey::Pubkey::new_unique();
        let signer = solana_sdk::pubkey::Pubkey::new_unique();
        let tip_account = solana_sdk::pubkey::Pubkey::new_unique();
        let bh = Hash::new_unique();

        let pool_lookup = |pk: &DlPubkey| Some(dummy_pool(pk.0, [10u8; 32], [20u8; 32]));
        use dl_executor::metrics::LiveMetrics;
        let metrics = LiveMetrics::new();
        let output = build_unsigned_bundle(
            &cycle,
            pool_lookup,
            &mock,
            &metrics,
            assert_program,
            50_000,
            10_000,
            tip_account,
            signer,
            bh,
        )
        .unwrap();
        assert_eq!(output.signed_transactions.len(), 5);
        // Leg metadata carries the Jupiter quote's out_amount
        // (the mock returns 2 for every leg).
        assert!(output.legs.iter().all(|l| l.expected_out == 2));
        // Last tx is the tip tx.
        match &output.signed_transactions[4].message {
            VersionedMessage::Legacy(m) => {
                assert!(m.account_keys.contains(&tip_account));
            }
            _ => panic!("expected Legacy message"),
        }
    }
}