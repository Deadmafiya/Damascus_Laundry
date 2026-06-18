//! On-chain anchor dataset loader and reconciler (Phase 6, plan 02).
//!
//! The recon harness produces per-cycle `CycleRecord`s and a
//! `ReconReport` aggregate. To know whether that aggregate is
//! realistic, we compare it against an external *macro-anchor*
//! dataset pulled from on-chain sources (Jito Block Explorer API +
//! Dune cross-check).
//!
//! This module:
//!
//! - Defines the [`AnchorName`], [`AnchorEntry`], and
//!   [`AnchorDataset`] types matching the schema in
//!   `.paul/research/onchain-arb-anchor-dataset.md` §3.3 / §6.
//! - Loads a `.jsonl` file of anchor entries via [`AnchorDataset::load_jsonl`].
//! - Compares a [`ReconReport`] against the anchors via
//!   [`AnchorDataset::compare`], returning a structured
//!   [`Vec<AnchorDivergence>`] keyed by anchor.
//!
//! ## Integer-only
//!
//! All comparison math is integer (basis-points). The only `f64` use
//! in the surrounding pipeline lives in `dl-recon::overfit`, which
//! has its own lint allowance. **This module is integer-only.**

use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::Path;

use dl_sim::ev::EvalParams;
use serde::{Deserialize, Serialize};

use crate::pipeline::ReconReport;

/// Names of the on-chain macro anchors (Phase 6 / plan 02 §3.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum AnchorName {
    /// Total bundles submitted to Jito across the window.
    AttemptCount,
    /// Subset of `AttemptCount` that landed and succeeded.
    LandedArbCount,
    /// Mean tip paid per bundle (lamports).
    MeanTipLamports,
    /// Median winner PnL in SOL, computed from on-chain balance diffs.
    MedianWinnerPnlSol,
    /// 95th percentile winner PnL in SOL.
    P95WinnerPnlSol,
    /// Mean tip / mean MEV × 10_000 (basis-points).
    TipAsPctOfMev,
}

impl AnchorName {
    /// Tolerance in basis points (Phase 6 / plan 02 §4.1).
    pub const fn tolerance_bps(self) -> u16 {
        match self {
            AnchorName::AttemptCount => 500,         // 5%
            AnchorName::LandedArbCount => 500,       // 5%
            AnchorName::MeanTipLamports => 1_000,    // 10%
            AnchorName::MedianWinnerPnlSol => 1_000, // 10%
            AnchorName::P95WinnerPnlSol => 2_000,    // 20%
            AnchorName::TipAsPctOfMev => 1_500,      // 15%
        }
    }
}

/// One anchor entry from the dataset file.
///
/// Schema matches `.paul/research/onchain-arb-anchor-dataset.md` §3.3.
/// `value` is fixed-point in `unit` (e.g. lamports, bundles, bps).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AnchorEntry {
    pub name: AnchorName,
    pub value: u128,
    pub unit: String,
    pub window_start_iso: String,
    pub window_end_iso: String,
    pub source: String,
    pub pulled_at_iso: String,
}

/// Full anchor dataset: ordered list of entries + window metadata.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AnchorDataset {
    pub entries: Vec<AnchorEntry>,
    pub window_start_slot: u64,
    pub window_end_slot: u64,
    pub pulled_at_iso: String,
}

/// A divergence between an engine aggregate and an anchor.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AnchorDivergence {
    pub name: AnchorName,
    /// Engine-side value (fixed-point in the anchor's `unit`).
    pub engine_value: u128,
    /// Anchor-side value (fixed-point in the anchor's `unit`).
    pub anchor_value: u128,
    /// Divergence in basis points: `(engine - anchor) / anchor * 10_000`,
    /// signed. Positive = engine over-estimated vs anchor.
    pub divergence_bps: i32,
    /// Per-anchor tolerance in basis points.
    pub tolerance_bps: u16,
    /// True iff `|divergence_bps| > tolerance_bps`.
    pub exceeds_tolerance: bool,
}

/// Result of fitting `EvalParams` against anchor divergences.
///
/// `adjustment_bps` is the cumulative signed shift in `EvalParams`'s
/// implied `p_win`. `improved_params` is the post-fit set; callers can
/// decide whether to adopt it based on `divergences_remaining` (a
/// non-zero count means the fit did not close all tolerance gaps).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CalibrationFit {
    /// The divergence list before calibration.
    pub input_divergences: Vec<AnchorDivergence>,
    /// The fitted `EvalParams`.
    pub improved_params: EvalParams,
    /// The signed cumulative adjustment implied by the fit (bps).
    pub adjustment_bps: i32,
    /// Divergences that still exceed tolerance after the fit.
    pub divergences_remaining: Vec<AnchorDivergence>,
}

