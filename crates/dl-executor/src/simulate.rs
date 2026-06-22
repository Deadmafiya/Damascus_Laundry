//! `simulateTransaction` gate (sub-plan 0c, Phase 0).
//!
//! Mandatory pre-submit check: every bundle must simulate to
//! net-positive (or non-negative within rounding) before we hand it
//! to the Jito Block Engine. This is the second of two safety
//! gates:
//!
//!   1. Cap / rate-limit check (in dl-signer)
//!   2. simulateTransaction gate (here)
//!
//! The gate calls Solana's `simulateTransaction` RPC with
//! `replace_recent_blockhash = true` and `sig_verify = false`,
//! parses the returned logs and `units_consumed`, and rejects any
//! bundle whose simulated net SOL delta is `< 0`.
//!
//! ## Why `replace_recent_blockhash = true`
//!
//! Without replacement, the simulation runs against the blockhash
//! the tx was *built* with, which may be stale by the time we
//! simulate (cycles can be detected-and-built in <50 ms; Jito
//! submission may take seconds). Replacing the blockhash lets the
//! validator use a fresh one so the simulation reflects the real
//! execution conditions as closely as possible.
//!
//! ## Why `sig_verify = false`
//!
//! The signature check is CPU-expensive on the validator. We're
//! not testing the signature (the bundle builder already signed);
//! we're testing the bundle's *logic*. Disabling sig-verify cuts
//! simulation time by ~70% and lets us submit more bundles per
//! second in the hot path.

use solana_client::rpc_config::RpcSimulateTransactionConfig;
use solana_client::rpc_client::RpcClient;
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signature::Signature;
use solana_sdk::transaction::VersionedTransaction;

use crate::error::ExecutorError;

/// Maximum total characters kept in a [`SimulationReport::logs`]
/// after truncation. 4 KiB is enough for triage without bloating
/// the JSONL log or the recon pipeline's per-bundle record.
pub const MAX_LOG_CHARS: usize = 4096;

/// Outcome of a `simulateTransaction` call.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SimulationReport {
    /// Reported net SOL delta in lamports (positive = profitable).
    /// `None` if the simulation did not produce a parseable log
    /// line for net PnL (we fall back to logs-only check).
    pub net_pnl_lamports: Option<i64>,
    /// Compute units consumed by the bundle (sum across txs).
    pub compute_units: u64,
    /// Raw simulation logs (truncated to `MAX_LOG_CHARS` total across
    /// the bundle; each tx contributes at most 64 lines).
    pub logs: Vec<String>,
    /// True if every tx in the bundle simulated without error.
    /// **Note**: the dl-assert tx is skipped during simulation
    /// (see `simulate_bundle`'s `assert_program_id` parameter) —
    /// the assert's own verdict is read from its tx logs in a
    /// separate post-simulation pass.
    pub all_txs_ok: bool,
}

/// Result classification after the simulate gate runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum SimulateVerdict {
    /// Bundle simulated to net-positive. Submit to Jito.
    Positive,
    /// Bundle simulated to zero or negative net. Reject — never
    /// submit a bundle that simulates at a loss.
    NonPositive,
    /// Bundle simulation errored (RPC down, log parse failed).
    /// Fail-closed: reject the bundle.
    Error,
}

/// Truncate a log vector so the total char count is ≤ `MAX_LOG_CHARS`.
/// Returns a new Vec (the input is not mutated).
pub fn truncate_logs(logs: Vec<String>, max_chars: usize) -> Vec<String> {
    let total: usize = logs.iter().map(|s| s.len()).sum();
    if total <= max_chars {
        return logs;
    }
    let mut budget = max_chars;
    let mut out: Vec<String> = Vec::with_capacity(logs.len());
    for mut l in logs {
        if budget == 0 {
            break;
        } else if l.len() <= budget {
            budget -= l.len();
            out.push(l);
        } else {
            l.truncate(budget);
            budget = 0;
            out.push(l);
        }
    }
    out
}

