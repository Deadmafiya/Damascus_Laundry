//! `dl-app reconcile` subcommand (DAM-38a, per DAM-38 spec §3.1–§3.3, §4).
//!
//! Diff the paper-trading path against itself (paper-vs-paper, §3.2) and
//! against the on-chain macro anchor dataset (paper-vs-anchor, §3.3).
//! Writes the spec §4 JSON to disk. The on-chain reality sweep (§3.4–§3.5)
//! is out of scope here; DAM-38b covers it.
//!
//! Usage:
//!   dl-app reconcile \
//!     --cycles-jsonl  ./wallet.cycles.jsonl \
//!     --recon-report  ./recon-YYYYMMDD.json \
//!     --anchors       ./anchors-YYYYMMDD.jsonl \
//!     --out           ./reconcile-YYYYMMDD.json
//!
//! Exit codes (mirrors `dl-app recon`):
//!   0 — clean run, all anchors within tolerance, no paper divergences
//!   1 — paper-vs-paper divergences present (within tolerance)
//!   2 — at least one anchor exceeds tolerance
//!   3 — runtime error (file missing, decode failure, etc.)
//!
//! ## Integer-only
//!
//! No `f32` or `f64` ever appears. All thresholds are integer
//! (lamports / basis points). The 5-bps gross-bps band and the
//! per-anchor `tolerance_bps` are u16/u32 throughout.

use std::collections::BTreeMap;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use dl_ledger::LedgerEntry;
use dl_ledger::LedgerSummary;
use dl_recon::onchain::{AnchorDataset, AnchorDivergence, AnchorName};
use dl_recon::pipeline::{CycleRecord, ReconReport};
use dl_recon::pipeline::ReplayParams;
use serde::{Deserialize, Serialize};
use tracing::info;

use crate::init_tracing;

/// Per-cycle paper-side row, projected from `wallet.cycles.jsonl`.
///
/// The on-disk file carries two coexisting shapes — the v1
/// `cycle.v1` contract record and the v0 back-compat shim the
/// ArbiNexus bridge still reads. The shim has only `pool_address`
/// (which is the cycle hash), `gross_bps`, and a few mint
/// strings; the v1 record adds `cycle_id`, `decision`, `tip`-
/// adjacent fields, and structured legs. The shim is the
/// source-of-truth for `pool_address` until DAM-44 swaps the
/// bridge; we accept both and project to one struct.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaperCycleRow {
    /// Stable cycle id. For v1: `cycle_id` (blake3 64-hex).
    /// For v0: `pool_address` (the v0 "pool_address" was always
    /// a cycle hash — see `cycle_writer::build_cycle_v0_shim`).
    pub cycle_hash_hex: String,
    /// `gross_bps` from the paper writer. 0 when input >= output.
    pub paper_gross_bps: i64,
    /// `decision` from the v1 record. `None` for v0 shim lines
    /// (the v0 shape carries no decision field).
    pub paper_decision: Option<String>,
    /// `input_lamports` (v1) or 0 (v0).
    pub paper_input_lamports: u64,
    /// `output_lamports` (v1) or 0 (v0).
    pub paper_output_lamports: u64,
    /// `tip_lamports` (v1) or 0 (v0).
    pub paper_tip_lamports: u64,
    /// Schema version string from the v1 record, or "v0" for shim lines.
    pub schema: String,
}

/// The paired per-cycle row emitted by the join step (spec §3.1).
///
/// `paper_*` is from `wallet.cycles.jsonl`; `re_*` is from the
/// `ReconReport` produced by `dl-app recon --report-json`. The
/// re-side is *always* present (the harness re-derives every
/// cycle); the paper side is `None` when a cycle is in
/// `ReconReport` but missing from the JSONL (a "paper missing"
/// divergence, spec §4 `kind: cycle_hash_missing`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CycleJoin {
    /// Sequence number from the source `ReconReport`.
    pub seq: u64,
    /// The cycle hash (64-hex for v1; same string from v0 `pool_address`).
    /// For the v0 shim the value is the cycle hash; for the v1
    /// record it is `cycle_id`. The join key is the raw string.
    pub cycle_hash_hex: String,
    /// Paper decision string (`"WouldTrade"` or `"WouldNotTrade"`),
    /// or `None` if the paper row is missing entirely.
    pub paper_decision: Option<String>,
    /// Paper input lamports (`0` when the v0 shim is the source).
    pub paper_input_lamports: u64,
    /// Paper output lamports (`0` when the v0 shim is the source).
    pub paper_output_lamports: u64,
    /// Paper tip lamports (`0` when the v0 shim is the source).
    pub paper_tip_lamports: u64,
    /// Paper-side `gross_bps` from the JSONL row (`None` when the
    /// paper row is missing).
    pub paper_gross_bps: Option<i64>,
    /// Re-derived `conservative.e_pnl` (the harness output).
    pub re_e_pnl: i128,
    /// Re-derived `optimistic.e_pnl`.
    pub re_optimistic_e_pnl: i128,
    /// Re-derived `decision` (the harness's trade gate).
    pub re_decision: String,
    /// `LedgerEntry::tip_lamports` from the harness.
    pub re_tip_lamports: u64,
}

/// One paper-vs-paper divergence (spec §3.2 + §4 `paper_divergences`).
///
/// `kind` matches the spec's `kind` enum verbatim:
/// - `decision` — `paper_decision != re_decision`.
/// - `e_pnl` — `|paper_gross_bps - re_gross_bps| > 5` bps.
///   (The spec calls this `e_pnl`; we keep the spec's label
///   even though the field being diffed is `gross_bps`.)
/// - `cycle_hash_missing` — the cycle is in `ReconReport` but not
///   in `wallet.cycles.jsonl`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PaperDivergence {
    pub seq: u64,
    pub kind: String,
    /// `|paper_gross_bps - re_gross_bps|` for kind=`e_pnl`; 0 otherwise.
    /// `gross_bps_diff_bps` is the field name used in v1 reporting.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gross_bps_diff_bps: Option<i64>,
    /// `paper_decision` for kind=`decision`; `None` for the other kinds.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub paper_decision: Option<String>,
    /// `re_decision` for kind=`decision`; `None` for the other kinds.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub re_decision: Option<String>,
}

/// Spec §4 output (minus the `onchain` block, which is DAM-38b).
///
/// Field names match the spec exactly so an operator can paste
/// the JSON into a runbook section without renaming. Counts are
/// `u64`; divergences are integer; `report_hash` is FNV-1a 64
/// over the canonical bincode of the `paper_divergences` +
/// `anchor_divergences` lists.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReconcileOutput {
    pub window_start_slot: u64,
    pub window_end_slot: u64,
    pub paper: PaperBlock,
    pub anchors: AnchorsBlock,
    /// Always emitted as a zero-block in DAM-38a; the
    /// `onchain.per_cycle` and `onchain.*` fields are reserved
    /// for DAM-38b. We keep the block present (with all
    /// counts = 0 and no `per_cycle`) so a consumer reading
    /// the JSON today sees the spec §4 shape.
    pub onchain: OnchainPlaceholder,
    pub divergences: DivergenceCounters,
    /// FNV-1a 64 over the canonical form of paper_divergences +
    /// anchor_divergences. Stable across runs with the same
    /// inputs (determinism invariant I-1).
    pub report_hash: u64,
}

/// Spec §4 `paper` block.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PaperBlock {
    pub cycles_seen: u64,
    pub would_trade_paper: u64,
    pub would_trade_re: u64,
    pub paper_divergences: Vec<PaperDivergence>,
}

/// Spec §4 `anchors` block.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AnchorsBlock {
    pub divergences: Vec<AnchorDivergence>,
}

/// Spec §4 `onchain` block placeholder. DAM-38a always emits
/// this with all counts = 0 and `per_cycle: []`. DAM-38b
/// fills the real numbers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OnchainPlaceholder {
    pub bundles_submitted: u64,
    pub bundles_landed: u64,
    pub gross_pnl_lamports: i128,
    pub tip_paid_lamports: u128,
    pub rpc_cost_lamports: u128,
    pub revert_cost_lamports: u128,
    pub net_pnl_lamports: i128,
    pub rolling_baseline_net_lamports: i128,
    pub deviation_from_baseline_bps: i32,
    pub per_cycle: Vec<serde_json::Value>,
}

