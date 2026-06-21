//! Error types for the reconciliation harness.
//!
//! The recon crate never silently truncates, never auto-migrates, and never
//! swallows a decoder error — every failure path is named.

use dl_ledger::LedgerError;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ReconError {
    /// The source ledger could not be opened (bad magic, schema mismatch,
    /// truncated, I/O). Wraps the underlying [`LedgerError`].
    #[error("ledger open failed: {0}")]
    Ledger(#[from] LedgerError),

    /// The capture file could not be opened or its header is invalid.
    #[error("capture open failed: {0}")]
    Capture(String),

    /// An `AccountUpdate` payload had a length that matched neither
    /// `AMM_INFO_SIZE` (752) nor `SPL_TOKEN_ACCOUNT_SIZE` (165). The
    /// harness refuses to guess — every unknown blob is an error.
    #[error("unknown account update size {0}; expected 752 (AmmInfo) or 165 (SPL token account)")]
    UnknownAccountSize(usize),

    /// Decoder error from `dl-state` while assembling pools from a capture.
    #[error("pool decode failed: {0}")]
    Decode(#[from] dl_state::DecodeError),

    /// Detector error while walking the captured pool graph.
    #[error("detector failed: {0}")]
    Detect(#[from] dl_detect::DetectError),

    /// Sim error while sizing or evaluating a cycle.
    #[error("simulation failed: {0}")]
    Sim(#[from] dl_sim::error::SimError),

    /// Generic math overflow surfaced from integer paths.
    #[error("math overflow: {0}")]
    Math(String),

    /// Hash recorded in a `ReconReport` body did not match the recomputed
    /// hash. Per invariant I-6, this is a hard failure — never quietly
    /// overwritten.
    #[error("recon report hash mismatch: recorded {recorded}, computed {computed}")]
    HashMismatch { recorded: u64, computed: u64 },

    /// Bincode (de)serialization failure.
    #[error("bincode: {0}")]
    Bincode(#[from] bincode::Error),

    /// JSON (de)serialization failure (used by `dl-recon::reconcile`
    /// for the DAM-64 reconciliation report and the
    /// `emit-reconciliation` binary).
    #[error("json: {0}")]
    Json(String),

    /// I/O error from a caller-provided reader/writer.
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
}

impl From<dl_core::MathError> for ReconError {
    fn from(e: dl_core::MathError) -> Self {
        ReconError::Math(e.to_string())
    }
}
