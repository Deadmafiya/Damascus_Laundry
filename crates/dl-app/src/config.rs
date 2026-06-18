//! Engine-wide configuration (Phase 7 / plan 01).
//!
//! `EngineConfig` is the single source of truth for every tunable
//! parameter in the engine: `EvalParams` (probabilities, latency,
//! landing, failed-cost), `CostModel` (signature/CU/Jito tip),
//! capture settings (RPC URL, capture path, capture seconds), and
//! recon settings (anchor path, calibrate-on/off). All fields are
//! overridable via TOML or environment variables; no recompile
//! required to retune the engine.
//!
//! ## Loading precedence
//!
//! 1. Defaults from `EvalParams::conservative_default()` and friends.
//! 2. Override from TOML file (path via `DL_ENGINE_CONFIG`).
//! 3. Override from environment variables (one var per field,
//!    prefixed with `DL_`).
//! 4. Missing fields fall back to the conservative default. Invalid
//!    values return a typed `ConfigError` rather than panicking.
//!
//! ## Integer-only
//!
//! All values are integer (no `f64`). The one place floats are
//! permitted is `dl-core::display`; `EngineConfig` does not touch
//! `display` at all.

use std::fs;
use std::path::Path;

use dl_sim::cost::CostModel;
use dl_sim::ev::{
    CompetitionParams, EvalParams, FailedCostModel, LandingParams, LatencyBudget, Prob, SubmitPath,
};
use serde::{Deserialize, Serialize};

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("toml: {0}")]
    Toml(#[from] toml::de::Error),
    #[error("invalid value for {field}: {message}")]
    Invalid { field: String, message: String },
}

/// Top-level config. Flatten the sub-configs so a single TOML file
/// holds everything.
///
/// `Default` is implemented manually (not derived) because the
/// eval/cost sub-configs need to be initialized from
/// `EvalParams::conservative_default()` and
/// `CostModel::default_busy()` — not the zero values that
/// `#[derive(Default)]` would produce. The same logic is reused
/// for the `#[serde(default)]` paths so partial TOML overrides
/// don't silently zero out the eval/cost config.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EngineConfig {
    #[serde(default = "default_eval_config")]
    pub eval: EvalConfig,
    #[serde(default = "default_cost_config")]
    pub cost: CostConfig,
    #[serde(default)]
    pub capture: CaptureConfig,
    #[serde(default)]
    pub recon: ReconConfig,
}

fn default_eval_config() -> EvalConfig {
    // Inline the relevant fields from the conservative default.
    let ep = EvalParams::conservative_default();
    let cp = ep.competition.clone();
    let lb = ep.latency.clone();
    let lp = ep.landing.clone();
    let fc = ep.failed.clone();
    EvalConfig {
        p_detect_ppm: ep.p_detect.to_ppm(),
        base_win_ppm: cp.base_win_ppm,
        richness_threshold_bps: cp.richness_threshold_bps,
        decay_ppm_per_bps: cp.decay_ppm_per_bps,
        t_detect_ms: lb.t_detect_ms,
        t_decide_ms: lb.t_decide_ms,
        t_build_ms: lb.t_build_ms,
        t_network_ms: lb.t_network_ms,
        t_auction_ms: lb.t_auction_ms,
        p_land_grace_ms: lp.grace_ms,
        p_land_decay_ppm_per_ms: lp.decay_ppm_per_ms,
        failed_attempts_per_win: fc.attempts_per_win,
        failed_per_attempt_lamports: fc.per_attempt_lamports,
        submit_path: match fc.path {
            SubmitPath::Spam => SubmitPathTag::Spam,
            SubmitPath::JitoBundle => SubmitPathTag::JitoBundle,
        },
    }
}

fn default_cost_config() -> CostConfig {
    let cm = CostModel::default_busy();
    CostConfig {
        n_signatures: u32::from(cm.n_signatures),
        cu_limit: cm.cu_limit,
        cu_price_micro_lamports: cm.cu_price_micro_lamports,
        jito_tip_lamports: cm.jito_tip_lamports,
    }
}

