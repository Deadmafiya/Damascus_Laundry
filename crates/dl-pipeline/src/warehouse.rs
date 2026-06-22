//! Warehouse abstraction and the production [`JsonlWarehouse`] implementation.

use std::collections::HashSet;
use std::fs::{self, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

use chrono::Utc;
use serde::{Deserialize, Serialize};
use tracing::{debug, warn};

use crate::error::PipelineError;
use crate::reject::{Reject, RejectReason};
use crate::CycleV1;

/// Default warehouse root. The CLI accepts `--root <path>` to override.
pub const DEFAULT_WAREHOUSE_ROOT: &str = "data/warehouse";

/// Configuration for a warehouse instance.
#[derive(Debug, Clone)]
pub struct WarehouseConfig {
    pub root: PathBuf,
}

impl WarehouseConfig {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        WarehouseConfig { root: root.into() }
    }

    pub fn default_under(under: &Path) -> Self {
        WarehouseConfig {
            root: under.join(DEFAULT_WAREHOUSE_ROOT),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Partition {
    pub table: String,
    pub date: String,
}

impl Partition {
    pub fn new(table: impl Into<String>, date: impl Into<String>) -> Self {
        Partition {
            table: table.into(),
            date: date.into(),
        }
    }
}

/// Per-day checksum sidecar.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DailyChecksum {
    pub table: String,
    pub date: String,
    pub row_count: u64,
    /// blake3 hex of the concatenation of every row's UTF-8 bytes, in
    /// order. Newline is **not** included between rows.
    pub row_set_blake3: String,
    pub pipeline_run_id: String,
    pub sealed_at_unix_ms: i64,
}

/// Result of `Warehouse::verify_partition` — the live checksum + the
/// stored sidecar checksum, if any.
pub type VerifyResult = (DailyChecksum, Option<DailyChecksum>);

/// The warehouse trait. Every consumer (ingest, reconcile, verify,
/// compact) talks to this.
pub trait Warehouse {
    /// Insert one cycle row. Returns `true` if the row was new, `false`
    /// if it was a duplicate (already present in this partition).
    fn insert_cycle(&mut self, row: &CycleV1) -> Result<bool, PipelineError>;

    /// Append a reject. Rejects are append-only; the pipeline never
    /// dedupes them.
    fn append_reject(&mut self, reject: &Reject) -> Result<(), PipelineError>;

    /// Read every cycle row in a date partition, sorted by
    /// `detected_at_unix_ms` ascending.
    fn read_cycle_partition(
        &self,
        date: &str,
    ) -> Result<Vec<CycleV1>, PipelineError>;

    /// Read every reject in the warehouse (across all pipeline runs).
    fn read_rejects(&self) -> Result<Vec<Reject>, PipelineError>;

    /// Compute the live checksum of a partition and compare against the
    /// stored one. Returns the live checksum either way; the caller
    /// decides what to do on mismatch.
    fn verify_partition(&self, date: &str) -> Result<VerifyResult, PipelineError>;

    /// Seal a partition: write the checksum sidecar.
    fn seal_partition(
        &mut self,
        date: &str,
        pipeline_run_id: &str,
    ) -> Result<DailyChecksum, PipelineError>;

    /// Compact (archive) partitions older than `older_than_days`.
    fn compact(
        &mut self,
        older_than_days: u64,
        pipeline_run_id: &str,
    ) -> Result<Vec<Partition>, PipelineError>;
}

// ─── JSONL warehouse ────────────────────────────────────────────────────────

/// Production warehouse. Date-partitioned append-only JSONL with a
/// `blake3` checksum sidecar.
pub struct JsonlWarehouse {
    config: WarehouseConfig,
    /// Per-process dedup of cycle_id within a partition. Lost on restart;
    /// the warehouse falls back to a linear scan of the partition on
    /// restart to rehydrate.
    seen_cycle_ids: HashSet<(String, String)>, // (date, cycle_id)
}

impl JsonlWarehouse {
    pub fn open(config: WarehouseConfig) -> Result<Self, PipelineError> {
        fs::create_dir_all(config.root.join("cycle_v1"))?;
        fs::create_dir_all(config.root.join("trade_v1"))?;
        fs::create_dir_all(config.root.join("recon_report_v1"))?;
        fs::create_dir_all(config.root.join("daily_recon_v1"))?;
        fs::create_dir_all(config.root.join("dl_pipeline_rejects"))?;
        Ok(JsonlWarehouse {
            config,
            seen_cycle_ids: HashSet::new(),
        })
    }

    /// Open a warehouse rooted under a temporary directory. Used by the
    /// test-mode CLI flag to ensure no test run mutates the real
    /// warehouse.
    pub fn open_in_temp() -> Result<Self, PipelineError> {
        let dir = tempfile::tempdir()
            .map_err(|e| PipelineError::Warehouse(format!("tempdir: {e}")))?;
        // We intentionally leak the tempdir; it's cleaned up at process
        // exit and the tests are short-lived.
        std::mem::forget(dir);
        Self::open(WarehouseConfig::new(
            std::env::temp_dir().join(format!("dl-pipeline-test-{}", std::process::id())),
        ))
    }

    pub fn config(&self) -> &WarehouseConfig {
        &self.config
    }

    pub fn cycle_partition_path(&self, date: &str) -> PathBuf {
        self.config.root.join("cycle_v1").join(date).join("cycle_v1.jsonl")
    }

    pub fn cycle_checksum_path(&self, date: &str) -> PathBuf {
        self.config
            .root
            .join("cycle_v1")
            .join(date)
            .join("cycle_v1.checksum")
    }

    pub fn trade_partition_path(&self, date: &str) -> PathBuf {
        self.config.root.join("trade_v1").join(date).join("trade_v1.jsonl")
    }

    pub fn daily_recon_path(&self, date: &str) -> PathBuf {
        self.config.root.join("daily_recon_v1").join(format!("{date}.jsonl"))
    }

    pub fn rejects_dir(&self) -> PathBuf {
        self.config.root.join("dl_pipeline_rejects")
    }

    /// Read the stored checksum for a date (if any).
    pub fn read_stored_checksum(&self, date: &str) -> Result<Option<DailyChecksum>, PipelineError> {
        let p = self.cycle_checksum_path(date);
        if !p.exists() {
            return Ok(None);
        }
        let s = fs::read_to_string(&p)?;
        let c: DailyChecksum = serde_json::from_str(&s)?;
        Ok(Some(c))
    }

    /// Compute the live checksum of a partition.
    pub fn compute_live_checksum(
        &self,
        date: &str,
        pipeline_run_id: &str,
    ) -> Result<DailyChecksum, PipelineError> {
        let path = self.cycle_partition_path(date);
        let mut hasher = blake3::Hasher::new();
        let mut count: u64 = 0;
        if path.exists() {
            let f = fs::File::open(&path)?;
            for line in BufReader::new(f).lines() {
                let line = line?;
                if line.is_empty() {
                    continue;
                }
                hasher.update(line.as_bytes());
                count += 1;
            }
        }
        Ok(DailyChecksum {
            table: "cycle_v1".to_string(),
            date: date.to_string(),
            row_count: count,
            row_set_blake3: hasher.finalize().to_hex().to_string(),
            pipeline_run_id: pipeline_run_id.to_string(),
            sealed_at_unix_ms: Utc::now().timestamp_millis(),
        })
    }
}

impl Warehouse for JsonlWarehouse {
    fn insert_cycle(&mut self, row: &CycleV1) -> Result<bool, PipelineError> {
        let date = crate::DatePartition::from_unix_ms(row.detected_at_unix_ms)?;
        let key = (date.0.clone(), row.cycle_id.clone());
        if self.seen_cycle_ids.contains(&key) {
            return Ok(false);
        }
        // Re-hydrate dedup from disk on first write of a session.
        let path = self.cycle_partition_path(&date.0);
        if path.exists() {
            let f = fs::File::open(&path)?;
            for line in BufReader::new(f).lines() {
                let line = line?;
                if line.is_empty() {
                    continue;
                }
                if let Some(id) = extract_cycle_id_from_jsonl(&line) {
                    self.seen_cycle_ids.insert((date.0.clone(), id));
                }
            }
        }
        if self.seen_cycle_ids.contains(&key) {
            return Ok(false);
        }
        // Append.
        fs::create_dir_all(path.parent().unwrap())?;
        let mut f = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)?;
        let mut s = serde_json::to_string(row)?;
        s.push('\n');
        f.write_all(s.as_bytes())?;
        self.seen_cycle_ids.insert(key);
        debug!(
            "ingest cycle_v1 date={} cycle_id={}",
            date.0, row.cycle_id
        );
        Ok(true)
    }

    fn append_reject(&mut self, reject: &Reject) -> Result<(), PipelineError> {
        let path = self
            .rejects_dir()
            .join(format!("{}.jsonl", reject.pipeline_run_id));
        let mut f = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)?;
        let s = reject.to_jsonl()?;
        f.write_all(s.as_bytes())?;
        Ok(())
    }

    fn read_cycle_partition(&self, date: &str) -> Result<Vec<CycleV1>, PipelineError> {
        let path = self.cycle_partition_path(date);
        let mut out = Vec::new();
        if !path.exists() {
            return Ok(out);
        }
        let f = fs::File::open(&path)?;
        for (i, line) in BufReader::new(f).lines().enumerate() {
            let line = line?;
            if line.is_empty() {
                continue;
            }
            match serde_json::from_str::<CycleV1>(&line) {
                Ok(row) => out.push(row),
                Err(e) => {
                    warn!(
                        "read_cycle_partition: skipping un-parseable line {} of {}: {}",
                        i + 1,
                        path.display(),
                        e
                    );
                }
            }
        }
        out.sort_by_key(|r| r.detected_at_unix_ms);
        Ok(out)
    }

    fn read_rejects(&self) -> Result<Vec<Reject>, PipelineError> {
        let mut out = Vec::new();
        let dir = self.rejects_dir();
        if !dir.exists() {
            return Ok(out);
        }
        for entry in walkdir::WalkDir::new(&dir)
            .into_iter()
            .filter_map(|e| e.ok())
        {
            let p = entry.path();
            if !p.is_file() || p.extension().and_then(|s| s.to_str()) != Some("jsonl") {
                continue;
            }
            let f = fs::File::open(p)?;
            for line in BufReader::new(f).lines() {
                let line = line?;
                if line.is_empty() {
                    continue;
                }
                match serde_json::from_str::<Reject>(&line) {
                    Ok(r) => out.push(r),
                    Err(e) => {
                        warn!(
                            "read_rejects: skipping un-parseable line in {}: {}",
                            p.display(),
                            e
                        );
                    }
                }
            }
        }
        Ok(out)
    }

    fn verify_partition(&self, date: &str) -> Result<VerifyResult, PipelineError> {
        let live = self.compute_live_checksum(date, "verify-cli")?;
        let stored = self.read_stored_checksum(date)?;
        Ok((live, stored))
    }

    fn seal_partition(
        &mut self,
        date: &str,
        pipeline_run_id: &str,
    ) -> Result<DailyChecksum, PipelineError> {
        let live = self.compute_live_checksum(date, pipeline_run_id)?;
        let path = self.cycle_checksum_path(date);
        fs::create_dir_all(path.parent().unwrap())?;
        // Atomic write: tmp + rename.
        let tmp = path.with_extension("checksum.tmp");
        let s = serde_json::to_string(&live)?;
        fs::write(&tmp, s)?;
        fs::rename(&tmp, &path)?;
        Ok(live)
    }

    fn compact(
        &mut self,
        older_than_days: u64,
        _pipeline_run_id: &str,
    ) -> Result<Vec<Partition>, PipelineError> {
        let now = Utc::now();
        let mut archived = Vec::new();
        let cycle_dir = self.config.root.join("cycle_v1");
        if !cycle_dir.exists() {
            return Ok(archived);
        }
        for entry in fs::read_dir(&cycle_dir)? {
            let entry = entry?;
            let p = entry.path();
            if !p.is_dir() {
                continue;
            }
            let date = match p.file_name().and_then(|s| s.to_str()) {
                Some(s) => s.to_string(),
                None => continue,
            };
            let nd = match chrono::NaiveDate::parse_from_str(&date, "%Y-%m-%d") {
                Ok(nd) => nd,
                Err(_) => continue,
            };
            let age = now.date_naive().signed_duration_since(nd).num_days();
            if age < older_than_days as i64 {
                continue;
            }
            let archive_path = self
                .config
                .root
                .join("compact")
                .join("archive")
                .join(format!("{}", nd.format("%Y/%m")))
                .join(format!("cycle_v1-{date}.jsonl"));
            if archive_path.exists() {
                debug!("compact: {date} already archived, skipping");
                continue;
            }
            fs::create_dir_all(archive_path.parent().unwrap())?;
            let src_jsonl = p.join("cycle_v1.jsonl");
            if src_jsonl.exists() {
                fs::rename(&src_jsonl, &archive_path)?;
            }
            let cs = p.join("cycle_v1.checksum");
            if cs.exists() {
                let _ = fs::remove_file(&cs);
            }
            let _ = fs::remove_dir(&p);
            archived.push(Partition::new("cycle_v1", date));
        }
        Ok(archived)
    }
}