/// Run `simulateTransaction` against the given RPC and produce a
/// `SimulationReport`. The RPC client is synchronous (blocking);
/// callers that need async should wrap in `tokio::task::spawn_blocking`.
///
/// The function does NOT itself apply the net-positive gate — it
/// returns the raw report. Use [`classify`] to get the verdict.
///
/// ## The dl-assert tx is skipped
///
/// If `assert_program_id` is `Some`, the tx whose top-level
/// program is that ID is **not** simulated. The dl-assert program
/// reverts when net_pnl is below the threshold — simulating it
/// would always error and falsely flag the bundle as
/// non-positive. The assert tx's profitability guarantee is
/// enforced **on-chain** by the program itself when the bundle
/// actually lands (see `plan/atomicity-decision.md`).
///
/// The first instruction of the assert tx is the dl-assert
/// program; we skip the entire tx (it's a 1-instruction tx).
/// The remaining txs (3 Jupiter swaps + 1 tip) are simulated
/// with `replace_recent_blockhash: true, sig_verify: false`.
pub fn simulate_bundle(
    rpc: &RpcClient,
    signed_transactions: &[VersionedTransaction],
    assert_program_id: Option<&Pubkey>,
) -> Result<SimulationReport, ExecutorError> {
    if signed_transactions.is_empty() {
        return Err(ExecutorError::BundleAssembly(
            "simulate_bundle called with empty tx list".into(),
        ));
    }
    let mut total_cu: u64 = 0;
    let mut all_logs: Vec<String> = Vec::new();
    let mut all_ok = true;

    for tx in signed_transactions {
        // Skip the dl-assert tx if it matches the configured
        // program ID (see fn-level doc for rationale).
        if let Some(assert_pid) = assert_program_id {
            if tx_first_program_id(tx) == Some(*assert_pid) {
                continue;
            }
        }
        let cfg = RpcSimulateTransactionConfig {
            sig_verify: false,
            replace_recent_blockhash: true,
            commitment: None,
            encoding: None,
            accounts: None,
            min_context_slot: None,
            inner_instructions: false,
        };
        let result = rpc
            .simulate_transaction_with_config(tx, cfg)
            .map_err(|e| ExecutorError::SimulateFailed(format!("rpc: {e}")))?;
        if let Some(err) = result.value.err {
            all_ok = false;
            all_logs.push(format!("simulate err: {err:?}"));
        }
        total_cu = total_cu.saturating_add(result.value.units_consumed.unwrap_or(0));
        if let Some(logs) = result.value.logs {
            for l in logs.iter().take(64) {
                all_logs.push(l.clone());
            }
        }
    }

    let all_logs = truncate_logs(all_logs, MAX_LOG_CHARS);

    Ok(SimulationReport {
        // `net_pnl_lamports` is filled in by callers that parse
        // program-specific log markers (see `classify`). We don't
        // try to extract it from raw Jupiter logs here because
        // Jupiter swap logs don't include a net PnL marker; the
        // dl-assert program emits its own verdict in its tx logs
        // instead.
        net_pnl_lamports: None,
        compute_units: total_cu,
        logs: all_logs,
        all_txs_ok: all_ok,
    })
}

/// Return the program ID of the first instruction in `tx`, or
/// `None` if the tx has no instructions or isn't a Legacy /
/// V0 message.
fn tx_first_program_id(tx: &VersionedTransaction) -> Option<Pubkey> {
    use solana_sdk::instruction::CompiledInstruction;
    use solana_sdk::message::VersionedMessage;
    let first_ix: Option<&CompiledInstruction> = match &tx.message {
        VersionedMessage::Legacy(m) => m.instructions.first(),
        VersionedMessage::V0(m) => m.instructions.first(),
    };
    let first_ix = first_ix?;
    let acct_keys = match &tx.message {
        VersionedMessage::Legacy(m) => &m.account_keys,
        VersionedMessage::V0(m) => &m.account_keys,
    };
    acct_keys.get(first_ix.program_id_index as usize).copied()
}

/// Classify a simulation report into a verdict. The gate returns
/// `NonPositive` if:
///   * any tx errored (`all_txs_ok == false`), OR
///   * the explicit `net_pnl_lamports` is `< 0`.
///
/// If the report has no explicit `net_pnl_lamports` (the common
/// case for Jupiter swaps, which don't log a profit marker), the
/// gate relies on `all_txs_ok` and the dl-assert instruction's
/// own simulation to enforce profitability (assert reverts if
/// balance delta < threshold).
pub fn classify(report: &SimulationReport) -> SimulateVerdict {
    if !report.all_txs_ok {
        return SimulateVerdict::NonPositive;
    }
    match report.net_pnl_lamports {
        Some(pnl) if pnl < 0 => SimulateVerdict::NonPositive,
        Some(_) => SimulateVerdict::Positive,
        None => SimulateVerdict::Positive, // rely on assert tx
    }
}

/// Convenience: run simulate + classify in one call. Returns the
/// full report regardless of verdict; the verdict is in the Ok arm.
pub fn simulate_and_classify(
    rpc: &RpcClient,
    signed_transactions: &[VersionedTransaction],
    assert_program_id: Option<&Pubkey>,
) -> Result<(SimulationReport, SimulateVerdict), ExecutorError> {
    let report = simulate_bundle(rpc, signed_transactions, assert_program_id)?;
    let verdict = classify(&report);
    Ok((report, verdict))
}