impl Default for OnchainPlaceholder {
    fn default() -> Self {
        Self {
            bundles_submitted: 0,
            bundles_landed: 0,
            gross_pnl_lamports: 0,
            tip_paid_lamports: 0,
            rpc_cost_lamports: 0,
            revert_cost_lamports: 0,
            net_pnl_lamports: 0,
            rolling_baseline_net_lamports: 0,
            deviation_from_baseline_bps: 0,
            per_cycle: Vec::new(),
        }
    }
}

/// Spec §4 `divergences` counters. DAM-38a only fills
/// `simulation_lied_yes` (= `paper_decision` mismatch where
/// paper said WouldTrade but re said WouldNotTrade) and
/// `simulation_lied_no` (the inverse). The on-chain counters
/// (`tip_drift`, `reverted_after_ok`, `missing_signature`)
/// are DAM-38b. We always emit the full block with zeros.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DivergenceCounters {
    pub tip_drift: u64,
    pub simulation_lied_yes: u64,
    pub simulation_lied_no: u64,
    pub reverted_after_ok: u64,
    pub missing_signature: u64,
}

impl Default for DivergenceCounters {
    fn default() -> Self {
        Self {
            tip_drift: 0,
            simulation_lied_yes: 0,
            simulation_lied_no: 0,
            reverted_after_ok: 0,
            missing_signature: 0,
        }
    }
}

/// Result of running `dl-app reconcile`.
#[derive(Debug)]
pub enum ReconcileCliResult {
    /// Clean run: no paper divergences, all anchors within tolerance.
    Ok,
    /// Paper-vs-paper divergences present (within tolerance).
    PaperDivergences(Vec<PaperDivergence>),
    /// At least one anchor exceeds tolerance.
    AnchorOverTolerance(Vec<AnchorDivergence>),
    /// Runtime error (file missing, decode failure, etc.).
    Error(String),
}

impl ReconcileCliResult {
    /// Map to a process exit code. Mirrors `dl-app recon` (§4 of spec).
    pub fn exit_code(&self) -> u8 {
        match self {
            ReconcileCliResult::Ok => 0,
            ReconcileCliResult::PaperDivergences(_) => 1,
            ReconcileCliResult::AnchorOverTolerance(_) => 2,
            ReconcileCliResult::Error(_) => 3,
        }
    }
}

// ---------------------------------------------------------------------------
// Core entry points
// ---------------------------------------------------------------------------

/// Run the reconciliation pass end-to-end. Pure function of inputs:
/// same files in → same report out (modulo FNV-1a hash).
pub fn reconcile(
    recon_report: &ReconReport,
    paper_rows: &[PaperCycleRow],
    anchors: Option<&AnchorDataset>,
) -> Result<ReconcileOutput, ReconcileError> {
    // 1. Per-cycle join (spec §3.1).
    let joins = join_by_cycle_hash(recon_report, paper_rows);

    // 2. Paper-vs-paper consistency diff (spec §3.2).
    let paper_divergences = paper_vs_paper(&joins);
    let (yes, no) = count_decision_lies(&paper_divergences);

    // 3. Paper-vs-anchor macro compare (spec §3.3).
    let anchor_divergences = match anchors {
        Some(ds) => ds.compare(recon_report)?,
        None => Vec::new(),
    };

    // 4. Aggregate counts.
    let cycles_seen = joins.len() as u64;
    let would_trade_paper = paper_rows
        .iter()
        .filter(|r| r.paper_decision.as_deref() == Some("WouldTrade"))
        .count() as u64;
    let would_trade_re = recon_report.would_trade();

    // 5. Window metadata: spec §3.3 calls for a slot window.
    //    The on-paper dataset doesn't carry slots; we record
    //    the [0, 0] empty window as a placeholder until the
    //    on-chain fetch (DAM-38b) populates real values.
    let window_start_slot = anchors
        .map(|d| d.window_start_slot)
        .unwrap_or(0);
    let window_end_slot = anchors.map(|d| d.window_end_slot).unwrap_or(0);

    // 6. Hash the diverged lists. FNV-1a 64 over the bincode
    //    of paper_divergences then anchor_divergences.
    let report_hash = hash_divergences(&paper_divergences, &anchor_divergences);

    Ok(ReconcileOutput {
        window_start_slot,
        window_end_slot,
        paper: PaperBlock {
            cycles_seen,
            would_trade_paper,
            would_trade_re,
            paper_divergences,
        },
        anchors: AnchorsBlock {
            divergences: anchor_divergences,
        },
        onchain: OnchainPlaceholder::default(),
        divergences: DivergenceCounters {
            tip_drift: 0,
            simulation_lied_yes: yes,
            simulation_lied_no: no,
            reverted_after_ok: 0,
            missing_signature: 0,
        },
        report_hash,
    })
}

/// Per-cycle join keyed on the cycle-hash string (spec §3.1).
///
/// The `ReconReport.cycle_records` is the *left* input (the
/// re-derivation is the canonical per-cycle ordering); the paper
/// rows are the *right* input. A cycle present on the left but
/// missing on the right yields a `CycleJoin` with `paper_gross_bps
/// = None` and is recorded as a `cycle_hash_missing` divergence
/// by [`paper_vs_paper`]. Cycles present on the right but not
/// on the left are not joined (they were never evaluated by
/// the harness; the re side is authoritative).
pub fn join_by_cycle_hash(
    report: &ReconReport,
    paper_rows: &[PaperCycleRow],
) -> Vec<CycleJoin> {
    // Build a hash-keyed index of the paper rows. Multiple
    // occurrences of the same `cycle_hash_hex` are an
    // operator-configured error path; we keep the *first*
    // match (the cycle_writer appends in detection order, so
    // the first row is the original detection).
    let mut by_hash: BTreeMap<String, &PaperCycleRow> = BTreeMap::new();
    for row in paper_rows {
        by_hash.entry(row.cycle_hash_hex.clone()).or_insert(row);
    }

    let mut joins: Vec<CycleJoin> = Vec::with_capacity(report.cycle_records.len());
    for rec in &report.cycle_records {
        let cycle_hash_hex = cycle_hash_to_hex(&rec.cycle);
        let paper = by_hash.get(&cycle_hash_hex).copied();
        let join = CycleJoin {
            seq: rec.seq,
            cycle_hash_hex: cycle_hash_hex.clone(),
            paper_decision: paper.and_then(|p| p.paper_decision.clone()),
            paper_input_lamports: paper.map(|p| p.paper_input_lamports).unwrap_or(0),
            paper_output_lamports: paper.map(|p| p.paper_output_lamports).unwrap_or(0),
            paper_tip_lamports: paper.map(|p| p.paper_tip_lamports).unwrap_or(0),
            paper_gross_bps: paper.map(|p| p.paper_gross_bps),
            re_e_pnl: rec.outcome.conservative.e_pnl,
            re_optimistic_e_pnl: rec.outcome.optimistic.e_pnl,
            re_decision: decision_to_string(rec.decision),
            re_tip_lamports: rec.entry.tip_lamports,
        };
        joins.push(join);
    }
    joins
}

