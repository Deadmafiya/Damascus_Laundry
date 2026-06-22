//! `dl-executor` — v1.1+ live trade executor (Phase 8 / plan 01).
//!
//! 08-01 ships the **paper-mode** executor: builds bundles, calculates
//! tip, signs via `dl-signer`, and submits to a **mock** Jito client.
//! The real `jito-bundle` + `solana-sdk` + `reqwest` integration lands
//! in 08-02/08-03 when we add the live HTTP / RPC stack.
//!
//! The design separates concerns:
//!
//! - `bundle.rs` — bundle structure (1 tip + up to 4 swap legs)
//! - `tip.rs` — tip-lamports calculation
//! - `jupiter.rs` — Jupiter Aggregator v6 client (trait + mock impl)
//! - `jito.rs` — Jito Block Engine client (trait + mock impl)
//! - `error.rs` — error types
//! - `metrics.rs` — live-mode counters
//!
//! The high-level entry point is `submit_opportunity`, which the
//! `dl-app run` subcommand will call per detected cycle.

#![deny(unsafe_code)]

pub mod bundle;
pub mod error;
pub mod jito;
pub mod jupiter;
pub mod killswitch;
pub mod landing;
pub mod metrics;
pub mod signer_integration;
pub mod simulate;
pub mod tip;

pub use bundle::{build_bundle_from_signed, Bundle, BundleBuilder, SwapLeg, TipLeg};
pub use error::ExecutorError;
pub use jito::{JitoClient, JitoHealth, JitoSubmitResult, LandingResult, MockJitoClient};
pub use jupiter::{JupiterClient, JupiterQuote, JupiterRouteStep, MockJupiterClient, QuoteRequest};
pub use killswitch::{stop_file_age_secs, BundleOutcome, KillSwitch, KillSwitchConfig};
pub use landing::{poll_bundle_landing, poll_with_mock, LandingPollConfig};
pub use metrics::{LiveMetrics, LiveMetricsSnapshot, LandingLatencySnapshot, LANDING_LATENCY_CAPACITY};
pub use signer_integration::{
    keystore_to_keypair, sign_transactions, sign_with_keystore,
};
pub use simulate::{
    classify, report_from_parts, simulate_and_classify, simulate_bundle, truncate_logs,
    SimulateVerdict, SimulationReport, MAX_LOG_CHARS,
};
pub use tip::{tip_lamports, TipConfig};
