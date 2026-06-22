//! Jupiter Aggregator v6 client (v2.0 Phase 1).
//!
//! Two implementations of the [`JupiterClient`] trait:
//!
//! - [`MockJupiterClient`] — placeholder quotes for tests and
//!   paper-mode. Pre-dates v2.0; unchanged in interface.
//! - [`HttpJupiterClient`] — real HTTP against
//!   `https://quote-api.jup.ag/v6`. Used by sub-plans 1a–1d.
//!
//! ## Real Jupiter Aggregator v6 API
//!
//! - `POST https://quote-api.jup.ag/v6/quote` with
//!   `{inputMint, outputMint, amount, slippageBps, onlyDirectRoutes,
//!    asLegacyTransaction}`.
//! - `POST https://quote-api.jup.ag/v6/swap` with the route
//!   response + `userPublicKey`. Returns a `swapTransaction` field
//!   that is base64-encoded wire bytes for a
//!   `VersionedTransaction`.
//! - We deserialize the base64 → `VersionedTransaction` via
//!   `bincode`. The transaction is then signed in
//!   `signer_integration::sign_with_keystore`.
//!
//! ## Synchronous vs async
//!
//! The trait is **synchronous** for now. `HttpJupiterClient` uses
//! `reqwest::blocking`. The hot path in `dl-app` will wrap calls
//! in `tokio::task::spawn_blocking` when running inside the
//! async pipeline (added in 1b). An async variant of the trait
//! can come later if profiling shows blocking calls hurt throughput.

use std::time::Duration;

use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use serde::{Deserialize, Serialize};
use solana_sdk::pubkey::Pubkey;
use solana_sdk::transaction::VersionedTransaction;

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
    /// Base64-encoded swap-transaction. In v2.0 this is a real
    /// `VersionedTransaction` returned from the `/swap` endpoint.
    pub swap_transaction_base64: String,
}

impl JupiterQuote {
    /// True if the quote is the all-empty placeholder (paper mode).
    pub fn is_placeholder(&self) -> bool {
        self.route_plan.is_empty() && self.swap_transaction_base64.is_empty()
    }
}

/// The Jupiter client trait.
pub trait JupiterClient: Send + Sync {
    /// Fetch a quote for the given input/output/amount.
    fn quote(&self, req: &QuoteRequest) -> Result<JupiterQuote, ExecutorError>;

    /// Build a swap transaction from a previously-fetched quote.
    /// `user_pubkey` is the signer's pubkey (the hot wallet).
    /// Returns a base64-encoded `VersionedTransaction` ready for
    /// signing.
    fn swap_tx_base64(
        &self,
        quote: &JupiterQuote,
        user_pubkey: &Pubkey,
    ) -> Result<String, ExecutorError>;

    /// Convenience: fetch quote, then fetch swap-tx, then
    /// deserialize to a `VersionedTransaction`. Combines the two
    /// API calls into one hot-path call.
    fn build_swap_tx(
        &self,
        req: &QuoteRequest,
        user_pubkey: &Pubkey,
    ) -> Result<(JupiterQuote, VersionedTransaction), ExecutorError> {
        let quote = self.quote(req)?;
        let b64 = self.swap_tx_base64(&quote, user_pubkey)?;
        let bytes = BASE64
            .decode(b64.as_bytes())
            .map_err(|e| ExecutorError::JupiterDeserialize(format!("base64: {e}")))?;
        bincode::deserialize(&bytes)
            .map_err(|e| ExecutorError::JupiterDeserialize(format!("bincode: {e}")))
    }
}

/// Mock Jupiter client. Returns deterministic placeholder quotes.
/// Used for tests, paper-mode dry-runs, and the 08-01 AC-7.
#[derive(Debug, Default, Clone)]
pub struct MockJupiterClient {
    /// Optional fixed quote for `(input_mint, output_mint)` pairs.
    /// If `None`, a placeholder is returned.
    pub fixed_quotes: std::collections::HashMap<(String, String), JupiterQuote>,
    /// Optional pre-built swap transaction (base64) returned by
    /// `swap_tx_base64`. If `None`, an empty string is returned
    /// (placeholder behavior).
    pub fixed_swap_tx_base64: Option<String>,
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

    /// Register a fixed swap-tx base64 returned by
    /// `swap_tx_base64`.
    pub fn with_swap_tx_base64(mut self, b64: impl Into<String>) -> Self {
        self.fixed_swap_tx_base64 = Some(b64.into());
        self
    }
}

