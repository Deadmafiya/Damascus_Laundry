//! Ingest module — public API: `ingest_cycle_v1`, `ingest_trade_v1`,
//! `parse_cycle_v1_line`, `parse_trade_v1_line`.

use std::path::Path;

use serde_json::Value;
use tracing::{debug, warn};

use crate::error::PipelineError;
use crate::reject::{Reject, RejectReason};
use crate::validate::{
    line_contains_signing_material, validate_cycle_v1, validate_trade_v1,
};
use crate::warehouse::Warehouse;
use crate::{CycleV1, PipelineRunId, TradeV1};

pub mod cycle;
pub mod trade;

/// Aggregate ingest stats over a batch (one or more files).
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct IngestStats {
    pub files_processed: u64,
    pub lines_read: u64,
    pub lines_parsed: u64,
    pub lines_written: u64,
    pub lines_ignored_dup: u64,
    pub rejects: u64,
}

impl IngestStats {
    pub fn merge(&mut self, other: IngestStats) {
        self.files_processed += other.files_processed;
        self.lines_read += other.lines_read;
        self.lines_parsed += other.lines_parsed;
        self.lines_written += other.lines_written;
        self.lines_ignored_dup += other.lines_ignored_dup;
        self.rejects += other.rejects;
    }
}

impl std::fmt::Display for IngestStats {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "files={} read={} written={} dup={} rejects={}",
            self.files_processed,
            self.lines_read,
            self.lines_written,
            self.lines_ignored_dup,
            self.rejects
        )
    }
}

// Re-export the cycle/trade IngestStats names under the unified
// `IngestStats` so callers don't need to care which one they got.
impl From<IngestCycleStats> for IngestStats {
    fn from(s: IngestCycleStats) -> Self {
        IngestStats {
            files_processed: s.files_processed,
            lines_read: s.lines_read,
            lines_parsed: s.lines_parsed,
            lines_written: s.lines_written,
            lines_ignored_dup: s.lines_ignored_dup,
            rejects: s.rejects,
        }
    }
}

impl From<IngestTradeStats> for IngestStats {
    fn from(s: IngestTradeStats) -> Self {
        IngestStats {
            files_processed: s.files_processed,
            lines_read: s.lines_read,
            lines_parsed: s.lines_parsed,
            lines_written: s.lines_written,
            lines_ignored_dup: s.lines_ignored_dup,
            rejects: s.rejects,
        }
    }
}

/// Parse one line of `cycle.v1` JSONL.
pub fn parse_cycle_v1_line(line: &str) -> Result<CycleV1, ValidationErrorOut> {
    if line_contains_signing_material(line) {
        return Err(ValidationErrorOut {
            reason: RejectReason::SigningMaterial,
            message: "line matches signing_material regex".to_string(),
        });
    }
    let value: Value = serde_json::from_str(line).map_err(|e| ValidationErrorOut {
        reason: RejectReason::NotJson,
        message: e.to_string(),
    })?;
    validate_cycle_v1(&value).map_err(|e| ValidationErrorOut {
        reason: e.reason,
        message: e.message,
    })?;
    let row: CycleV1 = serde_json::from_value(value).map_err(|e| ValidationErrorOut {
        reason: RejectReason::FieldTypeWrong,
        message: format!("post-validate re-parse: {e}"),
    })?;
    Ok(row)
}

/// Parse one line of `trade.v1` JSONL.
pub fn parse_trade_v1_line(line: &str) -> Result<TradeV1, ValidationErrorOut> {
    if line_contains_signing_material(line) {
        return Err(ValidationErrorOut {
            reason: RejectReason::SigningMaterial,
            message: "line matches signing_material regex".to_string(),
        });
    }
    let value: Value = serde_json::from_str(line).map_err(|e| ValidationErrorOut {
        reason: RejectReason::NotJson,
        message: e.to_string(),
    })?;
    validate_trade_v1(&value).map_err(|e| ValidationErrorOut {
        reason: e.reason,
        message: e.message,
    })?;
    let row: TradeV1 = serde_json::from_value(value).map_err(|e| ValidationErrorOut {
        reason: RejectReason::FieldTypeWrong,
        message: format!("post-validate re-parse: {e}"),
    })?;
    Ok(row)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidationErrorOut {
    pub reason: RejectReason,
    pub message: String,
}

impl std::fmt::Display for ValidationErrorOut {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.reason, self.message)
    }
}

