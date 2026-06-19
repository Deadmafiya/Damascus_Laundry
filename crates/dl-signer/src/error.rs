//! Signer error types.

use thiserror::Error;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum SignerError {
    /// The keyfile does not exist or cannot be read.
    #[error("keyfile I/O error: {0}")]
    Io(String),

    /// The keyfile is malformed (bad magic, bad ciphertext, bad version).
    #[error("keyfile format error: {0}")]
    BadFormat(String),

    /// The passphrase is wrong (decryption failed). After 3 strikes, the
    /// process exits; the operator must restart.
    #[error("wrong passphrase")]
    WrongPassphrase,

    /// The daily SOL cap would be breached by this signature.
    #[error("daily cap exceeded: would charge {attempted} lamports, only {remaining} remaining")]
    DailyCapExceeded { attempted: u64, remaining: u64 },

    /// The per-bundle SOL cap would be breached by this signature.
    #[error("per-bundle cap exceeded: {attempted} > {limit} lamports")]
    PerBundleCapExceeded { attempted: u64, limit: u64 },

    /// The rate limit (bundles/minute) would be breached.
    #[error("rate limit exceeded: {bundles_per_minute} bundles/min")]
    RateLimitExceeded { bundles_per_minute: u32 },

    /// The transaction was empty or otherwise invalid.
    #[error("invalid transaction: {0}")]
    InvalidTransaction(String),

    /// Arithmetic overflow in the cap math (defensive; unreachable in
    /// practice since lamport values are < u64::MAX).
    #[error("arithmetic overflow: {0}")]
    Overflow(String),
}
