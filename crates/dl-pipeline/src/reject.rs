//! Reject writer and the canonical `RejectReason` enum.
//!
//! A `Reject` is the structured audit row the pipeline writes for every line
//! it could not ingest. See `docs/contracts/cycle.v1.md` §"Reject reasons"
//! for the canonical list. The list is closed: a new reason is a contract
//! change and must be added to the contract doc in the same commit.

use std::path::Path;

use chrono::Utc;
use serde::{Deserialize, Serialize};

use crate::PipelineRunId;

/// Closed enum of reject reasons. Adding a variant is a contract change.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum RejectReason {
    /// No `schema` field at all on the line.
    SchemaMissing,
    /// `schema` value is not `cycle.v1` (or `trade.v1` for the trade path).
    SchemaUnknown,
    /// `cycle_id` is not 64 lowercase hex chars.
    CycleIdInvalid,
    /// `legs` array is empty.
    LegsEmpty,
    /// `decision` is not `WouldTrade` / `WouldNotTrade`.
    DecisionInvalid,
    /// `evaluator` is not a known `EvalParams` variant.
    EvaluatorInvalid,
    /// `source_feed` is not in the enum.
    SourceFeedInvalid,
    /// `direction` on a leg is not `BaseToQuote` / `QuoteToBase`.
    DirectionInvalid,
    /// `dex` on a leg is not `raydium` / `orca` / `meteora`.
    DexInvalid,
    /// A required field failed its type check (integer / string / array).
    FieldTypeWrong,
    /// A line contained a field name starting with `_priv_`.
    PrivateField,
    /// A line matched the `signing_material` regex.
    SigningMaterial,
    /// The line was not valid JSON.
    NotJson,
}

impl RejectReason {
    /// Stable short string used in the `dl_pipeline_rejects` table and in
    /// the `verify --date` checksum. Must match the contract doc.
    pub fn as_str(&self) -> &'static str {
        match self {
            RejectReason::SchemaMissing => "schema_missing",
            RejectReason::SchemaUnknown => "schema_unknown",
            RejectReason::CycleIdInvalid => "cycle_id_invalid",
            RejectReason::LegsEmpty => "legs_empty",
            RejectReason::DecisionInvalid => "decision_invalid",
            RejectReason::EvaluatorInvalid => "evaluator_invalid",
            RejectReason::SourceFeedInvalid => "source_feed_invalid",
            RejectReason::DirectionInvalid => "direction_invalid",
            RejectReason::DexInvalid => "dex_invalid",
            RejectReason::FieldTypeWrong => "field_type_wrong",
            RejectReason::PrivateField => "private_field",
            RejectReason::SigningMaterial => "signing_material",
            RejectReason::NotJson => "not_json",
        }
    }
}

impl std::fmt::Display for RejectReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A single reject row, written to `dl_pipeline_rejects`. The `raw_line`
/// is the original bytes (UTF-8 lossy-decoded) so the operator can see
/// exactly what was rejected.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Reject {
    /// Pipeline run that produced this reject. Lineage.
    pub pipeline_run_id: PipelineRunId,
    /// Source file the line came from.
    pub source_path: String,
    /// 1-based line number within the source file.
    pub line_number: u64,
    pub reason: RejectReason,
    /// The raw line as it appeared in the source.
    pub raw_line: String,
    /// UTC ms at the moment the reject was recorded.
    pub rejected_at_unix_ms: i64,
}

impl Reject {
    pub fn new(
        source_path: &Path,
        line_number: u64,
        reason: RejectReason,
        raw_line: &str,
        pipeline_run_id: PipelineRunId,
    ) -> Self {
        Reject {
            pipeline_run_id,
            source_path: source_path.display().to_string(),
            line_number,
            reason,
            raw_line: raw_line.to_string(),
            rejected_at_unix_ms: Utc::now().timestamp_millis(),
        }
    }

    /// Serialize for append to `dl_pipeline_rejects` (one JSON object per line).
    pub fn to_jsonl(&self) -> Result<String, serde_json::Error> {
        let mut s = serde_json::to_string(self)?;
        s.push('\n');
        Ok(s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reject_reason_strs_match_contract_doc() {
        // The strings must match docs/contracts/cycle.v1.md verbatim.
        assert_eq!(RejectReason::SchemaMissing.as_str(), "schema_missing");
        assert_eq!(RejectReason::SchemaUnknown.as_str(), "schema_unknown");
        assert_eq!(RejectReason::CycleIdInvalid.as_str(), "cycle_id_invalid");
        assert_eq!(RejectReason::LegsEmpty.as_str(), "legs_empty");
        assert_eq!(RejectReason::DecisionInvalid.as_str(), "decision_invalid");
        assert_eq!(RejectReason::EvaluatorInvalid.as_str(), "evaluator_invalid");
    }

    #[test]
    fn reject_serializes_to_single_jsonl_line() {
        let run = PipelineRunId::new();
        let r = Reject::new(
            std::path::Path::new("/tmp/foo.jsonl"),
            7,
            RejectReason::LegsEmpty,
            "{\"legs\":[]}",
            run,
        );
        let s = r.to_jsonl().unwrap();
        assert!(s.ends_with('\n'));
        // The full buffer includes a single trailing newline. The
        // serialized JSON body itself (before the trailing newline) must
        // contain no newline characters.
        let body = s.trim_end_matches('\n');
        assert!(!body.contains('\n'));
        let parsed: Reject = serde_json::from_str(body).unwrap();
        assert_eq!(parsed.line_number, 7);
        assert_eq!(parsed.reason, RejectReason::LegsEmpty);
    }
}