/// `EvalParams` overrides. Mirrors the `EvalParams` struct but with
/// `u32` / `u64` / `i32` types so it can round-trip through TOML.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct EvalConfig {
    /// `p_detect` in ppm (0..=1_000_000).
    pub p_detect_ppm: u32,
    /// `CompetitionParams.base_win_ppm`.
    pub base_win_ppm: u32,
    /// `CompetitionParams.richness_threshold_bps` (signed: negative
    /// means threshold is below zero, decays from origin).
    pub richness_threshold_bps: i32,
    /// `CompetitionParams.decay_ppm_per_bps`.
    pub decay_ppm_per_bps: u32,
    /// `LatencyBudget.t_detect_ms`.
    pub t_detect_ms: u32,
    /// `LatencyBudget.t_decide_ms`.
    pub t_decide_ms: u32,
    /// `LatencyBudget.t_build_ms`.
    pub t_build_ms: u32,
    /// `LatencyBudget.t_network_ms`.
    pub t_network_ms: u32,
    /// `LatencyBudget.t_auction_ms` (Jito relayer speed bump).
    pub t_auction_ms: u32,
    /// `LandingParams.grace_ms` (latency budget under which
    /// `p_land == 1.0`).
    pub p_land_grace_ms: u32,
    /// `LandingParams.decay_ppm_per_ms` (landing-probability decay
    /// per ms of latency above grace).
    pub p_land_decay_ppm_per_ms: u32,
    /// `FailedCostModel.attempts_per_win`.
    pub failed_attempts_per_win: u32,
    /// `FailedCostModel.per_attempt_lamports` (spam path: base sig fee).
    pub failed_per_attempt_lamports: u64,
    /// Submit path: `spam` or `jito_bundle`.
    pub submit_path: SubmitPathTag,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SubmitPathTag {
    Spam,
    JitoBundle,
}

impl Default for SubmitPathTag {
    fn default() -> Self {
        SubmitPathTag::Spam
    }
}

impl From<SubmitPathTag> for SubmitPath {
    fn from(t: SubmitPathTag) -> Self {
        match t {
            SubmitPathTag::Spam => SubmitPath::Spam,
            SubmitPathTag::JitoBundle => SubmitPath::JitoBundle,
        }
    }
}

/// `CostModel` overrides.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct CostConfig {
    /// `CostModel.n_signatures` (u16 in dl-sim, max 65_535; clamped
    /// on `cost_model()`).
    pub n_signatures: u32,
    /// `CostModel.cu_limit` (compute units per cycle).
    pub cu_limit: u32,
    /// `CostModel.cu_price_micro_lamports` (price per CU, micro-lamports).
    pub cu_price_micro_lamports: u64,
    /// `CostModel.jito_tip_lamports` (0 if spam path).
    pub jito_tip_lamports: u64,
}

