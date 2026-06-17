//! `dl-feed` — real-time Solana data ingestion.
//!
//! Phase 2 will add a JSON-RPC WebSocket [`dl_core::Feed`] implementation (gRPC-ready),
//! plus raw capture-to-disk for deterministic replay. Placeholder for now.

pub mod capture;
pub mod capturing;
pub mod error;
#[cfg(feature = "ws")]
pub mod ws_feed;

pub use error::FeedError;
