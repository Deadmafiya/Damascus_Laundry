//! Reconciliation harness (Phase 6, plan 06-01).
//!
//! Replays a previously written ledger under alternative `EvalParams`
//! and surfaces divergences between the original decision and the
//! re-derived decision. Pure offline pipeline: reads bytes, writes bytes,
//! returns structured reports.
//!
//! ## Module map
//!
//! - [`error`]: failure surface for the harness.
//! - [`pipeline`]: top-level driver (filled in Task 3).
//! - [`fault`]: fault-injection middlewares (filled in Task 4).
//! - [`invariants`]: numbered property assertions for CI (Task 5).
//! - [`fixture`]: synthetic capture/ledger builders (Task 6).
//!
//! ## Invariants
//!
//! The recon crate is **integer-only** in every value path: no
//! `f32` or `f64` ever appears in production code. Tests in
//! `tests/floats.rs` enforce this at compile time on every run.

#![deny(unsafe_code)]

pub mod error;
pub mod fault;
pub mod fixture;
pub mod invariants;
pub mod onchain;
pub mod onchain_sweep;
pub mod pipeline;
pub mod reconcile;

pub use error::ReconError;
pub use onchain::{AnchorDataset, AnchorDivergence, AnchorEntry, AnchorName, OnchainError};