impl Default for CaptureConfig {
    fn default() -> Self {
        Self {
            rpc_url: String::new(),
            capture_path: String::new(),
            capture_secs: 60,
            test_pool_pubkey: String::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct CaptureConfig {
    /// WebSocket RPC URL. Empty means "use env var DL_RPC_URL".
    pub rpc_url: String,
    /// Capture file path. Empty disables capture-to-disk.
    pub capture_path: String,
    /// Capture duration in seconds.
    pub capture_secs: u64,
    /// Optional pool pubkey to subscribe to (base58). Empty means
    /// "no single-pool subscription, only slot updates".
    pub test_pool_pubkey: String,
}

#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ReconConfig {
    /// Anchor dataset path. Empty disables recon.
    pub anchor_path: String,
    /// When true, run `calibrate()` after `compare()`.
    pub calibrate: bool,
}

impl Default for EngineConfig {
    fn default() -> Self {
        // The eval/cost sub-configs pull from dl-sim's conservative
        // defaults; capture/recon use their `Default` impls (zeros
        // for the most part).
        let ep = EvalParams::conservative_default();
        let cm = CostModel::default_busy();
        let cp = ep.competition.clone();
        let lb = ep.latency.clone();
        let lp = ep.landing.clone();
        let fc = ep.failed.clone();
        Self {
            eval: EvalConfig {
                p_detect_ppm: ep.p_detect.to_ppm(),
                base_win_ppm: cp.base_win_ppm,
                richness_threshold_bps: cp.richness_threshold_bps,
                decay_ppm_per_bps: cp.decay_ppm_per_bps,
                t_detect_ms: lb.t_detect_ms,
                t_decide_ms: lb.t_decide_ms,
                t_build_ms: lb.t_build_ms,
                t_network_ms: lb.t_network_ms,
                t_auction_ms: lb.t_auction_ms,
                p_land_grace_ms: lp.grace_ms,
                p_land_decay_ppm_per_ms: lp.decay_ppm_per_ms,
                failed_attempts_per_win: fc.attempts_per_win,
                failed_per_attempt_lamports: fc.per_attempt_lamports,
                submit_path: match fc.path {
                    SubmitPath::Spam => SubmitPathTag::Spam,
                    SubmitPath::JitoBundle => SubmitPathTag::JitoBundle,
                },
            },
            cost: CostConfig {
                n_signatures: u32::from(cm.n_signatures),
                cu_limit: cm.cu_limit,
                cu_price_micro_lamports: cm.cu_price_micro_lamports,
                jito_tip_lamports: cm.jito_tip_lamports,
            },
            capture: CaptureConfig::default(),
            recon: ReconConfig::default(),
        }
    }
}

impl EngineConfig {
    /// Load from a TOML file, then apply env-var overrides.
    pub fn load(path: &Path) -> Result<Self, ConfigError> {
        let mut cfg = if path.exists() {
            let text = fs::read_to_string(path)?;
            toml::from_str::<Self>(&text).map_err(ConfigError::Toml)?
        } else {
            Self::default()
        };
        cfg.apply_env()?;
        Ok(cfg)
    }

    /// Apply overrides from `DL_*` environment variables. Every
    /// field has a corresponding env var; missing vars leave the
    /// field unchanged.
    pub fn apply_env(&mut self) -> Result<(), ConfigError> {
        use std::env;
        if let Ok(v) = env::var("DL_P_DETECT_PPM") {
            self.eval.p_detect_ppm = parse_u32("DL_P_DETECT_PPM", &v)?;
        }
        if let Ok(v) = env::var("DL_BASE_WIN_PPM") {
            self.eval.base_win_ppm = parse_u32("DL_BASE_WIN_PPM", &v)?;
        }
        if let Ok(v) = env::var("DL_RICHNESS_THRESHOLD_BPS") {
            self.eval.richness_threshold_bps = parse_i32("DL_RICHNESS_THRESHOLD_BPS", &v)?;
        }
        if let Ok(v) = env::var("DL_DECAY_PPM_PER_BPS") {
            self.eval.decay_ppm_per_bps = parse_u32("DL_DECAY_PPM_PER_BPS", &v)?;
        }
        if let Ok(v) = env::var("DL_T_DETECT_MS") {
            self.eval.t_detect_ms = parse_u32("DL_T_DETECT_MS", &v)?;
        }
        if let Ok(v) = env::var("DL_T_DECIDE_MS") {
            self.eval.t_decide_ms = parse_u32("DL_T_DECIDE_MS", &v)?;
        }
        if let Ok(v) = env::var("DL_T_BUILD_MS") {
            self.eval.t_build_ms = parse_u32("DL_T_BUILD_MS", &v)?;
        }
        if let Ok(v) = env::var("DL_T_NETWORK_MS") {
            self.eval.t_network_ms = parse_u32("DL_T_NETWORK_MS", &v)?;
        }
        if let Ok(v) = env::var("DL_T_AUCTION_MS") {
            self.eval.t_auction_ms = parse_u32("DL_T_AUCTION_MS", &v)?;
        }
        if let Ok(v) = env::var("DL_P_LAND_GRACE_MS") {
            self.eval.p_land_grace_ms = parse_u32("DL_P_LAND_GRACE_MS", &v)?;
        }
        if let Ok(v) = env::var("DL_P_LAND_DECAY_PPM_PER_MS") {
            self.eval.p_land_decay_ppm_per_ms = parse_u32("DL_P_LAND_DECAY_PPM_PER_MS", &v)?;
        }
        if let Ok(v) = env::var("DL_FAILED_ATTEMPTS_PER_WIN") {
            self.eval.failed_attempts_per_win = parse_u32("DL_FAILED_ATTEMPTS_PER_WIN", &v)?;
        }
        if let Ok(v) = env::var("DL_FAILED_PER_ATTEMPT_LAMPORTS") {
            self.eval.failed_per_attempt_lamports =
                parse_u64("DL_FAILED_PER_ATTEMPT_LAMPORTS", &v)?;
        }
        if let Ok(v) = env::var("DL_SUBMIT_PATH") {
            self.eval.submit_path = match v.to_ascii_lowercase().as_str() {
                "spam" => SubmitPathTag::Spam,
                "jito_bundle" | "jitobundle" | "bundle" => SubmitPathTag::JitoBundle,
                other => {
                    return Err(ConfigError::Invalid {
                        field: "DL_SUBMIT_PATH".to_string(),
                        message: format!("expected 'spam' or 'jito_bundle', got {other:?}"),
                    });
                }
            };
        }
        if let Ok(v) = env::var("DL_N_SIGNATURES") {
            self.cost.n_signatures = parse_u32("DL_N_SIGNATURES", &v)?;
        }
        if let Ok(v) = env::var("DL_CU_LIMIT") {
            self.cost.cu_limit = parse_u32("DL_CU_LIMIT", &v)?;
        }
        if let Ok(v) = env::var("DL_CU_PRICE_MICRO_LAMPORTS") {
            self.cost.cu_price_micro_lamports = parse_u64("DL_CU_PRICE_MICRO_LAMPORTS", &v)?;
        }
        if let Ok(v) = env::var("DL_JITO_TIP_LAMPORTS") {
            self.cost.jito_tip_lamports = parse_u64("DL_JITO_TIP_LAMPORTS", &v)?;
        }
        if let Ok(v) = env::var("DL_RPC_URL") {
            self.capture.rpc_url = v;
        }
        if let Ok(v) = env::var("DL_CAPTURE_PATH") {
            self.capture.capture_path = v;
        }
        if let Ok(v) = env::var("DL_CAPTURE_SECS") {
            self.capture.capture_secs = parse_u64("DL_CAPTURE_SECS", &v)?;
        }
        if let Ok(v) = env::var("DL_TEST_POOL_PUBKEY") {
            self.capture.test_pool_pubkey = v;
        }
        if let Ok(v) = env::var("DL_RECON_ANCHOR_PATH") {
            self.recon.anchor_path = v;
        }
        if let Ok(v) = env::var("DL_RECON_CALIBRATE") {
            self.recon.calibrate = match v.as_str() {
                "1" | "true" | "TRUE" | "yes" => true,
                "0" | "false" | "FALSE" | "no" => false,
                other => {
                    return Err(ConfigError::Invalid {
                        field: "DL_RECON_CALIBRATE".to_string(),
                        message: format!("expected 0/1/true/false/yes/no, got {other:?}"),
                    });
                }
            };
        }
        Ok(())
    }

    /// Build a `CostModel` from this config. Saturates
    /// `n_signatures` into `u16` (max 65_535).
    pub fn cost_model(&self) -> CostModel {
        CostModel {
            n_signatures: self.cost.n_signatures.min(u16::MAX as u32) as u16,
            cu_limit: self.cost.cu_limit,
            cu_price_micro_lamports: self.cost.cu_price_micro_lamports,
            jito_tip_lamports: self.cost.jito_tip_lamports,
        }
    }

    /// Build an `EvalParams` from this config.
    #[allow(non_snake_case)]
    pub fn eval_params(&self) -> EvalParams {
        EvalParams {
            p_detect: Prob::from_ppm(self.eval.p_detect_ppm)
                .unwrap_or_else(|_| EvalParams::conservative_default().p_detect),
            competition: CompetitionParams {
                base_win_ppm: self.eval.base_win_ppm,
                richness_threshold_bps: self.eval.richness_threshold_bps,
                decay_ppm_per_bps: self.eval.decay_ppm_per_bps,
            },
            latency: LatencyBudget {
                t_detect_ms: self.eval.t_detect_ms,
                t_decide_ms: self.eval.t_decide_ms,
                t_build_ms: self.eval.t_build_ms,
                t_network_ms: self.eval.t_network_ms,
                t_auction_ms: self.eval.t_auction_ms,
            },
            landing: LandingParams {
                grace_ms: self.eval.p_land_grace_ms,
                decay_ppm_per_ms: self.eval.p_land_decay_ppm_per_ms,
            },
            failed: FailedCostModel {
                attempts_per_win: self.eval.failed_attempts_per_win,
                per_attempt_lamports: self.eval.failed_per_attempt_lamports,
                path: self.eval.submit_path.into(),
            },
        }
    }
}

fn parse_u32(field: &str, v: &str) -> Result<u32, ConfigError> {
    v.parse::<u32>().map_err(|e| ConfigError::Invalid {
        field: field.to_string(),
        message: format!("not a u32: {e}"),
    })
}

fn parse_u64(field: &str, v: &str) -> Result<u64, ConfigError> {
    v.parse::<u64>().map_err(|e| ConfigError::Invalid {
        field: field.to_string(),
        message: format!("not a u64: {e}"),
    })
}

fn parse_i32(field: &str, v: &str) -> Result<i32, ConfigError> {
    v.parse::<i32>().map_err(|e| ConfigError::Invalid {
        field: field.to_string(),
        message: format!("not an i32: {e}"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_matches_conservative_default() {
        let cfg = EngineConfig::default();
        let ep = cfg.eval_params();
        let ref_ep = EvalParams::conservative_default();
        assert_eq!(ep.competition.base_win_ppm, ref_ep.competition.base_win_ppm);
        assert_eq!(ep.latency.t_auction_ms, ref_ep.latency.t_auction_ms);
        assert_eq!(ep.landing.grace_ms, ref_ep.landing.grace_ms);
    }

    #[test]
    fn eval_params_differs_from_default_when_overridden() {
        let mut cfg = EngineConfig::default();
        cfg.eval.base_win_ppm = 500_000;
        cfg.eval.p_detect_ppm = 900_000;
        cfg.eval.t_auction_ms = 50;
        cfg.eval.submit_path = SubmitPathTag::JitoBundle;
        cfg.cost.jito_tip_lamports = 100_000;
        cfg.eval.failed_attempts_per_win = 1;

        let ep = cfg.eval_params();
        assert_eq!(ep.competition.base_win_ppm, 500_000);
        assert_eq!(ep.p_detect.to_ppm(), 900_000);
        assert_eq!(ep.latency.t_auction_ms, 50);
        assert!(matches!(ep.failed.path, SubmitPath::JitoBundle));
        assert_eq!(cfg.cost_model().jito_tip_lamports, 100_000);
    }

    #[test]
    fn cost_model_round_trip() {
        let cfg = EngineConfig::default();
        let cm = cfg.cost_model();
        assert_eq!(u32::from(cm.n_signatures), cfg.cost.n_signatures);
        assert_eq!(cm.cu_limit, cfg.cost.cu_limit);
        assert_eq!(cm.cu_price_micro_lamports, cfg.cost.cu_price_micro_lamports);
        assert_eq!(cm.jito_tip_lamports, cfg.cost.jito_tip_lamports);
    }

    #[test]
    fn toml_round_trip() {
        let original = r#"
[eval]
p_detect_ppm = 800000
base_win_ppm = 250000
richness_threshold_bps = 20
decay_ppm_per_bps = 15000
t_detect_ms = 15
t_decide_ms = 5
t_build_ms = 5
t_network_ms = 75
t_auction_ms = 200
p_land_grace_ms = 50
p_land_decay_ppm_per_ms = 2000
failed_attempts_per_win = 24
failed_per_attempt_lamports = 5000
submit_path = "spam"

[cost]
n_signatures = 3
cu_limit = 200000
cu_price_micro_lamports = 1000
jito_tip_lamports = 10000

[capture]
rpc_url = "wss://api.mainnet-beta.solana.com"
capture_path = "/tmp/cap.bincode"
capture_secs = 120
test_pool_pubkey = ""

[recon]
anchor_path = "/tmp/anchors.jsonl"
calibrate = true
"#;
        let cfg: EngineConfig = toml::from_str(original).expect("parse");
        assert_eq!(cfg.eval.p_detect_ppm, 800_000);
        assert_eq!(cfg.eval.base_win_ppm, 250_000);
        assert_eq!(cfg.cost.n_signatures, 3);
        assert_eq!(cfg.cost.cu_limit, 200_000);
        assert_eq!(cfg.recon.calibrate, true);
        assert_eq!(cfg.capture.capture_secs, 120);

        // Round-trip back to TOML.
        let serialized = toml::to_string(&cfg).expect("serialize");
        let cfg2: EngineConfig = toml::from_str(&serialized).expect("reparse");
        assert_eq!(cfg, cfg2);
    }

    #[test]
    fn toml_partial_override_falls_back_to_default() {
        // Only [cost] provided; the rest should fall back to default.
        let toml_str = r#"
[cost]
n_signatures = 5
"#;
        let cfg: EngineConfig = toml::from_str(toml_str).expect("parse");
        assert_eq!(cfg.cost.n_signatures, 5);
        // eval fields stay at default.
        assert_eq!(cfg.eval.p_detect_ppm, 700_000);
    }

    #[test]
    fn invalid_env_var_returns_typed_error() {
        // Direct call to `parse_*` helpers with bad input.
        assert!(parse_u32("X", "abc").is_err());
        assert!(parse_u64("X", "abc").is_err());
        assert!(parse_i32("X", "abc").is_err());
    }

    #[test]
    fn cost_model_clamps_n_signatures_to_u16() {
        // dl-sim's n_signatures is u16; the config field is u32.
        // Values above 65535 must be clamped (not panic).
        let mut cfg = EngineConfig::default();
        cfg.cost.n_signatures = 100_000;
        let cm = cfg.cost_model();
        assert_eq!(cm.n_signatures, u16::MAX);
    }
}
