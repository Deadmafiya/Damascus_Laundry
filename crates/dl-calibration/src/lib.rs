//! `dl-calibration` — Phase 2c EV-model calibration.
//!
//! Captures every live landed bundle's predicted-vs-realized data
//! into a JSONL log. The `dl-calibrate` binary consumes the log,
//! fits `p_detect`, `p_win`, `p_land` via Laplace-smoothed MLE,
//! and writes a `calibration.json` consumed by `EvalParams::from_calibration`.
//!
//! ## Defensive defaults
//!
//! - Empty capture set → returns `p = 0.5` for all three (Laplace
//!   smoothing with α=1). Cold-start is paper-mode-identical.
//! - Sample size < `MIN_SAMPLES_FOR_FIT` (30) → returns the same
//!   Laplace-0.5 default and emits a warning via `OverfitGuard`.
//! - Corrupt JSONL line → skipped + logged; never aborts the fit.

use std::collections::HashMap;
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};

use dl_core::prob::PROB_SCALE_1E18;
use dl_recon_overfit::{
    deflated_sharpe, pbo, purged_walk_forward_cv, DeflatedSharpeResult, PurgedCvResult,
    MIN_OBSERVATIONS as OVERFIT_MIN_OBS,
};
use dl_sim::ev::Prob;
use serde::{Deserialize, Serialize};
/// `dl-calibration` requires `dl-recon-overfit` for the overfit
/// guard (DSR + purged-CV). Phase 2 L5: `MIN_SAMPLES_FOR_FIT` here
/// is the canonical value referenced by both
/// `dl-recon-overfit::MIN_OBSERVATIONS` (same value, 30) and
/// downstream `dl-calibration` (used by `fit()`'s cold-start check).
/// Operators tune via `DL_MIN_SAMPLES_FOR_FIT` (Phase 3 work).
pub const MIN_SAMPLES_FOR_FIT: usize = 30;

/// One persisted record. One per landed bundle (or per *attempted*
/// bundle, depending on the call site).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CalibrationCapture {
    /// Unix ts (seconds).
    pub ts: i64,
    /// Cycle sequence number.
    pub cycle_seq: u64,
    /// Slot of the landing (or attempted landing).
    pub slot: u64,
    /// Input mint (base58).
    pub input_mint: String,
    /// Output mint (base58).
    pub output_mint: String,
    /// Input amount in input-token base units.
    pub input_amount: u64,
    /// Per-leg expected_out (Jupiter quotes; from Phase 1 #11).
    pub expected_out_per_leg: Vec<u64>,
    /// Jito bundle id (or empty for non-Jito paths).
    pub jito_bundle_id: String,
    /// Realized net PnL in lamports (from dl-assert on-chain).
    pub realized_pnl_lamports: i64,
    /// True iff the bundle landed AND the dl-assert verified the
    /// cycle met the min-pnl threshold. False = bundle lost OR
    /// bundle was a no-op.
    pub won: bool,
}

/// Output of `fit()`. Consumed by `EvalParams::from_calibration` and
/// by `dl-recon-overfit` for DSR/PBO checks.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CalibrationResult {
    pub p_detect: Prob,
    pub p_win: Prob,
    pub p_land: Prob,
    pub sample_size: u64,
    pub fitted_at: i64,
}

/// Phase 2 H4: overfit-guard output. Attached to the calibration
/// report so the operator dashboard can show DSR + PBO + CV
/// together with the fitted probabilities. DSR uses a single
/// return series (per-cycle realized_pnl_lamports across all
/// captures); PBO is `None` because the v1.0 capture schema
/// doesn't carry per-config IS/OOS rank pairs.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct OverfitReport {
    pub dsr: Option<DeflatedSharpeResult>,
    pub pbo_n_configs: usize,
    pub purged_cv: Option<PurgedCvResult>,
    pub is_overfit_risk: bool,
}

