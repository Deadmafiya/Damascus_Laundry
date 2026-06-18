//! `dl-app` library — exposes the recon subcommand and shared
//! helpers for `dl-app` binaries and integration tests.
//!
//! See `crates/dl-app/src/recon.rs` for the recon subcommand.

use tracing_subscriber::EnvFilter;

pub mod config;
pub mod dry_run;
pub mod metrics;
pub mod recon;

/// Initialize structured logging once. Idempotent — calls after the
/// first are no-ops.
pub fn init_tracing() {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(true)
        .try_init();
}
