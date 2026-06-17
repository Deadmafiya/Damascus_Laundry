//! Injectable market-data feed.
//!
//! Production will stream account/transaction updates from Solana (Phase 2: JSON-RPC
//! WebSocket, gRPC-ready). For tests and replay, [`ScriptedFeed`] yields a fixed list of
//! events deterministically, so the same script always drives the same engine behavior.

use crate::clock::Slot;

/// A single unit of market data observed from our vantage point.
///
/// Intentionally minimal for Phase 1; Phase 2 extends this with decoded pool/transaction
/// variants. The `slot` on each event is "when our stream saw it" — the basis for the
/// no-look-ahead replay guarantee.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum FeedEvent {
    /// A slot boundary was observed.
    Slot { slot: Slot },
    /// An account's data changed at `slot`. `data` is the raw account bytes (decoded later).
    AccountUpdate {
        slot: Slot,
        pubkey: [u8; 32],
        data: Vec<u8>,
    },
}

impl FeedEvent {
    /// The slot at which our vantage point observed this event.
    pub fn slot(&self) -> Slot {
        match self {
            FeedEvent::Slot { slot } => *slot,
            FeedEvent::AccountUpdate { slot, .. } => *slot,
        }
    }
}

/// A source of [`FeedEvent`]s. Pull-based iterator semantics: `next_event` returns `None`
/// when the stream is exhausted (replay) or, for a live feed, blocks/awaits elsewhere.
/// Object-safe so the app can hold `&mut dyn Feed`.
pub trait Feed {
    /// Next event, or `None` when exhausted.
    fn next_event(&mut self) -> Option<FeedEvent>;
}

/// Deterministic feed that replays a fixed, in-memory list of events in order. Two
/// `ScriptedFeed`s built from the same `Vec<FeedEvent>` yield identical sequences.
#[derive(Debug, Clone)]
pub struct ScriptedFeed {
    events: Vec<FeedEvent>,
    cursor: usize,
}

impl ScriptedFeed {
    /// Build from a script of events.
    pub fn new(events: Vec<FeedEvent>) -> Self {
        Self { events, cursor: 0 }
    }

    /// An empty feed (the "null" feed: always `None`).
    pub fn empty() -> Self {
        Self::new(Vec::new())
    }

    /// Number of events remaining.
    pub fn remaining(&self) -> usize {
        self.events.len().saturating_sub(self.cursor)
    }
}

impl Feed for ScriptedFeed {
    fn next_event(&mut self) -> Option<FeedEvent> {
        let ev = self.events.get(self.cursor).cloned();
        if ev.is_some() {
            self.cursor += 1;
        }
        ev
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scripted_feed_replays_in_order() {
        let script = vec![
            FeedEvent::Slot { slot: 1 },
            FeedEvent::AccountUpdate {
                slot: 1,
                pubkey: [7u8; 32],
                data: vec![1, 2, 3],
            },
            FeedEvent::Slot { slot: 2 },
        ];
        let mut a = ScriptedFeed::new(script.clone());
        let mut b = ScriptedFeed::new(script);
        for _ in 0..3 {
            assert_eq!(a.next_event(), b.next_event());
        }
        assert_eq!(a.next_event(), None);
        assert_eq!(b.next_event(), None);
    }

    #[test]
    fn empty_feed_yields_none() {
        let mut f = ScriptedFeed::empty();
        assert_eq!(f.next_event(), None);
        assert_eq!(f.remaining(), 0);
    }
}
