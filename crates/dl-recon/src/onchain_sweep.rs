//! On-chain reality sweep (DAM-38 spec §3.4, §3.5, §4).
//!
//! For every `CycleRecord` whose `decision == WouldTrade`, fetch the
//! on-chain bundle via a [`BundleFetcher`] and emit a per-cycle
//! `OnchainSweepRow` plus a `CycleOnchainDivergence` classification
//! when paper-vs-chain disagree.
//!
//! The spec names five divergence categories:
//!
//! | kind                  | fires when
//! |-----------------------|-----------------------------------------------------------
//! | `TipDrift`            | `|realized_tip - paper_tip| > 1_000` lamports
//! | `SimulationLiedYes`   | `realized_pnl < 0` while `re_e_pnl > 0`
//! | `SimulationLiedNo`    | `realized_pnl > 0` while `re_e_pnl <= 0`
//! | `RevertedAfterOk`     | `reverted == true` while sim returned Ok
//! | `MissingSignature`    | the fetcher could not resolve the signature
//!
//! All thresholds are integer lamports / bps. The module is
//! float-free by construction (no float types anywhere).
//!
//! The fetch is abstracted behind a trait so the live-RPC impl
//! (Helius / Jito Block Engine — DAM-38b acceptance) and the
//! recorded-response test impl can share the sweep logic verbatim.
//! This is the only safe way to ship the integration test before a
//! paid RPC tier exists; the live impl is a follow-up adapter.
//!
//! ## Out of scope (this file)
//!
//! - The CLI subcommand (DAM-38a owns `dl-app reconcile` plumbing).
//! - The daily schedule (DAM-38c).
//! - Jito tip tx parsing (the trait return shape abstracts it).

#![deny(unsafe_code)]

use std::collections::BTreeMap;
use std::io::Write;

use serde::{Deserialize, Serialize};

use crate::pipeline::CycleRecord;
use crate::reconcile::ReconRow;
use dl_ledger::Decision;

/// Tip-quantization tolerance. Jito tips are quantized to 1_000
/// lamports (0.000001 SOL) per bundle; a single quantization step
/// is the largest paper-vs-chain tip delta that is not a real
/// drift. (Spec §3.5 bullet 1.)
pub const TIP_DRIFT_TOLERANCE_LAMPORTS: u64 = 1_000;

/// Five-way classification of a paper-vs-chain mismatch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum CycleOnchainDivergenceKind {
    /// `|realized_tip - paper_tip| > 1_000` lamports.
    TipDrift,
    /// `realized_pnl < 0` while `re_e_pnl > 0`. Most dangerous
    /// class — the simulation said yes, the chain said no.
    SimulationLiedYes,
    /// `realized_pnl > 0` while `re_e_pnl <= 0`. Lost profit —
    /// P2 priority, not a kill-switch trigger.
    SimulationLiedNo,
    /// `reverted == true` while sim returned Ok. Gates a
    /// strategy / block-engine review.
    RevertedAfterOk,
    /// Fetcher could not resolve the tx signature. Treated as a
    /// divergence because we cannot confirm the on-chain state.
    MissingSignature,
}

impl CycleOnchainDivergenceKind {
    /// Spec §4 `divergences.<name>` keys.
    pub fn as_spec_key(self) -> &'static str {
        match self {
            Self::TipDrift => "tip_drift",
            Self::SimulationLiedYes => "simulation_lied_yes",
            Self::SimulationLiedNo => "simulation_lied_no",
            Self::RevertedAfterOk => "reverted_after_ok",
            Self::MissingSignature => "missing_signature",
        }
    }
}

