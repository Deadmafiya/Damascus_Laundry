//! Capture-file format for `dl-feed`.
//!
//! This module IS the spec. If the format changes, the spec text and the
//! `format_spec()` function change in lockstep, and the round-trip test
//! (in `tests/capture_roundtrip.rs`) keeps the two in sync.
//!
//! ## File layout
//!
//! ```text
//! +------------------------------------------------+
//! | 8 bytes   | MAGIC      | b"DLF-CAP1"           |  file header
//! | 4 bytes   | u32 LE     | schema_version (== 1) |  one-time, validated at open
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
//! | N bytes   | bincode    | serialized FeedEvent  |
//! +------------------------------------------------+
//! ```
//!
//! Frames are written back-to-back with no padding. End-of-stream is signalled
//! by EOF on the underlying reader — no terminator frame is written (avoids
//! the chicken-and-egg of a one-byte terminator that could collide with a
//! truncated read).
//!
//! ## Why this format
//!
//! - bincode payload → bit-identical across machines (AC-1 determinism in
//!   `.paul/phases/02-ingestion-pool-state/...`).
//! - Schema version constant at the start (not per frame) — cheap to validate
//!   once, no per-frame overhead.
//! - No compression, no encryption — replay is a debug/research tool; the
//!   consumer of a `.bincode` capture is a Rust binary under our control.
//! - No checksums yet — Phase 2's first iteration prioritises correctness over
//!   corruption detection. Add a CRC32 footer in a v2 schema when we have a
//!   failure mode that motivates it.
//!
//! ## Schema version policy
//!
//! Schema version 1 is current. Bumping to 2 requires:
//!   1. A `From`/`TryFrom` from v1 to v2 frames in this module, OR a hard
//!      `SchemaMismatch` rejection of v1 files (the latter is the default).
//!   2. A new round-trip test in `tests/capture_roundtrip.rs` that asserts
//!      the spec text mentions the new fields.
//!   3. An entry in `docs/CHANGELOG.md` (TODO when the project gets one).

use std::io::{Read, Write};

use dl_core::{Feed, FeedEvent};

use crate::error::FeedError;

/// Current capture-file schema version. Bumped only on a breaking change
/// to the file layout.
pub const CAPTURE_SCHEMA_VERSION: u32 = 1;

/// 8-byte magic prefix. Lets us tell a `.bincode` capture apart from any
/// other binary blob in the workspace.
pub const CAPTURE_MAGIC: &[u8; 8] = b"DLF-CAP1";

/// Returns the spec text. The round-trip test asserts this string mentions
/// every layout field — paranoid but cheap, and prevents the spec drifting
/// from the impl.
pub fn format_spec() -> &'static str {
    "Capture file = magic(8) + schema_version(u32 LE) + frames.\n\
     Frame = payload_len(u32 LE) + bincode(FeedEvent).\n\
     Schema version 1."
}

/// Writes a sequence of `FeedEvent`s to an underlying `Write` in the format
/// described at the module level. Errors are surfaced — a partial capture is
/// worse than no capture for replay.
pub struct CaptureWriter<W: Write> {
    sink: W,
    frames_written: u64,
}

impl<W: Write> CaptureWriter<W> {
    /// Create a new writer, emitting the file header (magic + schema version)
    /// immediately. Returns `Io` if the header write fails.
    pub fn new(mut sink: W) -> Result<Self, FeedError> {
        sink.write_all(CAPTURE_MAGIC)?;
        sink.write_all(&CAPTURE_SCHEMA_VERSION.to_le_bytes())?;
        Ok(Self {
            sink,
            frames_written: 0,
        })
    }

    /// Number of frames written so far.
    pub fn frames_written(&self) -> u64 {
        self.frames_written
    }

    /// Serialize and write a single `FeedEvent` as one frame.
    pub fn write_event(&mut self, ev: &FeedEvent) -> Result<(), FeedError> {
        let payload = bincode::serialize(ev)?;
        let len = u32::try_from(payload.len()).map_err(|_| FeedError::Truncated {
            frame: self.frames_written,
        })?;
        self.sink.write_all(&len.to_le_bytes())?;
        self.sink.write_all(&payload)?;
        self.frames_written += 1;
        Ok(())
    }

    /// Consume the writer and return the underlying sink. Flushes first.
    pub fn into_inner(mut self) -> Result<W, FeedError> {
        self.sink.flush()?;
        Ok(self.sink)
    }
}

/// Replays a capture from an underlying `Read`. Implements `Feed` so it is
/// a drop-in for the other `Feed` impls in the engine.
#[derive(Debug)]
pub struct CapturedFeed<R: Read> {
    source: R,
    /// Bytes we've already consumed from the source but not yet parsed into
    /// a complete event. Length-prefixed framing means we have to buffer.
    pending: Vec<u8>,
    /// True once we've returned `None` (EOF). Sticky so we don't keep
    /// returning `None` indefinitely on every `next_event` call.
    exhausted: bool,
    /// Total events returned so far. Used for error messages.
    events_returned: u64,
}

