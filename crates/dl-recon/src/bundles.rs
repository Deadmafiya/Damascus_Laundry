//! DAM-79 / SLO #3: gate_approved -> outcome join.
//!
//! Implements the silent-revert counter that backs the
//! `DlSilentRevert` Prometheus alert. Joins two JSONL files
//! (gate_events.jsonl, outcomes.jsonl) on `bundle_id` and
//! classifies every gate approval into Landed, FailedCleanly,
//! or SilentRevert.

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Read};

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::error::ReconError;

pub const DEFAULT_LANDING_TIMEOUT_MS: u64 = 30_000;
pub const SILENT_REVERT_TIMEOUT_MULTIPLIER: u64 = 3;

#[derive(Debug, Error)]
pub enum BundlesError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("json parse error on line {line}: {msg}; raw: {raw}")]
    Json { line: u64, msg: String, raw: String },
    #[error("invalid schema on line {line}: expected 'bundle_event.v1', got {raw:?}")]
    Schema { line: u64, raw: String },
    #[error("missing bundle_id on line {line}")]
    MissingBundleId { line: u64 },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GateApprovedRow {
    pub schema: String,
    pub kind: String,
    pub ts_unix_ms: u64,
    pub cycle_id: String,
    pub bundle_id: String,
    #[serde(default)]
    pub slot: u64,
    #[serde(default)]
    pub sim_net_lamports: i64,
    #[serde(default)]
    pub input_mint: String,
    #[serde(default)]
    pub output_mint: String,
    #[serde(default)]
    pub input_amount_lamports: u64,
    #[serde(default)]
    pub tip_lamports: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct OutcomeRow {
    pub schema: String,
    pub kind: String,
    pub ts_unix_ms: u64,
    pub bundle_id: String,
    #[serde(default)]
    pub slot: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OutcomeKind {
    Landed,
    FailedCleanly,
    SilentRevert,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BundleRow {
    pub bundle_id: String,
    pub cycle_id: String,
    pub gate_ts_unix_ms: u64,
    pub outcome_ts_unix_ms: Option<u64>,
    pub outcome: OutcomeKind,
    pub gate_slot: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JoinReport {
    pub landing_timeout_ms: u64,
    pub silent_revert_multiplier: u64,
    pub rows: Vec<BundleRow>,
    pub total_approved: u64,
    pub total_outcomes: u64,
    pub landed_count: u64,
    pub failed_cleanly_count: u64,
    pub silent_revert_count: u64,
    pub report_hash: u64,
}

const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

fn fnv1a_64(bytes: &[u8]) -> u64 {
    let mut h = FNV_OFFSET;
    for byte in bytes {
        h ^= *byte as u64;
        h = h.wrapping_mul(FNV_PRIME);
    }
    h
}

pub fn read_jsonl<T: for<'de> Deserialize<'de>, R: Read>(r: R) -> Result<Vec<T>, BundlesError> {
    let buf = BufReader::new(r);
    let mut out: Vec<T> = Vec::new();
    for (i, line) in buf.lines().enumerate() {
        let line = line?;
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let val: T = serde_json::from_str(trimmed).map_err(|e| BundlesError::Json {
            line: (i + 1) as u64,
            msg: e.to_string(),
            raw: trimmed.to_string(),
        })?;
        out.push(val);
    }
    Ok(out)
}

pub fn join_bundles(
    approved: &[GateApprovedRow],
    outcomes: &[OutcomeRow],
    now_unix_ms: u64,
    landing_timeout_ms: u64,
) -> JoinReport {
    let mut outcome_by_id: BTreeMap<String, &OutcomeRow> = BTreeMap::new();
    for o in outcomes {
        outcome_by_id.insert(o.bundle_id.clone(), o);
    }

    let cutoff_ms = landing_timeout_ms.saturating_mul(SILENT_REVERT_TIMEOUT_MULTIPLIER);

    let mut rows: Vec<BundleRow> = Vec::with_capacity(approved.len());
    let mut landed = 0u64;
    let mut failed = 0u64;
    let mut silent = 0u64;

    for a in approved {
        let outcome = outcome_by_id.get(&a.bundle_id);
        let (kind, outcome_ts) = match outcome {
            Some(o) if o.kind == "outcome_landed" => {
                landed += 1;
                (OutcomeKind::Landed, Some(o.ts_unix_ms))
            }
            Some(_) => {
                failed += 1;
                (OutcomeKind::FailedCleanly, outcome.map(|o| o.ts_unix_ms))
            }
            None => {
                let age_ms = now_unix_ms.saturating_sub(a.ts_unix_ms);
                if age_ms > cutoff_ms {
                    silent += 1;
                    (OutcomeKind::SilentRevert, None)
                } else {
                    (OutcomeKind::SilentRevert, None)
                }
            }
        };
        rows.push(BundleRow {
            bundle_id: a.bundle_id.clone(),
            cycle_id: a.cycle_id.clone(),
            gate_ts_unix_ms: a.ts_unix_ms,
            outcome_ts_unix_ms: outcome_ts,
            outcome: kind,
            gate_slot: a.slot,
        });
    }
    rows.sort_by(|a, b| a.bundle_id.cmp(&b.bundle_id));

    let report_hash = {
        let mut h = FNV_OFFSET;
        for row in &rows {
            let bytes = bincode::serialize(row).expect("BundleRow bincode");
            for byte in bytes {
                h ^= byte as u64;
                h = h.wrapping_mul(FNV_PRIME);
            }
        }
        h
    };

    JoinReport {
        landing_timeout_ms,
        silent_revert_multiplier: SILENT_REVERT_TIMEOUT_MULTIPLIER,
        rows,
        total_approved: approved.len() as u64,
        total_outcomes: outcomes.len() as u64,
        landed_count: landed,
        failed_cleanly_count: failed,
        silent_revert_count: silent,
        report_hash,
    }
}

impl JoinReport {
    pub fn to_pretty_json(&self) -> Result<String, BundlesError> {
        serde_json::to_string_pretty(self).map_err(|e| BundlesError::Json {
            line: 0,
            msg: e.to_string(),
            raw: String::new(),
        })
    }
}

impl From<BundlesError> for ReconError {
    fn from(e: BundlesError) -> Self {
        ReconError::Json(e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn approved(bundle: &str, ts: u64) -> GateApprovedRow {
        GateApprovedRow {
            schema: "bundle_event.v1".to_string(),
            kind: "gate_approved".to_string(),
            ts_unix_ms: ts,
            cycle_id: "0".repeat(64),
            bundle_id: bundle.to_string(),
            slot: 0,
            sim_net_lamports: 0,
            input_mint: String::new(),
            output_mint: String::new(),
            input_amount_lamports: 0,
            tip_lamports: 0,
        }
    }

    fn outcome(bundle: &str, kind: &str, ts: u64) -> OutcomeRow {
        OutcomeRow {
            schema: "bundle_event.v1".to_string(),
            kind: kind.to_string(),
            ts_unix_ms: ts,
            bundle_id: bundle.to_string(),
            slot: 0,
        }
    }

    #[test]
    fn empty_inputs_yield_empty_report() {
        let r = join_bundles(&[], &[], 1_000_000, DEFAULT_LANDING_TIMEOUT_MS);
        assert_eq!(r.total_approved, 0);
        assert_eq!(r.silent_revert_count, 0);
        assert!(r.rows.is_empty());
        assert_eq!(r.report_hash, FNV_OFFSET);
    }

    #[test]
    fn approved_with_landed_outcome_counts_as_landed() {
        let a = vec![approved("bid-1", 1_000)];
        let o = vec![outcome("bid-1", "outcome_landed", 1_500)];
        let r = join_bundles(&a, &o, 100_000, DEFAULT_LANDING_TIMEOUT_MS);
        assert_eq!(r.landed_count, 1);
        assert_eq!(r.rows[0].outcome, OutcomeKind::Landed);
    }

    #[test]
    fn approved_with_failed_cleanly_outcome_counts_as_failed() {
        let a = vec![approved("bid-1", 1_000)];
        let o = vec![outcome("bid-1", "outcome_failed_cleanly", 1_500)];
        let r = join_bundles(&a, &o, 100_000, DEFAULT_LANDING_TIMEOUT_MS);
        assert_eq!(r.failed_cleanly_count, 1);
        assert_eq!(r.rows[0].outcome, OutcomeKind::FailedCleanly);
    }

    #[test]
    fn approved_no_outcome_after_cutoff_is_silent_revert() {
        // cutoff = 30_000 * 3 = 90_000; age = 200_000 > 90_000.
        let a = vec![approved("bid-1", 1_000_000)];
        let r = join_bundles(&a, &[], 1_200_000, DEFAULT_LANDING_TIMEOUT_MS);
        assert_eq!(r.silent_revert_count, 1);
        assert_eq!(r.rows[0].outcome, OutcomeKind::SilentRevert);
    }

    #[test]
    fn approved_no_outcome_within_cutoff_is_not_silent() {
        let a = vec![approved("bid-1", 1_000_000)];
        let r = join_bundles(&a, &[], 1_010_000, DEFAULT_LANDING_TIMEOUT_MS);
        assert_eq!(r.silent_revert_count, 0);
    }

    #[test]
    fn mixed_bundles_aggregate_correctly() {
        // bid-d is older than the cutoff (1_200_000 - 1_000_000 = 200_000
        // > 90_000 cutoff) -> silent revert. bid-c is recent
        // (1_200_000 - 1_195_000 = 5_000 < cutoff) -> pending, not
        // counted in any bucket.
        let a = vec![
            approved("bid-a", 1_000),
            approved("bid-b", 2_000),
            approved("bid-c", 1_195_000),
            approved("bid-d", 1_000_000),
        ];
        let o = vec![
            outcome("bid-a", "outcome_landed", 1_500),
            outcome("bid-b", "outcome_failed_cleanly", 2_500),
        ];
        let r = join_bundles(&a, &o, 1_200_000, DEFAULT_LANDING_TIMEOUT_MS);
        assert_eq!(r.landed_count, 1);
        assert_eq!(r.failed_cleanly_count, 1);
        assert_eq!(r.silent_revert_count, 1);
        assert_eq!(r.total_approved, 4);
    }

    #[test]
    fn rows_are_sorted_by_bundle_id() {
        let a = vec![
            approved("bid-zzz", 1_000),
            approved("bid-aaa", 2_000),
            approved("bid-mmm", 3_000),
        ];
        let r = join_bundles(&a, &[], 100_000, DEFAULT_LANDING_TIMEOUT_MS);
        let order: Vec<&str> = r.rows.iter().map(|r| r.bundle_id.as_str()).collect();
        assert_eq!(order, vec!["bid-aaa", "bid-mmm", "bid-zzz"]);
    }

    #[test]
    fn identical_inputs_produce_identical_hash() {
        let a = vec![approved("bid-1", 1_000), approved("bid-2", 2_000)];
        let o = vec![outcome("bid-1", "outcome_landed", 1_500)];
        let r1 = join_bundles(&a, &o, 100_000, DEFAULT_LANDING_TIMEOUT_MS);
        let r2 = join_bundles(&a, &o, 100_000, DEFAULT_LANDING_TIMEOUT_MS);
        assert_eq!(r1, r2);
        assert_eq!(r1.report_hash, r2.report_hash);
    }

    #[test]
    fn read_jsonl_skips_blank_lines_and_parses_rows() {
        let raw = "{\"schema\":\"bundle_event.v1\",\"kind\":\"gate_approved\",\"ts_unix_ms\":1,\"cycle_id\":\"x\",\"bundle_id\":\"bid-1\"}\n\n{\"schema\":\"bundle_event.v1\",\"kind\":\"gate_approved\",\"ts_unix_ms\":2,\"cycle_id\":\"x\",\"bundle_id\":\"bid-2\"}\n";
        let rows: Vec<GateApprovedRow> = read_jsonl(raw.as_bytes()).expect("parse");
        assert_eq!(rows.len(), 2);
    }

    #[test]
    fn read_jsonl_reports_parse_error_with_line_number() {
        let raw = "{\"schema\":\"bundle_event.v1\",\"kind\":\"gate_approved\",\"ts_unix_ms\":1,\"cycle_id\":\"x\",\"bundle_id\":\"bid-1\"}\nNOT JSON\n";
        let err = read_jsonl::<GateApprovedRow, _>(raw.as_bytes()).expect_err("should fail");
        match err {
            BundlesError::Json { line, .. } => assert_eq!(line, 2),
            other => panic!("wrong error: {other:?}"),
        }
    }

    #[test]
    fn report_to_pretty_json_round_trips() {
        let a = vec![approved("bid-1", 1_000)];
        let o = vec![outcome("bid-1", "outcome_landed", 1_500)];
        let r = join_bundles(&a, &o, 100_000, DEFAULT_LANDING_TIMEOUT_MS);
        let s = r.to_pretty_json().expect("serialize");
        let parsed: JoinReport = serde_json::from_str(&s).expect("parse back");
        assert_eq!(parsed, r);
    }
}