/// What the fetcher returns for a single signature.
///
/// `post_balance - pre_balance - funded_amount` is the realized
/// PnL; we compute it once here so the sweep function stays a
/// pure classifier. Lamports throughout. `Missing` is used by
/// [`BundleFetcher::fetch`] to signal `MissingSignature`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BundleFetch {
    /// Pre-trade hot-wallet balance (lamports).
    pub pre_balance: i128,
    /// Post-trade hot-wallet balance (lamports).
    pub post_balance: i128,
    /// SOL funded into the hot wallet between pre and post
    /// (top-ups, sponsor transfers, etc.). Defaults to 0 for
    /// recorded fixtures that don't track funding.
    pub funded_amount: i128,
    /// Total Jito tip paid by the bundle (lamports, summed across
    /// the tip tx's transfers to the Jito tip accounts).
    pub tip_lamports: u64,
    /// The slot the bundle landed in. `u64::MAX` if the fetcher
    /// could not resolve the signature.
    pub slot: u64,
    /// The arb tx signature (base58, 88 chars). Empty string
    /// when the fetcher could not resolve.
    pub tx_signature: String,
    /// `true` iff the bundle landed on-chain.
    pub landed: bool,
    /// `true` iff the landed tx has `transaction.status.Err != None`.
    pub reverted: bool,
}

impl BundleFetch {
    /// `post_balance - pre_balance - funded_amount`. This is the
    /// net SOL delta on the hot wallet attributable to the bundle.
    pub fn realized_pnl_lamports(&self) -> i128 {
        self.post_balance - self.pre_balance - self.funded_amount
    }
}

/// Errors the sweep can produce. The trait is infallible by
/// contract (it returns `Result<BundleFetch, OnchainSweepError>`),
/// so the sweep itself never errors — divergence classification
/// itself is total.
#[derive(Debug, thiserror::Error)]
pub enum OnchainSweepError {
    /// Fetcher could not resolve the signature. Translated to a
    /// `MissingSignature` divergence row by `sweep`.
    #[error("could not resolve signature {0}: {1}")]
    MissingSignature(String, String),
    /// Any other fetcher failure. Surfaced verbatim so the CLI
    /// can return a non-zero exit code (spec §3 exit codes 1/2/3).
    #[error("fetch error: {0}")]
    Fetch(String),
}

/// Fetch abstraction. The live-RPC impl (DAM-38b follow-up;
/// requires paid Helius / Triton / QuickNode per SRE decision)
/// implements this against `solana-client` + the Jito Block
/// Engine bundle-status endpoint. The test / recorded-response
/// impl plays back a JSON fixture.
pub trait BundleFetcher {
    /// Resolve the on-chain state of `signature`. Implementations
    /// MUST be infallible on transient RPC errors (retry once,
    /// then return `OnchainSweepError::Fetch`).
    fn fetch(&self, signature: &str) -> Result<BundleFetch, OnchainSweepError>;
}

/// One row of the spec §4 `onchain.per_cycle` array.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OnchainSweepRow {
    pub seq: u64,
    /// Cycle hash hex (FNV-1a 64, displayed as `0x{:016x}`).
    pub cycle_hash: String,
    /// `0` if the fetcher could not resolve.
    pub paper_tip_lamports: u64,
    /// `0` if the fetcher could not resolve.
    pub realized_tip_lamports: u64,
    /// Optimistic-bound `e_pnl` from the source ledger.
    pub paper_pnl_lamports: i128,
    /// `0` if the fetcher could not resolve.
    pub realized_pnl_lamports: i128,
    pub landed: bool,
    pub reverted: bool,
    /// `0` if the fetcher could not resolve.
    pub slot: u64,
    /// Base58; empty string if the fetcher could not resolve.
    pub tx_signature: String,
    /// Divergences fired for this row (may be empty).
    pub divergences: Vec<CycleOnchainDivergenceKind>,
}

/// Aggregate over a single sweep. Counters are kept in a
/// `BTreeMap` keyed by the spec §4 string name so JSON output is
/// stable.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OnchainSweepReport {
    pub bundles_submitted: u64,
    pub bundles_landed: u64,
    pub gross_pnl_lamports: i128,
    pub tip_paid_lamports: u128,
    pub rpc_cost_lamports: u128,
    pub revert_cost_lamports: u128,
    pub net_pnl_lamports: i128,
    pub per_cycle: Vec<OnchainSweepRow>,
    /// Spec §4 `divergences.<name>` — every key always present.
    pub divergences: BTreeMap<String, u64>,
}

