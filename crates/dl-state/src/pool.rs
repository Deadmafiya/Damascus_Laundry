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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Pubkey(pub [u8; 32]);

impl Pubkey {
    pub const ZERO: Self = Pubkey([0u8; 32]);

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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AmmKind {
    /// Raydium AMM v4 — constant-product. The only kind we decode in v1.0.
    RaydiumAmmV4,
    // v1.1+:
    // OrcaWhirlpool,  // CLMM
    // MeteoraDlmm,   // bin
}

impl AmmKind {
    /// Discriminator byte used in some Solana account layouts.
    /// (Raydium AMM v4 does *not* use a discriminator byte in the account
    /// data; we identify pools by the program that owns them. This method
    /// is reserved for layouts that do.)
    pub fn discriminator(self) -> Option<u8> {
        match self {
            AmmKind::RaydiumAmmV4 => None,
        }
    }
}

/// Normalized pool state. Every field is integer or fixed-point-only.
#[derive(Debug, Clone, PartialEq, Eq)]
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
    pub base_reserve: u64,
    /// Reserve of the quote side, in *base units* of `quote_mint`.
    pub quote_reserve: u64,

    /// Trading fee in basis points (1/10000). Raydium AMM v4's default
    /// is 25 (0.25%).
    pub fee_bps: u16,

    /// Slot at which the reserves were last observed. Useful for
    /// staleness checks in the detector.
    pub last_update_slot: u64,
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