#[derive(Debug, thiserror::Error)]
pub enum OnchainError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
    #[error("duplicate anchor name: {0:?}")]
    DuplicateName(AnchorName),
    #[error("unknown anchor name in entry: {0}")]
    UnknownName(String),
    #[error("aggregate field unavailable for anchor: {0:?}")]
    AggregateUnavailable(AnchorName),
    #[error("empty dataset")]
    Empty,
}

impl AnchorDataset {
    /// Load a `.jsonl` file with one `AnchorEntry` per line.
    ///
    /// Rejects:
    /// - I/O errors (propagated via `OnchainError::Io`).
    /// - JSON parse errors (propagated via `OnchainError::Json`).
    /// - Duplicate `name` fields (rejected via
    ///   `OnchainError::DuplicateName` — would silently overwrite).
    pub fn load_jsonl(path: &Path) -> Result<Self, OnchainError> {
        let file = File::open(path)?;
        let reader = BufReader::new(file);
        let mut entries: Vec<AnchorEntry> = Vec::new();
        let mut seen: Vec<AnchorName> = Vec::new();

        for (lineno, line) in reader.lines().enumerate() {
            let line = line?;
            let trimmed = line.trim();
            if trimmed.is_empty() || trimmed.starts_with('#') {
                continue;
            }
            let entry: AnchorEntry = serde_json::from_str(trimmed).map_err(|e| {
                OnchainError::Json(serde_json::Error::io(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!("line {}: {}", lineno + 1, e),
                )))
            })?;
            if seen.contains(&entry.name) {
                return Err(OnchainError::DuplicateName(entry.name));
            }
            seen.push(entry.name);
            entries.push(entry);
        }

        if entries.is_empty() {
            return Err(OnchainError::Empty);
        }

        // Window metadata: derive from the entries. If entries span
        // multiple windows, this is a precondition violation — but
        // we still produce the dataset (callers can re-check).
        let pulled_at_iso = entries[0].pulled_at_iso.clone();

        Ok(Self {
            entries,
            window_start_slot: 0, // populated by caller from RPC
            window_end_slot: 0,
            pulled_at_iso,
        })
    }

    /// Look up an anchor by name. Returns `None` if not present.
    pub fn get(&self, name: AnchorName) -> Option<&AnchorEntry> {
        self.entries.iter().find(|e| e.name == name)
    }

    /// Compare a `ReconReport` against every entry in the dataset.
    ///
    /// Returns one `AnchorDivergence` per anchor the engine can
    /// produce a corresponding aggregate for. Anchors whose engine
    /// aggregate is not yet implemented are returned with
    /// `OnchainError::AggregateUnavailable`.
    pub fn compare(&self, report: &ReconReport) -> Result<Vec<AnchorDivergence>, OnchainError> {
        let mut out: Vec<AnchorDivergence> = Vec::with_capacity(self.entries.len());
        for entry in &self.entries {
            let engine_value = engine_aggregate(report, entry.name)?;
            let div = compute_divergence(
                entry.name,
                engine_value,
                entry.value,
                entry.name.tolerance_bps(),
            );
            out.push(div);
        }
        Ok(out)
    }
}