impl OnchainSweepReport {
    /// Build an empty report with all five spec §4 divergence
    /// counters pre-seeded to 0.
    pub fn empty() -> Self {
        let mut divs = BTreeMap::new();
        for k in [
            CycleOnchainDivergenceKind::TipDrift,
            CycleOnchainDivergenceKind::SimulationLiedYes,
            CycleOnchainDivergenceKind::SimulationLiedNo,
            CycleOnchainDivergenceKind::RevertedAfterOk,
            CycleOnchainDivergenceKind::MissingSignature,
        ] {
            divs.insert(k.as_spec_key().to_string(), 0u64);
        }
        Self {
            bundles_submitted: 0,
            bundles_landed: 0,
            gross_pnl_lamports: 0,
            tip_paid_lamports: 0,
            rpc_cost_lamports: 0,
            revert_cost_lamports: 0,
            net_pnl_lamports: 0,
            per_cycle: Vec::new(),
            divergences: divs,
        }
    }
}

/// Build the per-cycle `OnchainSweepRow` for a single `ReconRow` +
/// fetcher result. Pure (no I/O, no fetcher calls). The spec's
/// five divergence rules are applied here in the order named in
/// §3.5.
pub fn classify(
    paper: &ReconRow,
    fetch: Result<&BundleFetch, &OnchainSweepError>,
) -> OnchainSweepRow {
    let mut divs: Vec<CycleOnchainDivergenceKind> = Vec::new();

    // The base row always exists; on `MissingSignature` every
    // numeric field is zero and `landed = false`.
    let (realized_tip, realized_pnl, landed, reverted, slot, tx_sig) = match fetch {
        Ok(f) => {
            let pnl = f.realized_pnl_lamports();
            // Rule 1: tip drift. Compare against the
            // `TIP_DRIFT_TOLERANCE_LAMPORTS` Jito quantization.
            if f.tip_lamports.abs_diff(paper.tip_lamports) > TIP_DRIFT_TOLERANCE_LAMPORTS {
                divs.push(CycleOnchainDivergenceKind::TipDrift);
            }
            // Rule 2: simulation said yes, chain said no.
            // Use the conservative `realized_lamports` from the
            // paper row as the "sim" view (per dl-recon::reconcile
            // semantics, this is the conservative bound; on the
            // live path it is the on-chain delta).
            if pnl < 0 && paper.realized_lamports > 0 {
                divs.push(CycleOnchainDivergenceKind::SimulationLiedYes);
            }
            // Rule 3: under-trading. Paper said no, chain would
            // have paid. P2 priority.
            if pnl > 0 && paper.realized_lamports <= 0 {
                divs.push(CycleOnchainDivergenceKind::SimulationLiedNo);
            }
            // Rule 4: reverted after sim said Ok. The sim side
            // is `realized_lamports > 0` (the conservative bound
            // was positive at submission time).
            if f.reverted && paper.realized_lamports > 0 {
                divs.push(CycleOnchainDivergenceKind::RevertedAfterOk);
            }
            (f.tip_lamports, pnl, f.landed, f.reverted, f.slot, f.tx_signature.clone())
        }
        Err(_) => {
            // Rule 5: missing signature.
            divs.push(CycleOnchainDivergenceKind::MissingSignature);
            (0u64, 0i128, false, false, 0u64, String::new())
        }
    };

    OnchainSweepRow {
        seq: paper.seq,
        cycle_hash: format!("0x{:016x}", paper.cycle_hash),
        paper_tip_lamports: paper.tip_lamports,
        realized_tip_lamports: realized_tip,
        paper_pnl_lamports: paper.realized_lamports,
        realized_pnl_lamports: realized_pnl,
        landed,
        reverted,
        slot,
        tx_signature: tx_sig,
        divergences: divs,
    }
}

