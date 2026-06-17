//! Mint-decimals source. The `AmmInfo` itself carries the decimals
//! (offsets 32 and 40) so we don't strictly need to fetch the mint
//! account — but a real pipeline may want to sanity-check, or look up
//! a different mint's decimals for cross-pool routes.
//!
//! For v1.0 the `RpcMintSource` is a stub: it takes a closure that
//! receives a mint pubkey and returns `Result<u8, DecodeError>`. This
//! keeps the crate free of an RPC dep while leaving the door open to
//! an async implementation later.

use crate::error::DecodeError;
use crate::pool::Pubkey;

/// Anything that can look up the decimals of a mint pubkey.
pub trait MintDecimalsSource {
    fn fetch(&self, mint: &Pubkey) -> Result<u8, DecodeError>;
}

/// Lookup backed by a precomputed table. Useful in tests and as a
/// "trust me, these are the mints" fast path in production.
#[derive(Debug, Default, Clone)]
pub struct HardcodedMintSource {
    table: std::collections::HashMap<[u8; 32], u8>,
}

impl HardcodedMintSource {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with(mut self, mint: Pubkey, decimals: u8) -> Self {
        self.table.insert(mint.0, decimals);
        self
    }
}

impl MintDecimalsSource for HardcodedMintSource {
    fn fetch(&self, mint: &Pubkey) -> Result<u8, DecodeError> {
        self.table
            .get(&mint.0)
            .copied()
            .ok_or_else(|| DecodeError::BadDiscriminator {
                expected: b"mint in table".to_vec(),
                got: mint.0.to_vec(),
            })
    }
}

/// Closure-backed source. `f` may be a blocking HTTP call, a cached
/// RPC handle, or anything else.
pub struct ClosureMintSource<F: Fn(&Pubkey) -> Result<u8, DecodeError>> {
    f: F,
}

impl<F: Fn(&Pubkey) -> Result<u8, DecodeError>> ClosureMintSource<F> {
    pub fn new(f: F) -> Self {
        Self { f }
    }
}

impl<F: Fn(&Pubkey) -> Result<u8, DecodeError>> MintDecimalsSource for ClosureMintSource<F> {
    fn fetch(&self, mint: &Pubkey) -> Result<u8, DecodeError> {
        (self.f)(mint)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hardcoded_source_lookup() {
        let mint = Pubkey([7u8; 32]);
        let src = HardcodedMintSource::new().with(mint, 6);
        assert_eq!(src.fetch(&mint).unwrap(), 6);
    }

    #[test]
    fn hardcoded_source_miss_is_err() {
        let src = HardcodedMintSource::new();
        assert!(src.fetch(&Pubkey([0u8; 32])).is_err());
    }

    #[test]
    fn closure_source_delegates() {
        let src = ClosureMintSource::new(|mint| {
            if mint.0 == [1u8; 32] {
                Ok(9)
            } else {
                Err(DecodeError::TooShort { need: 1, got: 0 })
            }
        });
        assert_eq!(src.fetch(&Pubkey([1u8; 32])).unwrap(), 9);
        assert!(src.fetch(&Pubkey([0u8; 32])).is_err());
    }
}
