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

    #[error("simulateTransaction failed: {0}")]
    SimulateFailed(String),

    #[error("simulateTransaction reported negative net PnL: {reported_pnl} lamports")]
    SimulateNegativeNet { reported_pnl: i64 },

    #[error("kill switch tripped: {0}")]
    KillSwitchTripped(String),

    #[error("landing poll error: {0}")]
    LandingPoll(String),
}