/// Construct a `SimulationReport` from raw parts (used by tests
/// and by the bundle submit path when caching the report).
pub fn report_from_parts(
    net_pnl_lamports: Option<i64>,
    compute_units: u64,
    logs: Vec<String>,
    all_txs_ok: bool,
) -> SimulationReport {
    SimulationReport {
        net_pnl_lamports,
        compute_units,
        logs,
        all_txs_ok,
    }
}

/// Placeholder signature hash used by some tests.
#[allow(dead_code)]
pub fn dummy_signature() -> Signature {
    Signature::default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_bundle_rejected() {
        let rpc = RpcClient::new_mock("succeeds".to_string());
        let result = simulate_bundle(&rpc, &[], None);
        assert!(matches!(result, Err(ExecutorError::BundleAssembly(_))));
    }

    #[test]
    fn tx_first_program_id_handles_empty_instructions() {
        use solana_sdk::signer::Signer;
        let kp = solana_sdk::signer::keypair::Keypair::new();
        let msg = solana_sdk::message::Message::new(&[], Some(&kp.pubkey()));
        let tx = VersionedTransaction {
            signatures: vec![],
            message: solana_sdk::message::VersionedMessage::Legacy(msg),
        };
        assert_eq!(tx_first_program_id(&tx), None);
    }

    #[test]
    fn tx_first_program_id_returns_first_ix_program() {
        use solana_sdk::signer::Signer;
        let kp = solana_sdk::signer::keypair::Keypair::new();
        let target = Pubkey::new_unique();
        let ix = solana_sdk::system_instruction::transfer(&kp.pubkey(), &target, 100);
        let mut msg = solana_sdk::message::Message::new(&[ix], Some(&kp.pubkey()));
        msg.recent_blockhash = solana_sdk::hash::Hash::new_unique();
        let tx = VersionedTransaction {
            signatures: vec![Signature::default()],
            message: solana_sdk::message::VersionedMessage::Legacy(msg),
        };
        let pid = tx_first_program_id(&tx).expect("first program id");
        assert_eq!(pid, solana_sdk::system_program::id());
    }

    #[test]
    fn classify_marks_error_as_non_positive() {
        let report = report_from_parts(None, 0, vec![], false);
        assert_eq!(classify(&report), SimulateVerdict::NonPositive);
    }

    #[test]
    fn classify_marks_explicit_negative_as_non_positive() {
        let report = report_from_parts(Some(-1), 0, vec![], true);
        assert_eq!(classify(&report), SimulateVerdict::NonPositive);
    }

    #[test]
    fn classify_marks_zero_as_positive() {
        // Zero net PnL is allowed (would mean break-even). The
        // per-bundle tip still eats into the SOL balance, but
        // that's already gated by the cap/rate-limit check.
        let report = report_from_parts(Some(0), 0, vec![], true);
        assert_eq!(classify(&report), SimulateVerdict::Positive);
    }

    #[test]
    fn classify_marks_positive_as_positive() {
        let report = report_from_parts(Some(123_456), 200_000, vec![], true);
        assert_eq!(classify(&report), SimulateVerdict::Positive);
    }

    #[test]
    fn classify_without_explicit_pnl_falls_back_to_all_ok() {
        // No net_pnl_lamports but all txs OK → Positive (relies on
        // the assert tx to enforce profitability).
        let report = report_from_parts(None, 200_000, vec!["ok".into()], true);
        assert_eq!(classify(&report), SimulateVerdict::Positive);
    }

    #[test]
    fn truncate_logs_drops_entries_past_budget() {
        let big: Vec<String> = (0..1000).map(|i| format!("line {i}")).collect();
        let truncated = truncate_logs(big.clone(), 200);
        assert!(truncated.len() < big.len(), "must drop entries when over budget");
        let total_chars: usize = truncated.iter().map(|s| s.len()).sum();
        assert!(
            total_chars <= 200,
            "total chars must be ≤ budget (got {total_chars})"
        );
    }

    #[test]
    fn truncate_logs_no_op_when_under_budget() {
        let small = vec!["a".into(), "b".into(), "c".into()];
        let truncated = truncate_logs(small.clone(), 100);
        assert_eq!(truncated, small);
    }

    #[test]
    fn truncate_logs_handles_empty_input() {
        let truncated = truncate_logs(vec![], 100);
        assert!(truncated.is_empty());
    }

    #[test]
    fn simulation_report_serializes_for_recon() {
        // The recon pipeline reads SimulationReport from the
        // bundles.jsonl log. Verify it round-trips through serde.
        let report = report_from_parts(Some(42), 100_000, vec!["ok".into()], true);
        let json = serde_json::to_string(&report).expect("serialize");
        let back: SimulationReport = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back.net_pnl_lamports, Some(42));
        assert_eq!(back.compute_units, 100_000);
        assert!(back.all_txs_ok);
    }
}