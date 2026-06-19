//! Append-only writer for the paper ledger.
//!
//! Mirrors `dl-feed::capture::CaptureWriter` in shape:
//!   - `new(sink)` writes the file header (magic + schema version) immediately.
//!   - `write_entry(&mut self, &entry)` serializes a `LedgerEntry` and writes
//!     a length-prefixed bincode frame.
//!   - `into_inner()` flushes and returns the underlying sink.
//!   - No terminator frame is written; EOF on the reader signals end.
//!
//! See `format::format_spec` for the byte-level layout. The
//! `tests/ledger_roundtrip.rs` test keeps the spec and the impl in lock-step.

use std::io::Write;

use crate::entry::LedgerEntry;
use crate::error::LedgerError;
use crate::format::{LEDGER_MAGIC, LEDGER_SCHEMA_VERSION};

/// Maximum payload size the writer will accept. A single `LedgerEntry`
/// is on the order of a few hundred bytes; 16 MiB is a defensive
/// ceiling that still fits comfortably in `u32` and rejects obviously
/// corrupt inputs.
pub const MAX_ENTRY_BYTES: usize = 16 * 1024 * 1024;

/// Append-only ledger writer.
pub struct LedgerWriter<W: Write> {
    sink: W,
    frames_written: u64,
}

impl<W: Write> LedgerWriter<W> {
    /// Create a new writer. Writes the magic + schema version to
    /// `sink` immediately. Returns `Io` on write failure.
    pub fn new(mut sink: W) -> Result<Self, LedgerError> {
        sink.write_all(LEDGER_MAGIC)?;
        sink.write_all(&LEDGER_SCHEMA_VERSION.to_le_bytes())?;
        Ok(Self {
            sink,
            frames_written: 0,
        })
    }

    /// Write one entry. Bincode-serializes the entry, prefixes it with
    /// a u32 LE byte count, and increments `frames_written`.
    pub fn write_entry(&mut self, entry: &LedgerEntry) -> Result<(), LedgerError> {
        let bytes = bincode::serialize(entry)?;
        if bytes.len() > MAX_ENTRY_BYTES {
            return Err(LedgerError::Truncated); // reuse: payload is implausibly large
        }
        let len = bytes.len() as u32;
        self.sink.write_all(&len.to_le_bytes())?;
        self.sink.write_all(&bytes)?;
        self.frames_written += 1;
        Ok(())
    }

    /// Number of entries written so far.
    pub fn frames_written(&self) -> u64 {
        self.frames_written
    }

    /// Flush the sink and return it. Returns `Io` if the flush fails.
    pub fn into_inner(mut self) -> Result<W, LedgerError> {
        self.sink.flush()?;
        Ok(self.sink)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entry::Decision;
    use crate::hash::LedgerHash;

    fn dummy_entry(seq: u64) -> LedgerEntry {
        LedgerEntry {
            seq,
            entry_id: seq,
            cycle_hash: LedgerHash(0xdead_beef),
            net: dl_sim::net_profit::NetProfit {
                input_amount: 0,
                gross_output: 0,
                total_costs: dl_sim::cost::CostBreakdown {
                    base_sig_fee_lamports: 0,
                    priority_fee_lamports: 0,
                    jito_tip_lamports: 0,
                    jito_tip_fee_lamports: 0,
                    total_lamports: 0,
                },
                net_profit: 0,
                net_profit_bps: 0,
                profitable: false,
            },
            optimistic: dl_sim::ev::ExpectedValue {
                e_pnl: 0,
                p_detect: dl_sim::ev::Prob::from_scaled_clamped(1_000_000_000_000_000_000),
                p_win: dl_sim::ev::Prob::from_scaled_clamped(1_000_000_000_000_000_000),
                p_land: dl_sim::ev::Prob::from_scaled_clamped(1_000_000_000_000_000_000),
                expected_failed_cost: 0,
                tip_lamports: 0,
            },
            conservative: dl_sim::ev::ExpectedValue {
                e_pnl: 0,
                p_detect: dl_sim::ev::Prob::from_scaled_clamped(1_000_000_000_000_000_000),
                p_win: dl_sim::ev::Prob::from_scaled_clamped(1_000_000_000_000_000_000),
                p_land: dl_sim::ev::Prob::from_scaled_clamped(1_000_000_000_000_000_000),
                expected_failed_cost: 0,
                tip_lamports: 0,
            },
            decision: Decision::WouldNotTrade,
            tip_lamports: 0,
        }
    }

    #[test]
    fn new_writes_magic_and_schema() {
        let mut buf = Vec::new();
        let w = LedgerWriter::new(&mut buf).unwrap();
        let _ = w.into_inner().unwrap();
        assert_eq!(&buf[0..8], b"DLD-LDG1");
        let schema = u32::from_le_bytes([buf[8], buf[9], buf[10], buf[11]]);
        assert_eq!(schema, 3);
        assert_eq!(buf.len(), 12, "header only, no entries");
    }

    #[test]
    fn write_entry_increments_count() {
        let mut buf = Vec::new();
        let mut w = LedgerWriter::new(&mut buf).unwrap();
        w.write_entry(&dummy_entry(0)).unwrap();
        w.write_entry(&dummy_entry(1)).unwrap();
        assert_eq!(w.frames_written(), 2);
        let _ = w.into_inner().unwrap();
    }
}