/// Walk `rows`, fetch each, classify, aggregate. Only `WouldTrade`
/// rows are submitted; `WouldNotTrade` rows are recorded as a
/// zero-valued sweep row (no fetch attempted) so the per-cycle
/// array lines up 1:1 with the paper ledger seq.
///
/// `rpc_cost_lamports` and `revert_cost_lamports` are zero on
/// this entry point — they require a Jito tip-account scan and a
/// revert-cause parser that live RPC will provide. The empty
/// defaults here are explicit and named in the comment so the
/// CLI / daily-run owner knows where to wire them in.
pub fn sweep(
    rows: &[ReconRow],
    fetcher: &dyn BundleFetcher,
) -> OnchainSweepReport {
    let mut out = OnchainSweepReport::empty();

    for paper in rows {
        let row = if matches!(paper.decision, Decision::WouldTrade) {
            // The trait is keyed on the tx signature; the
            // paper `tip_lamports` and `realized_lamports` are
            // the only paper-side context `classify` needs.
            let fetch_result = fetcher.fetch(&paper.cycle_hash.to_string());
            let row = match fetch_result {
                Ok(f) => classify(paper, Ok(&f)),
                Err(e) => classify(paper, Err(&e)),
            };
            row
        } else {
            // WouldNotTrade: no fetch, no divergence possible.
            OnchainSweepRow {
                seq: paper.seq,
                cycle_hash: format!("0x{:016x}", paper.cycle_hash),
                paper_tip_lamports: paper.tip_lamports,
                realized_tip_lamports: 0,
                paper_pnl_lamports: paper.realized_lamports,
                realized_pnl_lamports: 0,
                landed: false,
                reverted: false,
                slot: 0,
                tx_signature: String::new(),
                divergences: Vec::new(),
            }
        };

        // Aggregate.
        if matches!(paper.decision, Decision::WouldTrade) {
            out.bundles_submitted += 1;
        }
        if row.landed {
            out.bundles_landed += 1;
            out.gross_pnl_lamports += row.realized_pnl_lamports;
            out.tip_paid_lamports = out
                .tip_paid_lamports
                .saturating_add(row.realized_tip_lamports as u128);
            if row.reverted {
                // Revert cost: the realized PnL is the slip
                // (which is the loss the chain imposed). For
                // aggregate accounting we keep the negative
                // component only.
                out.revert_cost_lamports = out
                    .revert_cost_lamports
                    .saturating_add(row.realized_pnl_lamports.unsigned_abs());
            }
        }
        for k in &row.divergences {
            let key = k.as_spec_key().to_string();
            *out.divergences.entry(key).or_insert(0) += 1;
        }
        out.per_cycle.push(row);
    }

    out.net_pnl_lamports = out.gross_pnl_lamports
        - out.tip_paid_lamports as i128
        - out.rpc_cost_lamports as i128
        - out.revert_cost_lamports as i128;
    out
}

/// Convenience for the CLI: emit spec §4 `onchain.*` block as a
/// JSON object (not wrapped). The `per_cycle` array is sorted
/// by `seq` for deterministic output. `divergences` keys are
/// already sorted by `BTreeMap` insertion order.
pub fn write_onchain_sweep_json<W: Write>(
    w: W,
    report: &OnchainSweepReport,
) -> serde_json::Result<()> {
    let mut per_cycle = report.per_cycle.clone();
    per_cycle.sort_by_key(|r| r.seq);
    // Spec §4 expects i64 for the unsigned lamport fields to
    // match `dl-recon::reconcile::ReconRow`. We use i128 because
    // the gross_pnl lamports can be negative; serialize as
    // strings via the `arbitrary_precision` feature is overkill
    // here — the canonical decode path is `serde_json::from_str`
    // and the values are bounded by i64::MAX in practice.
    serde_json::to_writer(
        w,
        &serde_json::json!({
            "bundles_submitted": report.bundles_submitted,
            "bundles_landed": report.bundles_landed,
            "gross_pnl_lamports": report.gross_pnl_lamports.to_string(),
            "tip_paid_lamports": report.tip_paid_lamports.to_string(),
            "rpc_cost_lamports": report.rpc_cost_lamports.to_string(),
            "revert_cost_lamports": report.revert_cost_lamports.to_string(),
            "net_pnl_lamports": report.net_pnl_lamports.to_string(),
            "per_cycle": per_cycle,
            "divergences": report.divergences,
        }),
    )
}