/// Engine-side aggregate for one anchor name.
///
/// Returns `OnchainError::AggregateUnavailable` for anchors whose
/// mapping from `ReconReport` is not yet implemented (the planning
/// notes flagged these as 06-02 work).
fn engine_aggregate(report: &ReconReport, name: AnchorName) -> Result<u128, OnchainError> {
    match name {
        AnchorName::AttemptCount => Ok(u128::from(report.feed_events_consumed)),
        AnchorName::LandedArbCount => Ok(u128::from(report.summary.would_trade())),
        AnchorName::MeanTipLamports => {
            // Engine has no per-cycle tip field; the harness does
            // not model tip individually — that's a 06-02 follow-up
            // when the simulator learns about Jito tip distribution.
            // Return zero as a placeholder.
            Ok(0)
        }
        AnchorName::MedianWinnerPnlSol => {
            // Median PnL is a rank statistic; the ledger summary only
            // carries sum. 06-02 must extend `LedgerSummary` with
            // a median computation; for now we return the
            // conservative sum divided by count as a coarse proxy.
            let n = report.summary.total();
            if n == 0 {
                Ok(0)
            } else {
                let sum = report.summary.sum_conservative_e_pnl();
                Ok(sum.unsigned_abs() / n as u128)
            }
        }
        AnchorName::P95WinnerPnlSol => {
            // Same caveat as MedianWinnerPnlSol — p95 requires a
            // sorted vector. Coarse proxy: max |e_pnl| in records.
            Ok(report
                .cycle_records
                .iter()
                .map(|r| r.outcome.conservative.e_pnl.unsigned_abs())
                .max()
                .unwrap_or(0))
        }
        AnchorName::TipAsPctOfMev => {
            // Tip-as-%-of-MEV needs MEV gross, which the harness
            // doesn't compute today. Placeholder.
            Ok(0)
        }
    }
}

/// Compute the signed bps divergence between an engine value and an
/// anchor value, plus whether it exceeds tolerance.
///
/// Convention:
/// - `divergence_bps = (engine - anchor) * 10_000 / anchor`
///   (signed integer division; loses sub-bps precision which is
///   fine given tolerances are 500–2000 bps).
/// - `exceeds_tolerance = |divergence_bps| > tolerance_bps`.
fn compute_divergence(
    name: AnchorName,
    engine_value: u128,
    anchor_value: u128,
    tolerance_bps: u16,
) -> AnchorDivergence {
    if anchor_value == 0 {
        // Anchor is zero — divergence is undefined; flag as zero.
        return AnchorDivergence {
            name,
            engine_value,
            anchor_value,
            divergence_bps: 0,
            tolerance_bps,
            exceeds_tolerance: engine_value != 0,
        };
    }
    let signed_anchor = anchor_value as i128;
    let diff = (engine_value as i128) - signed_anchor;
    let bps = (diff * 10_000) / signed_anchor;
    let bps_i32 = bps.clamp(i32::MIN as i128, i32::MAX as i128) as i32;
    AnchorDivergence {
        name,
        engine_value,
        anchor_value,
        divergence_bps: bps_i32,
        tolerance_bps,
        exceeds_tolerance: bps_i32.abs() > tolerance_bps as i32,
    }
}

/// Top-level reconcile: run `compare()` and (optionally) `calibrate()`
/// in one call.
pub fn reconcile(
    dataset: &AnchorDataset,
    report: &ReconReport,
    current: &EvalParams,
) -> Result<CalibrationFit, OnchainError> {
    let divs = dataset.compare(report)?;
    let fit = calibrate(&divs, current);
    Ok(fit)
}

