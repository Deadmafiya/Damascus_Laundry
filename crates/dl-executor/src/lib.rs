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
pub mod metrics;
pub mod tip;

pub use bundle::{Bundle, BundleBuilder, SwapLeg, TipLeg};
pub use error::ExecutorError;
pub use jito::{JitoClient, JitoHealth, JitoSubmitResult, LandingResult, MockJitoClient};
pub use jupiter::{JupiterClient, JupiterQuote, JupiterRouteStep, MockJupiterClient, QuoteRequest};
pub use metrics::{LiveMetrics, LiveMetricsSnapshot};
pub use tip::{tip_lamports, TipConfig};