impl<R: Read> CapturedFeed<R> {
    /// Open a capture. Validates the magic + schema version, then primes the
    /// read buffer for streaming frame consumption.
    pub fn open(mut source: R) -> Result<Self, FeedError> {
        let mut magic = [0u8; 8];
        source.read_exact(&mut magic)?;
        if &magic != CAPTURE_MAGIC {
            return Err(FeedError::BadMagic);
        }
        let mut schema_bytes = [0u8; 4];
        source.read_exact(&mut schema_bytes)?;
        let file_schema = u32::from_le_bytes(schema_bytes);
        if file_schema != CAPTURE_SCHEMA_VERSION {
            return Err(FeedError::SchemaMismatch {
                file: file_schema,
                build: CAPTURE_SCHEMA_VERSION,
            });
        }
        Ok(Self {
            source,
            pending: Vec::new(),
            exhausted: false,
            events_returned: 0,
        })
    }

    /// Number of events returned so far.
    pub fn events_returned(&self) -> u64 {
        self.events_returned
    }

    /// Consume the reader and return it.
    pub fn into_inner(self) -> R {
        self.source
    }
}

impl<R: Read> Feed for CapturedFeed<R> {
    fn next_event(&mut self) -> Option<FeedEvent> {
        if self.exhausted {
            return None;
        }
        loop {
            // Drain any fully-buffered frame from `pending` first.
            if self.pending.len() >= 4 {
                let len = u32::from_le_bytes([
                    self.pending[0],
                    self.pending[1],
                    self.pending[2],
                    self.pending[3],
                ]) as usize;
                if self.pending.len() >= 4 + len {
                    let payload: Vec<u8> = self.pending.drain(..4 + len).collect();
                    let payload = &payload[4..];
                    match bincode::deserialize::<FeedEvent>(payload) {
                        Ok(ev) => {
                            self.events_returned += 1;
                            return Some(ev);
                        }
                        Err(e) => {
                            // A bincode error inside a frame is corruption
                            // we can't recover from. Mark exhausted and
                            // surface the error via a tracing event; the
                            // `Feed` trait can't return Result so this is
                            // the best we can do (a clean `None` is
                            // indistinguishable from EOF to the consumer).
                            tracing::error!(
                                events_returned = self.events_returned,
                                error = %e,
                                "capture frame bincode decode failed; treating as EOF"
                            );
                            self.exhausted = true;
                            return None;
                        }
                    }
                }
            }

            // Read more bytes from the source. EOF on a non-empty pending
            // is a truncated frame; EOF on an empty pending is clean EOF.
            let mut chunk = [0u8; 4096];
            match self.source.read(&mut chunk) {
                Ok(0) => {
                    // EOF.
                    if self.pending.is_empty() {
                        self.exhausted = true;
                        return None;
                    } else {
                        // We were mid-frame.
                        let frame = self.events_returned;
                        self.exhausted = true;
                        tracing::error!(frame, "capture truncated at end of stream");
                        // We still can't return Err from `Feed::next_event`,
                        // so return None. The error is logged; consumers that
                        // need to distinguish truncation from EOF can check
                        // `events_returned` against the expected count.
                        return None;
                    }
                }
                Ok(n) => {
                    self.pending.extend_from_slice(&chunk[..n]);
                    // Loop and try to drain a complete frame.
                }
                Err(e) => {
                    tracing::error!(error = %e, "capture read error; treating as EOF");
                    self.exhausted = true;
                    return None;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn round_trip_via_cursor() {
        let script = vec![
            FeedEvent::Slot { slot: 1 },
            FeedEvent::AccountUpdate {
                slot: 1,
                pubkey: [7u8; 32],
                data: vec![0xDE, 0xAD, 0xBE, 0xEF],
            },
        ];
        let mut w = CaptureWriter::new(Vec::new()).unwrap();
        for ev in &script {
            w.write_event(ev).unwrap();
        }
        let bytes = w.into_inner().unwrap();

        let mut feed = CapturedFeed::open(Cursor::new(bytes)).unwrap();
        let mut got = Vec::new();
        while let Some(ev) = feed.next_event() {
            got.push(ev);
        }
        assert_eq!(got, script);
        assert_eq!(feed.events_returned(), 2);
    }

    #[test]
    fn format_spec_mentions_key_fields() {
        let s = format_spec();
        assert!(s.contains("magic"), "spec must mention magic");
        assert!(
            s.contains("schema_version"),
            "spec must mention schema_version"
        );
        assert!(s.contains("payload_len"), "spec must mention payload_len");
        assert!(s.contains("bincode"), "spec must mention bincode");
    }
}