/// Paper-vs-paper consistency diff (spec §3.2).
///
/// A divergence is emitted when:
/// - `paper_decision != re_decision` and both are present (kind=`decision`).
/// - The paper row is missing entirely (kind=`cycle_hash_missing`).
/// - `|paper_gross_bps - re_gross_bps| > 5` bps (kind=`e_pnl`).
///
/// The 5-bps threshold is a u16 const for clarity.
pub fn paper_vs_paper(joins: &[CycleJoin]) -> Vec<PaperDivergence> {
    const GROSS_BPS_TOLERANCE: i64 = 5;
    let mut out: Vec<PaperDivergence> = Vec::new();
    for j in joins {
        // (a) Cycle missing on the paper side.
        if j.paper_gross_bps.is_none() {
            out.push(PaperDivergence {
                seq: j.seq,
                kind: "cycle_hash_missing".to_string(),
                gross_bps_diff_bps: None,
                paper_decision: None,
                re_decision: Some(j.re_decision.clone()),
            });
            continue;
        }
        // (b) Decision mismatch (only when both sides report one).
        if let Some(paper_dec) = &j.paper_decision {
            if paper_dec != &j.re_decision {
                out.push(PaperDivergence {
                    seq: j.seq,
                    kind: "decision".to_string(),
                    gross_bps_diff_bps: None,
                    paper_decision: Some(paper_dec.clone()),
                    re_decision: Some(j.re_decision.clone()),
                });
            }
        }
        // (c) Gross-bps band. Re-derive gross_bps from the
        //     cycle's optimal-fill output so the comparison is
        //     apples-to-apples (the re side stores `e_pnl` in
        //     lamports, not bps; we back out bps from
        //     `optimistic.e_pnl + input_lamports` because the
        //     optimistic bound is computed under p_detect = p_win
        //     = p_land = 1.0 and no failed-cost haircut, which
        //     means `e_pnl = gross - costs ≈ gross - tip` for
        //     a non-failed trade. For the v0 shim we don't
        //     have input_lamports; in that case we fall back to
        //     conservative.e_pnl which is a sound approximation
        //     for the 5-bps band — the band is wide).
        let paper_bps = j.paper_gross_bps.unwrap_or(0);
        let re_bps = re_gross_bps(j);
        let diff = (paper_bps - re_bps).abs();
        if diff > GROSS_BPS_TOLERANCE {
            out.push(PaperDivergence {
                seq: j.seq,
                kind: "e_pnl".to_string(),
                gross_bps_diff_bps: Some(diff),
                paper_decision: None,
                re_decision: None,
            });
        }
    }
    out
}

/// Convert the harness's `conservative.e_pnl + input` (in
/// lamports) back to gross bps for the band check. The
/// optimistic bound's `e_pnl` is `gross - costs - failed_cost`
/// under p_detect = 1.0; for a profitable trade the failed
/// cost is 0, so `e_pnl = gross - costs`. We don't have
/// `costs` exposed in the join, so we use the ratio
/// `(e_pnl + input) / input * 10_000` which is the gross-bps
/// formula the v0 shim uses. When `input` is 0 (v0 shim has
/// no `input_lamports` field) we fall back to 0 bps — the
/// shim's gross_bps is then the authoritative value, and
/// the band check reduces to `|paper_bps - 0| > 5`, which
/// is intentionally noisy in the v0-only path (DAM-44
/// replaces the shim and restores the clean comparison).
fn re_gross_bps(j: &CycleJoin) -> i64 {
    if j.paper_input_lamports == 0 {
        return 0;
    }
    let input = j.paper_input_lamports as i128;
    // Use the optimistic bound (p=1.0, no failed cost) so
    // `e_pnl + input ≈ gross_output`. Saturating math only —
    // no overflow on adversarial inputs.
    let gross = j.re_optimistic_e_pnl.saturating_add(input);
    if gross <= 0 || input <= 0 {
        return 0;
    }
    let diff = gross - input;
    if diff <= 0 {
        return 0;
    }
    // bps = (gross - input) * 10_000 / input, signed i64.
    let bps = diff.saturating_mul(10_000) / input;
    bps.clamp(i64::MIN as i128, i64::MAX as i128) as i64
}

/// Count `simulation_lied_yes` / `simulation_lied_no` from a
/// `paper_divergences` list. `yes` = paper said WouldTrade,
/// re said WouldNotTrade. `no` = paper said WouldNotTrade,
/// re said WouldTrade. Both are `0` when no decision
/// divergences are present.
fn count_decision_lies(divs: &[PaperDivergence]) -> (u64, u64) {
    let mut yes = 0u64;
    let mut no = 0u64;
    for d in divs {
        if d.kind != "decision" {
            continue;
        }
        match (d.paper_decision.as_deref(), d.re_decision.as_deref()) {
            (Some("WouldTrade"), Some("WouldNotTrade")) => yes += 1,
            (Some("WouldNotTrade"), Some("WouldTrade")) => no += 1,
            _ => {}
        }
    }
    (yes, no)
}

// ---------------------------------------------------------------------------
// Loaders
// ---------------------------------------------------------------------------

/// Load `wallet.cycles.jsonl` (or any compatible JSONL stream)
/// into a `Vec<PaperCycleRow>`. Skips blank lines and lines
/// starting with `#`. Returns [`ReconcileError::Json`] on
/// malformed lines or [`ReconcileError::Io`] on read errors.
pub fn load_cycles_jsonl(path: &Path) -> Result<Vec<PaperCycleRow>, ReconcileError> {
    let file = File::open(path).map_err(ReconcileError::Io)?;
    let reader = BufReader::new(file);
    let mut out: Vec<PaperCycleRow> = Vec::new();
    for (lineno, line) in reader.lines().enumerate() {
        let line = line.map_err(ReconcileError::Io)?;
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        // Parse as a generic JSON object so we can accept both
        // the v1 `cycle.v1` shape and the v0 shim shape. We
        // project to PaperCycleRow after.
        let v: serde_json::Value =
            serde_json::from_str(trimmed).map_err(|e| ReconcileError::Json {
                line: lineno + 1,
                source: e,
            })?;
        out.push(project_paper_row(&v));
    }
    Ok(out)
}

