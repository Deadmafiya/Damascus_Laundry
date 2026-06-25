//! AMM-kind tag — wire-level constant for which AMM produced an event.
//!
//! Re-exported from [`dl_state::AmmKind`] so the `dl-feed` value path
//! (and any crate that only depends on `dl-core` for event types) can
//! name a specific AMM without pulling in the rest of `dl-state`.
//!
//! The numeric values are kept in sync with `dl_state::AmmKind` via
//! the `from_dl_state` / `to_dl_state` conversions in
//! `dl-core::amm_tag`. The CI guard
//! `crates/dl-core/tests/amm_tag_sync.rs` enforces the two stay in
//! lock-step on every test run.

use serde::{Deserialize, Serialize};

/// Orca Whirlpool concentrated-liquidity program. Constant
/// `ORCA_WHIRLPOOL_PROGRAM_ID` lives in `dl_state::decoder::orca_whirlpool`.
pub const ORCA_WHIRLPOOL: AmmTag = AmmTag(2);

/// Meteora DLMM bin-based program. Constant `METEORA_DLMM_PROGRAM_ID`
/// lives in `dl_state::decoder::meteora_dlmm`.
pub const METEORA_DLMM: AmmTag = AmmTag(3);

/// AMM kind tag carried in [`crate::feed::FeedEvent::Pool`].
///
/// Newtype over `u8` so callers can't accidentally pass a `bool` or
/// `AmmKind`-discriminant-positioned integer. The numeric value is
/// the same as `dl_state::AmmKind`'s `repr`-equivalent (kept in sync
/// by `amm_tag_sync.rs`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct AmmTag(pub u8);

impl AmmTag {
    /// Discriminant for the Raydium AMM v4 constant-product path.
    pub const RAYDIUM_AMM_V4: AmmTag = AmmTag(1);

    /// Discriminant for the Orca Whirlpool concentrated-liquidity path.
    pub const ORCA_WHIRLPOOL: AmmTag = AmmTag(2);

    /// Discriminant for the Meteora DLMM bin-based path.
    pub const METEORA_DLMM: AmmTag = AmmTag(3);

    /// Raw `u8` value. Round-trips through `bincode` (used by
    /// `dl-feed::capture`).
    pub fn as_u8(self) -> u8 {
        self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tag_constants_match_dl_state_layout() {
        // 1 = Raydium, 2 = Whirlpool, 3 = DLMM. If dl-state ever
        // re-orders `AmmKind`, this guard fails and the
        // `amm_tag_sync` cross-crate test catches the drift.
        assert_eq!(AmmTag::RAYDIUM_AMM_V4.as_u8(), 1);
        assert_eq!(AmmTag::ORCA_WHIRLPOOL.as_u8(), 2);
        assert_eq!(AmmTag::METEORA_DLMM.as_u8(), 3);
    }

    #[test]
    fn re_exports_match_constants() {
        assert_eq!(ORCA_WHIRLPOOL, AmmTag::ORCA_WHIRLPOOL);
        assert_eq!(METEORA_DLMM, AmmTag::METEORA_DLMM);
    }
}
