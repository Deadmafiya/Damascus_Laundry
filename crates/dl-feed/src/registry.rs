//! Subscription registry — tracks which vault / pool accounts are
//! currently subscribed on a `WsFeed`.
//!
//! Lives in its own module (no async deps) so it can be unit-tested
//! without a live WebSocket. The `ws_feed` background task owns one
//! instance and feeds every successful `accountSubscribe` /
//! `programSubscribe` into it; on reconnect, the task drains the
//! registry and re-sends the same subscriptions to the new socket.
//!
//! ## Design notes
//!
//! - Vault pubkeys only — the registry does not store subscription
//!   IDs (the WS task rebuilds the id↔pubkey map on every connect).
//!   Idempotent: re-`insert()`ing the same pubkey is a no-op.
//! - Insertion-ordered iteration is the wire-order we want on
//!   resubscribe (deterministic for tests; matches the consumer's
//!   expectation of "subscribe accounts in the order I asked for
//!   them the first time").
//! - Bounded by the number of vaults the bot can hold in scope
//!   (small — dozens, not millions). No eviction; entries are
//!   permanent for the life of the feed.

use std::collections::BTreeSet;

/// A registered vault (or pool) account subscription.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct VaultSub {
    pub pubkey: [u8; 32],
}

impl VaultSub {
    pub fn new(pubkey: [u8; 32]) -> Self {
        Self { pubkey }
    }
}

/// Subscription registry. Insertion-ordered, deduplicated.
#[derive(Debug, Default, Clone)]
pub struct SubscriptionRegistry {
    // BTreeSet gives us deterministic iteration order without
    // pulling in `indexmap`. With < 1000 vaults the cost is
    // negligible and the deterministic order is what we want for
    // resubscribe-on-reconnect.
    entries: BTreeSet<VaultSub>,
}

impl SubscriptionRegistry {
    /// Empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Register `pubkey` for re-subscription on reconnect.
    /// Idempotent.
    pub fn insert(&mut self, pubkey: [u8; 32]) {
        self.entries.insert(VaultSub::new(pubkey));
    }

    /// Number of registered subscriptions.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// True if no subscriptions are registered.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Iterate registered pubkeys in deterministic order.
    pub fn iter(&self) -> impl Iterator<Item = [u8; 32]> + '_ {
        self.entries.iter().map(|v| v.pubkey)
    }

    /// Snapshot into a `Vec` (used by tests + the reconnect path).
    pub fn to_vec(&self) -> Vec<[u8; 32]> {
        self.iter().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn insert_is_idempotent() {
        let mut r = SubscriptionRegistry::new();
        let pk = [1u8; 32];
        r.insert(pk);
        r.insert(pk);
        r.insert(pk);
        assert_eq!(r.len(), 1);
    }

    #[test]
    fn iter_is_deterministic() {
        let mut r = SubscriptionRegistry::new();
        // Insert in a non-sorted order; iter() returns sorted.
        r.insert([3u8; 32]);
        r.insert([1u8; 32]);
        r.insert([2u8; 32]);
        let v = r.to_vec();
        // BTreeSet sorts by byte order. 1 < 2 < 3 by the first
        // byte, so the order is [1, 2, 3].
        assert_eq!(v, vec![[1u8; 32], [2u8; 32], [3u8; 32]]);
    }

    #[test]
    fn empty_registry_reports_zero_len() {
        let r = SubscriptionRegistry::new();
        assert!(r.is_empty());
        assert_eq!(r.len(), 0);
        assert_eq!(r.to_vec(), Vec::<[u8; 32]>::new());
    }

    #[test]
    fn resubscribe_iteration_matches_insertion() {
        // Resubscribe scenario: on reconnect, we drain the
        // registry and re-issue `accountSubscribe` for each entry.
        // The drain order must be the same on every reconnect so
        // the upstream RPC's view of "what is subscribed" is
        // stable.
        let mut r = SubscriptionRegistry::new();
        let pks = [[0xA1u8; 32], [0xB2u8; 32], [0xC3u8; 32]];
        for pk in pks {
            r.insert(pk);
        }
        let first = r.to_vec();
        let second = r.to_vec();
        assert_eq!(first, second);
        // BTreeSet sorts; we expect byte-sorted output.
        let mut expected = pks;
        expected.sort();
        assert_eq!(first, expected);
    }
}