/// Best-effort extraction of `"cycle_id":"<hex>"` from a single JSONL
/// line.
fn extract_cycle_id_from_jsonl(line: &str) -> Option<String> {
    let needle = "\"cycle_id\":\"";
    let i = line.find(needle)? + needle.len();
    let rest = &line[i..];
    let end = rest.find('"')?;
    Some(rest[..end].to_string())
}

/// Placeholder for the DuckDB-backed warehouse. When the `duckdb` crate
/// becomes available in the offline cache, this becomes the production
/// impl and `JsonlWarehouse` falls back to test mode.
pub struct DuckdbWarehouse {
    #[allow(dead_code)]
    config: WarehouseConfig,
}

impl DuckdbWarehouse {
    pub fn open(_config: WarehouseConfig) -> Result<Self, PipelineError> {
        Err(PipelineError::Warehouse(
            "DuckdbWarehouse is not yet implemented; the `duckdb` crate is not in the offline cargo cache. \
             See crates/dl-pipeline/src/warehouse.rs and the DAM-46 follow-up comment."
                .to_string(),
        ))
    }
}

// The `RejectReason` import silences an unused-warning on a re-export.
#[allow(dead_code)]
fn _reject_reason_marker(r: RejectReason) -> &'static str {
    r.as_str()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::CycleV1;

    fn make_cycle(id: &str, ts: i64) -> CycleV1 {
        CycleV1 {
            schema: "cycle.v1".to_string(),
            cycle_id: id.to_string(),
            detected_at_unix_ms: ts,
            detected_at_slot: 1,
            bot_run_id: "550e8400-e29b-41d4-a716-446655440000".to_string(),
            dexes: vec!["raydium".to_string(), "orca".to_string()],
            legs: vec![],
            base_mint: "".to_string(),
            quote_mint: "".to_string(),
            gross_bps: 100,
            fee_bps_sum: 60,
            decision: "WouldTrade".to_string(),
            evaluator: "conservative_default".to_string(),
            input_lamports: 1_000_000_000,
            output_lamports: 1_000_500_000,
            source_feed: "ws:mainnet".to_string(),
        }
    }

    #[test]
    fn open_creates_layout() {
        let tmp = tempfile::tempdir().unwrap();
        let w = JsonlWarehouse::open(WarehouseConfig::new(tmp.path())).unwrap();
        assert!(tmp.path().join("cycle_v1").exists());
        assert!(tmp.path().join("trade_v1").exists());
        assert!(tmp.path().join("recon_report_v1").exists());
        assert!(tmp.path().join("daily_recon_v1").exists());
        assert!(tmp.path().join("dl_pipeline_rejects").exists());
        let _ = w;
    }

    #[test]
    fn insert_cycle_is_idempotent() {
        let tmp = tempfile::tempdir().unwrap();
        let mut w = JsonlWarehouse::open(WarehouseConfig::new(tmp.path())).unwrap();
        let c = make_cycle(&"a".repeat(64), 1781913600000);
        assert!(w.insert_cycle(&c).unwrap());
        assert!(!w.insert_cycle(&c).unwrap());
        let rows = w.read_cycle_partition("2026-06-20").unwrap();
        assert_eq!(rows.len(), 1);
    }

    #[test]
    fn insert_cycle_dedup_after_reopen() {
        let tmp = tempfile::tempdir().unwrap();
        {
            let mut w = JsonlWarehouse::open(WarehouseConfig::new(tmp.path())).unwrap();
            w.insert_cycle(&make_cycle(&"b".repeat(64), 1781913600000)).unwrap();
        }
        let mut w2 = JsonlWarehouse::open(WarehouseConfig::new(tmp.path())).unwrap();
        let second = w2.insert_cycle(&make_cycle(&"b".repeat(64), 1781913600000)).unwrap();
        assert!(!second);
        let rows = w2.read_cycle_partition("2026-06-20").unwrap();
        assert_eq!(rows.len(), 1);
    }

    #[test]
    fn verify_partition_seal_and_match() {
        let tmp = tempfile::tempdir().unwrap();
        let mut w = JsonlWarehouse::open(WarehouseConfig::new(tmp.path())).unwrap();
        w.insert_cycle(&make_cycle(&"c".repeat(64), 1781913600000)).unwrap();
        w.seal_partition("2026-06-20", "test-run").unwrap();
        let (live, stored) = w.verify_partition("2026-06-20").unwrap();
        assert!(stored.is_some());
        let s = stored.unwrap();
        assert_eq!(s.row_count, 1);
        assert_eq!(s.row_set_blake3, live.row_set_blake3);
    }

    #[test]
    fn compact_archives_old_partitions() {
        let tmp = tempfile::tempdir().unwrap();
        let mut w = JsonlWarehouse::open(WarehouseConfig::new(tmp.path())).unwrap();
        let old_ts = chrono::Utc::now().timestamp_millis() - 100 * 86_400_000;
        w.insert_cycle(&make_cycle(&"d".repeat(64), old_ts)).unwrap();
        w.seal_partition(
            &crate::DatePartition::from_unix_ms(old_ts).unwrap().0,
            "test-run",
        )
        .unwrap();
        let archived = w.compact(90, "test-run").unwrap();
        assert!(!archived.is_empty());
        let archived2 = w.compact(90, "test-run").unwrap();
        assert!(archived2.is_empty());
    }
}