impl std::error::Error for ValidationErrorOut {}

/// Ingest one or more `cycle.v1` JSONL files into the warehouse.
pub fn ingest_cycle_v1<W: Warehouse + ?Sized>(
    warehouse: &mut W,
    path: &Path,
    pipeline_run_id: &PipelineRunId,
) -> Result<IngestStats, PipelineError> {
    let paths = crate::expand_input_paths(path)?;
    let mut total = IngestStats::default();
    for p in &paths {
        let s = ingest_cycle_v1_file(warehouse, p, pipeline_run_id)?;
        total.merge(s.into());
    }
    debug!(
        "ingest_cycle_v1 done: files={} read={} written={} dup={} rejects={}",
        total.files_processed, total.lines_read, total.lines_written,
        total.lines_ignored_dup, total.rejects
    );
    Ok(total)
}

/// Ingest one or more `trade.v1` JSONL files into the warehouse.
pub fn ingest_trade_v1<W: Warehouse + ?Sized>(
    _warehouse: &mut W,
    _path: &Path,
    _pipeline_run_id: &PipelineRunId,
) -> Result<IngestStats, PipelineError> {
    // Trade ingest is a future addition. The contract doc is still
    // draft; this stub returns an empty IngestStats. The real impl lands
    // when Quant publishes the trade.v1 contract.
    Ok(IngestStats::default())
}

/// Ingest a single `cycle.v1` JSONL file. Internal entry point used by
/// the public `ingest_cycle_v1`.
pub(crate) fn ingest_cycle_v1_file<W: Warehouse + ?Sized>(
    warehouse: &mut W,
    path: &Path,
    pipeline_run_id: &PipelineRunId,
) -> Result<IngestCycleStats, PipelineError> {
    let mut stats = IngestCycleStats::default();
    stats.files_processed = 1;
    let f = std::fs::File::open(path)?;
    use std::io::BufRead;
    for (i, line) in std::io::BufReader::new(f).lines().enumerate() {
        let line = line?;
        let line_no = (i + 1) as u64;
        if line.trim().is_empty() {
            continue;
        }
        stats.lines_read += 1;
        match parse_cycle_v1_line(&line) {
            Ok(row) => {
                stats.lines_parsed += 1;
                match warehouse.insert_cycle(&row) {
                    Ok(true) => stats.lines_written += 1,
                    Ok(false) => stats.lines_ignored_dup += 1,
                    Err(e) => {
                        warn!(
                            "insert_cycle failed for line {} of {}: {}",
                            line_no,
                            path.display(),
                            e
                        );
                        let reject = Reject::new(
                            path,
                            line_no,
                            RejectReason::FieldTypeWrong,
                            &line,
                            pipeline_run_id.clone(),
                        );
                        warehouse.append_reject(&reject)?;
                        stats.rejects += 1;
                    }
                }
            }
            Err(e) => {
                let reject = Reject::new(path, line_no, e.reason, &line, pipeline_run_id.clone());
                warehouse.append_reject(&reject)?;
                stats.rejects += 1;
            }
        }
    }
    Ok(stats)
}

/// Cycle-specific ingest stats. Use [`IngestStats::merge`] to aggregate.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct IngestCycleStats {
    pub files_processed: u64,
    pub lines_read: u64,
    pub lines_parsed: u64,
    pub lines_written: u64,
    pub lines_ignored_dup: u64,
    pub rejects: u64,
}

