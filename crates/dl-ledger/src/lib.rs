//! `dl-ledger` — paper portfolio, PnL attribution, metrics.
//!
//! v1.0 paper-trading: append-only length-prefixed bincode file
//! (`LEDGER_MAGIC` + `LEDGER_SCHEMA_VERSION` + frames of `LedgerEntry`).
//! See `format` for the byte-level spec; see `tests/ledger_roundtrip.rs`
//! for the lock-step test that prevents the spec from drifting.
//!
//! ## Int-only invariant
//!
//! Per AC, every value stored in the ledger is integer-only. No `f32` /
//! `f64` types, no `as f64` casts, no `f64::...` calls anywhere in
//! `dl-ledger`. Decimals are confined to `dl-core::display`. The CI
//! guard `tests/int_only_no_fractional.rs` (Task 4) enforces this.
//!
//! ## Phase-6 consumers
//!
//! - **Calibration**: re-read the ledger with different `EvalParams`
//!   (`p_win` / `p_land` / failed-cost defaults) and recompute
//!   conservative EVs to find a better default set.
//! - **Reconciliation**: compare ledger outcomes to on-chain observation
//!   (the net-profit estimate vs the realized fill).
//! - **PnL math**: walk the ledger to compute realized + unrealized
//!   PnL and per-leg attribution.
//!
//! Public surface:
//! - [`LedgerEntry`], [`Decision`], [`LedgerHash`]
//! - [`LedgerWriter`], [`LedgerReader`]
//! - [`LedgerSummary`], [`LedgerError`]
//! - [`LEDGER_MAGIC`], [`LEDGER_SCHEMA_VERSION`], [`format_spec`]

#![deny(unsafe_code)]
#![warn(missing_docs)]

pub mod entry;
pub mod error;
pub mod format;
pub mod hash;
pub mod reader;
pub mod summary;
pub mod writer;

pub use entry::{Decision, LedgerEntry};
pub use error::LedgerError;
pub use format::{format_spec, LEDGER_MAGIC, LEDGER_SCHEMA_VERSION};
pub use hash::LedgerHash;
pub use reader::LedgerReader;
pub use summary::LedgerSummary;
pub use writer::LedgerWriter;
