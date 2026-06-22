//! Error types for the data pipeline.
//!
//! Every failure path is named; the pipeline never silently truncates or
//! swallows a parse error. Callers get a structured error and a `tracing`
//! log line.

use thiserror::Error;

use crate::validate::ValidationError;

/// Top-level error type. Carries the source error so callers can downcast.
#[derive(Debug, Error)]
pub enum PipelineError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),

    #[error("json: {0}")]
    Json(#[from] serde_json::Error),

    #[error("validation: {0}")]
    Validation(#[from] ValidationError),

    #[error("warehouse: {0}")]
    Warehouse(String),

    #[error("date parse: {0}")]
    Date(String),

    #[error("checksum mismatch on {date}: recorded {recorded}, computed {computed}")]
    ChecksumMismatch {
        date: String,
        recorded: String,
        computed: String,
    },

    #[error("path not found: {0}")]
    NotFound(String),

    #[error("walk: {0}")]
    Walk(String),

    #[error("roundtrip count mismatch: input={input}, written={written}, ignored={ignored}")]
    Roundtrip {
        input: u64,
        written: u64,
        ignored: u64,
    },

    #[error("policy: {0}")]
    Policy(String),
}
