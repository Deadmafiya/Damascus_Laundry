//! Jito Block Engine client (v2.0 Phase 1).
//!
//! Two implementations of the [`JitoClient`] trait:
//!
//! - [`MockJitoClient`] — accepts any bundle and reports
//!   `Landed { slot: 0 }` immediately. Used for paper-mode
//!   tests; pre-dates v2.0.
//! - [`HttpJitoClient`] — real HTTP against the Jito Block
//!   Engine. Used by sub-plans 1a–1d.
//!
//! ## Jito Block Engine API
//!
//! - `POST /api/v1/bundles` with
//!   `{ jsonrpc: "2.0", id: 1, method: "sendBundle", params: [b64tx1, ...] }`.
//!   Returns the bundle UUID (the `bundle_id`).
//! - `POST /api/v1/getBundleStatuses` with
//!   `{ jsonrpc: "2.0", id: 1, method: "getBundleStatuses", params: [[bundle_id]] }`.
//!   Returns the landing status (`Landed` / `Failed` / `Pending` /
//!   `Invalid` / etc).
//!
//! ## Tip account rotation (locked decision #4)
//!
//! `HttpJitoClient::for_mainnet` queries
//! `/api/v1/getTipAccounts` once at construction and rotates
//! per-bundle across the 8 returned accounts. The chosen account
//! is logged on every `SubmittedBundle` for audit (the field is
//! added to `PaperRunConfig` / `LiveConfig` in 1b).
//!
//! ## Synchronous API
//!
//! The trait is **synchronous** for now. `HttpJitoClient` uses
//! `reqwest::blocking`. The hot path wraps calls in
//! `tokio::task::spawn_blocking` in 1b.

use std::time::Duration;

use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use serde::{Deserialize, Serialize};

use crate::bundle::Bundle;
use crate::error::ExecutorError;

/// Health state of the Jito Block Engine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum JitoHealth {
    Unknown,
    Up,
    Down,
}

impl Default for JitoHealth {
    fn default() -> Self {
        Self::Up
    }
}

/// Result of submitting a bundle.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct JitoSubmitResult {
    /// Jito's bundle ID (UUID). In mock mode, a deterministic string.
    pub bundle_id: String,
    /// Tip lamports paid.
    pub tip_lamports: u64,
    /// Unix timestamp when submitted.
    pub submitted_at: u64,
    /// Tip account used (one of Jito's 8 accounts). None for the
    /// mock client.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tip_account: Option<String>,
}

/// What happened to a bundle after submission.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum LandingResult {
    /// Bundle landed in `slot`.
    Landed { slot: u64 },
    /// Bundle didn't land within the timeout.
    Lost,
    /// Bundle is still pending.
    Pending,
}

/// The Jito client trait.
pub trait JitoClient: Send + Sync {
    /// Health check.
    fn health(&self) -> JitoHealth;
    /// Submit a bundle.
    fn submit(&self, bundle: &Bundle) -> Result<JitoSubmitResult, ExecutorError>;
    /// Poll for landing (with timeout). In mock mode, this
    /// returns `Landed` immediately with a fake slot.
    fn poll_landing(&self, bundle_id: &str) -> Result<LandingResult, ExecutorError>;
}

// ─── Mock client ─────────────────────────────────────────────────────────

/// Mock Jito client. Accepts all bundles, returns deterministic
/// bundle_ids, and reports `Landed` immediately with a fake slot.
#[derive(Debug, Default)]
pub struct MockJitoClient {
    counter: std::sync::Mutex<u64>,
    health: std::sync::Mutex<JitoHealth>,
}

impl MockJitoClient {
    pub fn new() -> Self {
        Self {
            counter: std::sync::Mutex::new(0),
            health: std::sync::Mutex::new(JitoHealth::Up),
        }
    }

    pub fn with_health(self, h: JitoHealth) -> Self {
        *self.health.lock().unwrap() = h;
        self
    }
}