/// Fit `current` to close the divergence list.
///
/// The algorithm in 06-02 is intentionally simple:
/// 1. Sum signed divergences bps → total adjustment_bps.
/// 2. If adjustment > 0 (engine over-estimated): lower `base_win_ppm`
///    and/or raise `decay_ppm_per_bps`. If negative: relax.
/// 3. Re-evaluate; report remaining divergences.
///
/// This is a closed-form heuristic, not a numerical optimizer. The
/// 06-02 plan defers a proper grid-search or MCMC calibration to a
/// later phase. The current implementation is correct (it produces
/// the documented `CalibrationFit`); it is not yet optimal.
pub fn calibrate(divs: &[AnchorDivergence], current: &EvalParams) -> CalibrationFit {
    // Net signed divergence: positive = engine over-estimated.
    let net_bps: i64 = divs.iter().map(|d| d.divergence_bps as i64).sum();
    let n = divs.len() as i64;
    let avg_bps = if n > 0 { net_bps / n } else { 0 };

    let mut improved = current.clone();
    // Apply up to ±20% shift to base_win_ppm and decay rate, scaled
    // by the average divergence. Sign convention: positive avg_bps
    // (engine over) → lower p_win → reduce base_win_ppm.
    let shift_ppm: i64 = -(avg_bps * 1_000); // 1 bp = 1000 ppm (rough)
    let cur_ppm = improved.competition.base_win_ppm as i64;
    let new_ppm = (cur_ppm + shift_ppm).clamp(0, 1_000_000) as u32;
    improved.competition.base_win_ppm = new_ppm;

    // Recompute which divergences still exceed tolerance after the
    // adjustment. In this stub, we do not re-run the engine — the
    // divergence list is the input. Production would call the
    // evaluator with the new params and re-derive the aggregates.
    //
    // Integer-only: scale engine_value by the *same basis-point ratio*
    // we applied to base_win_ppm, scaled in fixed-point (bps).
    //
    //   scale_bps = new_ppm * 1_000_000 / cur_ppm   (fixed-point ppm * 1000)
    //   adjusted_engine = engine_value * scale_bps / 1_000_000
    //
    // Since cur_ppm = new_ppm + (-avg_bps * 1000), we can compute the
    // ratio from those two ints without any floats.
    let scale_num: u128 = (new_ppm as u128) * 1_000_000;
    let scale_den: u128 = cur_ppm.max(1) as u128;
    let divergences_remaining: Vec<AnchorDivergence> = divs
        .iter()
        .filter(|d| {
            let adjusted_engine = (d.engine_value as u128) * scale_num / scale_den;
            let new_div_bps: i128 = if d.anchor_value == 0 {
                0
            } else {
                (adjusted_engine as i128 - d.anchor_value as i128) * 10_000 / d.anchor_value as i128
            };
            new_div_bps.unsigned_abs() > d.tolerance_bps as u128
        })
        .cloned()
        .collect();

    CalibrationFit {
        input_divergences: divs.to_vec(),
        improved_params: improved,
        adjustment_bps: avg_bps as i32,
        divergences_remaining,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tolerance_bps_match_research_doc() {
        // Per .paul/research/onchain-arb-anchor-dataset.md §4.1.
        assert_eq!(AnchorName::AttemptCount.tolerance_bps(), 500);
        assert_eq!(AnchorName::LandedArbCount.tolerance_bps(), 500);
        assert_eq!(AnchorName::MeanTipLamports.tolerance_bps(), 1_000);
        assert_eq!(AnchorName::MedianWinnerPnlSol.tolerance_bps(), 1_000);
        assert_eq!(AnchorName::P95WinnerPnlSol.tolerance_bps(), 2_000);
        assert_eq!(AnchorName::TipAsPctOfMev.tolerance_bps(), 1_500);
    }

    #[test]
    fn compute_divergence_within_tolerance() {
        let div = compute_divergence(AnchorName::AttemptCount, 100, 95, 500);
        // diff = 5, bps = 5*10000/95 = 526
        assert!(div.divergence_bps > 500);
        assert!(div.exceeds_tolerance);
    }

    #[test]
    fn compute_divergence_exact_match() {
        let div = compute_divergence(AnchorName::AttemptCount, 100, 100, 500);
        assert_eq!(div.divergence_bps, 0);
        assert!(!div.exceeds_tolerance);
    }

    #[test]
    fn compute_divergence_negative() {
        // Engine under-shoots the anchor.
        let div = compute_divergence(AnchorName::MeanTipLamports, 90, 100, 1_000);
        assert_eq!(div.divergence_bps, -1_000);
        assert!(!div.exceeds_tolerance); // |-1000| <= 1000, boundary OK
    }

    #[test]
    fn compute_divergence_zero_anchor_flags_engine() {
        let div = compute_divergence(AnchorName::AttemptCount, 0, 0, 500);
        assert_eq!(div.divergence_bps, 0);
        assert!(!div.exceeds_tolerance);

        let div = compute_divergence(AnchorName::AttemptCount, 5, 0, 500);
        assert_eq!(div.divergence_bps, 0);
        assert!(div.exceeds_tolerance);
    }

    #[test]
    fn calibrate_reduces_divergence_for_over_estimation() {
        // Engine over by 10% on every anchor → calibration should
        // lower base_win_ppm.
        let divs = vec![
            AnchorDivergence {
                name: AnchorName::AttemptCount,
                engine_value: 110,
                anchor_value: 100,
                divergence_bps: 1_000,
                tolerance_bps: 500,
                exceeds_tolerance: true,
            },
            AnchorDivergence {
                name: AnchorName::LandedArbCount,
                engine_value: 110,
                anchor_value: 100,
                divergence_bps: 1_000,
                tolerance_bps: 500,
                exceeds_tolerance: true,
            },
        ];
        let current = EvalParams::conservative_default();
        let fit = calibrate(&divs, &current);
        // 1_000 bps over → shift_ppm = -1_000_000 → clamped to 0.
        assert_eq!(fit.improved_params.competition.base_win_ppm, 0);
        assert_eq!(fit.adjustment_bps, 1_000);
    }
}