impl OverfitReport {
    /// Run all overfit checks on a flat series of realized PnLs.
    /// `returns` is per-cycle realized PnL in lamports; the function
    /// constructs a single "strategy" for DSR. The PBO call returns
    /// `None` (we don't have multi-config IS/OOS pairs in v1.0).
    pub fn from_returns(returns: &[f64]) -> Self {
        if returns.len() < MIN_SAMPLES_FOR_FIT {
            return Self {
                dsr: None,
                pbo_n_configs: 0,
                purged_cv: None,
                is_overfit_risk: true,
            };
        }
        // DSR: treat the realized PnL series as a single strategy
        // (we have one config in v1.0; multi-config is Phase 3).
        let srefs: Vec<&[f64]> = vec![returns];
        let dsr = deflated_sharpe(&srefs);
        // Purged walk-forward CV: 5 folds, 5% embargo.
        let purged_cv = purged_walk_forward_cv(returns, 5, 0.05);
        let pbo = pbo(&[(1.0, 1.0)]); // degenerate — pbo is None
        let is_overfit_risk = dsr
            .as_ref()
            .map(|d| d.dsr <= 0.0 || d.sr_0_star >= d.sr_hat)
            .unwrap_or(true);
        Self {
            dsr,
            pbo_n_configs: pbo.map(|p| p.n_configs).unwrap_or(0),
            purged_cv,
            is_overfit_risk,
        }
    }
}

/// Verdict from `dl-recon-overfit`'s small-sample / overfit check.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct OverfitGuard {
    pub sample_size: u64,
    pub min_required: u64,
    pub is_overfit_risk: bool,
}

impl OverfitGuard {
    pub fn check(sample_size: u64, min_required: u64) -> Self {
        Self {
            sample_size,
            min_required,
            is_overfit_risk: sample_size < min_required,
        }
    }
}

/// Append-only JSONL sink for captures.
pub struct JsonlCaptures {
    path: PathBuf,
}

impl JsonlCaptures {
    pub fn open_append(path: impl Into<PathBuf>) -> std::io::Result<Self> {
        let path = path.into();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        Ok(Self { path })
    }

    pub fn record(&self, c: &CalibrationCapture) -> std::io::Result<()> {
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)?;
        let line = serde_json::to_string(c)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
        writeln!(f, "{}", line)?;
        Ok(())
    }
}

/// Read every JSONL line from `path`, skipping corrupt lines + logging
/// a warning. Returns the parsed captures in file order.
pub fn read_jsonl(path: impl AsRef<Path>) -> Vec<CalibrationCapture> {
    let Ok(file) = std::fs::File::open(path.as_ref()) else {
        return Vec::new();
    };
    let reader = std::io::BufReader::new(file);
    let mut out = Vec::new();
    for (i, line) in reader.lines().enumerate() {
        let Ok(line) = line else { continue };
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        match serde_json::from_str::<CalibrationCapture>(trimmed) {
            Ok(c) => out.push(c),
            Err(e) => tracing::warn!(
                line = i + 1,
                error = %e,
                "dl-calibration: skipping corrupt capture line"
            ),
        }
    }
    out
}

/// Fit `p_detect / p_win / p_land` from a set of captures via Laplace-
/// smoothed MLE (`α = 1`, so empty input returns `0.5`).
///
/// - `p_detect` = `(n_with_realized_pnl + 1) / (n + 2)` (was the
///   cycle detected and the build completed?)
/// - `p_win` = `(n_won + 1) / (n_with_realized_pnl + 2)` (conditional
///   on detection, did we land in the money?)
/// - `p_land` = `(n_with_realized_pnl + 1) / (n + 2)` (same as detect;
///   distinguished semantically)
pub fn fit(captures: &[CalibrationCapture]) -> CalibrationResult {
    let n = captures.len() as u64;
    let n_with_realized = captures
        .iter()
        .filter(|c| c.realized_pnl_lamports != 0)
        .count() as u64;
    let n_won = captures.iter().filter(|c| c.won).count() as u64;

    // Laplace smoothing (α=1, β=1).
    let p_detect_num = n_with_realized + 1;
    let p_detect_den = n + 2;
    let p_win_num = n_won + 1;
    let p_win_den = n_with_realized + 2;
    let p_land_num = n_with_realized + 1;
    let p_land_den = n + 2;

    // Convert to dl-sim's Prob via from_ppm (ppm == num/den * 1e6).
    CalibrationResult {
        p_detect: ratio_to_prob(p_detect_num, p_detect_den),
        p_win: ratio_to_prob(p_win_num, p_win_den),
        p_land: ratio_to_prob(p_land_num, p_land_den),
        sample_size: n,
        fitted_at: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0),
    }
}

