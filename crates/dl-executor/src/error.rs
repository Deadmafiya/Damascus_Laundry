//! Executor error types.

use thiserror::Error;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ExecutorError {
    #[error("Jupiter quote error: {0}")]
    JupiterQuote(String),

    #[error("Jupiter returned a swap-transaction that we couldn't deserialize: {0}")]
    JupiterDeserialize(String),

    #[error("Jito submit error: {0}")]
    JitoSubmit(String),

    #[error("Bundle assembly error: {0}")]
    BundleAssembly(String),

    #[error("Tip config error: {0}")]
    TipConfig(String),

    #[error("Signer error: {0}")]
    Signer(String),
}
