//! Frame-by-frame reader for the paper ledger.
//!
//! Mirrors `dl-feed::capture::CapturedFeed`:
//!   - `open(source)` reads + validates magic + schema version once.
//!   - `read_entry()` returns the next `LedgerEntry`, or `Ok(None)` at
//!     EOF, or a `LedgerError` on corruption / truncation / decode
//!     failure.
//!   - The reader does not own a `BufReader`; it uses `Read::read_exact`
//!     with a small per-frame buffer to stay dependency-free.
//!
//! See `format::format_spec` for the byte-level layout.

use std::io::{Read, Seek, SeekFrom};

use crate::entry::LedgerEntry;
use crate::error::LedgerError;
use crate::format::{LEDGER_MAGIC, LEDGER_SCHEMA_VERSION};

/// Frame-by-frame ledger reader.
#[derive(Debug)]
pub struct LedgerReader<R: Read> {
    source: R,
    entries_read: u64,
}

impl<R: Read> LedgerReader<R> {
    /// Open a ledger file (or any `Read` source). Validates the magic
    /// (`LEDGER_MAGIC`) and schema version (`LEDGER_SCHEMA_VERSION`)
    /// at open time.
    pub fn open(mut source: R) -> Result<Self, LedgerError> {
        let mut magic = [0u8; 8];
        source.read_exact(&mut magic)?;
        if &magic != LEDGER_MAGIC {
            return Err(LedgerError::BadMagic);
        }
        let mut schema_bytes = [0u8; 4];
        source.read_exact(&mut schema_bytes)?;
        let file_schema = u32::from_le_bytes(schema_bytes);
        if file_schema != LEDGER_SCHEMA_VERSION {
            return Err(LedgerError::SchemaMismatch {
                found: file_schema,
                expected: LEDGER_SCHEMA_VERSION,
            });
        }
        Ok(Self {
            source,
            entries_read: 0,
        })
    }

    /// Read the next entry. Returns `Ok(None)` at clean EOF, an
    /// `Err(LedgerError::Truncated)` on a partial frame, or a
    /// `Bincode` error on a decode failure.
    pub fn read_entry(&mut self) -> Result<Option<LedgerEntry>, LedgerError> {
        // Read the 4-byte length. A short read (0 bytes) signals clean
        // EOF; a partial read (1-3 bytes) signals a truncated frame.
        let mut len_bytes = [0u8; 4];
        match self.source.read(&mut len_bytes) {
            Ok(0) => return Ok(None),
            Ok(n) if n < 4 => {
                // Partial length — truncated.
                return Err(LedgerError::Truncated);
            }
            Ok(_) => {}
            Err(e) => return Err(LedgerError::Io(e)),
        }
        let len = u32::from_le_bytes(len_bytes) as usize;
        let mut payload = vec![0u8; len];
        match self.source.read_exact(&mut payload) {
            Ok(()) => {}
            Err(_) => return Err(LedgerError::Truncated),
        }
        let entry: LedgerEntry = bincode::deserialize(&payload)?;
        self.entries_read += 1;
        Ok(Some(entry))
    }

    /// Number of entries successfully read so far.
    pub fn entries_read(&self) -> u64 {
        self.entries_read
    }

    /// Consume the reader, returning the inner source.
    pub fn into_inner(self) -> R {
        self.source
    }
}

/// Convenience: open a reader from any `Read + Seek` source by
/// validating the header at offset 0 (the standard case) without
/// consuming the reader's position.
impl<R: Read + Seek> LedgerReader<R> {
    /// Open by seeking to position 0 and validating the header. The
    /// reader's position is left at the start of frame data.
    pub fn open_seek(mut source: R) -> Result<Self, LedgerError> {
        source.seek(SeekFrom::Start(0))?;
        Self::open(source)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entry::Decision;
    use crate::hash::LedgerHash;
    use crate::writer::LedgerWriter;

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
            },
            conservative: dl_sim::ev::ExpectedValue {
                e_pnl: 0,
                p_detect: dl_sim::ev::Prob::from_scaled_clamped(1_000_000_000_000_000_000),
                p_win: dl_sim::ev::Prob::from_scaled_clamped(1_000_000_000_000_000_000),
                p_land: dl_sim::ev::Prob::from_scaled_clamped(1_000_000_000_000_000_000),
                expected_failed_cost: 0,
            },
            decision: Decision::WouldNotTrade,
        }
    }

    #[test]
    fn open_rejects_bad_magic() {
        let bytes = b"WRONGMAG\x02\x00\x00\x00";
        let r = LedgerReader::open(&bytes[..]);
        assert!(matches!(r, Err(LedgerError::BadMagic)));
    }

    #[test]
    fn open_rejects_wrong_schema() {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"DLD-LDG1");
        bytes.extend_from_slice(&1u32.to_le_bytes()); // schema 1, not 2
        let r = LedgerReader::open(&bytes[..]);
        match r {
            Err(LedgerError::SchemaMismatch { found, expected }) => {
                assert_eq!(found, 1);
                assert_eq!(expected, 2);
            }
            other => panic!("expected SchemaMismatch, got {:?}", other),
        }
    }

    #[test]
    fn read_then_eof_returns_none() {
        let mut buf = Vec::new();
        {
            let mut w = LedgerWriter::new(&mut buf).unwrap();
            w.write_entry(&dummy_entry(7)).unwrap();
        }
        let mut r = LedgerReader::open(buf.as_slice()).unwrap();
        let e = r.read_entry().unwrap().unwrap();
        assert_eq!(e.seq, 7);
        assert_eq!(r.read_entry().unwrap(), None);
    }

    #[test]
    fn read_truncated_payload_returns_truncated() {
        // Header + a length that says 100 bytes, but no payload.
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"DLD-LDG1");
        bytes.extend_from_slice(&2u32.to_le_bytes());
        bytes.extend_from_slice(&100u32.to_le_bytes());
        // intentionally no payload
        let mut r = LedgerReader::open(bytes.as_slice()).unwrap();
        match r.read_entry() {
            Err(LedgerError::Truncated) => {}
            other => panic!("expected Truncated, got {:?}", other),
        }
    }
}
