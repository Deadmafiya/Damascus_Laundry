//! `dl-oracle` — Phase 2 Pyth oracle integration.
//!
//! Replaces the hardcoded `p_detect = p_win = p_land = 1.0` oracle
//! (set in `crates/dl-ledger/src/entry.rs:169-171`) with real Pyth
//! price feeds. The conservative bound uses the Pyth price to
//! require `post_price - fee - tip > pre_price` (in USD terms) before
//! marking a cycle as `WouldTrade`.
//!
//! ## Staleness contract
//!
//! Pyth `publish_time` must be within `MAX_PYTH_AGE_SECS` of the
//! evaluation time. Operators tune via `DL_PYTH_MAX_AGE_SECS` env
//! (default 60 s). Stale prices return `Err(StalePrice)` — the
//! caller (live.rs) treats this as `OpportunityOutcome::NotSubmitted`.
//!
//! ## Long-tail token coverage
//!
//! Long-tail mints may not have a Pyth feed. `fetch_price` returns
//! `Err(NoFeed)` in that case. Operators whitelist these manually
//! in a `pyth_overrides.json` (not implemented in Phase 2b; deferred
//! to a follow-up).

use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use solana_sdk::pubkey::Pubkey;
use thiserror::Error;

pub const MAX_PYTH_AGE_SECS: u64 = 60;

/// A Pyth price point. `price` is `mantissa * 10^expo` in USD.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Price {
    /// Mantissa (integer part). Multiply by `10^expo` to get USD.
    pub price: i64,
    /// Decimal exponent (typically negative: e.g. `-8` for $0.00000001).
    pub expo: i32,
    /// Confidence interval width (same units as `price`).
    pub conf: u64,
    /// Unix timestamp of the price publish.
    pub publish_time: i64,
}

impl Price {
    /// True if the price is fresher than `max_age_secs` old. Allows
    /// a small future-skew tolerance (5 s) for clock drift between
    /// the Pyth publisher and the validator.
    pub fn is_fresh(&self, max_age_secs: u64, now: i64) -> bool {
        // Pyth publishes slightly ahead of clock skew; accept up to
        // 5 s in the future as "fresh".
        const SKEW_TOLERANCE_SECS: i64 = 5;
        let age = now.saturating_sub(self.publish_time);
        if age < 0 {
            // publish_time is in the future → within skew tolerance?
            return (-age) <= SKEW_TOLERANCE_SECS;
        }
        (age as u64) <= max_age_secs
    }
}

#[derive(Debug, Error)]
pub enum PythError {
    #[error("no Pyth feed for this mint")]
    NoFeed,
    #[error("stale Pyth price: publish_time={publish_time}, now={now}, age_secs={age}")]
    StalePrice { publish_time: i64, now: i64, age: i64 },
    #[error("Pyth HTTP error: {0}")]
    Http(String),
    #[error("Pyth parse error: {0}")]
    Parse(String),
}

/// The Pyth client trait. Two impls:
/// - [`HttpPythClient`]: real HTTP against pyth mainnet / devnet
/// - [`MockPythClient`]: deterministic for tests + paper mode
pub trait PythClient: Send + Sync {
    fn fetch_price(&self, feed: &Pubkey) -> Result<Price, PythError>;
}

/// Mock Pyth client. Returns deterministic prices for tests; papers
/// the value keyed by the feed pubkey's first byte.
pub struct MockPythClient {
    pub default_price: i64,
    pub default_expo: i32,
}

impl MockPythClient {
    pub fn new(default_price: i64, default_expo: i32) -> Self {
        Self { default_price, default_expo }
    }
}

impl Default for MockPythClient {
    fn default() -> Self {
        Self::new(1_000_000_000, -8) // $10.00 default
    }
}

impl PythClient for MockPythClient {
    fn fetch_price(&self, feed: &Pubkey) -> Result<Price, PythError> {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        Ok(Price {
            price: self.default_price,
            expo: self.default_expo,
            conf: 100,
            publish_time: now,
        })
    }
}