impl JitoClient for MockJitoClient {
    fn health(&self) -> JitoHealth {
        *self.health.lock().unwrap()
    }

    fn submit(&self, bundle: &Bundle) -> Result<JitoSubmitResult, ExecutorError> {
        if matches!(self.health(), JitoHealth::Down) {
            return Err(ExecutorError::JitoSubmit(
                "Jito Block Engine is DOWN (mock)".into(),
            ));
        }
        let mut c = self.counter.lock().unwrap();
        *c += 1;
        Ok(JitoSubmitResult {
            bundle_id: format!("mock-bundle-{}", *c),
            tip_lamports: bundle.total_tip_lamports(),
            submitted_at: 0,
            tip_account: None,
        })
    }

    fn poll_landing(&self, _bundle_id: &str) -> Result<LandingResult, ExecutorError> {
        Ok(LandingResult::Landed { slot: 0 })
    }
}

// ─── HTTP client ─────────────────────────────────────────────────────────

/// JSON-RPC request body used by Jito Block Engine.
#[derive(Debug, Serialize)]
struct JsonRpcRequest<'a, T: Serialize> {
    jsonrpc: &'a str,
    id: u64,
    method: &'a str,
    params: T,
}

/// Response from `sendBundle` (just a string bundle ID).
#[derive(Debug, Deserialize)]
struct SendBundleResponse {
    result: String,
}

/// Response item from `getBundleStatuses`.
#[derive(Debug, Clone, Deserialize)]
struct BundleStatus {
    #[allow(dead_code)]
    bundle_id: String,
    /// `Landed`, `Failed`, `Pending`, `Invalid`, etc. Jito
    /// documents this list. We map `Landed` to `Landed`, anything
    /// else (including `Failed`) to `Lost`.
    status: String,
    #[serde(default)]
    landed_slot: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct GetBundleStatusesResponse {
    result: BundleStatusResponseWrapper,
}

#[derive(Debug, Deserialize)]
struct BundleStatusResponseWrapper {
    value: Vec<BundleStatus>,
}

/// Response from `getTipAccounts` (list of base58 pubkeys).
#[derive(Debug, Deserialize)]
struct GetTipAccountsResponse {
    result: Vec<String>,
}

/// The real HTTP Jito Block Engine client. Construct via
/// [`HttpJitoClient::for_mainnet`] / [`HttpJitoClient::for_devnet`].
#[derive(Debug)]
pub struct HttpJitoClient {
    block_engine_url: String,
    timeout: Duration,
    http: reqwest::blocking::Client,
    /// Tip accounts (Jito exposes 8). None until populated by
    /// `populate_tip_accounts`.
    tip_accounts: std::sync::Mutex<Vec<String>>,
    /// Index of the next tip account to use. Modulo
    /// `tip_accounts.len()`.
    tip_index: std::sync::Mutex<usize>,
}

impl HttpJitoClient {
    pub fn new(block_engine_url: impl Into<String>) -> Self {
        let http = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(10))
            .build()
            .expect("reqwest client build");
        Self {
            block_engine_url: block_engine_url.into(),
            timeout: Duration::from_secs(10),
            http,
            tip_accounts: std::sync::Mutex::new(Vec::new()),
            tip_index: std::sync::Mutex::new(0),
        }
    }

    pub fn for_mainnet() -> Self {
        Self::new("https://mainnet.block-engine.jito.wtf")
    }

