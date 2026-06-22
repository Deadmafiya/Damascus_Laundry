//! Read-only query layer.
//!
//! This is the **only** module in `dl-pipeline` where `f32` / `f64` is
//! allowed. All aggregations in the value path (ingest, validate, store,
//! reconcile) are integer-only; floats live in the query surface that
//! reads back from the warehouse for the operator console and Quant's
//! notebooks.
//!
//! Today this module is a placeholder. The real query surface lands
//! when the `duckdb` crate becomes available in the offline cache.
//! The intent is:
//!
//! - `count_cycles(date_range) -> i64` (integer)
//! - `total_gross_bps(date_range) -> i128` (integer)
//! - `histogram_of_drift(date_range) -> Vec<Bin>` (integer bins)
//! - `daily_sharpe(date_range) -> Option<f64>` (float — sample std / mean)
//!
//! The `dl-recon/tests/floats.rs` style guard will live here when the
//! query surface is implemented: every other module in the crate must
//! remain float-free; only this one is allowed to touch f64.

#![allow(dead_code)]

/// Placeholder: counts the number of rows in a date range. Will be
/// implemented in terms of the `Warehouse` trait when the query surface
/// lands. Integer in, integer out.
pub fn count_cycles_stub() -> u64 {
    0
}