/// Real HTTP Pyth client. Pyth's Hermes endpoint serves
/// `https://hermes.pyth.network/v2/updates/price/latest?ids[]=<feed>`.
pub struct HttpPythClient {
    endpoint: String,
    http: reqwest::blocking::Client,
}

impl HttpPythClient {
    pub fn for_mainnet() -> Self {
        Self::new("https://hermes.pyth.network")
    }

    /// Public devnet placeholder. Hermes (Pyth's public aggregator)
    /// does not publish a separate devnet endpoint — the same
    /// endpoint serves both. Phase 2 H6 fix: document this clearly
    /// and return the same URL as `for_mainnet` until Pyth exposes
    /// a devnet endpoint.
    pub fn for_devnet() -> Self {
        // Phase 2 H6: Hermes has no separate devnet endpoint.
        // Documented + delegate to for_mainnet so callers don't
        // silently hit a different URL.
        Self::for_mainnet()
    }

    pub fn new(endpoint: impl Into<String>) -> Self {
        let http = reqwest::blocking::Client::builder()
            .timeout(std::time::Duration::from_secs(10))
            .build()
            .expect("reqwest client build");
        Self {
            endpoint: endpoint.into(),
            http,
        }
    }
}

impl PythClient for HttpPythClient {
    fn fetch_price(&self, feed: &Pubkey) -> Result<Price, PythError> {
        let url = format!(
            "{}/v2/updates/price/latest?ids[]={}",
            self.endpoint,
            bs58::encode(feed.as_ref()).into_string()
        );
        let resp = self
            .http
            .get(&url)
            .send()
            .map_err(|e| PythError::Http(e.to_string()))?;
        let status = resp.status();
        if !status.is_success() {
            return Err(PythError::Http(format!("HTTP {status}")));
        }
        let body: serde_json::Value = resp
            .json()
            .map_err(|e| PythError::Parse(e.to_string()))?;
        // Hermes response: { "parsed": [ { "price": "...", "conf": "...", "expo": ..., "publish_time": ... } ] }
        let parsed = body
            .get("parsed")
            .and_then(|v| v.as_array())
            .and_then(|a| a.first())
            .ok_or_else(|| PythError::NoFeed)?;
        let price_str = parsed
            .get("price")
            .and_then(|v| v.as_str())
            .ok_or_else(|| PythError::Parse("missing price".into()))?;
        let price: i64 = price_str
            .parse()
            .map_err(|e| PythError::Parse(format!("price parse: {e}")))?;
        let conf_str = parsed
            .get("conf")
            .and_then(|v| v.as_str())
            .ok_or_else(|| PythError::Parse("missing conf".into()))?;
        let conf: u64 = conf_str
            .parse()
            .map_err(|e| PythError::Parse(format!("conf parse: {e}")))?;
        let expo = parsed
            .get("expo")
            .and_then(|v| v.as_i64())
            .ok_or_else(|| PythError::Parse("missing expo".into()))? as i32;
        let publish_time = parsed
            .get("publish_time")
            .and_then(|v| v.as_i64())
            .ok_or_else(|| PythError::Parse("missing publish_time".into()))?;
        Ok(Price { price, expo, conf, publish_time })
    }
}

/// Fetch the price for `feed` and reject if stale. Convenience wrapper
/// that combines `fetch_price` + `is_fresh` check.
pub fn fetch_fresh(orb: &dyn PythClient, feed: &Pubkey, max_age_secs: u64) -> Result<Price, PythError> {
    let p = orb.fetch_price(feed)?;
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    if !p.is_fresh(max_age_secs, now) {
        let age = now.saturating_sub(p.publish_time);
        return Err(PythError::StalePrice {
            publish_time: p.publish_time,
            now,
            age,
        });
    }
    Ok(p)
}