impl JupiterClient for MockJupiterClient {
    fn quote(&self, req: &QuoteRequest) -> Result<JupiterQuote, ExecutorError> {
        let key = (req.input_mint.clone(), req.output_mint.clone());
        if let Some(q) = self.fixed_quotes.get(&key) {
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
                other_amount_threshold: (out_amount as u128
                    * (10_000 - req.slippage_bps as u128)
                    / 10_000) as u64,
                swap_transaction_base64: q.swap_transaction_base64.clone(),
            });
        }
        Ok(JupiterQuote {
            route_plan: vec![],
            in_amount: req.amount,
            out_amount: 0,
            other_amount_threshold: 0,
            swap_transaction_base64: String::new(),
        })
    }

    fn swap_tx_base64(
        &self,
        _quote: &JupiterQuote,
        _user_pubkey: &Pubkey,
    ) -> Result<String, ExecutorError> {
        Ok(self.fixed_swap_tx_base64.clone().unwrap_or_default())
    }
}

// ─── HTTP client ─────────────────────────────────────────────────────────

/// JSON body sent to Jupiter's `/v6/quote` endpoint.
#[derive(Debug, Serialize)]
struct QuoteRequestBody<'a> {
    inputMint: &'a str,
    outputMint: &'a str,
    amount: u64,
    slippageBps: u16,
    #[serde(rename = "onlyDirectRoutes")]
    only_direct_routes: bool,
    #[serde(rename = "asLegacyTransaction")]
    as_legacy_transaction: bool,
}

/// JSON response from Jupiter's `/v6/quote` endpoint. Only the
/// fields we use are extracted; the rest are ignored.
#[derive(Debug, Clone, Deserialize)]
struct QuoteResponse {
    #[serde(rename = "inputMint")]
    #[allow(dead_code)]
    input_mint: String,
    #[serde(rename = "outputMint")]
    #[allow(dead_code)]
    output_mint: String,
    #[serde(rename = "inAmount")]
    in_amount: u64,
    #[serde(rename = "outAmount")]
    out_amount: u64,
    #[serde(rename = "otherAmountThreshold")]
    other_amount_threshold: String,
    #[serde(rename = "routePlan")]
    route_plan: Vec<RoutePlanStep>,
}

#[derive(Debug, Clone, Deserialize)]
struct RoutePlanStep {
    #[serde(rename = "ammId")]
    amm_id: String,
    label: Option<String>,
    #[serde(rename = "inputMint")]
    input_mint: String,
    #[serde(rename = "outputMint")]
    output_mint: String,
    #[serde(rename = "inAmount")]
    in_amount: String,
    #[serde(rename = "outAmount")]
    out_amount: String,
    #[serde(rename = "feeAmount")]
    fee_amount: String,
}

/// JSON body sent to Jupiter's `/v6/swap` endpoint.
#[derive(Debug, Serialize)]
struct SwapRequestBody<'a> {
    #[serde(rename = "userPublicKey")]
    user_public_key: &'a str,
    #[serde(rename = "quoteResponse")]
    quote_response: &'a serde_json::Value,
    #[serde(rename = "wrapAndUnwrapSol")]
    wrap_and_unwrap_sol: bool,
    #[serde(rename = "dynamicComputeUnitLimit")]
    dynamic_compute_unit_limit: bool,
    #[serde(rename = "prioritizationFeeLamports")]
    prioritization_fee_lamports: u64,
}

/// JSON response from Jupiter's `/v6/swap` endpoint.
#[derive(Debug, Deserialize)]
struct SwapResponse {
    #[serde(rename = "swapTransaction")]
    swap_transaction: String,
}

/// The real HTTP Jupiter client. Construct via [`HttpJupiterClient::new`]
/// or [`HttpJupiterClient::for_mainnet`] / [`HttpJupiterClient::for_devnet`].
#[derive(Debug, Clone)]
pub struct HttpJupiterClient {
    base_url: String,
    api_key: Option<String>,
    timeout: Duration,
    http: reqwest::blocking::Client,
}