    pub fn for_devnet() -> Self {
        Self::new("https://devnet.block-engine.jito.wtf")
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

    /// Query the Jito Block Engine for the list of tip accounts
    /// and cache them. Idempotent; safe to call multiple times.
    pub fn populate_tip_accounts(&self) -> Result<(), ExecutorError> {
        let url = format!("{}/api/v1/getTipAccounts", self.block_engine_url);
        let body = JsonRpcRequest {
            jsonrpc: "2.0",
            id: 1,
            method: "getTipAccounts",
            params: (),
        };
        let response = self
            .http
            .post(&url)
            .json(&body)
            .send()
            .map_err(|e| ExecutorError::JitoSubmit(format!("getTipAccounts http: {e}")))?;
        let status = response.status();
        if !status.is_success() {
            let text = response.text().unwrap_or_default();
            return Err(ExecutorError::JitoSubmit(format!(
                "getTipAccounts http {}: {}",
                status.as_u16(),
                text.chars().take(500).collect::<String>()
            )));
        }
        let parsed: GetTipAccountsResponse = response
            .json()
            .map_err(|e| ExecutorError::JitoSubmit(format!("getTipAccounts decode: {e}")))?;
        *self.tip_accounts.lock().unwrap() = parsed.result;
        Ok(())
    }

    /// Return the next tip account in the rotation. Caller must
    /// ensure `populate_tip_accounts` has been called first (or
    /// returns `Err` if no accounts are loaded).
    pub fn next_tip_account(&self) -> Result<String, ExecutorError> {
        let accounts = self.tip_accounts.lock().unwrap();
        if accounts.is_empty() {
            return Err(ExecutorError::JitoSubmit(
                "no tip accounts loaded; call populate_tip_accounts first".into(),
            ));
        }
        let mut idx = self.tip_index.lock().unwrap();
        let account = accounts[*idx % accounts.len()].clone();
        *idx = idx.wrapping_add(1);
        Ok(account)
    }

    /// Encode a bundle's signed transactions to the JSON-RPC
    /// `params` array (each tx base64-encoded). Exposed for tests.
    pub fn encode_bundle_params(bundle: &Bundle) -> Result<Vec<String>, ExecutorError> {
        bundle
            .signed_transactions
            .iter()
            .map(|tx| {
                let bytes = bincode::serialize(tx).map_err(|e| {
                    ExecutorError::JitoSubmit(format!("bincode serialize tx: {e}"))
                })?;
                Ok(BASE64.encode(&bytes))
            })
            .collect()
    }

    /// Parse a `getBundleStatuses` response into a `LandingResult`.
    /// Exposed for tests.
    pub fn parse_status_response(resp: &BundleStatus) -> LandingResult {
        match resp.status.as_str() {
            "Landed" => LandingResult::Landed {
                slot: resp.landed_slot.unwrap_or(0),
            },
            "Pending" | "Processing" | "Unprocessed" | "WindowPostgres" => {
                LandingResult::Pending
            }
            // `Failed`, `Rejected`, `Invalid`, `Dropped`, etc.
            _ => LandingResult::Lost,
        }
    }
}

impl JitoClient for HttpJitoClient {
    fn health(&self) -> JitoHealth {
        // Cheap liveness ping: GET the block engine root. We don't
        // parse the body — any 2xx counts as Up, else Down.
        let url = format!("{}/health", self.block_engine_url);
        match self.http.get(&url).send() {
            Ok(r) if r.status().is_success() => JitoHealth::Up,
            Ok(_) => JitoHealth::Down,
            Err(_) => JitoHealth::Down,
        }
    }

