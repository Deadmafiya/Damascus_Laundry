//! Injectable market-data feed.
//!
//! Production will stream account/transaction updates from Solana (Phase 2: JSON-RPC
//! WebSocket, gRPC-ready). For tests and replay, [`ScriptedFeed`] yields a fixed list of
//! events deterministically, so the same script always drives the same engine behavior.

use crate::clock::Slot;

/// AMM family tag carried by `FeedEvent::Pool`. The integer value
/// is stable across the wire (bincode) and across versions; do
/// not renumber. New AMMs append; do not reorder.
pub mod amm_tag {
    /// Raydium AMM v4 — constant-product.
    pub const RAYDIUM_AMM_V4: u8 = 0;
    /// Orca Whirlpool — concentrated-liquidity.
    pub const ORCA_WHIRLPOOL: u8 = 1;
    /// Meteora DLMM — bin-based liquidity.
    pub const METEORA_DLMM: u8 = 2;
}

/// AMM-kind-specific extras for `FeedEvent::Pool`. Integer-only
/// (Q64.64 sqrt_price for Whirlpool; per-bin reserves + per-bin
/// `SCALE_OFFSET`-scaled price for DLMM). The fill math consumes
/// these directly without any `f32` / `f64` step. The
/// `dl-feed` value-path no-floats invariant still holds: every
/// field is `u64` or `u128`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum PoolExtrasWire {
    /// Constant-product (Raydium AMM v4). No extras; the fill
    /// math reads `base_reserve` / `quote_reserve` directly.
    Raydium,
    /// Concentrated liquidity (Orca Whirlpool). `sqrt_price` is
    /// Q64.64 — the tick-anchored price. `base_reserve` /
    /// `quote_reserve` are NOT used by the fill math; they
    /// hold the latest known SPL-token vault amounts for
    /// display only.
    Whirlpool {
        /// Tick-anchored price in Q64.64 fixed point.
        sqrt_price: u128,
    },
    /// Bin-based (Meteora DLMM). Carries the active bin's
    /// per-bin reserves and per-bin price. The full 65-bin
    /// window around `active_id` is preserved so the fill math
    /// can walk the bin array on replay without any
    /// collapse-to-single-price step. The active bin is at
    /// `bin_price[0]`, index `i` is bin `active_id + i` (as i32).
    Dlmm {
        /// Per-bin price step in basis points (e.g. 100 = 1%).
        bin_step: u16,
        /// Active bin ID; index 0 of the bin arrays is this bin.
        active_id: i32,
        /// Per-bin base reserves (`u64` raw base units). Length
        /// must equal `bin_price.len()`.
        bin_amount_x: Vec<u64>,
        /// Per-bin quote reserves (`u64` raw quote units).
        /// Length must equal `bin_amount_x.len()`.
        bin_amount_y: Vec<u64>,
        /// Per-bin price scaled by the AMM's `SCALE_OFFSET`
        /// (DLMM: 1e12). `u128` to cover the full dynamic
        /// range without losing precision. Length must equal
        /// `bin_amount_x.len()`.
        bin_price: Vec<u128>,
    },
}

impl Default for PoolExtrasWire {
    fn default() -> Self {
        PoolExtrasWire::Raydium
    }
}

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
    /// A decoded pool update. Emitted when a subscription was
    /// for a pool account whose layout we recognise at the feed
    /// layer (currently Meteora DLMM `LbPair` via
    /// `dl_feed::dlmm` and Orca Whirlpool via
    /// `dl_feed::whirlpool`). The detector and sim consume
    /// this directly without re-decoding the raw bytes. The
    /// capture file stays additive-only: a v1 file that
    /// contains only `Slot` and `AccountUpdate` frames
    /// decodes unchanged.
    Pool {
        /// Slot at which the pool update was observed.
        slot: Slot,
        /// AMM family discriminator. See `amm_tag` constants.
        amm: u8,
        /// Pool account address (the LbPair / Whirlpool / AmmInfo pubkey).
        pool: [u8; 32],
        /// Base token mint (token X for DLMM).
        base_mint: [u8; 32],
        /// Quote token mint (token Y for DLMM).
        quote_mint: [u8; 32],
        /// Trading fee in basis points. DLMM: `LbPair.bin_step`.
        fee_bps: u16,
        /// Latest known SPL-token base vault amount (`u64` raw
        /// base units). 0 if no vault updates have been seen
        /// yet.
        base_reserve: u64,
        /// Latest known SPL-token quote vault amount (`u64`
        /// raw quote units).
        quote_reserve: u64,
        /// AMM-kind-specific extras. See `PoolExtrasWire`.
        extras: PoolExtrasWire,
        /// Slot at which the pool's underlying account was last
        /// updated. Same as `slot` for in-order subscriptions.
        last_update_slot: Slot,
    },
    /// The feed has halted because a subscribed pool's
    /// last-seen slot is older than the staleness threshold
    /// (DAM-31.D / DAM-36). `last_seen_slot` is the slot at
    /// which the silent pool was last seen;
    /// `staleness_slots = now_slot - last_seen_slot`. The feed
    /// emits no further events after this one. Trading must
    /// stop on receipt.
    StalePoolHalt {
        /// Slot at which the silent pool was last seen.
        last_seen_slot: Slot,
        /// Pubkey of the pool (vault or pool account) that went stale.
        pubkey: [u8; 32],
        /// Number of slots since the last update for the silent pool.
        staleness_slots: u64,
    },
}

impl FeedEvent {
    /// The slot at which our vantage point observed this event.
    /// For [`FeedEvent::StalePoolHalt`] this returns
    /// `last_seen_slot` (the most recent confirmed slot before
    /// the trip).
    pub fn slot(&self) -> Slot {
        match self {
            FeedEvent::Slot { slot } => *slot,
            FeedEvent::AccountUpdate { slot, .. } => *slot,
            FeedEvent::Pool { slot, .. } => *slot,
            FeedEvent::StalePoolHalt { last_seen_slot, .. } => *last_seen_slot,
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

    #[test]
    fn stale_pool_halt_carries_trip_details() {
        let halt = FeedEvent::StalePoolHalt {
            last_seen_slot: 100,
            pubkey: [0xABu8; 32],
            staleness_slots: 75,
        };
        match halt {
            FeedEvent::StalePoolHalt {
                last_seen_slot,
                pubkey,
                staleness_slots,
            } => {
                assert_eq!(last_seen_slot, 100);
                assert_eq!(pubkey, [0xABu8; 32]);
                assert_eq!(staleness_slots, 75);
                // `slot()` returns last_seen_slot.
                assert_eq!(
                    FeedEvent::StalePoolHalt {
                        last_seen_slot,
                        pubkey,
                        staleness_slots,
                    }
                    .slot(),
                    100
                );
            }
            other => panic!("expected StalePoolHalt, got {other:?}"),
        }
    }
}