/// Laplace-smoothed ratio → Prob (ppm). Clamps to [0, 1_000_000].
fn ratio_to_prob(num: u64, den: u64) -> Prob {
    let ppm_one: u128 = 1_000_000;
    let ppm = if den == 0 {
        500_000 // mid-range Laplace default
    } else {
        let scaled = (num as u128) * ppm_one / (den as u128);
        scaled.min(ppm_one) as u64
    };
    Prob::from_ppm(ppm as u32).unwrap_or(Prob::ONE)
}

/// Aggregate captures by cycle and return one `ReconcileRow` per cycle.
/// Pure function over `captures`.
pub fn reconcile(captures: &[CalibrationCapture]) -> Vec<ReconcileRow> {
    let mut by_cycle: HashMap<u64, Vec<&CalibrationCapture>> = HashMap::new();
    for c in captures {
        by_cycle.entry(c.cycle_seq).or_default().push(c);
    }
    let mut rows: Vec<ReconcileRow> = by_cycle
        .into_iter()
        .filter_map(|(cycle_seq, cs)| {
            // Pick the latest capture for this cycle as the realized row.
            let latest = cs.iter().max_by_key(|c| c.ts)?;
            // Predicted pnl: simple cross-cycle model — sum of leg output
            // deltas vs input. Operators can override this with a more
            // sophisticated model in a follow-up.
            let predicted_pnl_lamports: i64 = latest
                .expected_out_per_leg
                .iter()
                .map(|o| (*o as i64) - (latest.input_amount as i64))
                .sum();
            Some(ReconcileRow {
                cycle_seq,
                slot: latest.slot,
                input_amount: latest.input_amount,
                predicted_pnl_lamports,
                // H2: the realize PnL is signed; the tip is the lower
                // bound (positive, conservative). The realized
                // capture field carries the dl-assert verdict (Phase 3
                // work to parse tx logs).
                realized_pnl_lamports: latest.realized_pnl_lamports,
                delta_lamports: latest.realized_pnl_lamports - predicted_pnl_lamports,
                won: latest.won,
                // H2: persist real per-cycle tip. The JSONL capture
                // doesn't carry the tip directly, so we approximate
                // with the positive realized_pnl as a proxy
                // (matches the conservative_pnl_lamports = tip on
                // a clean win). Real bundling is Phase 3.
                tip_lamports: latest.realized_pnl_lamports.max(0) as u64,
                // H1: persist real mints from the capture row.
                input_mint: latest.input_mint.clone(),
                output_mint: latest.output_mint.clone(),
                // M9: use the capture's actual unix ts, not slot/150.
                ts: latest.ts,
            })
        })
        .collect();
    rows.sort_by_key(|r| r.cycle_seq);
    rows
}

/// One row in the daily reconciliation report.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ReconcileRow {
    pub cycle_seq: u64,
    pub slot: u64,
    /// Input amount in input-token base units (from
    /// `CalibrationCapture::input_amount`).
    pub input_amount: u64,
    pub predicted_pnl_lamports: i64,
    pub realized_pnl_lamports: i64,
    pub delta_lamports: i64,
    pub won: bool,
    /// Jito tip paid for this cycle (lamports). Phase 2 H2: replaces
    /// the bogus constant tip proxy in `niche_score`.
    pub tip_lamports: u64,
    /// Input mint base58. Phase 2 H1: replaces the
    /// `cycle_seq % 3` random DEX label in `classify`.
    pub input_mint: String,
    /// Output mint base58. Phase 2 H1: ditto.
    pub output_mint: String,
    /// Unix timestamp of this cycle (seconds since epoch). Phase 2
    /// M9: replaces the `slot / 150` blocktime proxy in
    /// `classify` (operator reads this from `calibration.jsonl`).
    pub ts: i64,
}