impl HttpJupiterClient {
    pub fn new(base_url: impl Into<String>, api_key: Option<String>) -> Self {
        let http = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(10))
            .build()
            .expect("reqwest client build");
        Self {
            base_url: base_url.into(),
            api_key,
            timeout: Duration::from_secs(10),
            http,
        }
    }

    pub fn for_mainnet() -> Self {
        Self::new("https://quote-api.jup.ag/v6", None)
    }

    /// Jupiter's public aggregator API does not expose a separate
    /// devnet endpoint — the same `/v6/quote` and `/v6/swap`
    /// endpoints serve both. Devnet swaps go through the same URL
    /// but the user's wallet + mint addresses must be devnet
    /// tokens (which Jupiter quotes return with realistic rates).
    ///
    /// For a true devnet-only experience, build a custom client
    /// via `HttpJupiterClient::new(<devnet-proxy-url>, None)`.
    pub fn for_devnet() -> Self {
        Self::new("https://quote-api.jup.ag/v6", None)
    }

    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        let new_client = reqwest::blocking::Client::builder()
            .timeout(timeout)
            .build()
            .expect("reqwest client rebuild");
        self.http = new_client;
        self
    }

    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    fn quote_request_body(req: &QuoteRequest) -> QuoteRequestBody<'_> {
        QuoteRequestBody {
            inputMint: &req.input_mint,
            outputMint: &req.output_mint,
            amount: req.amount,
            slippageBps: req.slippage_bps,
            only_direct_routes: false,
            as_legacy_transaction: false,
        }
    }

    fn parse_quote_response(body: &QuoteResponse, fallback_swap_b64: &str) -> JupiterQuote {
        let route_plan: Vec<JupiterRouteStep> = body
            .route_plan
            .iter()
            .map(|s| JupiterRouteStep {
                amm_id: s.amm_id.clone(),
                label: s.label.clone().unwrap_or_default(),
                input_mint: s.input_mint.clone(),
                output_mint: s.output_mint.clone(),
                in_amount: s.in_amount.parse().unwrap_or(0),
                out_amount: s.out_amount.parse().unwrap_or(0),
                fee_amount: s.fee_amount.parse().unwrap_or(0),
            })
            .collect();
        JupiterQuote {
            route_plan,
            in_amount: body.in_amount,
            out_amount: body.out_amount,
            other_amount_threshold: body.other_amount_threshold.parse().unwrap_or(0),
            swap_transaction_base64: fallback_swap_b64.to_string(),
        }
    }
}

impl JupiterClient for HttpJupiterClient {
    fn quote(&self, req: &QuoteRequest) -> Result<JupiterQuote, ExecutorError> {
        let url = format!("{}/quote", self.base_url);
        let body = Self::quote_request_body(req);
        let mut request = self.http.post(&url).json(&body);
        if let Some(key) = &self.api_key {
            request = request.bearer_auth(key);
        }
        let response = request
            .send()
            .map_err(|e| ExecutorError::JupiterQuote(format!("http: {e}")))?;
        let status = response.status();
        if !status.is_success() {
            let text = response.text().unwrap_or_default();
            return Err(ExecutorError::JupiterQuote(format!(
                "http {}: {}",
                status.as_u16(),
                text.chars().take(500).collect::<String>()
            )));
        }
        let parsed: QuoteResponse = response
            .json()
            .map_err(|e| ExecutorError::JupiterQuote(format!("decode: {e}")))?;
        Ok(Self::parse_quote_response(&parsed, ""))
    }

    fn swap_tx_base64(
        &self,
        quote: &JupiterQuote,
        user_pubkey: &Pubkey,
    ) -> Result<String, ExecutorError> {
        let url = format!("{}/swap", self.base_url);
        // Re-serialize the quote as the `quoteResponse` field
        // Jupiter expects. We strip `swap_transaction_base64`
        // since Jupiter would echo it back (and reject if set).
        let mut quote_value = serde_json::to_value(quote)
            .map_err(|e| ExecutorError::JupiterQuote(format!("re-serialize: {e}")))?;
        if let Some(obj) = quote_value.as_object_mut() {
            obj.remove("swap_transaction_base64");
        }
        let body = SwapRequestBody {
            user_public_key: &user_pubkey.to_string(),
            quote_response: &quote_value,
            wrap_and_unwrap_sol: true,
            dynamic_compute_unit_limit: true,
            prioritization_fee_lamports: 0,
        };
        let mut request = self.http.post(&url).json(&body);
        if let Some(key) = &self.api_key {
            request = request.bearer_auth(key);
        }
        let response = request
            .send()
            .map_err(|e| ExecutorError::JupiterQuote(format!("swap http: {e}")))?;
        let status = response.status();
        if !status.is_success() {
            let text = response.text().unwrap_or_default();
            return Err(ExecutorError::JupiterQuote(format!(
                "swap http {}: {}",
                status.as_u16(),
                text.chars().take(500).collect::<String>()
            )));
        }
        let parsed: SwapResponse = response
            .json()
            .map_err(|e| ExecutorError::JupiterQuote(format!("swap decode: {e}")))?;
        Ok(parsed.swap_transaction)
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
        let req = QuoteRequest::new("SOL", "USDC", 2_000_000, 50);
        let q2 = j.quote(&req).unwrap();
        assert_eq!(q2.in_amount, 2_000_000);
        assert_eq!(q2.out_amount, 200_000_000);
        assert_eq!(q2.route_plan.len(), 1);
    }

