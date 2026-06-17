//! `dl-app` — binary entry point wiring the damascus_laundry pipeline together.
//!
//! v1.0 is paper-trading only: no keys, no signing, no network submission.

use tracing::info;
use tracing_subscriber::EnvFilter;

fn init_tracing() {
    // Structured logging, configurable via `RUST_LOG` (default `info`).
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(true)
        .init();
}

fn main() {
    init_tracing();
    info!(
        version = env!("CARGO_PKG_VERSION"),
        mode = "paper-trading",
        strategy = "atomic-dex-dex-arbitrage",
        "damascus_laundry starting (no keys, no live submission)"
    );
    // Pipeline crates (feed -> state -> detect -> sim -> ledger) are wired in later phases.
    info!("foundations ready; pipeline stages are placeholders until Phase 2+");
}
