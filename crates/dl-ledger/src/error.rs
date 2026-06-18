//! Error types for `dl-ledger`.
//!
//! All variants are constructed at well-defined points (open, frame read,
//! bincode decode, math). Variants that wrap foreign errors (`Io`,
//! `Bincode`) surface the underlying error via `Display` and `source()`.

use std::fmt;

/// Errors that can arise when writing, reading, or aggregating a ledger.
#[derive(Debug)]
pub enum LedgerError {
    /// Underlying `std::io` error (file open, write, read_exact).
    Io(std::io::Error),

    /// File magic did not match `LEDGER_MAGIC`.
    ///
    /// This is the expected error when the user points the reader at a
    /// non-ledger file (e.g. a Phase 2 capture file or a random binary).
    BadMagic,

    /// Schema version did not match `LEDGER_SCHEMA_VERSION`.
    ///
    /// The `found` field carries the version the file actually declared;
    /// the `expected` field carries the version this reader was compiled
    /// for. There is no automatic migration — opening a v3 file with a
    /// v2 reader is a hard error by design.
    SchemaMismatch {
        /// Schema version read from the file.
        found: u32,
        /// Schema version this reader was compiled against.
        expected: u32,
    },

    /// Frame declared a payload length larger than the bytes remaining
    /// in the file, or EOF was hit mid-frame.
    ///
    /// This usually means a crash mid-write (truncated file). Recovery
    /// is to stop at the last complete frame; the reader does not
    /// attempt partial recovery.
    Truncated,

    /// Bincode failed to decode a `LedgerEntry` payload.
    ///
    /// Surfaces the underlying `bincode::Error` via `source()`. Indicates
    /// either disk corruption or a writer that produced a different
    /// `LedgerEntry` shape than the reader expects.
    Bincode(bincode::Error),

    /// Integer overflow in summary aggregation.
    ///
    /// Only reachable from `LedgerSummary::from_entries` when the sum
    /// of `e_pnl` (`i128`) or `p_land` (`u128`) exceeds the type's max.
    /// Practical at ~10^38 / 10^38 lamports — unreachable for v1.0
    /// paper portfolios, but kept as a hard error so a future run
    /// doesn't silently wrap.
    Math,
}

impl fmt::Display for LedgerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LedgerError::Io(e) => write!(f, "ledger I/O error: {}", e),
            LedgerError::BadMagic => write!(
                f,
                "ledger: bad magic (expected {:?}, file is not a dl-ledger artifact)",
                std::str::from_utf8(crate::format::LEDGER_MAGIC).unwrap_or("?")
            ),
            LedgerError::SchemaMismatch { found, expected } => write!(
                f,
                "ledger: schema mismatch (file is v{}, reader is v{})",
                found, expected
            ),
            LedgerError::Truncated => {
                write!(f, "ledger: truncated frame (writer crash mid-payload?)")
            }
            LedgerError::Bincode(e) => write!(f, "ledger: bincode decode failed: {}", e),
            LedgerError::Math => write!(f, "ledger: integer overflow in summary aggregation"),
        }
    }
}

impl std::error::Error for LedgerError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            LedgerError::Io(e) => Some(e),
            LedgerError::Bincode(e) => Some(e),
            _ => None,
        }
    }
}

impl From<std::io::Error> for LedgerError {
    fn from(e: std::io::Error) -> Self {
        LedgerError::Io(e)
    }
}

impl From<bincode::Error> for LedgerError {
    fn from(e: bincode::Error) -> Self {
        LedgerError::Bincode(e)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_io_includes_underlying() {
        // Synthetic io::Error to verify Display mentions the source.
        let io = std::io::Error::other("disk full");
        let e = LedgerError::Io(io);
        let s = format!("{}", e);
        assert!(
            s.contains("disk full"),
            "Display should include io error text: {}",
            s
        );
    }

    #[test]
    fn display_bad_magic_mentions_expected() {
        let s = format!("{}", LedgerError::BadMagic);
        assert!(
            s.contains("DLD-LDG1"),
            "Display should include expected magic: {}",
            s
        );
    }

    #[test]
    fn display_schema_mismatch_includes_versions() {
        let s = format!(
            "{}",
            LedgerError::SchemaMismatch {
                found: 3,
                expected: 2
            }
        );
        assert!(
            s.contains("v3"),
            "Display should include found version: {}",
            s
        );
        assert!(
            s.contains("v2"),
            "Display should include expected version: {}",
            s
        );
    }

    #[test]
    fn display_truncated_is_descriptive() {
        let s = format!("{}", LedgerError::Truncated);
        assert!(
            s.to_lowercase().contains("truncat"),
            "Display should say truncated: {}",
            s
        );
    }

    #[test]
    fn from_io_wraps_correctly() {
        let io = std::io::Error::new(std::io::ErrorKind::PermissionDenied, "denied");
        let e: LedgerError = io.into();
        assert!(matches!(e, LedgerError::Io(_)));
    }
}