/// Trade-specific ingest stats.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct IngestTradeStats {
    pub files_processed: u64,
    pub lines_read: u64,
    pub lines_parsed: u64,
    pub lines_written: u64,
    pub lines_ignored_dup: u64,
    pub rejects: u64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::warehouse::{JsonlWarehouse, WarehouseConfig};
    use std::io::Write;

    fn write_jsonl(dir: &Path, name: &str, content: &str) -> std::path::PathBuf {
        let p = dir.join(name);
        let mut f = std::fs::File::create(&p).unwrap();
        f.write_all(content.as_bytes()).unwrap();
        p
    }

    #[test]
    fn parse_happy_cycle() {
        let line = r#"{"schema":"cycle.v1","cycle_id":"0000000000000000000000000000000000000000000000000000000000000000","detected_at_unix_ms":1782000000000,"detected_at_slot":1,"bot_run_id":"550e8400-e29b-41d4-a716-446655440000","dexes":["raydium","orca"],"legs":[{"pool":"58oQChx4yWmvKdwLLZzBi4ChoCc2fqCUWBkwMihLYQo2","dex":"raydium","direction":"BaseToQuote","weight":3000000000000000},{"pool":"Czfq3xZZDmsdGdUyrNLtRhGc47cXcZtLG4crryfu44zE","dex":"orca","direction":"QuoteToBase","weight":-1750000000000000000}],"base_mint":"","quote_mint":"","gross_bps":17470,"fee_bps_sum":60,"decision":"WouldTrade","evaluator":"conservative_default","input_lamports":1000000000,"output_lamports":1174700000,"source_feed":"ws:mainnet"}"#;
        let r = parse_cycle_v1_line(line).unwrap();
        assert_eq!(r.schema, "cycle.v1");
        assert_eq!(r.legs.len(), 2);
    }

    #[test]
    fn parse_rejects_missing_schema() {
        let line = "{}";
        let e = parse_cycle_v1_line(line).unwrap_err();
        assert_eq!(e.reason, RejectReason::SchemaMissing);
    }

    #[test]
    fn parse_rejects_garbage() {
        let line = "not even json";
        let e = parse_cycle_v1_line(line).unwrap_err();
        assert_eq!(e.reason, RejectReason::NotJson);
    }

    #[test]
    fn ingest_writes_and_dedups() {
        let dir = tempfile::tempdir().unwrap();
        let f = write_jsonl(
            dir.path(),
            "a.jsonl",
            r#"{"schema":"cycle.v1","cycle_id":"0000000000000000000000000000000000000000000000000000000000000000","detected_at_unix_ms":1782000000000,"detected_at_slot":1,"bot_run_id":"550e8400-e29b-41d4-a716-446655440000","dexes":["raydium","orca"],"legs":[{"pool":"58oQChx4yWmvKdwLLZzBi4ChoCc2fqCUWBkwMihLYQo2","dex":"raydium","direction":"BaseToQuote","weight":3000000000000000},{"pool":"Czfq3xZZDmsdGdUyrNLtRhGc47cXcZtLG4crryfu44zE","dex":"orca","direction":"QuoteToBase","weight":-1750000000000000000}],"base_mint":"","quote_mint":"","gross_bps":17470,"fee_bps_sum":60,"decision":"WouldTrade","evaluator":"conservative_default","input_lamports":1000000000,"output_lamports":1174700000,"source_feed":"ws:mainnet"}
"#,
        );
        let mut w = JsonlWarehouse::open(WarehouseConfig::new(dir.path())).unwrap();
        let run = PipelineRunId::new();
        let stats = ingest_cycle_v1_file(&mut w, &f, &run).unwrap();
        assert_eq!(stats.lines_read, 1);
        assert_eq!(stats.lines_written, 1);
        assert_eq!(stats.rejects, 0);
        let stats2 = ingest_cycle_v1_file(&mut w, &f, &run).unwrap();
        assert_eq!(stats2.lines_written, 0);
        assert_eq!(stats2.lines_ignored_dup, 1);
    }
}