    #[test]
    fn mock_swap_tx_base64_returns_empty_when_unset() {
        let j = MockJupiterClient::new();
        let req = QuoteRequest::new("SOL", "USDC", 1_000_000, 50);
        let q = j.quote(&req).unwrap();
        let user = Pubkey::new_unique();
        let b64 = j.swap_tx_base64(&q, &user).unwrap();
        assert!(b64.is_empty());
    }

    #[test]
    fn mock_swap_tx_base64_returns_fixed_when_set() {
        let j = MockJupiterClient::new().with_swap_tx_base64("dGVzdA==");
        let req = QuoteRequest::new("SOL", "USDC", 1_000_000, 50);
        let q = j.quote(&req).unwrap();
        let user = Pubkey::new_unique();
        let b64 = j.swap_tx_base64(&q, &user).unwrap();
        assert_eq!(b64, "dGVzdA==");
    }

    #[test]
    fn http_client_construction() {
        let client = HttpJupiterClient::for_mainnet();
        assert_eq!(client.base_url(), "https://quote-api.jup.ag/v6");
        let dev = HttpJupiterClient::for_devnet();
        assert_eq!(dev.base_url(), "https://quote-api.jup.ag/v6");
        let custom = HttpJupiterClient::new("https://example.com/v6", Some("test-key".into()));
        assert_eq!(custom.base_url(), "https://example.com/v6");
    }

    #[test]
    fn http_client_quote_request_body_shape() {
        let req = QuoteRequest::new("SOL", "USDC", 1_000_000, 50);
        let body = HttpJupiterClient::quote_request_body(&req);
        let json = serde_json::to_string(&body).unwrap();
        // Snake-case keys become camelCase in the JSON.
        assert!(json.contains("\"inputMint\":\"SOL\""));
        assert!(json.contains("\"outputMint\":\"USDC\""));
        assert!(json.contains("\"amount\":1000000"));
        assert!(json.contains("\"slippageBps\":50"));
        assert!(json.contains("\"onlyDirectRoutes\":false"));
        assert!(json.contains("\"asLegacyTransaction\":false"));
    }

    #[test]
    fn parse_quote_response_extracts_route_plan() {
        // Sample Jupiter /quote response.
        let body = QuoteResponse {
            input_mint: "So11111111111111111111111111111111111111112".into(),
            output_mint: "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v".into(),
            in_amount: 1_000_000,
            out_amount: 150_000_000,
            other_amount_threshold: "149250000".into(),
            route_plan: vec![RoutePlanStep {
                amm_id: "amm1".into(),
                label: Some("Raydium".into()),
                input_mint: "So11111111111111111111111111111111111111112".into(),
                output_mint: "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v".into(),
                in_amount: "1000000".into(),
                out_amount: "150000000".into(),
                fee_amount: "1000".into(),
            }],
        };
        let q = HttpJupiterClient::parse_quote_response(&body, "");
        assert_eq!(q.in_amount, 1_000_000);
        assert_eq!(q.out_amount, 150_000_000);
        assert_eq!(q.other_amount_threshold, 149_250_000);
        assert_eq!(q.route_plan.len(), 1);
        assert_eq!(q.route_plan[0].label, "Raydium");
    }

    #[test]
    fn parse_quote_response_handles_missing_label() {
        let body = QuoteResponse {
            input_mint: "A".into(),
            output_mint: "B".into(),
            in_amount: 100,
            out_amount: 200,
            other_amount_threshold: "199".into(),
            route_plan: vec![RoutePlanStep {
                amm_id: "amm1".into(),
                label: None,
                input_mint: "A".into(),
                output_mint: "B".into(),
                in_amount: "100".into(),
                out_amount: "200".into(),
                fee_amount: "1".into(),
            }],
        };
        let q = HttpJupiterClient::parse_quote_response(&body, "");
        assert_eq!(q.route_plan[0].label, "");
    }

    #[test]
    fn parse_quote_response_invalid_threshold_falls_back_to_zero() {
        let body = QuoteResponse {
            input_mint: "A".into(),
            output_mint: "B".into(),
            in_amount: 100,
            out_amount: 200,
            other_amount_threshold: "not-a-number".into(),
            route_plan: vec![],
        };
        let q = HttpJupiterClient::parse_quote_response(&body, "");
        assert_eq!(q.other_amount_threshold, 0);
    }

    #[test]
    fn base64_decode_invalid_input_fails() {
        // Demonstrate the error path for build_swap_tx when given
        // a base64 string that's invalid.
        let bytes_result = BASE64.decode("not-valid-base64!!!".as_bytes());
        assert!(bytes_result.is_err());
    }
}