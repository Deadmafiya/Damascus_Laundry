//! `dl-state` — in-memory pool/account state, decimals-normalized.
//!
//! Phase 2 adds AMM decoders (Raydium AMM v4 constant-product in v1.0;
//! CLMM and bin-lp in v1.1+) that turn raw account bytes into the
//! normalized [`Pool`] struct, and a [`PoolRegistry`] that the detector
//! queries.
//!
//! Floats are forbidden in this crate's value paths. The display layer
//! (Phase 1) is the only place floats may appear. The `no_floats_in_values`
//! CI guard in 02-02-06 enforces this.

pub mod decoder;
pub mod error;
pub mod mint;
pub mod pool;
pub mod registry;

pub use error::DecodeError;
pub use mint::{ClosureMintSource, HardcodedMintSource, MintDecimalsSource};
pub use pool::{AmmKind, Pool, Pubkey};
pub use registry::PoolRegistry;