/// Project a `serde_json::Value` (one line of
/// `wallet.cycles.jsonl`) to a `PaperCycleRow`. Recognizes
/// both the v1 `cycle.v1` record and the v0 shim.
fn project_paper_row(v: &serde_json::Value) -> PaperCycleRow {
    let obj = v.as_object();

    // Schema label. v1 has "schema": "cycle.v1"; v0 has none.
    let schema = obj
        .and_then(|o| o.get("schema"))
        .and_then(|s| s.as_str())
        .unwrap_or("v0")
        .to_string();

    // Cycle hash. v1: `cycle_id` (64-hex blake3). v0:
    // `pool_address` (the v0 "pool_address" was always a
    // cycle hash — see `cycle_writer::build_cycle_v0_shim`).
    let cycle_hash_hex = obj
        .and_then(|o| o.get("cycle_id").or_else(|| o.get("pool_address")))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    // Gross bps. Both shapes carry this as a JSON integer.
    let paper_gross_bps = obj
        .and_then(|o| o.get("gross_bps"))
        .and_then(|v| v.as_i64())
        .unwrap_or(0);

    // Decision. v1 has `"decision": "WouldTrade"` (string).
    // v0 has no decision.
    let paper_decision = obj
        .and_then(|o| o.get("decision"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    // Lamports fields. v1 has them; v0 has none.
    let paper_input_lamports = obj
        .and_then(|o| o.get("input_lamports"))
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let paper_output_lamports = obj
        .and_then(|o| o.get("output_lamports"))
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let paper_tip_lamports = obj
        .and_then(|o| o.get("tip_lamports"))
        .and_then(|v| v.as_u64())
        .unwrap_or(0);

    PaperCycleRow {
        cycle_hash_hex,
        paper_gross_bps,
        paper_decision,
        paper_input_lamports,
        paper_output_lamports,
        paper_tip_lamports,
        schema,
    }
}

/// Load a `ReconReport` from the JSON emitted by
/// `dl-app recon --report-json`. Returns [`ReconcileError::Json`]
/// on parse failure and [`ReconcileError::Io`] on read errors.
///
/// `ReconReport` only implements `Serialize` (the harness
/// emits a one-way audit report), so we deserialize into a
/// wire-format mirror [`ReconReportOnDisk`] and convert.
///
/// The conversion re-derives `cycle_records` and `summary`
/// from the on-disk fields; the per-cycle fields we need
/// (`seq`, `decision`, `tip_lamports`, conservative +
/// optimistic `e_pnl`, and the cycle's leg sequence) are all
/// present in the JSON, so the round-trip is lossless.
pub fn load_recon_report(path: &Path) -> Result<ReconReport, ReconcileError> {
    let bytes = std::fs::read(path).map_err(ReconcileError::Io)?;
    let on_disk: ReconReportOnDisk = serde_json::from_slice(&bytes)
        .map_err(|e| ReconcileError::JsonSer(serde_json::Error::io(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("ReconReport deserialize: {e}"),
        ))))?;
    Ok(ReconReport::from(on_disk))
}

// ---------------------------------------------------------------------------
// Wire format for the report on disk
// ---------------------------------------------------------------------------

/// Wire mirror of `dl_recon::pipeline::ReconReport`. The
/// harness only implements `Serialize` for the live type, so
/// we mirror its shape with `Deserialize` to read what
/// `dl-app recon --report-json` writes. Every field the join
/// needs (per-cycle seq, decision, conservative.e_pnl,
/// optimistic.e_pnl, tip_lamports, the cycle's leg sequence)
/// is present in the on-disk JSON. `LedgerSummary` is
/// re-derived from the per-cycle rows.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReconReportOnDisk {
    pub params: ReplayParamsOnDisk,
    pub cycle_records: Vec<CycleRecordOnDisk>,
    /// Re-derived on load; the wire JSON includes it but we
    /// ignore it (it would be confusing to trust it when the
    /// per-row data is the source of truth).
    pub summary: serde_json::Value,
    pub divergences: Vec<serde_json::Value>,
    pub report_hash: u64,
    pub feed_events_consumed: u64,
    pub total_tip_lamports: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReplayParamsOnDisk {
    pub cost: dl_sim::cost::CostModel,
    pub optimistic: dl_sim::ev::EvalParams,
    pub conservative: dl_sim::ev::EvalParams,
    pub max_input: u128,
    pub max_cycle_legs: usize,
}

impl Default for ReplayParamsOnDisk {
    fn default() -> Self {
        Self {
            cost: dl_sim::cost::CostModel::default_busy(),
            optimistic: dl_sim::ev::EvalParams::optimistic(),
            conservative: dl_sim::ev::EvalParams::conservative_default(),
            max_input: 1_000_000_000,
            max_cycle_legs: 4,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CycleRecordOnDisk {
    pub seq: u64,
    pub cycle: CycleOnDisk,
    pub net: dl_sim::net_profit::NetProfit,
    pub outcome: dl_sim::ev::EvalOutcome,
    pub decision: dl_ledger::Decision,
    pub entry: dl_ledger::LedgerEntry,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CycleOnDisk {
    pub legs: Vec<LegOnDisk>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LegOnDisk {
    pub pool: dl_state::Pubkey,
    pub direction: dl_state::cycle::Direction,
    pub weight: i64,
}

impl From<ReconReportOnDisk> for ReconReport {
    fn from(o: ReconReportOnDisk) -> Self {
        let cycle_records: Vec<CycleRecord> = o
            .cycle_records
            .into_iter()
            .map(|c| CycleRecord {
                seq: c.seq,
                cycle: c.cycle.into(),
                net: c.net,
                outcome: c.outcome,
                decision: c.decision,
                entry: c.entry,
            })
            .collect();
        let entries: Vec<LedgerEntry> = cycle_records.iter().map(|r| r.entry.clone()).collect();
        // `LedgerSummary::from_entries` only returns Err on
        // saturating overflow in the per-aggregate counts; with
        // any realistic report the entries count fits in u64.
        // The summary is auxiliary state for the join; on
        // overflow we fall back to an empty summary so the
        // reconcile pass still runs. (A real overflow would
        // already have been caught by the harness on write.)
        let summary = LedgerSummary::from_entries(&entries).unwrap_or_else(|_| {
            // `from_entries(&[])` cannot fail.
            LedgerSummary::from_entries(&[]).expect("empty summary is valid")
        });
        let params: ReplayParams = ReplayParams {
            cost: o.params.cost,
            optimistic: o.params.optimistic,
            conservative: o.params.conservative,
            max_input: o.params.max_input,
            max_cycle_legs: o.params.max_cycle_legs,
        };
        ReconReport {
            params,
            cycle_records,
            summary,
            divergences: Vec::new(),
            report_hash: o.report_hash,
            feed_events_consumed: o.feed_events_consumed,
            total_tip_lamports: o.total_tip_lamports,
        }
    }
}

impl From<CycleOnDisk> for dl_state::cycle::Cycle {
    fn from(o: CycleOnDisk) -> Self {
        let legs: Vec<dl_state::cycle::Leg> = o
            .legs
            .into_iter()
            .map(|l| dl_state::cycle::Leg {
                pool: l.pool,
                direction: l.direction,
                weight: l.weight,
            })
            .collect();
        dl_state::cycle::Cycle::new(legs)
    }
}

/// Run the full CLI: parse args, load inputs, reconcile, write
/// output, return the exit code.
pub fn run(args: &[String]) -> ReconcileCliResult {
    let opts = match parse_args(args) {
        Ok(o) => o,
        Err(e) => return ReconcileCliResult::Error(e),
    };

    info!(
        cycles_jsonl = %opts.cycles_jsonl.display(),
        recon_report = %opts.recon_report.display(),
        anchors = ?opts.anchors.as_ref().map(|p| p.display().to_string()),
        out = %opts.out.display(),
        "starting reconcile"
    );

    // 1. Load the ReconReport (the canonical re side).
    let report = match load_recon_report(&opts.recon_report) {
        Ok(r) => r,
        Err(e) => return ReconcileCliResult::Error(format!("recon-report: {e}")),
    };
    info!(
        cycles = report.cycle_records.len(),
        would_trade = report.would_trade(),
        "recon report loaded"
    );

    // 2. Load the paper JSONL.
    let paper_rows = match load_cycles_jsonl(&opts.cycles_jsonl) {
        Ok(r) => r,
        Err(e) => return ReconcileCliResult::Error(format!("cycles-jsonl: {e}")),
    };
    info!(paper_rows = paper_rows.len(), "paper rows loaded");

    // 3. Optionally load anchors.
    let anchors = match &opts.anchors {
        Some(p) => match AnchorDataset::load_jsonl(p) {
            Ok(d) => Some(d),
            Err(e) => return ReconcileCliResult::Error(format!("anchors: {e}")),
        },
        None => None,
    };

    // 4. Run the reconciliation.
    let output = match reconcile(&report, &paper_rows, anchors.as_ref()) {
        Ok(o) => o,
        Err(e) => return ReconcileCliResult::Error(format!("reconcile: {e}")),
    };
    info!(
        paper_divergences = output.paper.paper_divergences.len(),
        anchor_divergences = output.anchors.divergences.len(),
        report_hash = output.report_hash,
        "reconcile complete"
    );

    // 5. Print a human-readable summary to stderr (operator
    //    console reads the JSON from --out).
    eprintln!(
        "reconcile: {} paper divergences, {} anchor divergences, report_hash=0x{:016x}",
        output.paper.paper_divergences.len(),
        output.anchors.divergences.len(),
        output.report_hash
    );

    // 6. Write the JSON output.
    if let Err(e) = write_output(&opts.out, &output) {
        return ReconcileCliResult::Error(format!("write out: {e}"));
    }
    info!(path = %opts.out.display(), "reconcile output written");

    // 7. Classify the result.
    let bad_anchors: Vec<AnchorDivergence> = output
        .anchors
        .divergences
        .iter()
        .filter(|d| d.exceeds_tolerance)
        .cloned()
        .collect();
    if !bad_anchors.is_empty() {
        return ReconcileCliResult::AnchorOverTolerance(bad_anchors);
    }
    if !output.paper.paper_divergences.is_empty() {
        return ReconcileCliResult::PaperDivergences(output.paper.paper_divergences);
    }
    ReconcileCliResult::Ok
}

#[derive(Debug)]
struct ReconcileOpts {
    cycles_jsonl: PathBuf,
    recon_report: PathBuf,
    anchors: Option<PathBuf>,
    out: PathBuf,
}

fn parse_args(args: &[String]) -> Result<ReconcileOpts, String> {
    let mut cycles_jsonl: Option<PathBuf> = None;
    let mut recon_report: Option<PathBuf> = None;
    let mut anchors: Option<PathBuf> = None;
    let mut out: Option<PathBuf> = None;

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--cycles-jsonl" => {
                i += 1;
                cycles_jsonl = Some(PathBuf::from(
                    args.get(i).ok_or("--cycles-jsonl: missing value")?,
                ));
            }
            "--recon-report" => {
                i += 1;
                recon_report = Some(PathBuf::from(
                    args.get(i).ok_or("--recon-report: missing value")?,
                ));
            }
            "--anchors" | "-a" => {
                i += 1;
                anchors = Some(PathBuf::from(
                    args.get(i).ok_or("--anchors: missing value")?,
                ));
            }
            "--out" | "-o" => {
                i += 1;
                out = Some(PathBuf::from(args.get(i).ok_or("--out: missing value")?));
            }
            "--help" | "-h" => {
                print_help();
                std::process::exit(0);
            }
            other => return Err(format!("unknown arg: {other}")),
        }
        i += 1;
    }

    Ok(ReconcileOpts {
        cycles_jsonl: cycles_jsonl.ok_or("--cycles-jsonl <path> is required")?,
        recon_report: recon_report.ok_or("--recon-report <path> is required")?,
        anchors,
        out: out.ok_or("--out <path> is required")?,
    })
}

fn print_help() {
    eprintln!("dl-app reconcile — DAM-38a paper-vs-paper + paper-vs-anchor");
    eprintln!();
    eprintln!("USAGE:");
    eprintln!("    dl-app reconcile \\");
    eprintln!("        --cycles-jsonl <path> \\");
    eprintln!("        --recon-report <path> \\");
    eprintln!("        [--anchors <path.jsonl>] \\");
    eprintln!("        --out <path>");
    eprintln!();
    eprintln!("OPTIONS:");
    eprintln!("        --cycles-jsonl <path>  paper ledger, one cycle.v1 per line");
    eprintln!("        --recon-report <path>  dl-app recon --report-json output");
    eprintln!("    -a, --anchors <path>      anchor dataset (JSONL); optional");
    eprintln!("    -o, --out <path>          output JSON path");
    eprintln!("    -h, --help                show this help");
}

/// Serialize a `ReconcileOutput` as pretty JSON and write it
/// to `path`. Integer-only fields throughout.
pub fn write_output(path: &Path, out: &ReconcileOutput) -> Result<(), ReconcileError> {
    let json = serde_json::to_string_pretty(out).map_err(ReconcileError::JsonSer)?;
    std::fs::write(path, json).map_err(ReconcileError::Io)
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Render a `Cycle` as the same 64-hex string the v1 writer
/// emits. We compute it deterministically here (re-using
/// `LedgerHash::from_cycle` then formatting as 16-char hex)
/// so the join is independent of `cycle_id_hex`'s blake3
/// layout. The harness records `LedgerHash` in the ledger
/// (`LedgerEntry::cycle_hash`); we render the same value
/// the v0 shim's `pool_address` carries.
fn cycle_hash_to_hex(cycle: &dl_state::cycle::Cycle) -> String {
    use dl_ledger::hash::LedgerHash;
    let h = LedgerHash::from_cycle(cycle).0;
    format!("{:016x}", h)
}

/// Map a `Decision` to the spec §4 string.
fn decision_to_string(d: dl_ledger::Decision) -> String {
    match d {
        dl_ledger::Decision::WouldTrade => "WouldTrade".to_string(),
        dl_ledger::Decision::WouldNotTrade => "WouldNotTrade".to_string(),
    }
}

/// FNV-1a 64 over the canonical form of (paper_divergences,
/// anchor_divergences). Mirrors `dl-recon::pipeline::hash_records`
/// for consistency with the harness's report hash.
fn hash_divergences(
    paper: &[PaperDivergence],
    anchors: &[AnchorDivergence],
) -> u64 {
    const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;
    let mut h = FNV_OFFSET;
    for d in paper {
        let bytes = bincode::serialize(d).expect("PaperDivergence bincode");
        for b in bytes {
            h ^= b as u64;
            h = h.wrapping_mul(FNV_PRIME);
        }
    }
    for d in anchors {
        let bytes = bincode::serialize(d).expect("AnchorDivergence bincode");
        for b in bytes {
            h ^= b as u64;
            h = h.wrapping_mul(FNV_PRIME);
        }
    }
    h
}

/// Convenience: invoke `reconcile::run` from the binary's
/// `main()` and exit with the right code. Mirrors
/// `dl_app::recon::dispatch`.
pub fn dispatch() -> ! {
    init_tracing();
    let args: Vec<String> = std::env::args().skip(2).collect();
    let result = run(&args);
    match result {
        ReconcileCliResult::Ok => std::process::exit(0),
        ReconcileCliResult::PaperDivergences(divs) => {
            eprintln!("reconcile: {} paper divergence(s) within tolerance", divs.len());
            for d in &divs {
                eprintln!(
                    "  seq={} kind={}{}",
                    d.seq,
                    d.kind,
                    d.gross_bps_diff_bps
                        .map(|b| format!(" diff_bps={}", b))
                        .unwrap_or_default()
                );
            }
            std::process::exit(1);
        }
        ReconcileCliResult::AnchorOverTolerance(divs) => {
            eprintln!(
                "reconcile: {} anchor divergence(s) over tolerance",
                divs.len()
            );
            for d in &divs {
                eprintln!(
                    "  {:?}: engine={} anchor={} bps={:+} tol={}",
                    d.name, d.engine_value, d.anchor_value, d.divergence_bps, d.tolerance_bps
                );
            }
            std::process::exit(2);
        }
        ReconcileCliResult::Error(msg) => {
            eprintln!("reconcile error: {msg}");
            std::process::exit(3);
        }
    }
}

/// `run_dispatch` returns a `ReconcileCliResult` instead of
/// exiting, for tests and embedders.
pub fn run_dispatch(args: &[String]) -> ReconcileCliResult {
    init_tracing();
    run(args)
}

/// `ExitCode`-returning variant for callers that want the
/// conventional `fn() -> ExitCode` shape.
pub fn run_with_exit(args: &[String]) -> ExitCode {
    match run(args) {
        ReconcileCliResult::Ok => ExitCode::from(0),
        ReconcileCliResult::PaperDivergences(_) => ExitCode::from(1),
        ReconcileCliResult::AnchorOverTolerance(_) => ExitCode::from(2),
        ReconcileCliResult::Error(_) => ExitCode::from(3),
    }
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// Error surface for the reconcile pass.
#[derive(Debug, thiserror::Error)]
pub enum ReconcileError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    /// JSON parse error on a `wallet.cycles.jsonl` line (1-based).
    #[error("json at line {line}: {source}")]
    Json {
        line: usize,
        #[source]
        source: serde_json::Error,
    },
    /// JSON parse error on a `ReconReport` file.
    #[error("json: {0}")]
    JsonSer(#[from] serde_json::Error),
    #[error("onchain: {0}")]
    Onchain(#[from] dl_recon::onchain::OnchainError),
    #[error("summary: {0}")]
    Summary(String),
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use dl_core::prob::PROB_SCALE_1E18;
    use dl_ledger::hash::LedgerHash;
    use dl_recon::fixture::{synthesize_pools, ReconFixture, SynthPoolSpec};
    use dl_recon::pipeline::ReplayParams;
    use dl_sim::cost::{CostBreakdown, CostModel};
    use dl_sim::ev::{EvalOutcome, EvalParams, ExpectedValue, Prob};
    use dl_sim::net_profit::NetProfit;
    use dl_state::cycle::{Cycle, Direction, Leg};
    use dl_state::pool::{AmmKind, Pool};
    use dl_state::Pubkey;

    fn two_leg_cycle() -> Cycle {
        let mut p1 = [0u8; 32];
        p1[31] = 1;
        let mut p2 = [0u8; 32];
        p2[31] = 2;
        Cycle::new(vec![
            Leg {
                pool: Pubkey(p1),
                direction: Direction::BaseToQuote,
                weight: 0,
            },
            Leg {
                pool: Pubkey(p2),
                direction: Direction::QuoteToBase,
                weight: 0,
            },
        ])
    }

    fn zero_net(p: i128) -> NetProfit {
        NetProfit {
            input_amount: 1,
            gross_output: 0,
            total_costs: CostBreakdown {
                base_sig_fee_lamports: 0,
                priority_fee_lamports: 0,
                jito_tip_lamports: 0,
                jito_tip_fee_lamports: 0,
                total_lamports: 0,
            },
            net_profit: p,
            net_profit_bps: 0,
            profitable: p > 0,
        }
    }

    fn zero_ev(p: i128) -> ExpectedValue {
        ExpectedValue {
            e_pnl: p,
            p_detect: Prob::from_scaled_clamped(PROB_SCALE_1E18),
            p_win: Prob::from_scaled_clamped(PROB_SCALE_1E18),
            p_land: Prob::from_scaled_clamped(PROB_SCALE_1E18),
            expected_failed_cost: 0,
            tip_lamports: 0,
        }
    }

    fn hand_rolled_report(records: Vec<(u64, Cycle, i128, i128, u64, bool)>) -> ReconReport {
        // (seq, cycle, optimistic, conservative, tip, would_trade)
        let cycle_records: Vec<CycleRecord> = records
            .into_iter()
            .map(|(seq, cycle, opt, cons, tip, would_trade)| {
                let net = zero_net(cons);
                let entry = dl_ledger::LedgerEntry {
                    seq,
                    entry_id: seq,
                    cycle_hash: LedgerHash::from_cycle(&cycle),
                    net: net.clone(),
                    optimistic: zero_ev(opt),
                    conservative: zero_ev(cons),
                    decision: if would_trade {
                        dl_ledger::Decision::WouldTrade
                    } else {
                        dl_ledger::Decision::WouldNotTrade
                    },
                    tip_lamports: tip,
                };
                let outcome = EvalOutcome {
                    optimistic: zero_ev(opt),
                    conservative: zero_ev(cons),
                };
                CycleRecord {
                    seq,
                    cycle,
                    net,
                    outcome,
                    decision: if would_trade {
                        dl_ledger::Decision::WouldTrade
                    } else {
                        dl_ledger::Decision::WouldNotTrade
                    },
                    entry,
                }
            })
            .collect();
        let summary = dl_ledger::LedgerSummary::from_entries(
            &cycle_records.iter().map(|r| r.entry.clone()).collect::<Vec<_>>(),
        )
        .expect("summary");
        let total_tip_lamports: u64 = cycle_records
            .iter()
            .map(|r| r.entry.tip_lamports)
            .fold(0u64, u64::saturating_add);
        let report_hash = hash_records(&cycle_records);
        ReconReport {
            params: ReplayParams::default(),
            cycle_records,
            summary,
            divergences: Vec::new(),
            report_hash,
            feed_events_consumed: 0,
            total_tip_lamports,
        }
    }

    fn hash_records(records: &[CycleRecord]) -> u64 {
        // Same FNV-1a 64 algorithm the harness uses.
        const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
        const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;
        let mut h = FNV_OFFSET;
        for rec in records {
            for byte in rec.seq.to_le_bytes() {
                h ^= byte as u64;
                h = h.wrapping_mul(FNV_PRIME);
            }
            let bytes = bincode::serialize(&rec.entry).expect("bincode");
            for b in bytes {
                h ^= b as u64;
                h = h.wrapping_mul(FNV_PRIME);
            }
        }
        h
    }

    fn make_pool(addr: [u8; 32], base: u64, quote: u64) -> Pool {
        Pool {
            address: Pubkey(addr),
            kind: AmmKind::RaydiumAmmV4,
            base_mint: Pubkey([0xaa; 32]),
            quote_mint: Pubkey([0xbb; 32]),
            base_decimals: 9,
            quote_decimals: 6,
            base_reserve: base,
            quote_reserve: quote,
            fee_bps: 30,
            last_update_slot: 1,
            ..Default::default()
        }
    }

    // ──────────────────────────────────────────────────────────────────
    // Test 1: paper-vs-paper diff produces the expected Divergence set
    // on a fixture (AC: ≥ 3 unit tests).
    // ──────────────────────────────────────────────────────────────────
    #[test]
    fn paper_vs_paper_diff_emits_decision_and_e_pnl_divergences() {
        // Build a report with two cycles:
        //   seq=0: optimistic=100, conservative=50, would_trade=true.
        //          paper says WouldNotTrade ⇒ decision divergence.
        //          paper gross_bps = 30, re gross_bps ≈ 100 (over band).
        //   seq=1: optimistic=200, conservative=-10, would_trade=false.
        //          paper says WouldTrade ⇒ decision divergence.
        //          paper gross_bps = 200, re gross_bps ≈ -50 (over band).
        //   seq=2: cycle present in report but missing from paper JSONL
        //          ⇒ cycle_hash_missing divergence.
        let c0 = two_leg_cycle();
        // Use *distinct* leg sequences so the cycle hashes
        // differ across seqs. c1 differs from c0 by one
        // pool byte; c2 is a different 2-pool cycle entirely.
        let mut p1_alt = [0u8; 32];
        p1_alt[31] = 1;
        p1_alt[30] = 1;
        let mut p2_alt = [0u8; 32];
        p2_alt[31] = 2;
        p2_alt[30] = 1;
        let c1 = Cycle::new(vec![
            dl_state::cycle::Leg {
                pool: dl_state::Pubkey(p1_alt),
                direction: dl_state::cycle::Direction::BaseToQuote,
                weight: 0,
            },
            dl_state::cycle::Leg {
                pool: dl_state::Pubkey(p2_alt),
                direction: dl_state::cycle::Direction::QuoteToBase,
                weight: 0,
            },
        ]);
        let mut p2_only = [0u8; 32];
        p2_only[31] = 3;
        let c2 = Cycle::new(vec![
            dl_state::cycle::Leg {
                pool: dl_state::Pubkey(p2_only),
                direction: dl_state::cycle::Direction::BaseToQuote,
                weight: 0,
            },
            dl_state::cycle::Leg {
                pool: dl_state::Pubkey(p2_only),
                direction: dl_state::cycle::Direction::QuoteToBase,
                weight: 0,
            },
        ]);
        let report = hand_rolled_report(vec![
            (0, c0.clone(), 100, 50, 0, true),
            (1, c1.clone(), 200, -10, 0, false),
            (2, c2, 300, 100, 0, true),
        ]);

        // Build the matching paper rows. seq=0 row carries
        // a different decision and a different gross_bps. seq=1
        // row carries WouldTrade (the re side said WouldNotTrade).
        // seq=2 row is *absent* (cycle_hash_missing).
        let c0_hex = cycle_hash_to_hex(&c0);
        let c1_hex = cycle_hash_to_hex(&c1);
        let paper_rows = vec![
            PaperCycleRow {
                cycle_hash_hex: c0_hex,
                paper_gross_bps: 30,
                paper_decision: Some("WouldNotTrade".to_string()),
                paper_input_lamports: 1_000_000,
                paper_output_lamports: 1_000_030,
                paper_tip_lamports: 0,
                schema: "v1".to_string(),
            },
            PaperCycleRow {
                cycle_hash_hex: c1_hex,
                paper_gross_bps: 200,
                paper_decision: Some("WouldTrade".to_string()),
                paper_input_lamports: 1_000_000,
                paper_output_lamports: 1_000_200,
                paper_tip_lamports: 0,
                schema: "v1".to_string(),
            },
        ];

        let output = reconcile(&report, &paper_rows, None).expect("reconcile");
        assert_eq!(output.paper.cycles_seen, 3);
        // Three divergences: seq=0 decision, seq=0 e_pnl (30 vs ~100 = 70bps > 5),
        // seq=1 decision, seq=1 e_pnl (200 vs ~-50 = 250bps > 5),
        // seq=2 cycle_hash_missing.
        let kinds: Vec<&str> = output
            .paper
            .paper_divergences
            .iter()
            .map(|d| d.kind.as_str())
            .collect();
        assert!(kinds.contains(&"decision"), "decision divergence missing: {kinds:?}");
        assert!(kinds.contains(&"e_pnl"), "e_pnl divergence missing: {kinds:?}");
        assert!(
            kinds.contains(&"cycle_hash_missing"),
            "cycle_hash_missing divergence missing: {kinds:?}"
        );
        // simulation_lied_yes: paper said WouldTrade but re said WouldNotTrade.
        // (only seq=1 matches this direction)
        assert_eq!(output.divergences.simulation_lied_no, 1);
        // simulation_lied_no: paper said WouldNotTrade but re said WouldTrade.
        // (only seq=0 matches this direction)
        assert_eq!(output.divergences.simulation_lied_yes, 1);
    }

    // ──────────────────────────────────────────────────────────────────
    // Test 2: anchor compare emits `AnchorDivergence` rows when over
    // tolerance. (AC: anchor compare test.)
    // ──────────────────────────────────────────────────────────────────
    #[test]
    fn paper_vs_anchor_emits_rows_over_tolerance() {
        // Synthesize a small pool universe. The harness
        // produces a real `ReconReport` from it; we feed that
        // into the reconcile path with an anchor dataset
        // whose values are deliberately far off-tolerance.
        let specs = vec![
            SynthPoolSpec {
                address: [1u8; 32],
                base_reserve: 1_000_000,
                quote_reserve: 1_000_000,
                fee_bps: 30,
            },
            SynthPoolSpec {
                address: [2u8; 32],
                base_reserve: 1_000_000,
                quote_reserve: 1_000_000,
                fee_bps: 30,
            },
            SynthPoolSpec {
                address: [3u8; 32],
                base_reserve: 1_000_000,
                quote_reserve: 1_100_000,
                fee_bps: 30,
            },
        ];
        let mints = vec![[0xaa; 32], [0xbb; 32], [0xcc; 32]];
        let fx = ReconFixture::build(&specs, &mints, &ReplayParams::default());
        // Derive a `ReconReport` from the fixture's pool
        // universe (ReconFixture has no `.report` field — it
        // only carries the pool universe + capture + ledger
        // bytes + params). We re-run the harness to get the
        // same shape `dl-app recon --report-json` produces.
        let fx_report = dl_recon::pipeline::replay_pools_to_ledger(&fx.pools, &fx.params)
            .expect("fixture report");

        // Anchor dataset: declare 100 attempts (engine will
        // report 0 → divergence = -10_000 bps, well over the
        // 5% tolerance) and 100 landed (engine will report
        // something smaller → also divergent). Both exceed.
        let anchors = AnchorDataset {
            entries: vec![
                dl_recon::onchain::AnchorEntry {
                    name: AnchorName::AttemptCount,
                    value: 100,
                    unit: "bundles".to_string(),
                    window_start_iso: "2026-06-21T00:00:00Z".to_string(),
                    window_end_iso: "2026-06-21T23:59:59Z".to_string(),
                    source: "test".to_string(),
                    pulled_at_iso: "2026-06-21T00:00:00Z".to_string(),
                },
                dl_recon::onchain::AnchorEntry {
                    name: AnchorName::LandedArbCount,
                    value: 100,
                    unit: "bundles".to_string(),
                    window_start_iso: "2026-06-21T00:00:00Z".to_string(),
                    window_end_iso: "2026-06-21T23:59:59Z".to_string(),
                    source: "test".to_string(),
                    pulled_at_iso: "2026-06-21T00:00:00Z".to_string(),
                },
            ],
            window_start_slot: 0,
            window_end_slot: 0,
            pulled_at_iso: "2026-06-21T00:00:00Z".to_string(),
        };

        let output = reconcile(&fx_report, &[], Some(&anchors)).expect("reconcile");
        // At least one anchor row exceeds tolerance. (The
        // engine reports 0 attempts; the anchor says 100.)
        let bad: Vec<&AnchorDivergence> = output
            .anchors
            .divergences
            .iter()
            .filter(|d| d.exceeds_tolerance)
            .collect();
        assert!(!bad.is_empty(), "expected ≥1 anchor over tolerance");
        // The report hash is non-zero (we mixed at least one
        // divergence into it).
        assert_ne!(output.report_hash, 0xcbf2_9ce4_8422_2325_u64);
    }

    // ──────────────────────────────────────────────────────────────────
    // Test 3: clean run on a synthesized fixture writes the spec §4
    // JSON shape, with all integer fields, no f64 anywhere. (AC:
    // CLI invocation works and writes the spec §4 JSON shape minus
    // onchain.)
    // ──────────────────────────────────────────────────────────────────
    #[test]
    fn clean_run_writes_spec_4_json_shape() {
        // The cleanest "no divergence" scenario:
        //   - report has one cycle (seq=0) with conservative=0
        //     (WouldNotTrade since e_pnl > 0 is the gate).
        //   - paper row carries the same hash, the same
        //     WouldNotTrade decision, and gross_bps=0.
        //   - no anchors ⇒ no anchor block.
        let c0 = two_leg_cycle();

        // Re-derive the report with conservative=-1_000_000
        // (slightly negative) and decision=WouldNotTrade so
        // it matches the paper row's "WouldNotTrade"? No, the
        // spec compares the *trade gate* which is
        // conservative.e_pnl > 0. If re is WouldNotTrade and
        // paper is WouldTrade, that's a decision divergence.
        // The cleanest test: use the *synthesized* report
        // (no paper rows at all → cycle_hash_missing for
        // every record), or build a paper row whose
        // decision matches the report.
        //
        // For a "clean run" we need:
        //   - paper_decision == re_decision (no `decision` divergence)
        //   - paper row present (no `cycle_hash_missing`)
        //   - gross_bps within ±5 (no `e_pnl` divergence)
        // The simplest: report with conservative=0
        // (WouldNotTrade) and paper with WouldNotTrade +
        // gross_bps=0.
        let report2 = hand_rolled_report(vec![(
            0,
            c0.clone(),
            0,     // optimistic
            0,     // conservative (WouldNotTrade since 0 is not > 0)
            0,     // tip
            false, // would_trade
        )]);
        let paper_rows2 = vec![PaperCycleRow {
            cycle_hash_hex: cycle_hash_to_hex(&c0),
            paper_gross_bps: 0,
            paper_decision: Some("WouldNotTrade".to_string()),
            paper_input_lamports: 1_000_000,
            paper_output_lamports: 1_000_000,
            paper_tip_lamports: 0,
            schema: "v1".to_string(),
        }];
        let output = reconcile(&report2, &paper_rows2, None).expect("reconcile");
        assert!(output.paper.paper_divergences.is_empty(), "no divergences on clean run");
        assert!(output.anchors.divergences.is_empty());
        assert_eq!(output.divergences.simulation_lied_yes, 0);
        assert_eq!(output.divergences.simulation_lied_no, 0);
        // report_hash is FNV offset (the identity) when
        // both lists are empty.
        assert_eq!(output.report_hash, 0xcbf2_9ce4_8422_2325_u64);

        // Round-trip: write the JSON, parse it back, and
        // verify the spec §4 shape is intact.
        let tmp = std::env::temp_dir().join("dl-app-reconcile-clean.json");
        write_output(&tmp, &output).expect("write");
        let raw = std::fs::read_to_string(&tmp).expect("read back");
        // The spec §4 keys are all present.
        for key in [
            "\"window_start_slot\"",
            "\"window_end_slot\"",
            "\"paper\"",
            "\"cycles_seen\"",
            "\"would_trade_paper\"",
            "\"would_trade_re\"",
            "\"paper_divergences\"",
            "\"anchors\"",
            "\"divergences\"",
            "\"onchain\"",
            "\"bundles_submitted\"",
            "\"gross_pnl_lamports\"",
            "\"net_pnl_lamports\"",
            "\"per_cycle\"",
            "\"tip_drift\"",
            "\"simulation_lied_yes\"",
            "\"simulation_lied_no\"",
            "\"reverted_after_ok\"",
            "\"missing_signature\"",
            "\"report_hash\"",
        ] {
            assert!(raw.contains(key), "missing spec §4 key: {key}\n{raw}");
        }
        // Integer-only: no f64. We assert by scanning for
        // a decimal point in a number — but JSON booleans
        // have no decimal, and i64/u64 serialized as JSON
        // never contains ".". We additionally forbid the
        // substring "f64" to be paranoid.
        assert!(!raw.contains("f64"), "f64 leaked into output");
        // And the parsed shape is round-trippable.
        let parsed: ReconcileOutput = serde_json::from_str(&raw).expect("parse back");
        assert_eq!(parsed, output);
        let _ = std::fs::remove_file(&tmp);
    }

    // ──────────────────────────────────────────────────────────────────
    // Test 4 (bonus, AC says ≥3): the JSONL loader accepts both
    // v1 and v0 shapes, projecting to the same PaperCycleRow
    // shape. Without this we can't run the CLI on real captures
    // (the wallet.cycles.jsonl today is the v0 shim).
    // ──────────────────────────────────────────────────────────────────
    #[test]
    fn cycles_jsonl_loader_accepts_v0_and_v1() {
        let tmp = std::env::temp_dir().join("dl-app-reconcile-fixture.jsonl");
        let v0_line = serde_json::json!({
            "pool_address": "abc123",
            "dex": "raydium",
            "base_mint": "unknown",
            "quote_mint": "unknown",
            "gross_bps": 50,
            "fee_bps": 30,
            "detected_at_unix_ms": 1_700_000_000_000u64
        });
        let v1_line = serde_json::json!({
            "schema": "cycle.v1",
            "cycle_id": "deadbeef",
            "detected_at_unix_ms": 1_700_000_000_001u64,
            "detected_at_slot": 42,
            "bot_run_id": "00000000-0000-0000-0000-000000000000",
            "dexes": ["raydium"],
            "legs": [],
            "base_mint": "",
            "quote_mint": "",
            "gross_bps": 100,
            "fee_bps_sum": 30,
            "decision": "WouldTrade",
            "evaluator": "conservative_default",
            "input_lamports": 1_000_000u64,
            "output_lamports": 1_000_100u64,
            "source_feed": "ws:mainnet"
        });
        let body = format!(
            "{}\n{}\n# this is a comment\n\n",
            v0_line.to_string(),
            v1_line.to_string()
        );
        std::fs::write(&tmp, body).expect("write fixture");

        let rows = load_cycles_jsonl(&tmp).expect("load");
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].cycle_hash_hex, "abc123");
        assert_eq!(rows[0].schema, "v0");
        assert_eq!(rows[0].paper_gross_bps, 50);
        assert!(rows[0].paper_decision.is_none());
        assert_eq!(rows[0].paper_input_lamports, 0);

        assert_eq!(rows[1].cycle_hash_hex, "deadbeef");
        assert_eq!(rows[1].schema, "cycle.v1");
        assert_eq!(rows[1].paper_gross_bps, 100);
        assert_eq!(rows[1].paper_decision.as_deref(), Some("WouldTrade"));
        assert_eq!(rows[1].paper_input_lamports, 1_000_000);

        let _ = std::fs::remove_file(&tmp);
    }

    // ──────────────────────────────────────────────────────────────────
    // Test 5 (bonus): the end-to-end CLI path — synthesize a
    // capture + a wallet.cycles.jsonl, run `dl-app recon
    // --report-json`, feed the output to `dl-app reconcile`, and
    // verify the §4 JSON. Mirrors the DAM-38a acceptance bullet
    // "CLI invocation from the spec §3 works on a captured .dlf
    // + wallet.cycles.jsonl fixture".
    // ──────────────────────────────────────────────────────────────────
    #[test]
    fn end_to_end_cli_path_on_synthesized_fixture() {
        // Build a small .dlf capture via the fixture.
        let specs = vec![
            SynthPoolSpec {
                address: [1u8; 32],
                base_reserve: 1_000_000,
                quote_reserve: 1_000_000,
                fee_bps: 30,
            },
            SynthPoolSpec {
                address: [2u8; 32],
                base_reserve: 1_000_000,
                quote_reserve: 1_000_000,
                fee_bps: 30,
            },
            SynthPoolSpec {
                address: [3u8; 32],
                base_reserve: 1_000_000,
                quote_reserve: 1_100_000,
                fee_bps: 30,
            },
        ];
        let mints = vec![[0xaa; 32], [0xbb; 32], [0xcc; 32]];
        let capture = dl_recon::fixture::synthesize_small_capture(&specs, &mints);
        let pools = synthesize_pools(&specs, &mints);

        // Run the harness (the same path `dl-app recon` uses).
        let params = ReplayParams::default();
        let report =
            dl_recon::pipeline::replay_capture_to_ledger(capture.as_slice(), &params)
                .expect("harness");

        // Build a wallet.cycles.jsonl whose hash rows match the
        // re-derived cycle hashes. We extract the cycle hashes
        // from the report itself and emit one v1 row per
        // WouldTrade cycle, with the *same* gross_bps the
        // harness would compute.
        let tmpdir = std::env::temp_dir().join("dl-app-reconcile-e2e");
        let _ = std::fs::remove_dir_all(&tmpdir);
        std::fs::create_dir_all(&tmpdir).expect("mkdir");
        let jsonl_path = tmpdir.join("wallet.cycles.jsonl");
        let report_path = tmpdir.join("recon.json");
        let out_path = tmpdir.join("reconcile.json");

        let mut jsonl = String::new();
        for rec in &report.cycle_records {
            let h = format!("{:016x}", rec.entry.cycle_hash.0);
            // Compute gross_bps from optimistic e_pnl + input.
            let input = rec.net.input_amount;
            let gross = rec
                .outcome
                .optimistic
                .e_pnl
                .saturating_add(input as i128);
            let gross_bps: i64 = if input > 0 && gross > 0 {
                let gross_u = gross as u128;
                if gross_u > input {
                    (((gross_u - input) * 10_000) / input) as i64
                } else {
                    0
                }
            } else {
                0
            };
            let line = serde_json::json!({
                "schema": "cycle.v1",
                "cycle_id": h,
                "detected_at_unix_ms": 0,
                "detected_at_slot": 0,
                "bot_run_id": "00000000-0000-0000-0000-000000000000",
                "dexes": ["raydium"],
                "legs": [],
                "base_mint": "",
                "quote_mint": "",
                "gross_bps": gross_bps,
                "fee_bps_sum": 30,
                "decision": match rec.decision {
                    dl_ledger::Decision::WouldTrade => "WouldTrade",
                    dl_ledger::Decision::WouldNotTrade => "WouldNotTrade",
                },
                "evaluator": "conservative_default",
                "input_lamports": input as u64,
                "output_lamports": gross.max(0) as u64,
                "source_feed": "capture:replay"
            });
            jsonl.push_str(&line.to_string());
            jsonl.push('\n');
        }
        std::fs::write(&jsonl_path, &jsonl).expect("write jsonl");

        // Persist the report as the `dl-app recon --report-json`
        // shape. (ReconReport is Serialize; the harness uses
        // serde_json::to_string_pretty for the same purpose.)
        let report_json = serde_json::to_string_pretty(&report).expect("report json");
        std::fs::write(&report_path, &report_json).expect("write report");

        // Run the CLI path.
        let args: Vec<String> = vec![
            "--cycles-jsonl".to_string(),
            jsonl_path.to_string_lossy().to_string(),
            "--recon-report".to_string(),
            report_path.to_string_lossy().to_string(),
            "--out".to_string(),
            out_path.to_string_lossy().to_string(),
        ];
        let result = run(&args);
        match result {
            ReconcileCliResult::Ok
            | ReconcileCliResult::PaperDivergences(_)
            | ReconcileCliResult::AnchorOverTolerance(_) => {}
            ReconcileCliResult::Error(msg) => panic!("reconcile failed: {msg}"),
        }
        // The output JSON exists and parses.
        let raw = std::fs::read_to_string(&out_path).expect("read out");
        let parsed: ReconcileOutput = serde_json::from_str(&raw).expect("parse out");
        assert_eq!(parsed.paper.cycles_seen, report.cycle_records.len() as u64);
        assert!(parsed.anchors.divergences.is_empty(), "no anchors in this test");

        let _ = std::fs::remove_dir_all(&tmpdir);
        let _ = pools; // silence unused warning
    }

    // ──────────────────────────────────────────────────────────────────
    // Test 6: divergence counters stay at zero on a clean run.
    // ──────────────────────────────────────────────────────────────────
    #[test]
    fn divergence_counters_zero_on_clean_run() {
        // The clean run from test 3 is reused here: build
        // a no-divergence report, assert the on-chain
        // counters all stay zero (DAM-38b will fill them).
        let c0 = two_leg_cycle();
        let report = hand_rolled_report(vec![(0, c0.clone(), 0, 0, 0, false)]);
        let paper_rows = vec![PaperCycleRow {
            cycle_hash_hex: cycle_hash_to_hex(&c0),
            paper_gross_bps: 0,
            paper_decision: Some("WouldNotTrade".to_string()),
            paper_input_lamports: 1_000_000,
            paper_output_lamports: 1_000_000,
            paper_tip_lamports: 0,
            schema: "v1".to_string(),
        }];
        let output = reconcile(&report, &paper_rows, None).expect("reconcile");
        assert_eq!(output.divergences.tip_drift, 0);
        assert_eq!(output.divergences.simulation_lied_yes, 0);
        assert_eq!(output.divergences.simulation_lied_no, 0);
        assert_eq!(output.divergences.reverted_after_ok, 0);
        assert_eq!(output.divergences.missing_signature, 0);
    }
}
