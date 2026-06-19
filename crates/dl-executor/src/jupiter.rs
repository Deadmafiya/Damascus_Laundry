//! Jupiter Aggregator v6 client (08-01: paper mode).
//!
//! 08-01 ships the **mock** implementation: a `JupiterClient` trait
//! with a `MockJupiterClient` that returns deterministic quotes for
//! test inputs. The real `reqwest`-backed implementation lands in
//! 08-02 when we have the devnet and mainnet-paper gates set up.
//!
//! ## Real implementation (08-02 stub)
//!
//! The real Jupiter Aggregator v6 API:
//! - `POST https://quote-api.jup.ag/v6/quote` with
//!   `{inputMint, outputMint, amount, slippageBps}`
//! - Returns a `JupiterQuote` with the route plan and a base64-
//!   encoded `swapTransaction` (a Solana VersionedTransaction).
//! - We deserialize the base64 into the actual transaction bytes
//!   and pass to the bundle builder.

use crate::error::ExecutorError;

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct QuoteRequest {
    /// Input mint (base58, 32 bytes).
    pub input_mint: String,
    /// Output mint.
    pub output_mint: String,
    /// Input amount in input-token base units.
    pub amount: u64,
    /// Slippage tolerance in basis points (default 50 = 0.5%).
    pub slippage_bps: u16,
}

impl QuoteRequest {
    pub fn new(
        input_mint: impl Into<String>,
        output_mint: impl Into<String>,
        amount: u64,
        slippage_bps: u16,
    ) -> Self {
        Self {
            input_mint: input_mint.into(),
            output_mint: output_mint.into(),
            amount,
            slippage_bps,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct JupiterRouteStep {
    /// AMM id pubkey (e.g. Raydium AMM v4 pool).
    pub amm_id: String,
    /// Human label (e.g. "Raydium", "Orca", "Meteora").
    pub label: String,
    pub input_mint: String,
    pub output_mint: String,
    pub in_amount: u64,
    pub out_amount: u64,
    pub fee_amount: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct JupiterQuote {
    /// Route plan (1+ steps).
    pub route_plan: Vec<JupiterRouteStep>,
    /// Input amount in input-token base units.
    pub in_amount: u64,
    /// Output amount in output-token base units.
    pub out_amount: u64,
    /// Slippage threshold (otherAmountThreshold from Jupiter).
    pub other_amount_threshold: u64,
    /// Base64-encoded swap-transaction. In 08-01 (paper mode) this
    /// is a placeholder; in 08-02 it's a real VersionedTransaction.
    pub swap_transaction_base64: String,
}

impl JupiterQuote {
    /// True if the quote is the all-empty placeholder (paper mode).
    pub fn is_placeholder(&self) -> bool {
        self.route_plan.is_empty() && self.swap_transaction_base64.is_empty()
    }
}

/// The Jupiter client trait. Both the real HTTP client (08-02) and
/// the mock (08-01) implement this.
pub trait JupiterClient: Send + Sync {
    /// Fetch a quote for the given input/output/amount.
    fn quote(&self, req: &QuoteRequest) -> Result<JupiterQuote, ExecutorError>;
}

/// Mock Jupiter client. Returns deterministic placeholder quotes.
/// Used for tests, paper-mode dry-runs, and the 08-01 AC-7.
#[derive(Debug, Default, Clone)]
pub struct MockJupiterClient {
    /// Optional fixed quote for `(input_mint, output_mint)` pairs.
    /// If `None`, a placeholder is returned.
    pub fixed_quotes: std::collections::HashMap<(String, String), JupiterQuote>,
}

impl MockJupiterClient {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a fixed quote for a (input_mint, output_mint) pair.
    pub fn with_quote(mut self, input: &str, output: &str, quote: JupiterQuote) -> Self {
        self.fixed_quotes
            .insert((input.to_string(), output.to_string()), quote);
        self
    }
}

impl JupiterClient for MockJupiterClient {
    fn quote(&self, req: &QuoteRequest) -> Result<JupiterQuote, ExecutorError> {
        let key = (req.input_mint.clone(), req.output_mint.clone());
        if let Some(q) = self.fixed_quotes.get(&key) {
            // Adjust amounts to match the request.
            let in_amount = req.amount;
            let out_amount = if req.amount > 0 && q.in_amount > 0 {
                (q.out_amount as u128 * req.amount as u128 / q.in_amount as u128) as u64
            } else {
                q.out_amount
            };
            return Ok(JupiterQuote {
                route_plan: q.route_plan.clone(),
                in_amount,
                out_amount,
                other_amount_threshold: (out_amount as u128 * (10_000 - req.slippage_bps as u128)
                    / 10_000) as u64,
                swap_transaction_base64: q.swap_transaction_base64.clone(),
            });
        }
        // No fixed quote registered: return a placeholder.
        Ok(JupiterQuote {
            route_plan: vec![],
            in_amount: req.amount,
            out_amount: 0,
            other_amount_threshold: 0,
            swap_transaction_base64: String::new(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mock_returns_placeholder_when_no_fixed_quote() {
        let j = MockJupiterClient::new();
        let req = QuoteRequest::new("SOL", "USDC", 1_000_000, 50);
        let q = j.quote(&req).unwrap();
        assert!(q.is_placeholder());
        assert_eq!(q.in_amount, 1_000_000);
        assert_eq!(q.out_amount, 0);
    }

    #[test]
    fn mock_returns_fixed_quote_with_scaled_out_amount() {
        let q = JupiterQuote {
            route_plan: vec![JupiterRouteStep {
                amm_id: "amm1".into(),
                label: "Raydium".into(),
                input_mint: "SOL".into(),
                output_mint: "USDC".into(),
                in_amount: 1_000_000,
                out_amount: 100_000_000,
                fee_amount: 1_000,
            }],
            in_amount: 1_000_000,
            out_amount: 100_000_000,
            other_amount_threshold: 99_500_000,
            swap_transaction_base64: "base64==".into(),
        };
        let j = MockJupiterClient::new().with_quote("SOL", "USDC", q);
        // Request 2 SOL; the mock scales out_amount proportionally.
        let req = QuoteRequest::new("SOL", "USDC", 2_000_000, 50);
        let q2 = j.quote(&req).unwrap();
        assert_eq!(q2.in_amount, 2_000_000);
        assert_eq!(q2.out_amount, 200_000_000);
        assert_eq!(q2.route_plan.len(), 1);
    }
}