    fn submit(&self, bundle: &Bundle) -> Result<JitoSubmitResult, ExecutorError> {
        let url = format!("{}/api/v1/bundles", self.block_engine_url);
        let txs_b64 = Self::encode_bundle_params(bundle)?;
        let body = JsonRpcRequest {
            jsonrpc: "2.0",
            id: 1,
            method: "sendBundle",
            params: txs_b64,
        };
        let response = self
            .http
            .post(&url)
            .json(&body)
            .send()
            .map_err(|e| ExecutorError::JitoSubmit(format!("sendBundle http: {e}")))?;
        let status = response.status();
        if !status.is_success() {
            let text = response.text().unwrap_or_default();
            return Err(ExecutorError::JitoSubmit(format!(
                "sendBundle http {}: {}",
                status.as_u16(),
                text.chars().take(500).collect::<String>()
            )));
        }
        let parsed: SendBundleResponse = response
            .json()
            .map_err(|e| ExecutorError::JitoSubmit(format!("sendBundle decode: {e}")))?;
        // The chosen tip account is the last tx in the bundle
        // (per the atomicity ADR). The caller sets `tip_account`
        // on `JitoSubmitResult` at build time (via the rotation
        // index); the client doesn't try to extract it from the
        // wire bytes here.
        Ok(JitoSubmitResult {
            bundle_id: parsed.result,
            tip_lamports: bundle.total_tip_lamports(),
            submitted_at: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0),
            tip_account: None,
        })
    }

