//! `dl-pipeline` — Data pipeline crate (DAM-46).
//!
//! Ingests `cycle.v1` records (and the `trade.v1`, `recon_report_v1` shapes
//! when their contracts land) into the local warehouse, runs daily
//! reconciliation against `recon_report_v1`, and verifies idempotent re-runs.
//!
//! ## Scope
//!
//! This crate is owned by **Data**. It is the substrate that Quant, BotSRE,
//! Product, and the operator console all read from. See
//! `docs/architecture/data-architecture-v1.md` for the design and
//! `docs/contracts/cycle.v1.md` for the wire contract.
//!
//! ## Working rules (AGENTS.md)
//!
//! - **Data correctness over completeness.** The slice we cover is provably
//!   right; the slice we don't cover is honest about what it doesn't cover.
//! - **Idempotency.** `ingest_cycle_v1` re-run on the same input is a no-op
//!   for the second call (the `cycle_id` is a stable hash of the record).
//! - **Backfill safety.** A backfill is a replay; the warehouse's
//!   `INSERT OR IGNORE` semantics make that safe.
//! - **Integer-only.** The value path never touches `f32` / `f64`; the only
//!   place a float is allowed is the read-only query layer
//!   (`src/query/mod.rs`, empty placeholder).
//! - **Lineage.** Every row in `cycle_v1` carries `pipeline_run_id` and
//!   `ingested_at_unix_ms`; every reject in `dl_pipeline_rejects` carries the
//!   raw line and the reason.
//!
//! ## Warehouse abstraction
//!
//! The crate talks to a `Warehouse` trait, not to a specific store. Today the
//! production impl is [`JsonlWarehouse`] — date-partitioned append-only JSONL
//! with a `blake3` checksum sidecar. When the `duckdb` crate becomes
//! available in the workspace (it is not in the offline cache as of 2026-06-21;
//! tracked as a follow-up), `DuckdbWarehouse` becomes the production impl
//! and `JsonlWarehouse` falls back to "test mode only." The trait surface
//! was designed for that swap: the parser, validator, reject writer,
//! reconciler, verifier, and CLI do not change.
//!
//! ## Reject reasons
//!
//! See `docs/contracts/cycle.v1.md` §"Reject reasons" for the canonical list.
//! The validator (`src/validate.rs`) emits one of these as a `Reject` row.
//!
//! ## Privacy
//!
//! The validator rejects any line whose field names start with `_priv_` or
//! that match the `signing_material` regex (defensive; the writer should
//! never emit such a field). Pipeline policy per
//! `docs/architecture/data-architecture-v1.md` §4.
//!
//! ## Float-free CI guard
//!
//! `tests/floats.rs` greps `src/` and `tests/` for `f32` / `f64` as bare
//! tokens. The test fails on a hit. This mirrors the `dl-recon` crate's
//! `tests/floats.rs` and is part of the project-wide integer-only invariant.

#![deny(unsafe_code)]
#![warn(clippy::all)]

use std::path::{Path, PathBuf};

use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};

pub mod error;
pub mod ingest;
pub mod query;
pub mod recon;
pub mod reject;
pub mod validate;
pub mod warehouse;

pub use error::PipelineError;
pub use ingest::{
    ingest_cycle_v1, ingest_trade_v1, parse_cycle_v1_line, parse_trade_v1_line, IngestStats,
};
pub use recon::{reconcile, DailyReconV1, ReconRow, ReconStats};
pub use reject::{Reject, RejectReason};
pub use validate::{validate_cycle_v1, validate_trade_v1, ValidationError};
pub use warehouse::{
    DailyChecksum, JsonlWarehouse, Partition, Warehouse, WarehouseConfig, DEFAULT_WAREHOUSE_ROOT,
};

/// Cycle v1 wire type. One JSON object per line in `cycle.v1.jsonl`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CycleV1 {
    /// Always `"cycle.v1"`. The contract version sentinel.
    pub schema: String,
    /// 64-char lowercase hex. `blake3(sorted_legs_json || detected_at_slot)`.
    pub cycle_id: String,
    /// UTC milliseconds since epoch.
    pub detected_at_unix_ms: i64,
    /// Solana slot at detection.
    pub detected_at_slot: u64,
    /// UUIDv4 of the bot run.
    pub bot_run_id: String,
    /// One entry per leg, from the AmmKind enum.
    pub dexes: Vec<String>,
    /// The cycle path, in order. Length 2 or more.
    pub legs: Vec<LegV1>,
    pub base_mint: String,
    pub quote_mint: String,
    /// Integer basis points, signed.
    pub gross_bps: i64,
    pub fee_bps_sum: u32,
    pub decision: String,
    pub evaluator: String,
    pub input_lamports: u64,
    pub output_lamports: u64,
    pub source_feed: String,
}

/// One leg in a `cycle.v1` cycle path.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LegV1 {
    /// Base58-encoded 32-byte pubkey.
    pub pool: String,
    pub dex: String,
    pub direction: String,
    pub weight: i64,
}

/// Paper trade v1 wire type. The pipeline ingests this in parallel to
/// `cycle.v1` so downstream consumers can join the two on `cycle_id`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TradeV1 {
    pub schema: String,
    pub trade_id: String,
    pub cycle_id: String,
    pub bot_run_id: String,
    pub ts_unix_ms: i64,
    pub input_lamports: u64,
    pub output_lamports: u64,
    pub decision: String,
    pub evaluator: String,
}

