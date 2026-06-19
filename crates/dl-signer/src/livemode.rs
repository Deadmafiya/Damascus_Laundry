//! Live execution mode (Phase 8 / plan 03).
//!
//! The primary operational contract: the engine will NOT touch
//! the Jito Block Engine or any real mainnet RPC unless this
//! gate is explicitly enabled. The three modes are:
//!
//! - [`LiveMode::Refused`] (default): any live-mode invocation
//!   is refused at boot. The `dl-app run` subcommand exits
//!   non-zero before touching the network. This is the safe
//!   default — every other mode requires explicit operator
//!   opt-in.
//! - [`LiveMode::Devnet`]: the engine connects to the Solana
//!   **devnet** Jito Block Engine and submits paper bundles.
//!   Used in 08-03-a to prove the e2e pipeline works against
//!   a real Jito infrastructure without risking real SOL.
//! - [`LiveMode::MainnetPaper`]: the engine connects to the
//!   Solana **mainnet** Jito Block Engine with a hard cap of
//!   `0.001 SOL/day` (1,000,000 lamports). Used in 08-03-b to
//!   prove the wallet, signing, and tip-flow all work on
//!   mainnet.
//! - [`LiveMode::Mainnet`]: production. Cap = the configured
//!   `DL_DAILY_CAP_LAMPORTS` (default 5 SOL). Used in 08-03-c.
//!
//! ## Operator Runbook
//!
//! The mode is set by the env var `DL_LIVE_MODE`:
//!
//! - `DL_LIVE_MODE=` (empty): refused
//! - `DL_LIVE_MODE=devnet`: devnet
//! - `DL_LIVE_MODE=mainnet-paper`: mainnet, 0.001 SOL/day cap
//! - `DL_LIVE_MODE=mainnet`: mainnet, configured cap
//!
//! The default cap override for `mainnet-paper` is hard-coded
//! at 1,000,000 lamports (0.001 SOL). It cannot be raised via
//! env var; this is a deliberate safety floor. Operators who
//! want to override it must change the constant in source and
//! re-build.
//!
//! ## Why this is hard-coded
//!
//! A hot-wallet security model that allows the cap to be raised
//! via env var is not a security model — it's a misconfiguration
//! waiting to happen. The 0.001 SOL cap is the production
//! hard floor; the `DL_DAILY_CAP_LAMPORTS` env var is the upper
//! bound for the `mainnet` mode, not a way to defeat the floor.

use std::str::FromStr;

use thiserror::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LiveMode {
    /// Default. Any live-mode invocation is refused.
    Refused,
    /// Devnet. Connects to Solana devnet Jito Block Engine.
    Devnet,
    /// Mainnet with a hard-coded 0.001 SOL/day cap.
    MainnetPaper,
    /// Mainnet with the configured `DL_DAILY_CAP_LAMPORTS` cap.
    Mainnet,
}

impl LiveMode {
    /// Human-readable label.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Refused => "refused",
            Self::Devnet => "devnet",
            Self::MainnetPaper => "mainnet-paper",
            Self::Mainnet => "mainnet",
        }
    }

    /// Hard-coded cap for the mainnet-paper mode.
    /// 0.001 SOL = 1,000,000 lamports. This is the production
    /// safety floor; it cannot be raised.
    pub const MAINNET_PAPER_DAILY_CAP_LAMPORTS: u64 = 1_000_000;
}

impl Default for LiveMode {
    fn default() -> Self {
        Self::Refused
    }
}

#[derive(Debug, Error)]
pub enum LiveModeParseError {
    #[error("DL_LIVE_MODE is empty (refused). Set it to `devnet`, `mainnet-paper`, or `mainnet` to opt in.")]
    Empty,

    #[error("DL_LIVE_MODE={0:?} is not a valid mode. Valid: devnet, mainnet-paper, mainnet.")]
    Invalid(String),
}

impl Default for LiveModeParseError {
    fn default() -> Self {
        Self::Empty
    }
}

