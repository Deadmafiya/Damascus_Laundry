//! Normalized AMM pool state. No floats anywhere — reserves are raw
//! base units (lamports / smallest unit of each mint). Decimals-aware
//! display is the *display* layer's job (Phase 1 boundary).
//!
//! The `Pool` struct is the canonical "what does this market look like
//! right now?" representation that the rest of the engine reads. It is
//! produced by the decoders in `crate::decoder` and stored in
//! `PoolRegistry`.

use dl_core::MathError;

/// 32-byte Solana public key. Newtype (not a type alias) so we can give
/// it a `Debug`/`Hash`/`Eq` impl and use it as a `HashMap` key.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct Pubkey(pub [u8; 32]);

impl Pubkey {
    pub const ZERO: Self = Pubkey([0u8; 32]);

    /// Base58 string of the inner 32 bytes. Cheap; calls `bs58::encode`.
    pub fn to_base58_string(&self) -> String {
        bs58::encode(self.0).into_string()
    }

    pub fn from_slice_32(slice: &[u8]) -> Option<Self> {
        if slice.len() == 32 {
            let mut out = [0u8; 32];
            out.copy_from_slice(slice);
            Some(Pubkey(out))
        } else {
            None
        }
    }
}

impl AsRef<[u8]> for Pubkey {
    fn as_ref(&self) -> &[u8] {
        &self.0
    }
}

impl From<[u8; 32]> for Pubkey {
    fn from(bytes: [u8; 32]) -> Self {
        Pubkey(bytes)
    }
}

/// Which AMM produced this pool's on-chain layout. Drives decoder dispatch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum AmmKind {
    /// Raydium AMM v4 — constant-product. The only kind we decode in v1.0.
    RaydiumAmmV4,
    /// Orca Whirlpool — concentrated-liquidity (Q64.64 sqrt_price,
    /// tick-indexed). Decoded in Phase 7 / plan 02.
    OrcaWhirlpool,
    /// Meteora DLMM — bin-based (per-bin reserves + per-bin price,
    /// `SCALE_OFFSET` = 1e12). Decoded in Phase 7 / plan 02.
    MeteoraDlmm,
    // v1.1+:
    // Phoenix, // orderbook
    // OpenBook, // v2 orderbook
}

impl Default for AmmKind {
    /// Default to Raydium (constant-product). The most common case
    /// for v1.0; lets test helpers use `..Default::default()` without
    /// naming the kind explicitly.
    fn default() -> Self {
        AmmKind::RaydiumAmmV4
    }
}

impl AmmKind {
    /// Discriminator byte used in some Solana account layouts.
    /// (Raydium AMM v4 does *not* use a discriminator byte in the account
    /// data; we identify pools by the program that owns them. This method
    /// is reserved for layouts that do.)
    pub fn discriminator(self) -> Option<u8> {
        match self {
            AmmKind::RaydiumAmmV4 => None,
            AmmKind::OrcaWhirlpool => None,
            AmmKind::MeteoraDlmm => None,
        }
    }
}

/// AMM-kind-specific extras carried by a `Pool`.
///
/// `Pool.base_reserve` / `quote_reserve` are the *normalized* reserves for
/// the constant-product path (Raydium AMM v4). The other AMM kinds need
/// additional state to drive their fill math:
///
/// - **Raydium**: none (constant-product).
/// - **Orca Whirlpool**: `sqrt_price` Q64.64 — the tick-anchored price.
///   Reserves are derived per-tick from the active liquidity; for v1.0
///   we approximate via `fill_orca_single_tick` using only `sqrt_price`.
/// - **Meteora DLMM**: the active bin's `(amount_x, amount_y, price)`.
///   `price` is the per-bin price scaled by `SCALE_OFFSET = 1e12`. For
///   v1.0 we approximate multi-bin walks via the active bin only.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum PoolExtras {
    /// Constant-product (Raydium AMM v4). `base_reserve` / `quote_reserve`
    /// are the AMM's reserves. The sim pipeline uses
    /// `fill_constant_product` directly on these.
    Raydium,
    /// Concentrated liquidity (Orca Whirlpool). `sqrt_price` is Q64.64.
    /// `base_reserve` / `quote_reserve` are NOT used by the fill math;
    /// they hold the latest known SPL-token vault amounts for display
    /// only.
    Whirlpool { sqrt_price: u128 },
    /// Bin-based (Meteora DLMM). Carries the active bin's data. The
    /// active bin is the bin at `LbPair.active_id`. `bin_step` is the
    /// per-bin price step in basis points.
    Dlmm {
        bin_step: u16,
        active_amount_x: u64,
        active_amount_y: u64,
        active_price_scaled: u128,
    },
}