/// Recon report v1 — emitted by `dl-recon` (BotSRE). One per capture run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReconReportV1 {
    pub schema: String,
    pub report_id: String,
    pub bot_run_id: String,
    pub captured_at_unix_ms: i64,
    pub reconciled_at_unix_ms: i64,
    /// One entry per cycle the recon pass examined, keyed by `cycle_id`.
    pub cycles: Vec<ReconCycleEntryV1>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReconCycleEntryV1 {
    pub cycle_id: String,
    pub matched_in_capture: bool,
    pub gross_bps_drift: i64,
    pub evaluator_differs: bool,
}

/// Pipeline run id, attached to every row and reject so lineage is queryable.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PipelineRunId(pub String);

impl PipelineRunId {
    pub fn new() -> Self {
        use std::time::{SystemTime, UNIX_EPOCH};
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let hi = (nanos as u64).wrapping_add(std::process::id() as u64);
        let mut bytes = [0u8; 16];
        bytes[..8].copy_from_slice(&hi.to_le_bytes());
        bytes[8..].copy_from_slice(&(nanos as u64).wrapping_add(1).to_le_bytes());
        let mut s = String::with_capacity(36);
        for (i, b) in bytes.iter().enumerate() {
            s.push_str(&format!("{:02x}", b));
            if i == 3 || i == 5 || i == 7 || i == 9 {
                s.push('-');
            }
        }
        PipelineRunId(s)
    }
}

impl Default for PipelineRunId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for PipelineRunId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Partition key: a UTC date in YYYY-MM-DD form, derived from a Unix-ms
/// timestamp.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct DatePartition(pub String);

impl DatePartition {
    /// Derive a YYYY-MM-DD partition string from a UTC Unix-ms timestamp.
    pub fn from_unix_ms(ms: i64) -> Result<Self, PipelineError> {
        let dt: DateTime<Utc> = DateTime::<Utc>::from_timestamp_millis(ms)
            .ok_or_else(|| PipelineError::Date(format!("invalid unix ms: {ms}")))?;
        Ok(DatePartition(dt.format("%Y-%m-%d").to_string()))
    }

    /// Parse a YYYY-MM-DD string into a `DatePartition`. Used by the CLI's
    /// `--date YYYY-MM-DD` flag.
    pub fn parse(s: &str) -> Result<Self, PipelineError> {
        let nd = NaiveDate::parse_from_str(s, "%Y-%m-%d")
            .map_err(|e| PipelineError::Date(e.to_string()))?;
        Ok(DatePartition(nd.format("%Y-%m-%d").to_string()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for DatePartition {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Helper: a list of `cycle.v1` JSONL files or directories. The CLI accepts
/// either a single file path or a directory path (in which case we walk
/// every `*.jsonl` underneath, sorted lexicographically for determinism).
pub fn expand_input_paths(input: &Path) -> Result<Vec<PathBuf>, PipelineError> {
    if !input.exists() {
        return Err(PipelineError::NotFound(input.display().to_string()));
    }
    if input.is_file() {
        return Ok(vec![input.to_path_buf()]);
    }
    let mut out = Vec::new();
    for entry in walkdir::WalkDir::new(input)
        .follow_links(false)
        .into_iter()
        .filter_map(|e| e.ok())
    {
        let p = entry.path();
        if p.is_file() && p.extension().and_then(|s| s.to_str()) == Some("jsonl") {
            out.push(p.to_path_buf());
        }
    }
    out.sort();
    if out.is_empty() {
        return Err(PipelineError::NotFound(format!(
            "no .jsonl files under {}",
            input.display()
        )));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn date_partition_from_unix_ms_round_trip() {
        // 2026-06-20T08:00:00Z (UTC). 1781913600000 ms.
        let p = DatePartition::from_unix_ms(1781913600000).unwrap();
        assert_eq!(p.as_str(), "2026-06-20");
    }

    #[test]
    fn date_partition_parse() {
        let p = DatePartition::parse("2026-06-21").unwrap();
        assert_eq!(p.as_str(), "2026-06-21");
        assert!(DatePartition::parse("not-a-date").is_err());
    }

    #[test]
    fn pipeline_run_id_is_uuid_shaped() {
        let id = PipelineRunId::new();
        let s = id.0.clone();
        assert_eq!(s.len(), 36);
        let parts: Vec<&str> = s.split('-').collect();
        assert_eq!(parts.len(), 5);
        assert_eq!(parts[0].len(), 8);
        assert_eq!(parts[1].len(), 4);
    }

    #[test]
    fn expand_input_paths_handles_file_and_dir() {
        let dir = tempfile::tempdir().unwrap();
        let f1 = dir.path().join("a.jsonl");
        let f2 = dir.path().join("b.jsonl");
        std::fs::write(&f1, b"{\"schema\":\"cycle.v1\"}\n").unwrap();
        std::fs::write(&f2, b"{}\n").unwrap();
        let got = expand_input_paths(&f1).unwrap();
        assert_eq!(got, vec![f1.clone()]);
        let got = expand_input_paths(dir.path()).unwrap();
        assert_eq!(got, vec![f1, f2]);
        let bad = dir.path().join("nope.jsonl");
        assert!(expand_input_paths(&bad).is_err());
    }
}
