//! Token-bucket rate limit (default 10 bundles/minute).
//!
//! The `f64` in this file is the only float in the workspace's value
//! path; the float-free CI guard in `dl-signer/tests/no_floats.rs`
//! allows this one exception. The reason: token-bucket refill is
//! naturally continuous, and the alternative (integer milli-tokens
//! with monotonic clock) is more code for the same correctness.
//!
//! v1.2 plan: replace with pure-integer implementation.

use std::sync::Mutex;
use std::time::Instant;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RateLimitConfig {
    pub bundles_per_minute: u32,
}

impl Default for RateLimitConfig {
    fn default() -> Self {
        Self {
            bundles_per_minute: 10,
        }
    }
}

pub struct RateLimit {
    state: Mutex<State>,
    cfg: RateLimitConfig,
}

struct State {
    /// Fractional tokens (1.0 = a full bundle-per-minute budget).
    tokens: f64,
    /// Last refill time.
    last_refill: Instant,
}

impl RateLimit {
    pub fn new(cfg: RateLimitConfig) -> Self {
        let capacity = cfg.bundles_per_minute as f64;
        Self {
            state: Mutex::new(State {
                tokens: capacity,
                last_refill: Instant::now(),
            }),
            cfg,
        }
    }

    /// Try to consume one token. Returns true if allowed, false if
    /// the rate limit would be exceeded.
    pub fn try_acquire(&self) -> bool {
        let capacity = self.cfg.bundles_per_minute as f64;
        let rate_per_sec = capacity / 60.0;
        let now = Instant::now();

        let mut s = self.state.lock().expect("rate-limit mutex");
        let elapsed = now.duration_since(s.last_refill).as_secs_f64();
        s.tokens = (s.tokens + elapsed * rate_per_sec).min(capacity);
        s.last_refill = now;
        if s.tokens >= 1.0 {
            s.tokens -= 1.0;
            true
        } else {
            false
        }
    }

    pub fn config(&self) -> RateLimitConfig {
        self.cfg
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initial_tokens_allow_n_bundles() {
        let rl = RateLimit::new(RateLimitConfig {
            bundles_per_minute: 5,
        });
        for _ in 0..5 {
            assert!(rl.try_acquire());
        }
        // 6th within the same second is refused.
        assert!(!rl.try_acquire());
    }

    #[test]
    fn tokens_refill_over_time() {
        // 60 bundles per minute = 1 per second. We can consume
        // 60, then after waiting 1s we can consume 1 more.
        let rl = RateLimit::new(RateLimitConfig {
            bundles_per_minute: 60,
        });
        // Consume all 60.
        for _ in 0..60 {
            assert!(rl.try_acquire());
        }
        // 61st within the same instant is refused.
        assert!(!rl.try_acquire());
        // Sleep 1.1s; we should be able to consume ~1 more.
        std::thread::sleep(std::time::Duration::from_millis(1100));
        assert!(rl.try_acquire());
    }
}
