//! Staleness policy for pool edges held in [`crate::registry::PoolRegistry`].
//!
//! The DAM-89 acceptance criteria (AC-2) require a `MAX_POOL_STALENESS_SLOTS`
//! env-var-driven drop check inside `dl-state`. The dl-feed layer owns the
//! async, slot-driven guard that halts the live stream on a trip; this
//! module owns the *passive* policy the detector / paper-trader uses when
//! it scans the registry: a pool whose `last_update_slot` is older than
//! `max_staleness_slots` is treated as "dropped from the graph" and the
//! consumer should not build a path through it.
//!
//! ## Why two layers
//!
//! The dl-feed guard is *active*: it emits a `FeedEvent::StalePoolHalt` and
//! halts the stream. The dl-state policy is *passive*: a query that reads the
//! registry simply filters out stale entries. A bot running on a hot
//! (non-halting) stale feed still refuses to trade on pools whose last update
//! is older than the policy threshold.
//!
//! Both layers read the same env var (`MAX_POOL_STALENESS_SLOTS`, default 50
//! slots ~ 20 s) so the two views of "stale" stay in lock-step.

/// Default staleness threshold in slots, used when the env var is unset
/// or unparseable. 50 slots ~ 20 s at the 400 ms/slot Solana cadence.
///
/// Matches the v3 spec's "stale after 20 s" guidance.
pub const DEFAULT_MAX_POOL_STALENESS_SLOTS: u64 = 50;

/// Env-var name. Operators can override the staleness threshold without
/// recompiling. The ws task reads this at startup; the registry policy
/// reads it on every prune call (cheap — one `getenv`).
pub const MAX_POOL_STALENESS_SLOTS_ENV: &str = "MAX_POOL_STALENESS_SLOTS";

/// Read the configured `MAX_POOL_STALENESS_SLOTS`. Returns the
/// default if the env var is unset, unparseable, or zero. A `0` is
/// treated as "disabled" (the consumer's `prune_stale` call
/// short-circuits when the threshold is `0`).
pub fn max_pool_staleness_slots_from_env() -> u64 {
    match std::env::var(MAX_POOL_STALENESS_SLOTS_ENV) {
        Ok(s) => s
            .parse::<u64>()
            .ok()
            .filter(|&n| n > 0)
            .unwrap_or(DEFAULT_MAX_POOL_STALENESS_SLOTS),
        Err(_) => DEFAULT_MAX_POOL_STALENESS_SLOTS,
    }
}

/// Pure staleness check. Returns `true` when `(now_slot -
/// last_seen_slot) > max_staleness_slots` (strictly greater — a
/// pool whose `last_seen_slot == now_slot - threshold` is still
/// fresh). When `max_staleness_slots == 0` the predicate returns
/// `false` (the threshold is "disabled").
///
/// `last_seen_slot == 0` is treated as "never seen" — the pool is
/// considered stale so a freshly-registered pool that has not yet
/// produced an update is dropped from queries. This matches the
/// no-look-ahead replay guarantee: a graph edge with no observed
/// state is a "ghost" edge and must not be priced.
pub fn is_pool_stale(last_seen_slot: u64, now_slot: u64, max_staleness_slots: u64) -> bool {
    if max_staleness_slots == 0 {
        return false;
    }
    if last_seen_slot == 0 {
        return true;
    }
    now_slot.saturating_sub(last_seen_slot) > max_staleness_slots
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_threshold_matches_spec() {
        // The default is 50 slots ~ 20 s. Locked in at 50 — if
        // the spec ever wants a different default, change the
        // constant deliberately.
        assert_eq!(DEFAULT_MAX_POOL_STALENESS_SLOTS, 50);
    }

    #[test]
    fn env_var_default_is_50_when_unset() {
        // The test sets / unsets MAX_POOL_STALENESS_SLOTS in a
        // single test body. cargo test runs tests in parallel,
        // but env-var reads are not racy with other tests that
        // don't touch the same var.
        unsafe { std::env::remove_var(MAX_POOL_STALENESS_SLOTS_ENV) };
        assert_eq!(max_pool_staleness_slots_from_env(), 50);
    }

    #[test]
    fn env_var_overrides_default() {
        unsafe { std::env::set_var(MAX_POOL_STALENESS_SLOTS_ENV, "100") };
        let got = max_pool_staleness_slots_from_env();
        unsafe { std::env::remove_var(MAX_POOL_STALENESS_SLOTS_ENV) };
        assert_eq!(got, 100);
    }

    #[test]
    fn env_var_zero_treated_as_default() {
        // The contract is "0 disables the guard". We don't
        // return 0; we return the default. That way the
        // registry / guard never silently no-op on a typo'd
        // `MAX_POOL_STALENESS_SLOTS=0`.
        unsafe { std::env::set_var(MAX_POOL_STALENESS_SLOTS_ENV, "0") };
        let got = max_pool_staleness_slots_from_env();
        unsafe { std::env::remove_var(MAX_POOL_STALENESS_SLOTS_ENV) };
        assert_eq!(got, DEFAULT_MAX_POOL_STALENESS_SLOTS);
    }

    #[test]
    fn env_var_unparseable_treated_as_default() {
        unsafe { std::env::set_var(MAX_POOL_STALENESS_SLOTS_ENV, "fifty") };
        let got = max_pool_staleness_slots_from_env();
        unsafe { std::env::remove_var(MAX_POOL_STALENESS_SLOTS_ENV) };
        assert_eq!(got, DEFAULT_MAX_POOL_STALENESS_SLOTS);
    }

    #[test]
    fn is_pool_stale_fresh_is_false() {
        // age = 50, threshold = 50 -> fresh.
        assert!(!is_pool_stale(100, 150, 50));
        // age = 49, threshold = 50 -> fresh.
        assert!(!is_pool_stale(101, 150, 50));
    }

    #[test]
    fn is_pool_stale_age_above_threshold_is_true() {
        // age = 51, threshold = 50 -> stale.
        assert!(is_pool_stale(99, 150, 50));
    }

    #[test]
    fn is_pool_stale_disabled_threshold_never_stale() {
        // threshold = 0 -> never stale (guard is off).
        assert!(!is_pool_stale(0, u64::MAX, 0));
        assert!(!is_pool_stale(1, u64::MAX, 0));
    }

    #[test]
    fn is_pool_stale_never_seen_is_stale() {
        // last_seen_slot = 0 -> stale (graph ghost).
        assert!(is_pool_stale(0, 1000, 50));
    }

    #[test]
    fn is_pool_stale_uses_saturating_sub() {
        // Defensive: if last_seen_slot > now_slot (clock skew,
        // out-of-order notifications), the predicate returns
        // `false` (not stale) because saturating_sub clamps to 0.
        assert!(!is_pool_stale(200, 100, 50));
    }
}
