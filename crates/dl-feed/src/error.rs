//! Error types for [`dl-feed`].
//!
//! The `Feed` trait is infallible by design (it cannot return a `Result` from
//! `next_event` — that would change the object-safety contract from Phase 1).
//! Errors are surfaced through construction, open, and configuration paths
//! instead. `WsFeed` errors are exposed at the async setup methods
//! (`connect`, `subscribe_*`); capture/tee errors are exposed via the typed
//! return values of `CaptureWriter::new`, `CapturedFeed::open`, and the
//! `into_inner`/`bytes` paths.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum FeedError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),

    #[error("bincode: {0}")]
    Bincode(#[from] bincode::Error),

    #[error("capture schema version mismatch: file={file}, build={build}")]
    SchemaMismatch { file: u32, build: u32 },

    #[error("capture truncated at frame {frame}")]
    Truncated { frame: u64 },

    #[error("capture magic header missing or wrong")]
    BadMagic,

    #[error("ws: {0}")]
    Ws(String),

    #[error("rpc returned error: code={code}, message={message}")]
    Rpc { code: i64, message: String },
}
