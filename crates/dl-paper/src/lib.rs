//! `dl-paper` — Phase 9 paper-trading wallet.
//!
//! A persistent JSON-backed fake wallet for the live paper
//! trader. Records every "would trade" cycle as a `Trade`,
//! debits cost + tip, credits the conservative-bound PnL,
//! writes the file atomically (tmp + rename).
//!
//! The trade is recorded at the **conservative bound**, not
//! the optimistic bound. The conservative bound is the
//! project's stated trade-gate threshold (per
//! `EvalOutcome::decision = WouldTrade`); reusing it keeps
//! the wallet number honest and matches the project's
//! "model losing first" principle.

use std::path::{Path, PathBuf};

use chrono::Utc;
use serde::{Deserialize, Serialize};
use thiserror::Error;

pub const DEFAULT_STARTING_BALANCE_LAMPORTS: u64 = 10_000_000_000; // 10 SOL

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Side {
    BaseToQuote,
    QuoteToBase,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Trade {
    pub id: u64,
    pub ts_unix_ms: i64,
    pub pair: String,
    pub side: Side,
    pub input_lamports: u64,
    pub output_lamports: u64,
    pub profit_lamports: i64,
    pub tip_lamports: u64,
    pub balance_after_lamports: u64,
    pub cycle_hash_hex: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PaperWallet {
    pub balance_lamports: u64,
    pub starting_balance_lamports: u64,
    pub trades: Vec<Trade>,
    pub updated_at_unix_ms: i64,
}

impl PaperWallet {
    pub fn new(starting_balance_lamports: u64) -> Self {
        Self {
            balance_lamports: starting_balance_lamports,
            starting_balance_lamports,
            trades: Vec::new(),
            updated_at_unix_ms: Utc::now().timestamp_millis(),
        }
    }

    pub fn load(path: &Path) -> Result<Self, PaperError> {
        let s = std::fs::read_to_string(path).map_err(|e| PaperError::Io {
            path: path.to_path_buf(),
            source: e,
        })?;
        serde_json::from_str(&s).map_err(|e| PaperError::Parse {
            path: path.to_path_buf(),
            source: e,
        })
    }

    pub fn save(&self, path: &Path) -> Result<(), PaperError> {
        // Atomic write: write to tmp, fsync, rename.
        // Reject paths with `..` to avoid path-traversal.
        validate_path(path)?;
        let tmp = path.with_extension("json.tmp");
        let bytes = serde_json::to_vec_pretty(self).map_err(|e| PaperError::Serialize(e))?;
        std::fs::write(&tmp, &bytes).map_err(|e| PaperError::Io {
            path: tmp.clone(),
            source: e,
        })?;
        std::fs::rename(&tmp, path).map_err(|e| PaperError::AtomicWriteFailed {
            from: tmp,
            to: path.to_path_buf(),
            source: e,
        })?;
        Ok(())
    }

    /// Execute a fill: debit cost + tip, credit output, append
    /// a Trade. Returns the appended trade's id. Errors if
    /// the wallet has insufficient funds.
    pub fn execute(&mut self, fill: TradeFill) -> Result<u64, PaperError> {
        // The caller passes `profit_lamports` (i64) which is
        // output - input - tip already, so:
        //   new_balance = old_balance + profit_lamports
        if (fill.profit_lamports < 0)
            && (fill.profit_lamports.unsigned_abs() as u64) > self.balance_lamports
        {
            return Err(PaperError::InsufficientFunds {
                needed: fill.profit_lamports.unsigned_abs(),
                available: self.balance_lamports,
            });
        }
        let id = self.trades.len() as u64;
        let balance_after = if fill.profit_lamports >= 0 {
            self.balance_lamports.saturating_add(fill.profit_lamports as u64)
        } else {
            self.balance_lamports.saturating_sub(fill.profit_lamports.unsigned_abs())
        };
        let ts = Utc::now().timestamp_millis();
        self.trades.push(Trade {
            id,
            ts_unix_ms: ts,
            pair: fill.pair,
            side: fill.side,
            input_lamports: fill.input_lamports,
            output_lamports: fill.output_lamports,
            profit_lamports: fill.profit_lamports,
            tip_lamports: fill.tip_lamports,
            balance_after_lamports: balance_after,
            cycle_hash_hex: fill.cycle_hash_hex,
        });
        self.balance_lamports = balance_after;
        self.updated_at_unix_ms = ts;
        Ok(id)
    }

    pub fn stats(&self) -> WalletStats {
        let mut wins = 0u64;
        let mut losses = 0u64;
        let mut total_pnl: i64 = 0;
        let mut peak = self.starting_balance_lamports;
        let mut max_dd: u64 = 0;
        let mut running = self.starting_balance_lamports;
        for t in &self.trades {
            if t.profit_lamports > 0 {
                wins += 1;
            } else if t.profit_lamports < 0 {
                losses += 1;
            }
            total_pnl = total_pnl.saturating_add(t.profit_lamports);
            running = if t.profit_lamports >= 0 {
                running.saturating_add(t.profit_lamports as u64)
            } else {
                running.saturating_sub(t.profit_lamports.unsigned_abs())
            };
            if running > peak {
                peak = running;
            }
            let dd = peak.saturating_sub(running);
            if dd > max_dd {
                max_dd = dd;
            }
        }
        WalletStats {
            current_balance_lamports: self.balance_lamports,
            starting_balance_lamports: self.starting_balance_lamports,
            total_trades: self.trades.len() as u64,
            wins,
            losses,
            total_pnl_lamports: total_pnl,
            max_drawdown_lamports: max_dd,
            peak_balance_lamports: peak,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TradeFill {
    pub pair: String,
    pub side: Side,
    pub input_lamports: u64,
    pub output_lamports: u64,
    pub profit_lamports: i64,
    pub tip_lamports: u64,
    pub cycle_hash_hex: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WalletStats {
    pub current_balance_lamports: u64,
    pub starting_balance_lamports: u64,
    pub total_trades: u64,
    pub wins: u64,
    pub losses: u64,
    pub total_pnl_lamports: i64,
    pub max_drawdown_lamports: u64,
    pub peak_balance_lamports: u64,
}

#[derive(Debug, Error)]
pub enum PaperError {
    #[error("I/O error on {path:?}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("parse error on {path:?}: {source}")]
    Parse {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
    #[error("serialize error: {0}")]
    Serialize(#[source] serde_json::Error),
    #[error("insufficient funds: need {needed} lamports, have {available}")]
    InsufficientFunds { needed: u64, available: u64 },
    #[error("path rejected: {0:?} contains '..' or is absolute")]
    PathRejected(PathBuf),
    #[error("atomic write failed: {from:?} -> {to:?}: {source}")]
    AtomicWriteFailed {
        from: PathBuf,
        to: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

fn validate_path(path: &Path) -> Result<(), PaperError> {
    if path.components().any(|c| matches!(c, std::path::Component::ParentDir)) {
        return Err(PaperError::PathRejected(path.to_path_buf()));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_fill(profit: i64) -> TradeFill {
        TradeFill {
            pair: "SOL/USDC".to_string(),
            side: Side::BaseToQuote,
            input_lamports: 1_000_000,
            output_lamports: 1_010_000,
            profit_lamports: profit,
            tip_lamports: 10_000,
            cycle_hash_hex: "abcd".to_string(),
        }
    }

    #[test]
    fn new_wallet_starts_at_balance() {
        let w = PaperWallet::new(10_000_000_000);
        assert_eq!(w.balance_lamports, 10_000_000_000);
        assert_eq!(w.trades.len(), 0);
    }

    #[test]
    fn execute_appends_trade_and_updates_balance() {
        let mut w = PaperWallet::new(10_000_000_000);
        let id = w.execute(sample_fill(50_000)).unwrap();
        assert_eq!(id, 0);
        assert_eq!(w.balance_lamports, 10_000_000_000 + 50_000);
        assert_eq!(w.trades.len(), 1);
    }

    #[test]
    fn execute_loss_debits_balance() {
        let mut w = PaperWallet::new(10_000_000_000);
        w.execute(sample_fill(-100_000)).unwrap();
        assert_eq!(w.balance_lamports, 10_000_000_000 - 100_000);
    }

    #[test]
    fn execute_insufficient_funds_errors() {
        let mut w = PaperWallet::new(100);
        let r = w.execute(sample_fill(-200));
        assert!(matches!(r, Err(PaperError::InsufficientFunds { .. })));
    }

    #[test]
    fn stats_track_wins_losses_pnl_drawdown() {
        let mut w = PaperWallet::new(10_000_000_000);
        w.execute(sample_fill(100)).unwrap();
        w.execute(sample_fill(-50)).unwrap();
        w.execute(sample_fill(200)).unwrap();
        w.execute(sample_fill(-1_000_000_000)).unwrap(); // big loss
        let s = w.stats();
        assert_eq!(s.total_trades, 4);
        assert_eq!(s.wins, 2);
        assert_eq!(s.losses, 2);
        assert_eq!(s.total_pnl_lamports, 100 - 50 + 200 - 1_000_000_000);
        assert!(s.max_drawdown_lamports > 0);
    }

    #[test]
    fn path_validation_rejects_parent_dir() {
        let bad = std::path::PathBuf::from("..");
        assert!(matches!(validate_path(&bad), Err(PaperError::PathRejected(_))));
    }
}