impl FromStr for LiveMode {
    type Err = LiveModeParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s.is_empty() {
            return Err(LiveModeParseError::Empty);
        }
        match s.to_ascii_lowercase().as_str() {
            "devnet" => Ok(Self::Devnet),
            "mainnet-paper" | "mainnet_paper" => Ok(Self::MainnetPaper),
            "mainnet" => Ok(Self::Mainnet),
            _ => Err(LiveModeParseError::Invalid(s.to_string())),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResolvedLiveMode {
    pub mode: LiveMode,
    /// The cap that will be applied. For `MainnetPaper`, this
    /// is the hard-coded 0.001 SOL floor regardless of the
    /// `DL_DAILY_CAP_LAMPORTS` env var.
    pub daily_cap_lamports: u64,
    /// The per-bundle cap (default 0.5 SOL).
    pub per_bundle_cap_lamports: u64,
}

impl ResolvedLiveMode {
    /// Resolve the live mode from env vars.
    ///
    /// - `DL_LIVE_MODE`: the mode (default: Refused).
    /// - `DL_DAILY_CAP_LAMPORTS`: the per-day cap (default
    ///   5_000_000_000 = 5 SOL). Ignored in `MainnetPaper`
    ///   mode (where the hard-coded 0.001 SOL cap wins).
    /// - `DL_PER_BUNDLE_CAP_LAMPORTS`: the per-bundle cap
    ///   (default 500_000_000 = 0.5 SOL).
    pub fn from_env() -> Result<Self, LiveModeParseError> {
        let mode = std::env::var("DL_LIVE_MODE")
            .ok()
            .as_deref()
            .map(LiveMode::from_str)
            .unwrap_or(Ok(LiveMode::default()))?;
        let per_bundle_cap = std::env::var("DL_PER_BUNDLE_CAP_LAMPORTS")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(500_000_000);
        let daily_cap = match mode {
            LiveMode::Refused | LiveMode::Devnet => std::env::var("DL_DAILY_CAP_LAMPORTS")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(5_000_000_000),
            // The hard floor: mainnet-paper ignores the env var.
            LiveMode::MainnetPaper => LiveMode::MAINNET_PAPER_DAILY_CAP_LAMPORTS,
            LiveMode::Mainnet => std::env::var("DL_DAILY_CAP_LAMPORTS")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(5_000_000_000),
        };
        Ok(Self {
            mode,
            daily_cap_lamports: daily_cap,
            per_bundle_cap_lamports: per_bundle_cap,
        })
    }

    /// True if this mode will refuse to boot. Refused mode is
    /// the only one that does (the others are explicit opt-ins).
    pub fn refuses(&self) -> bool {
        matches!(self.mode, LiveMode::Refused)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_refused() {
        assert_eq!(LiveMode::default(), LiveMode::Refused);
    }

    #[test]
    fn parse_empty_returns_empty_error() {
        let err = "".parse::<LiveMode>().unwrap_err();
        assert!(matches!(err, LiveModeParseError::Empty));
    }

    #[test]
    fn parse_devnet() {
        let m = "devnet".parse::<LiveMode>().unwrap();
        assert_eq!(m, LiveMode::Devnet);
    }

    #[test]
    fn parse_mainnet() {
        let m = "mainnet".parse::<LiveMode>().unwrap();
        assert_eq!(m, LiveMode::Mainnet);
    }

    #[test]
    fn parse_mainnet_paper_accepts_hyphen_and_underscore() {
        assert_eq!("mainnet-paper".parse::<LiveMode>().unwrap(), LiveMode::MainnetPaper);
        assert_eq!("mainnet_paper".parse::<LiveMode>().unwrap(), LiveMode::MainnetPaper);
    }

    #[test]
    fn parse_case_insensitive() {
        assert_eq!("DEVNET".parse::<LiveMode>().unwrap(), LiveMode::Devnet);
        assert_eq!("MainNet".parse::<LiveMode>().unwrap(), LiveMode::Mainnet);
    }

    #[test]
    fn parse_invalid_returns_invalid_error() {
        let err = "garbage".parse::<LiveMode>().unwrap_err();
        assert!(matches!(err, LiveModeParseError::Invalid(_)));
    }

    #[test]
    fn mainnet_paper_cap_is_hard_coded() {
        assert_eq!(LiveMode::MAINNET_PAPER_DAILY_CAP_LAMPORTS, 1_000_000);
    }

    #[test]
    fn refused_mode_refuses() {
        let r = ResolvedLiveMode {
            mode: LiveMode::Refused,
            daily_cap_lamports: 5_000_000_000,
            per_bundle_cap_lamports: 500_000_000,
        };
        assert!(r.refuses());
    }

    #[test]
    fn devnet_mode_does_not_refuse() {
        let r = ResolvedLiveMode {
            mode: LiveMode::Devnet,
            daily_cap_lamports: 5_000_000_000,
            per_bundle_cap_lamports: 500_000_000,
        };
        assert!(!r.refuses());
    }
}
