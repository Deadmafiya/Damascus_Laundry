//! `dl-stream` — v1.1+ streaming detector (Phase 8 / plan 02).
//!
//! The 08-02 sub-tag v1.1.0-streaming ships:
//!
//! 1. [`StreamingGraph`] — incremental update of the price graph. As
//!    `AccountUpdate` events arrive, only the edges incident to the
//!    updated pool's tokens are re-computed.
//! 2. [`StreamingDetector`] — wraps a `StreamingGraph` and emits
//!    detected cycles via a callback.
//! 3. [`LatencyHistogram`] — atomic-bucket latency tracker (p50, p95,
//!    p99, mean, count). Integer-only; no float math.
//! 4. [`run`] — high-level entry point. Drives the full pipeline:
//!    feed → stream → detect → executor → submit.

#![deny(unsafe_code)]

pub mod detector;
pub mod latency;
pub mod pipeline;

pub use detector::{StreamingDetector, StreamingGraph};
pub use latency::{LatencyHistogram, LatencySnapshot};
pub use pipeline::{run, PipelineExit, RunConfig};