impl Default for PoolExtras {
    /// Default to constant-product. Tests that don't care about the
    /// AMM kind can write `..Default::default()` for `extras`.
    fn default() -> Self {
        PoolExtras::Raydium
    }
}

/// Normalized pool state. Every field is integer or fixed-point-only.
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct Pool {
    /// The pool account's own address.
    pub address: Pubkey,

    /// Which AMM family this pool belongs to (drives decoder).
    pub kind: AmmKind,

    pub base_mint: Pubkey,
    pub quote_mint: Pubkey,
    pub base_decimals: u8,
    pub quote_decimals: u8,

    /// Reserve of the base side, in *base units* (the smallest unit of
    /// `base_mint`). For SOL/USDC pools: lamports for SOL, micro-USDC
    /// for USDC.
    ///
    /// For `AmmKind::OrcaWhirlpool` and `AmmKind::MeteoraDlmm`, the fill
    /// math uses `PoolExtras` instead. These fields hold the latest
    /// vault-account amounts for display only.
    pub base_reserve: u64,
    /// Reserve of the quote side, in *base units* of `quote_mint`.
    pub quote_reserve: u64,

    /// Trading fee in basis points (1/10000). Raydium AMM v4's default
    /// is 25 (0.25%). For Whirlpool this is `fee_rate`. For DLMM this
    /// is `bin_step` (the per-bin price step in bps; the per-swap fee
    /// is also derived from `bin_step` in the SDK but we treat the
    /// per-bin step as the fee proxy for v1.0).
    pub fee_bps: u16,

    /// Slot at which the reserves were last observed. Useful for
    /// staleness checks in the detector.
    pub last_update_slot: u64,

    /// AMM-kind-specific extras. Default `PoolExtras::Raydium` when
    /// not populated. Use [`PoolExtras::Whirlpool`] and
    /// [`PoolExtras::Dlmm`] for the other kinds.
    #[serde(default)]
    pub extras: PoolExtras,
}

impl Pool {
    /// Mid-price as `quote per base`, scaled by 1e9. Integer-only via
    /// `mul_div_floor`. Returns `DivByZero` if `base_reserve == 0`.
    ///
    /// Why 1e9? The dynamic range of crypto prices needs at least 9
    /// significant digits to keep the truncation error below 1 bp at
    /// realistic price levels. Display layer scales this back to a
    /// human-readable form.
    pub fn mid_price_scaled_1e9(&self) -> Result<u128, MathError> {
        if self.base_reserve == 0 {
            return Err(MathError::DivByZero);
        }
        dl_core::fixed::mul_div_floor(
            self.quote_reserve as u128,
            1_000_000_000u128,
            self.base_reserve as u128,
        )
    }

    /// Effective constant-product invariant `k = base * quote`. Cheap
    /// sanity check for a well-formed pool: k should never decrease
    /// (fees excluded).
    pub fn invariant(&self) -> u128 {
        (self.base_reserve as u128).saturating_mul(self.quote_reserve as u128)
    }
}

impl Default for Pool {
    /// Default to a zero Raydium pool. Used by test fixtures that
    /// build synthetic pools without caring about the AMM kind.
    fn default() -> Self {
        Self {
            address: Pubkey::ZERO,
            kind: AmmKind::default(),
            base_mint: Pubkey::ZERO,
            quote_mint: Pubkey::ZERO,
            base_decimals: 0,
            quote_decimals: 0,
            base_reserve: 0,
            quote_reserve: 0,
            fee_bps: 0,
            last_update_slot: 0,
            extras: PoolExtras::default(),
        }
    }
}
