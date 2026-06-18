//! Dry-run end-to-end pipeline (Phase 7 / plan 01, AC-5 closure).
//!
//! `run_dry_run` is a decode-only path that doesn't feed its
//! decoded pools through detection/simulation. To produce a
//! real ledger (≥1 entry) from a dry-run, this module builds a
//! synthetic triangle of pools and runs them through the
//! recon pipeline.
//!
//! The synthetic triangle is a USDC/SOL/USDT universe with
//! deliberately asymmetric reserves so the detection graph
//! finds a 3-leg cycle. Whether the cycle's `WouldTrade` flag
//! is `true` depends on the conservative bound (default
//! `FailedCostModel::Spam` may eat the edge), but the
//! detection side is deterministic and always finds the
//! cycle.

use std::io::Write;

use dl_ledger::LedgerWriter;
use dl_recon::pipeline::{replay_pools_to_ledger, ReconReport, ReplayParams};
use dl_state::pool::{AmmKind, Pool};
use dl_state::Pubkey;
use tracing::info;

/// Result of a synthetic dry-run: the report + how many entries
/// were written to the ledger.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DryRunLedger {
    pub report: ReconReport,
    pub entries_written: usize,
}

/// Build the canonical synthetic triangle of pools.
///
/// Reserves are sized so a 3-leg cycle (USDC → SOL → USDT → USDC)
/// has a price edge: 100 × 105 × 1.001 = 1.051, i.e., 1 USDC
/// invested returns ~1.051 USDC, before fees and the
/// conservative haircut. The `WouldTrade` decision depends on
/// the conservative bound's parameters, but the cycle is
/// always detected.
pub fn synth_triangle_pools() -> Vec<Pool> {
    let usdc = [0x01u8; 32];
    let sol = [0x02u8; 32];
    let usdt = [0x03u8; 32];
    vec![
        // Pool 1: USDC/SOL — 100 USDC worth 1 SOL.
        // base = USDC (6 dec), quote = SOL (9 dec).
        Pool {
            address: Pubkey([0xA1u8; 32]),
            kind: AmmKind::RaydiumAmmV4,
            base_mint: Pubkey(usdc),
            quote_mint: Pubkey(sol),
            base_decimals: 6,
            quote_decimals: 9,
            base_reserve: 100_000_000,    // 100 USDC
            quote_reserve: 1_000_000_000, // 1 SOL
            fee_bps: 30,
            last_update_slot: 0,
        },
        // Pool 2: SOL/USDT — 1 SOL = 105 USDT.
        // base = SOL (9 dec), quote = USDT (6 dec).
        Pool {
            address: Pubkey([0xA2u8; 32]),
            kind: AmmKind::RaydiumAmmV4,
            base_mint: Pubkey(sol),
            quote_mint: Pubkey(usdt),
            base_decimals: 9,
            quote_decimals: 6,
            base_reserve: 1_000_000_000, // 1 SOL
            quote_reserve: 105_000_000,  // 105 USDT
            fee_bps: 30,
            last_update_slot: 0,
        },
        // Pool 3: USDT/USDC — 1 USDC = 1.001 USDT.
        // base = USDT (6 dec), quote = USDC (6 dec).
        Pool {
            address: Pubkey([0xA3u8; 32]),
            kind: AmmKind::RaydiumAmmV4,
            base_mint: Pubkey(usdt),
            quote_mint: Pubkey(usdc),
            base_decimals: 6,
            quote_decimals: 6,
            base_reserve: 1_001_000_000_000, // 1.001M USDT
            quote_reserve: 1_000_000_000,    // 1M USDC
            fee_bps: 30,
            last_update_slot: 0,
        },
    ]
}

/// Build the synthetic triangle, run detection + simulation,
/// and write every detected cycle's `LedgerEntry` to `w`.
///
/// Returns the number of entries written. The function is
/// deterministic: same input → same number of entries, same
/// hash.
///
/// Metrics emission (Task 5): emits a `tracing::info!` event
/// with stable field names (`cycles_evaluated`, `would_trade`,
/// `total_tip_lamports`, `report_hash`) so a downstream log
/// scraper can ingest the dry-run outcome.
pub fn write_synth_ledger<W: Write>(
    w: &mut LedgerWriter<W>,
) -> Result<DryRunLedger, Box<dyn std::error::Error>> {
    let pools = synth_triangle_pools();
    let params = ReplayParams::default();
    let report = replay_pools_to_ledger(&pools, &params)?;

    let cycles_evaluated = report.cycle_records.len() as u64;
    let would_trade = report.would_trade();
    let total_tip_lamports = report.total_tip_lamports;
    info!(
        cycles_evaluated,
        would_trade,
        total_tip_lamports,
        report_hash = report.report_hash,
        "synth dry-run: cycle detection complete"
    );

    let mut written = 0usize;
    for record in &report.cycle_records {
        w.write_entry(&record.entry)?;
        written += 1;
    }
    info!(written, "synth dry-run: ledger entries written");
    Ok(DryRunLedger {
        report,
        entries_written: written,
    })
}

/// Build the synthetic triangle report without writing a
/// ledger. Useful for tests that just want the report.
pub fn synth_report() -> Result<ReconReport, Box<dyn std::error::Error>> {
    let pools = synth_triangle_pools();
    let params = ReplayParams::default();
    Ok(replay_pools_to_ledger(&pools, &params)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn synth_triangle_has_three_pools() {
        let pools = synth_triangle_pools();
        assert_eq!(pools.len(), 3);
        assert_eq!(pools[0].base_mint.0, [0x01u8; 32]);
        assert_eq!(pools[1].base_mint.0, [0x02u8; 32]);
        assert_eq!(pools[2].base_mint.0, [0x03u8; 32]);
    }

    #[test]
    fn synth_report_finds_at_least_one_cycle() {
        let r = synth_report().expect("synth_report");
        assert!(
            !r.cycle_records.is_empty(),
            "synth triangle must produce ≥1 cycle"
        );
    }

    #[test]
    fn synth_report_is_deterministic() {
        let a = synth_report().expect("a");
        let b = synth_report().expect("b");
        assert_eq!(a.report_hash, b.report_hash);
        assert_eq!(a.cycle_records.len(), b.cycle_records.len());
    }

    #[test]
    fn synth_dry_run_writes_cycles_to_ledger() {
        let mut buf: Vec<u8> = Vec::new();
        let mut w = LedgerWriter::new(&mut buf).expect("writer");
        let result = write_synth_ledger(&mut w).expect("synth");
        assert!(
            result.entries_written > 0,
            "must write ≥1 entry, got {}",
            result.entries_written
        );
        drop(w);

        // Read it back.
        let mut r = dl_ledger::LedgerReader::open(buf.as_slice()).expect("reader");
        let mut count = 0;
        while let Some(_e) = r.read_entry().expect("read") {
            count += 1;
        }
        assert_eq!(count, result.entries_written);
    }
}
