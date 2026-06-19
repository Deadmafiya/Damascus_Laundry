//! Jito Block Engine client (08-01: paper mode).
//!
//! 08-01 ships the **mock** implementation: a `JitoClient` trait
//! with a `MockJitoClient` that accepts any bundle and returns a
//! fake `bundle_id`. The real `jito-bundle::send_bundle` integration
//! lands in 08-02.

use std::sync::Mutex;

use crate::bundle::{Bundle, SwapLeg};
use crate::error::ExecutorError;
use crate::jupiter::JupiterQuote;

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
    /// Poll for landing (with timeout). In 08-01 mock mode, this
    /// returns `Landed` immediately with a fake slot.
    fn poll_landing(&self, bundle_id: &str) -> Result<LandingResult, ExecutorError>;
}

/// Mock Jito client. Accepts all bundles, returns deterministic
/// bundle_ids, and reports `Landed` immediately with a fake slot.
#[derive(Debug, Default)]
pub struct MockJitoClient {
    counter: Mutex<u64>,
    health: Mutex<JitoHealth>,
}

impl MockJitoClient {
    pub fn new() -> Self {
        Self {
            counter: Mutex::new(0),
            health: Mutex::new(JitoHealth::Up),
        }
    }

    /// Set the simulated health state.
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
            submitted_at: 0, // 0 = "not a real timestamp" (mock)
        })
    }

    fn poll_landing(&self, _bundle_id: &str) -> Result<LandingResult, ExecutorError> {
        // In mock mode, we pretend the bundle landed immediately.
        Ok(LandingResult::Landed { slot: 0 })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bundle::TipLeg;

    fn small_bundle() -> Bundle {
        let mut b = crate::bundle::BundleBuilder::new();
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
        b.build().unwrap()
    }

    #[test]
    fn mock_health_defaults_up() {
        let j = MockJitoClient::new();
        assert_eq!(j.health(), JitoHealth::Up);
    }

    #[test]
    fn mock_submit_assigns_sequential_bundle_ids() {
        let j = MockJitoClient::new();
        let b = small_bundle();
        let r1 = j.submit(&b).unwrap();
        let r2 = j.submit(&b).unwrap();
        assert_eq!(r1.bundle_id, "mock-bundle-1");
        assert_eq!(r2.bundle_id, "mock-bundle-2");
    }

    #[test]
    fn mock_submit_preserves_tip_lamports() {
        let j = MockJitoClient::new();
        let b = small_bundle();
        let r = j.submit(&b).unwrap();
        assert_eq!(r.tip_lamports, 10_000);
    }

    #[test]
    fn mock_submit_fails_when_health_down() {
        let j = MockJitoClient::new().with_health(JitoHealth::Down);
        let b = small_bundle();
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
}