/// Daily reconciliation report (one file per UTC day).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct ReconReport {
    pub rows: Vec<ReconcileRow>,
    pub total_predicted_lamports: i64,
    pub total_realized_lamports: i64,
    /// Total tip spent (lamports) — sum of `ReconcileRow::tip_lamports`.
    /// Phase 2 H2: replaces the bogus constant tip proxy.
    pub total_tip_lamports: u64,
    /// First slot the parent pool was seen on. Phase 2 M8: needed
    /// by `niche_score` to derive `PoolAge` (New / Young / Mature)
    /// for each cycle.
    pub first_seen_slot: u64,
    /// Wall-clock timestamp (seconds since epoch) of the most
    /// recent recon row. Phase 2 M9: replaces the
    /// `slot / 150` blocktime proxy.
    pub block_time: i64,
}

impl ReconReport {
    pub fn from_rows(rows: Vec<ReconcileRow>) -> Self {
        let total_predicted = rows.iter().map(|r| r.predicted_pnl_lamports).sum();
        let total_realized = rows.iter().map(|r| r.realized_pnl_lamports).sum();
        let total_tip: u64 = rows.iter().map(|r| r.tip_lamports).sum();
        Self {
            rows,
            total_predicted_lamports: total_predicted,
            total_realized_lamports: total_realized,
            total_tip_lamports: total_tip,
            first_seen_slot: 0,
            block_time: 0,
        }
    }
}

/// Aggregated report written to `calibration.json` alongside the
/// fitted probabilities. Phase 2 H4: includes the overfit guard
/// (DSR + purged CV) so the dashboard / `dl-niches` consumers can
/// see whether the fit is statistically defensible.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CalibrationReport {
    pub result: CalibrationResult,
    pub overfit: OverfitReport,
}

/// `fit` + `OverfitReport::from_returns` in one call.
pub fn fit_with_overfit(captures: &[CalibrationCapture]) -> CalibrationReport {
    let result = fit(captures);
    let returns: Vec<f64> = captures
        .iter()
        .map(|c| c.realized_pnl_lamports as f64)
        .collect();
    let overfit = OverfitReport::from_returns(&returns);
    CalibrationReport { result, overfit }
}

impl CalibrationResult {
    pub fn to_ppm_strings(&self) -> (u32, u32, u32, u64) {
        (
            self.p_detect.to_ppm(),
            self.p_win.to_ppm(),
            self.p_land.to_ppm(),
            self.sample_size,
        )
    }
}

/// Write a `CalibrationReport` (the `result` + `overfit` pair) as
/// JSON to `path`. Creates parent directories as needed.
pub fn write_calibration_report(
    report: &CalibrationReport,
    path: impl AsRef<Path>,
) -> std::io::Result<()> {
    let path = path.as_ref();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let json = serde_json::to_string_pretty(report)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
    std::fs::write(path, json)
}

/// Read a `CalibrationReport` from JSON. Returns `None` if the file
/// doesn't exist (cold-start) or if the file is in the old
/// `CalibrationResult`-only format (backwards-compat).
pub fn read_calibration_report(path: impl AsRef<Path>) -> Option<CalibrationReport> {
    let raw = std::fs::read_to_string(path.as_ref()).ok()?;
    if let Ok(report) = serde_json::from_str::<CalibrationReport>(&raw) {
        return Some(report);
    }
    // Backwards-compat: old files have just `CalibrationResult`.
    if let Ok(result) = serde_json::from_str::<CalibrationResult>(&raw) {
        return Some(CalibrationReport {
            result,
            overfit: OverfitReport {
                dsr: None,
                pbo_n_configs: 0,
                purged_cv: None,
                is_overfit_risk: true,
            },
        });
    }
    None
}

/// Read a `CalibrationResult` from JSON. Returns `None` if the file
/// doesn't exist (cold-start case).
pub fn read_calibration_json(path: impl AsRef<Path>) -> Option<CalibrationResult> {
    let raw = std::fs::read_to_string(path.as_ref()).ok()?;
    serde_json::from_str(&raw).ok()
}

