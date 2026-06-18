//! Ledger file format. This module IS the spec.
//!
//! ## File layout
//!
//! ```text
//! +------------------------------------------------+
//! | 8 bytes   | MAGIC      | b"DLD-LDG1"           |  file header
//! | 4 bytes   | u32 LE     | schema_version (== 3) |  one-time, validated at open
//! +------------------------------------------------+
//! | ... frame 0, frame 1, ...                     |
//! +------------------------------------------------+
//! ```
//!
//! ## Frame layout
//!
//! ```text
//! +------------------------------------------------+
//! | 4 bytes   | u32 LE     | payload_len (bytes)   |
//! | N bytes   | bincode    | serialized LedgerEntry|
//! +------------------------------------------------+
//! ```
//!
//! Frames are written back-to-back with no padding. End-of-stream is signalled
//! by EOF on the underlying reader — no terminator frame is written (avoids
//! the chicken-and-egg of a one-byte terminator that could collide with a
//! truncated read). Mirrors the Phase 2 capture format (`dl-feed::capture`).
//!
//! ## Schema version policy
//!
//! Schema version 2 is current. Bumping to 3 requires:
//!   1. A `From` / `TryFrom` from v2 to v3 frames in this module, OR a hard
//!      `SchemaMismatch` rejection of v2 files (the latter is the default).
//!   2. A new round-trip test in `tests/ledger_roundtrip.rs` that asserts
//!      the spec text mentions the new fields.
//!
//! ## Why a separate file format
//!
//! The ledger is a write-once, read-many audit trail distinct from the
//! capture stream. Reusing the Phase 2 capture format would conflate
//! "what events arrived" (capture) with "what decisions were made"
//! (ledger) — useful in replay, confusing in audit. Schema v2 is
//! distinct from capture schema v1 by magic (`DLD-LDG1` vs `DLF-CAP1`).

/// 8-byte magic prefix. Tells a ledger file apart from any other binary
/// blob — including a Phase 2 capture file (whose magic is `DLF-CAP1`).
pub const LEDGER_MAGIC: &[u8; 8] = b"DLD-LDG1";

/// Current ledger-file schema version. Bumped only on a breaking change
/// to the file layout.
///
/// v3 (2026-06-18, Phase 7 / plan 01): adds `tip_lamports: u64` to
/// each `LedgerEntry`. v3 readers must reject v2 files via
/// `SchemaMismatch` (per the schema policy). Downward compat is
/// not preserved: re-deriving tip from v2 data is impossible.
pub const LEDGER_SCHEMA_VERSION: u32 = 3;

/// Returns the spec text. The round-trip test asserts this string mentions
/// every layout field — paranoid but cheap, and prevents the spec drifting
/// from the impl.
pub fn format_spec() -> &'static str {
    "Ledger file = magic(8 bytes, \"DLD-LDG1\") + schema_version(u32 LE) + frames.\n\
     Frame = payload_len(u32 LE) + bincode(LedgerEntry).\n\
     Schema version 3."
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_spec_mentions_every_field() {
        let spec = format_spec();
        assert!(
            spec.contains("DLD-LDG1"),
            "spec must mention magic: {}",
            spec
        );
        assert!(spec.contains("u32"), "spec must mention u32: {}", spec);
        assert!(
            spec.contains("bincode"),
            "spec must mention bincode: {}",
            spec
        );
        assert!(
            spec.contains("LedgerEntry"),
            "spec must mention LedgerEntry: {}",
            spec
        );
        assert!(
            spec.contains("payload_len"),
            "spec must mention payload_len: {}",
            spec
        );
        assert!(
            spec.contains("Schema version 3"),
            "spec must pin schema v3: {}",
            spec
        );
    }

    #[test]
    fn magic_is_eight_bytes() {
        assert_eq!(LEDGER_MAGIC.len(), 8);
        assert_ne!(
            LEDGER_MAGIC, b"DLF-CAP1",
            "ledger magic must not collide with capture"
        );
    }

    #[test]
    fn schema_version_is_three() {
        assert_eq!(LEDGER_SCHEMA_VERSION, 3);
    }
}
