//! `dl-signer` — v1.1+ hot-wallet key custodian (Phase 8 / plan 01).
//!
//! This crate owns the **private key** for live trading. It is the only
//! crate in the value path that touches key material. The flow:
//!
//! ```text
//! dl-app run --feed ws
//!   → on detected opportunity → dl-executor::submit_opportunity()
//!     → dl_signer::sign(keystore, cap_state, ratelimit, tx)
//!       → reads keyfile (AES-256-GCM + Argon2id)
//!       → checks daily cap + per-bundle cap + rate limit
//!       → if all checks pass: signs the transaction, returns (signed_tx, cap_state_after)
//!       → if any check fails: returns SignerError
//! ```
//!
//! ## Security model (hot-wallet, daily cap)
//!
//! - **Daily cap** (default 5 SOL/day, configurable via `DL_DAILY_CAP_LAMPORTS`)
//!   limits the worst-case loss to one day's cap.
//! - **Per-bundle cap** (default 0.5 SOL, `DL_PER_BUNDLE_CAP_LAMPORTS`)
//!   prevents a single bundle from draining the daily cap.
//! - **Rate limit** (default 10 bundles/minute, `DL_BUNDLES_PER_MINUTE`)
//!   prevents rapid-fire signing in a runaway condition.
//! - **Keyfile encryption**: `aes-256-gcm` with a unique nonce; the
//!   key is derived from the operator's passphrase via `argon2id`.
//! - **Memory hygiene**: the `KeyStore` zeroizes the keypair on `Drop`.
//!
//! ## Float-free invariant
//!
//! The only `f64` in this crate is in `ratelimit.rs` (token-bucket math).
//! The float-free CI guard in `dl-signer/tests/no_floats.rs` allows this
//! one exception; all other modules are pure integer.

#![deny(unsafe_code)]

pub mod cap;
pub mod error;
pub mod keystore;
pub mod ratelimit;

pub use cap::{CapConfig, CapError, CapState};
pub use error::SignerError;
pub use keystore::{KeyFile, KeyStore};
pub use ratelimit::{RateLimit, RateLimitConfig};