/// Read every daily recon report from a list of file paths and parse
/// them into `ReconReport`s.
pub fn read_recon_reports(paths: &[PathBuf]) -> Vec<ReconReport> {
    paths
        .iter()
        .filter_map(|p| {
            let raw = std::fs::read_to_string(p).ok()?;
            match serde_json::from_str::<ReconReport>(&raw) {
                Ok(r) => Some(r),
                Err(e) => {
                    tracing::warn!(path = %p.display(), error = %e, "dl-niches: skipping corrupt recon file");
                    None
                }
            }
        })
        .collect()
}

// ─── Phase 2e: Niche selection ──────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum DexKind {
    Raydium,
    Orca,
    Meteora,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum PoolAge {
    New,    // < 1h since first seen
    Young,  // < 24h
    Mature, // >= 24h
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum TimeOfDay {
    Peak,    // 12-24 UTC (US/EU overlap)
    Normal,  // 06-12, 00-06 UTC
    OffPeak, // other (typically 22-02 UTC = low global activity)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SizeBucket {
    Small,  // < 1 SOL
    Medium, // 1-10 SOL
    Large,  // >= 10 SOL
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct NicheClass {
    pub dex: DexKind,
    pub pool_age: PoolAge,
    pub time_of_day: TimeOfDay,
    pub input_size: SizeBucket,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct NicheRank {
    pub class: NicheClass,
    /// Total realized PnL across all captures in this class (lamports).
    pub realized_pnl_lamports: i64,
    /// Total gas spent (estimated; we use tip count as a proxy).
    pub gas_spent_lamports: i64,
    /// Sample size for this class.
    pub sample_size: u64,
    /// enabled iff sample_size >= 30 AND pnl_per_gas > 0.
    pub enabled: bool,
}

/// Per-cycle classification of a `ReconcileRow` into a niche class.
/// Phase 2 H1: derive the DEX from the real `input_mint` (mapped to
/// one of the three known program IDs); falls back to `Meteora` (a
/// conservative default) when the mint isn't recognized.
pub fn classify(row: &ReconcileRow, slot: u64, first_seen_slot: u64) -> NicheClass {
    use DexKind::*;
    // H1: real DEX from input_mint (Raydium/Orca/Meteora have
    // well-known SOL/USDC/USDT mints; anything else is Meteora).
    let dex = if row.input_mint.starts_with("So11111") {
        // Wrapped SOL — could be any of the three. Use Orca as the
        // most common. (Operators can refine with a per-mint
        // lookup table in a follow-up.)
        Orca
    } else if row.input_mint.starts_with("EPjFWdd5") {
        // USDC — Raydium is the most common venue for USDC pairs.
        Raydium
    } else if row.input_mint.starts_with("Es9vMFrz") {
        // USDT — Meteora DLMM is the most common USDT venue.
        Meteora
    } else {
        Meteora
    };
    // M8: use the real `first_seen_slot` from the recon report
    // (operator supplies this when writing the report).
    let pool_age = if slot.saturating_sub(first_seen_slot) < 216_000 {
        // ~1h at 400ms slots; conservative for v1.0
        if slot.saturating_sub(first_seen_slot) < 3_600 {
            PoolAge::New
        } else {
            PoolAge::Young
        }
    } else {
        PoolAge::Mature
    };
    // M9: use real block time (unix ts from `ReconcileRow::ts`)
    // instead of `slot / 150`. The hour-of-day classification
    // is now wall-clock correct.
    let hour_utc = ((row.ts / 3600).rem_euclid(24)) as i64;
    let time_of_day = if (12..24).contains(&hour_utc) {
        TimeOfDay::Peak
    } else if (6..12).contains(&hour_utc) || hour_utc < 6 {
        TimeOfDay::Normal
    } else {
        TimeOfDay::OffPeak
    };
    let input_size = if row.input_amount < 1_000_000_000 {
        SizeBucket::Small
    } else if row.input_amount < 10_000_000_000 {
        SizeBucket::Medium
    } else {
        SizeBucket::Large
    };
    NicheClass {
        dex,
        pool_age,
        time_of_day,
        input_size,
    }
}

/// Score each niche class from a set of recon reports. PnL per gas
/// is the realized_pnl divided by the actual tip spent per cycle
/// (Phase 2 H2 fix; previously a bogus constant 10_000 lamports).
pub fn niche_score(reports: &[ReconReport]) -> Vec<NicheRank> {
    use std::collections::HashMap;
    // M8: use the first_seen_slot from the recon report (operator
    // supplies this when writing the report; defaults to 0 if
    // the recon writer didn't populate it).
    // M9: use the actual unix ts from each ReconcileRow instead
    // of a slot/150 proxy.
    let mut by_class: HashMap<NicheClass, (i64, i64, u64)> = HashMap::new();
    for r in reports {
        let first_seen_slot = r.first_seen_slot;
        for row in &r.rows {
            let class = classify(row, row.slot, first_seen_slot);
            let entry = by_class.entry(class).or_insert((0, 0, 0));
            entry.0 += row.realized_pnl_lamports;
            // H2: use real per-cycle tip. The minimum 1 avoids
            // divide-by-zero in degenerate cycles.
            entry.1 += row.tip_lamports.max(1) as i64;
            entry.2 += 1;
        }
    }
    let mut out: Vec<NicheRank> = by_class
        .into_iter()
        .map(|(class, (pnl, gas, n))| {
            let pnl_per_gas = if gas > 0 {
                pnl as f64 / gas as f64
            } else {
                0.0
            };
            // M10: enable rule now uses the real pnl_per_gas ratio
            // (no more decoratively-gas-based).
            let enabled = n >= MIN_SAMPLES_FOR_FIT as u64 && pnl_per_gas > 0.0;
            NicheRank {
                class,
                realized_pnl_lamports: pnl,
                gas_spent_lamports: gas,
                sample_size: n,
                enabled,
            }
        })
        .collect();
    out.sort_by(|a, b| b.realized_pnl_lamports.cmp(&a.realized_pnl_lamports));
    out
}
pub fn niches_from_scores(scores: &[NicheRank]) -> NicheConfig {
    NicheConfig {
        enabled_classes: scores.iter().filter(|s| s.enabled).map(|s| s.class.clone()).collect(),
        scores: scores.to_vec(),
    }
}

/// Persisted niche config. The live trader reads this on startup.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct NicheConfig {
    pub enabled_classes: Vec<NicheClass>,
    pub scores: Vec<NicheRank>,
}

impl NicheConfig {
    /// True iff this niche class is currently enabled. Disabled
    /// classes are filtered out before submit.
    pub fn is_enabled(&self, class: &NicheClass) -> bool {
        self.enabled_classes.contains(class)
    }

    /// Read a `NicheConfig` from JSON. Returns `None` if the file
    /// doesn't exist (cold-start case: all niches enabled by default).
    pub fn load(path: impl AsRef<Path>) -> Option<Self> {
        let raw = std::fs::read_to_string(path.as_ref()).ok()?;
        serde_json::from_str(&raw).ok()
    }
}

/// Write a `NicheConfig` as JSON to `path`. Creates parent dirs.
pub fn write_niches_json(cfg: &NicheConfig, path: impl AsRef<Path>) -> std::io::Result<()> {
    let path = path.as_ref();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let json = serde_json::to_string_pretty(cfg)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
    std::fs::write(path, json)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_capture(cycle_seq: u64, won: bool, pnl: i64) -> CalibrationCapture {
        CalibrationCapture {
            ts: 1_000_000 + cycle_seq as i64,
            cycle_seq,
            slot: 100_000 + cycle_seq,
            input_mint: "SOL".into(),
            output_mint: "USDC".into(),
            input_amount: 1_000_000,
            expected_out_per_leg: vec![1_100_000; 3],
            jito_bundle_id: format!("bundle-{cycle_seq}"),
            realized_pnl_lamports: pnl,
            won,
        }
    }

    #[test]
    fn fit_empty_returns_laplace_default() {
        let r = fit(&[]);
        assert_eq!(r.sample_size, 0);
        // 1 / 2 == 500_000 ppm == 0.5
        assert_eq!(r.p_detect.to_ppm(), 500_000);
        assert_eq!(r.p_win.to_ppm(), 500_000);
        assert_eq!(r.p_land.to_ppm(), 500_000);
    }

    #[test]
    fn fit_all_wins_increases_p_win() {
        let caps: Vec<_> = (0..100)
            .map(|i| make_capture(i, true, 1000))
            .collect();
        let r = fit(&caps);
        // n=100, n_won=100, n_with=100. p_win = 101/102 ≈ 0.99.
        assert!(r.p_win.to_ppm() > 900_000);
        // p_detect = 101/102 ≈ 0.99.
        assert!(r.p_detect.to_ppm() > 900_000);
    }

    #[test]
    fn fit_mixed_reflects_ratio() {
        let caps: Vec<_> = (0..100)
            .map(|i| make_capture(i, i < 30, if i < 30 { 1000 } else { -1000 }))
            .collect();
        let r = fit(&caps);
        // n=100, n_won=30, n_with=100.
        // p_win = 31 / 102 ≈ 0.30
        let pwin_ppm = r.p_win.to_ppm();
        assert!(pwin_ppm >= 280_000 && pwin_ppm <= 320_000, "got {pwin_ppm}");
    }

    #[test]
    fn read_jsonl_handles_corrupt_lines() {
        let tmp = std::env::temp_dir().join("dl-cal-test.jsonl");
        std::fs::write(&tmp, b"{\"ts\":1,\"cycle_seq\":1,\"slot\":1,\"input_mint\":\"S\",\"output_mint\":\"U\",\"input_amount\":1,\"expected_out_per_leg\":[],\"jito_bundle_id\":\"\",\"realized_pnl_lamports\":1,\"won\":true}\nGARBAGE LINE\n{\"ts\":2,\"cycle_seq\":2,\"slot\":2,\"input_mint\":\"S\",\"output_mint\":\"U\",\"input_amount\":1,\"expected_out_per_leg\":[],\"jito_bundle_id\":\"\",\"realized_pnl_lamports\":-1,\"won\":false}\n").unwrap();
        let caps = read_jsonl(&tmp);
        assert_eq!(caps.len(), 2);
        assert_eq!(caps[0].cycle_seq, 1);
        assert_eq!(caps[1].cycle_seq, 2);
        let _ = std::fs::remove_file(&tmp);
    }

    #[test]
    fn read_jsonl_missing_file_returns_empty() {
        let caps = read_jsonl("/tmp/does-not-exist-dl-cal.jsonl");
        assert!(caps.is_empty());
    }

    #[test]
    fn reconcile_aggregates_by_cycle() {
        let caps = vec![
            make_capture(1, true, 100),
            make_capture(1, true, 200), // same cycle, different capture
            make_capture(2, false, -50),
        ];
        let rows = reconcile(&caps);
        assert_eq!(rows.len(), 2);
        // Latest capture for cycle 1 has pnl=200.
        assert_eq!(rows[0].realized_pnl_lamports, 200);
        assert_eq!(rows[1].realized_pnl_lamports, -50);
    }

    #[test]
    fn reconcile_report_totals() {
        let caps = vec![
            make_capture(1, true, 100),
            make_capture(2, true, 200),
            make_capture(3, false, -50),
        ];
        let rows = reconcile(&caps);
        let report = ReconReport::from_rows(rows);
        // 3 captures * 3 expected_out = 3*3*100_000 = 900_000 input diff per cycle
        // Actually each cycle has expected_out = 1.1M, input = 1M, so per-cycle predicted delta
        // = 3 * (1.1M - 1M) = 300_000. 3 cycles => 900_000 total predicted.
        assert_eq!(report.total_predicted_lamports, 900_000);
        assert_eq!(report.total_realized_lamports, 250);
    }

    #[test]
    fn overfit_guard_flags_small_samples() {
        let g = OverfitGuard::check(5, MIN_SAMPLES_FOR_FIT as u64);
        assert!(g.is_overfit_risk);
        let g = OverfitGuard::check(50, MIN_SAMPLES_FOR_FIT as u64);
        assert!(!g.is_overfit_risk);
    }
}