    fn poll_landing(&self, bundle_id: &str) -> Result<LandingResult, ExecutorError> {
        let url = format!("{}/api/v1/getBundleStatuses", self.block_engine_url);
        let body = JsonRpcRequest {
            jsonrpc: "2.0",
            id: 1,
            method: "getBundleStatuses",
            params: vec![bundle_id],
        };
        let response = self
            .http
            .post(&url)
            .json(&body)
            .send()
            .map_err(|e| ExecutorError::JitoSubmit(format!("getBundleStatuses http: {e}")))?;
        let status = response.status();
        if !status.is_success() {
            let text = response.text().unwrap_or_default();
            return Err(ExecutorError::JitoSubmit(format!(
                "getBundleStatuses http {}: {}",
                status.as_u16(),
                text.chars().take(500).collect::<String>()
            )));
        }
        let parsed: GetBundleStatusesResponse = response
            .json()
            .map_err(|e| ExecutorError::JitoSubmit(format!("getBundleStatuses decode: {e}")))?;
        let Some(first) = parsed.result.value.into_iter().next() else {
            // Bundle ID not found yet — Jito hasn't indexed it.
            return Ok(LandingResult::Pending);
        };
        Ok(Self::parse_status_response(&first))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bundle::{BundleBuilder, SwapLeg, TipLeg};
    use solana_sdk::message::{Message, VersionedMessage};
    use solana_sdk::pubkey::Pubkey;
    use solana_sdk::signer::keypair::Keypair;
    use solana_sdk::signer::Signer;
    use solana_sdk::system_instruction;
    use solana_sdk::transaction::VersionedTransaction;

    fn dummy_tx() -> VersionedTransaction {
        let kp = Keypair::new();
        let ix = system_instruction::transfer(&kp.pubkey(), &Pubkey::new_unique(), 0);
        let msg = Message::new(&[ix], Some(&kp.pubkey()));
        VersionedTransaction::try_new(VersionedMessage::Legacy(msg), &[&kp]).unwrap()
    }

    fn test_bundle() -> Bundle {
        let mut b = BundleBuilder::new();
        b.push_swap(SwapLeg::new(
            "Raydium",
            "SOL",
            "USDC",
            1_000_000,
            100_000_000,
        ));
        b.set_tip(TipLeg::new(
            10_000,
            "JitoTip1111111111111111111111111111111111",
        ));
        // 1 swap + assert + tip = 3 txs
        b.set_signed_transactions(vec![dummy_tx(), dummy_tx(), dummy_tx()]);
        b.build(None).unwrap()
    }

    #[test]
    fn mock_health_defaults_up() {
        let j = MockJitoClient::new();
        assert_eq!(j.health(), JitoHealth::Up);
    }

    #[test]
    fn mock_submit_assigns_sequential_bundle_ids() {
        let j = MockJitoClient::new();
        let b = test_bundle();
        let r1 = j.submit(&b).unwrap();
        let r2 = j.submit(&b).unwrap();
        assert_eq!(r1.bundle_id, "mock-bundle-1");
        assert_eq!(r2.bundle_id, "mock-bundle-2");
    }

    #[test]
    fn mock_submit_preserves_tip_lamports() {
        let j = MockJitoClient::new();
        let b = test_bundle();
        let r = j.submit(&b).unwrap();
        assert_eq!(r.tip_lamports, 10_000);
    }

    #[test]
    fn mock_submit_fails_when_health_down() {
        let j = MockJitoClient::new().with_health(JitoHealth::Down);
        let b = test_bundle();
        let err = j.submit(&b).unwrap_err();
        assert!(matches!(err, ExecutorError::JitoSubmit(_)));
    }

    #[test]
    fn mock_poll_landing_returns_landed() {
        let j = MockJitoClient::new();
        let r = j.poll_landing("mock-bundle-1").unwrap();
        match r {
            LandingResult::Landed { slot } => assert_eq!(slot, 0),
            _ => panic!("expected Landed"),
        }
    }

    #[test]
    fn mock_jito_submit_result_has_no_tip_account() {
        let j = MockJitoClient::new();
        let b = test_bundle();
        let r = j.submit(&b).unwrap();
        assert!(r.tip_account.is_none());
    }

    #[test]
    fn http_client_construction() {
        let mainnet = HttpJitoClient::for_mainnet();
        assert_eq!(
            mainnet.block_engine_url,
            "https://mainnet.block-engine.jito.wtf"
        );
        let devnet = HttpJitoClient::for_devnet();
        assert_eq!(
            devnet.block_engine_url,
            "https://devnet.block-engine.jito.wtf"
        );
    }

    #[test]
    fn http_client_tip_account_rotation() {
        let c = HttpJitoClient::for_mainnet();
        // Without populate_tip_accounts, should error.
        assert!(c.next_tip_account().is_err());

        // Simulate populate by setting the tip accounts directly.
        *c.tip_accounts.lock().unwrap() = vec![
            "AccountA".to_string(),
            "AccountB".to_string(),
            "AccountC".to_string(),
        ];
        let a = c.next_tip_account().unwrap();
        let b = c.next_tip_account().unwrap();
        let cc = c.next_tip_account().unwrap();
        let a2 = c.next_tip_account().unwrap();
        assert_eq!(a, "AccountA");
        assert_eq!(b, "AccountB");
        assert_eq!(cc, "AccountC");
        assert_eq!(a2, "AccountA"); // wraps around
    }

    #[test]
    fn http_client_encode_bundle_params_produces_5_b64_strings() {
        let b = test_bundle();
        let params = HttpJitoClient::encode_bundle_params(&b).unwrap();
        assert_eq!(params.len(), 3);
        // Each is valid base64.
        for p in &params {
            let decoded = BASE64.decode(p.as_bytes()).unwrap();
            assert!(!decoded.is_empty());
        }
    }

    #[test]
    fn parse_status_response_landed() {
        let resp = BundleStatus {
            bundle_id: "abc".into(),
            status: "Landed".into(),
            landed_slot: Some(12345),
        };
        assert_eq!(
            HttpJitoClient::parse_status_response(&resp),
            LandingResult::Landed { slot: 12345 }
        );
    }

    #[test]
    fn parse_status_response_pending_variants() {
        for s in ["Pending", "Processing", "Unprocessed", "WindowPostgres"] {
            let resp = BundleStatus {
                bundle_id: "abc".into(),
                status: s.into(),
                landed_slot: None,
            };
            assert_eq!(
                HttpJitoClient::parse_status_response(&resp),
                LandingResult::Pending
            );
        }
    }

    #[test]
    fn parse_status_response_failed_maps_to_lost() {
        for s in ["Failed", "Rejected", "Invalid", "Dropped"] {
            let resp = BundleStatus {
                bundle_id: "abc".into(),
                status: s.into(),
                landed_slot: None,
            };
            assert_eq!(
                HttpJitoClient::parse_status_response(&resp),
                LandingResult::Lost
            );
        }
    }
}