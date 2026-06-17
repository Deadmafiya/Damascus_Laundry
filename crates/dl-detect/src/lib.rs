//! `dl-detect` — atomic-arbitrage opportunity detection.
//!
//! Phase 3 will build a price graph (tokens = nodes, pools = edges weighted by
//! `-log(effective rate)`) and run Bellman-Ford negative-cycle detection. Placeholder.