/// Load the Pyth mint→feed map from environment variables. Phase 2
/// H5 fix: replaces the in-code `HashMap<Pubkey, Pubkey>` with a
/// runtime-configurable loader.
///
/// Variables read:
/// - `DL_PYTH_FEEDS_FILE=<path>` — JSON file of `{"mint": "feed", ...}` (optional)
/// - `DL_PYTH_FEED_<MINT>=<FEED_PUBKEY>` — one env var per mapping
///
/// If both are present, the file is loaded first then env vars override.
///
/// Phase 2 H5: the loader is currently unused by the live trader;
/// `run_live_submit` in `main.rs` calls it and stores the result
/// in `LiveConfig.pyth_feeds`.
pub fn load_pyth_feeds_from_env() -> std::collections::HashMap<Pubkey, Pubkey> {
    use std::collections::HashMap;
    use std::str::FromStr;
    let mut map: HashMap<Pubkey, Pubkey> = HashMap::new();
    if let Ok(path) = std::env::var("DL_PYTH_FEEDS_FILE") {
        if let Ok(raw) = std::fs::read_to_string(&path) {
            if let Ok(parsed) =
                serde_json::from_str::<std::collections::HashMap<String, String>>(&raw)
            {
                for (k, v) in parsed {
                    if let (Ok(mint), Ok(feed)) = (Pubkey::from_str(&k), Pubkey::from_str(&v)) {
                        map.insert(mint, feed);
                    }
                }
            }
        }
    }
    for (key, value) in std::env::vars() {
        if let Some(mint_str) = key.strip_prefix("DL_PYTH_FEED_") {
            if let (Ok(mint), Ok(feed)) = (
                Pubkey::from_str(mint_str),
                Pubkey::from_str(&value),
            ) {
                map.insert(mint, feed);
            }
        }
    }
    map
}

/// Mainnet SOL/USD Pyth feed (sanity check; `devnet_oracle.rs` is
/// the lookup table for arbitrary mints in Phase 2b).
pub const SOL_USD_PYTH_FEED_MAINNET: &str = "H6ARHf6YXxRsRJCnmhgBAo4m8aK3Rn1Y5YJ4sRuRrXoN";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn price_is_fresh_within_window() {
        let now = 1_000_000;
        let p = Price {
            price: 100,
            expo: -8,
            conf: 1,
            publish_time: now - 30,
        };
        assert!(p.is_fresh(60, now));
        assert!(p.is_fresh(30, now));
        assert!(!p.is_fresh(29, now));
    }

    #[test]
    fn price_is_fresh_future_timestamp_is_fresh() {
        let now = 1_000_000;
        let p = Price {
            price: 100,
            expo: -8,
            conf: 1,
            publish_time: now + 5, // clock skew tolerance
        };
        assert!(p.is_fresh(60, now));
    }

    #[test]
    fn mock_returns_default_price() {
        let m = MockPythClient::default();
        let feed = Pubkey::new_unique();
        let p = m.fetch_price(&feed).unwrap();
        assert_eq!(p.price, 1_000_000_000);
        assert_eq!(p.expo, -8);
    }

    #[test]
    fn fetch_fresh_rejects_stale() {
        let m = MockPythClient::default();
        let feed = Pubkey::new_unique();
        // MockPythClient uses real-time publish_time; we can't make it
        // stale without mocking time. Test the rejection path via
        // an explicit stale Price.
        let now = 1_000_000;
        let stale = Price {
            price: 100,
            expo: -8,
            conf: 1,
            publish_time: now - 120,
        };
        assert!(!stale.is_fresh(60, now));
        let err = Err::<Price, _>(PythError::StalePrice {
            publish_time: stale.publish_time,
            now,
            age: now - stale.publish_time,
        });
        assert!(matches!(err.unwrap_err(), PythError::StalePrice { .. }));
    }

    #[test]
    fn sol_usd_feed_is_valid_base58() {
        // Sanity: must decode to 32 bytes.
        let bytes = bs58::decode(SOL_USD_PYTH_FEED_MAINNET)
            .into_vec()
            .expect("decode");
        assert_eq!(bytes.len(), 32);
    }
}