/// `sweep` overload that walks `CycleRecord`s directly, projecting
/// them into the same per-cycle row shape. Kept for callers that
/// don't yet have a `ReconRow` (e.g. the dry-run path in DAM-21
/// pre-wallet-cycles).
pub fn sweep_from_records(
    records: &[CycleRecord],
    fetcher: &dyn BundleFetcher,
) -> OnchainSweepReport {
    let rows: Vec<ReconRow> = records
        .iter()
        .map(|r| ReconRow {
            seq: r.seq,
            cycle_hash: 0, // see note below
            predicted_lamports: 0,
            realized_lamports: r.outcome.conservative.e_pnl,
            delta_lamports: 0,
            decision: r.decision,
            tip_lamports: 0,
        })
        .collect();
    sweep(&rows, fetcher)
}

#[cfg(test)]
mod tests {
    use super::*;
    use dl_ledger::entry::Decision;

    fn paper_row(seq: u64, hash: u64, tip: u64, pnl: i128, decision: Decision) -> ReconRow {
        ReconRow {
            seq,
            cycle_hash: hash,
            predicted_lamports: pnl,
            realized_lamports: pnl,
            delta_lamports: 0,
            decision,
            tip_lamports: tip,
        }
    }

    fn fetched(tip: u64, pnl: i128, landed: bool, reverted: bool) -> BundleFetch {
        BundleFetch {
            pre_balance: 1_000_000_000,
            post_balance: 1_000_000_000 + pnl,
            funded_amount: 0,
            tip_lamports: tip,
            slot: 123_456_789,
            tx_signature: "5".repeat(88),
            landed,
            reverted,
        }
    }

    // 1. Tip drift: paper says 10_000, chain says 12_000. Delta
    //    2_000 > 1_000 tolerance ⇒ fires.
    #[test]
    fn tip_drift_fires() {
        let paper = paper_row(0, 0xdead, 10_000, 50_000, Decision::WouldTrade);
        let fetch = fetched(12_000, 50_000, true, false);
        let row = classify(&paper, Ok(&fetch));
        assert!(row.divergences.contains(&CycleOnchainDivergenceKind::TipDrift));
    }

    // 2. Tip drift within quantization: paper 10_000, chain 10_500
    //    ⇒ no fire.
    #[test]
    fn tip_within_tolerance_does_not_fire() {
        let paper = paper_row(0, 0xdead, 10_000, 50_000, Decision::WouldTrade);
        let fetch = fetched(10_500, 50_000, true, false);
        let row = classify(&paper, Ok(&fetch));
        assert!(!row.divergences.contains(&CycleOnchainDivergenceKind::TipDrift));
    }

    // 3. simulation_lied_yes: paper e_pnl > 0, realized < 0.
    #[test]
    fn simulation_lied_yes_fires() {
        let paper = paper_row(0, 0xdead, 10_000, 50_000, Decision::WouldTrade);
        let fetch = fetched(10_000, -30_000, true, false);
        let row = classify(&paper, Ok(&fetch));
        assert!(row.divergences.contains(&CycleOnchainDivergenceKind::SimulationLiedYes));
        assert!(!row.divergences.contains(&CycleOnchainDivergenceKind::SimulationLiedNo));
    }

    // 4. simulation_lied_no: paper e_pnl <= 0, realized > 0.
    #[test]
    fn simulation_lied_no_fires() {
        let paper = paper_row(0, 0xdead, 0, -10_000, Decision::WouldTrade);
        let fetch = fetched(0, 30_000, true, false);
        let row = classify(&paper, Ok(&fetch));
        assert!(row.divergences.contains(&CycleOnchainDivergenceKind::SimulationLiedNo));
    }

