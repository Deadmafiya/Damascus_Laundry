//! Optional metrics hook for the `ws_feed` background task.
//!
//! `dl-feed` is intentionally not a hard dependency of `dl-app`'s
//! metrics stack. The hook is a small trait the WS task can call
//! to surface counters and the `dl-app` Prometheus adapter can
//! implement to forward them into the registry.
//!
//! If no hook is supplied, the WS task still tracks counters
//! internally (via [`FeedStats`]) and the caller can read them
//! through the `WsFeed::stats()` accessor. The hook is purely a
//! push-side optimization so the counters show up in the same
//! `/metrics` body without the operator having to plumb them
//! through their own snapshot path.

use std::sync::Arc;

/// A single integer counter increment. The hook implementation is
/// expected to be cheap (an atomic add + maybe an event emission).
pub trait FeedMetrics: Send + Sync + std::fmt::Debug {
    /// Record one reconnect. `attempt` is 0-indexed; the first
    /// retry after a disconnect is attempt=0.
    fn reconnect(&self, _attempt: u64) {}
    /// Record a reconnect storm: more than `threshold` attempts
    /// in `window` slots.
    fn reconnect_storm(&self) {}
    /// Record one staleness-guard trip.
    fn stale_pool(&self) {}
    /// Record that the feed entered the halted state (post-stale
    /// or post-exhausted reconnect).
    fn halted(&self) {}
}

/// No-op hook. Default for `WsFeed::connect_with_policy`. Counters
/// still flow through `WsFeed::stats()`.
#[derive(Debug, Default, Clone, Copy)]
pub struct NoopMetrics;

impl FeedMetrics for NoopMetrics {}

/// Optional hook on the WS feed. Wrapped in `Arc` so it can be
/// cheaply cloned into the background task.
pub type SharedMetrics = Arc<dyn FeedMetrics>;