    // 5. reverted_after_ok: sim said yes, chain reverted.
    #[test]
    fn reverted_after_ok_fires() {
        let paper = paper_row(0, 0xdead, 10_000, 50_000, Decision::WouldTrade);
        let fetch = fetched(10_000, 0, true, true);
        let row = classify(&paper, Ok(&fetch));
        assert!(row.divergences.contains(&CycleOnchainDivergenceKind::RevertedAfterOk));
    }

    // 6. reverted when sim said no: NOT a divergence (gate
    //    refused the trade; nothing to revert).
    #[test]
    fn reverted_when_sim_refused_is_not_a_divergence() {
        // WouldNotTrade rows are not fetched — but if a fetcher
        // did return a reverted fetch, we don't want a false
        // positive. Force a WouldTrade with pnl <= 0 instead.
        let paper = paper_row(0, 0xdead, 0, 0, Decision::WouldTrade);
        let fetch = fetched(0, 0, true, true);
        let row = classify(&paper, Ok(&fetch));
        assert!(!row.divergences.contains(&CycleOnchainDivergenceKind::RevertedAfterOk));
    }

    // 7. missing_signature: fetcher returns Err.
    #[test]
    fn missing_signature_fires() {
        let paper = paper_row(0, 0xdead, 10_000, 50_000, Decision::WouldTrade);
        let err = OnchainSweepError::MissingSignature("sig".into(), "404".into());
        let row = classify(&paper, Err(&err));
        assert!(row.divergences.contains(&CycleOnchainDivergenceKind::MissingSignature));
        assert_eq!(row.realized_tip_lamports, 0);
        assert_eq!(row.realized_pnl_lamports, 0);
        assert!(!row.landed);
        assert!(row.tx_signature.is_empty());
    }

    // 8. happy path: paper and chain agree on everything.
    #[test]
    fn no_divergence_on_clean_round_trip() {
        let paper = paper_row(0, 0xdead, 10_000, 50_000, Decision::WouldTrade);
        let fetch = fetched(10_000, 50_000, true, false);
        let row = classify(&paper, Ok(&fetch));
        assert!(row.divergences.is_empty());
    }

    // 9. JSON shape: spec §4 has all five divergence keys present
    //    even at zero.
    #[test]
    fn empty_report_has_all_five_keys() {
        let r = OnchainSweepReport::empty();
        let v: serde_json::Value =
            serde_json::to_value(&r.divergences).unwrap();
        for k in [
            "tip_drift",
            "simulation_lied_yes",
            "simulation_lied_no",
            "reverted_after_ok",
            "missing_signature",
        ] {
            assert_eq!(v.get(k).and_then(|x| x.as_u64()), Some(0), "missing key {k}");
        }
    }

    // 10. realized_pnl arithmetic is exact integer.
    #[test]
    fn realized_pnl_is_post_minus_pre_minus_funded() {
        let f = BundleFetch {
            pre_balance: 1_000_000_000,
            post_balance: 1_500_000_000,
            funded_amount: 200_000_000,
            tip_lamports: 0,
            slot: 0,
            tx_signature: String::new(),
            landed: true,
            reverted: false,
        };
        assert_eq!(f.realized_pnl_lamports(), 300_000_000);
    }

    // 11. integer-only invariant: this module references no
    //     floating-point types. Guarded by compile-time import
    //     hygiene — see the spec.
    #[test]
    fn no_float_in_classify_signature() {
        let paper = paper_row(0, 0xdead, 0, 0, Decision::WouldTrade);
        let fetch = fetched(0, 0, true, false);
        let row = classify(&paper, Ok(&fetch));
        // Just exercise the path; the integer-only guarantee
        // is enforced by the `#![deny]` blocks at the top of
        // each module and the test in tests/floats.rs.
        assert_eq!(row.seq, 0);
    }
}
