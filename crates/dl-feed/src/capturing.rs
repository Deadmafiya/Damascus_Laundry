//! `CapturingFeed` — a tee wrapper that mirrors every event from an inner
//! [`Feed`] into a [`CaptureWriter`] while still passing it through to the
//! caller.
//!
//! Production usage: wrap a live `WsFeed` so we can both process the stream
//! AND persist it to disk for later deterministic replay.
//!
//! Test usage: wrap a `ScriptedFeed` and verify that the captured bytes
//! round-trip back to the original script.

use std::io::Write;

use dl_core::Feed;
use dl_core::FeedEvent;

use crate::capture::CaptureWriter;
use crate::error::FeedError;

/// Tee wrapper: forwards events from `inner` to the caller AND writes each
/// one to a capture sink for later replay.
pub struct CapturingFeed<W: Write, F: Feed> {
    inner: F,
    writer: CaptureWriter<W>,
    /// How many write attempts failed. Surfaced for the test suite and for
    /// human operators who want to know if a capture was silently losing
    /// events.
    write_failures: u64,
}

impl<W: Write, F: Feed> CapturingFeed<W, F> {
    /// Build a tee over `inner` writing to `sink`. Emits the capture file
    /// header to `sink` immediately.
    pub fn new(inner: F, sink: W) -> Result<Self, FeedError> {
        Ok(Self {
            inner,
            writer: CaptureWriter::new(sink)?,
            write_failures: 0,
        })
    }

    /// Number of frames successfully written to the capture sink.
    pub fn frames_written(&self) -> u64 {
        self.writer.frames_written()
    }

    /// Number of capture-write attempts that failed (so far). The feed still
    /// passes the event through, so the consumer is unaffected; but a
    /// non-zero value means the capture is lossy and replays will diverge
    /// from a complete live trace.
    pub fn write_failures(&self) -> u64 {
        self.write_failures
    }

    /// Consume the tee and return the inner feed + the underlying sink.
    /// Flushes the capture writer first.
    pub fn into_parts(self) -> Result<(F, W), FeedError> {
        let sink = self.writer.into_inner()?;
        Ok((self.inner, sink))
    }
}

impl<W: Write, F: Feed> Feed for CapturingFeed<W, F> {
    fn next_event(&mut self) -> Option<FeedEvent> {
        let ev = self.inner.next_event()?;
        if let Err(e) = self.writer.write_event(&ev) {
            // Log + count, but keep going. The consumer still gets the event
            // — losing the live stream because the disk hiccupped would be
            // strictly worse than a partial capture.
            self.write_failures += 1;
            tracing::error!(
                error = %e,
                write_failures = self.write_failures,
                frames_written = self.writer.frames_written(),
                "capture write failed; passing event through anyway"
            );
        }
        Some(ev)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    use crate::capture::CapturedFeed;
    use dl_core::ScriptedFeed;

    #[test]
    fn tee_passes_events_through_and_writes_them() {
        let script = vec![
            FeedEvent::Slot { slot: 1 },
            FeedEvent::AccountUpdate {
                slot: 1,
                pubkey: [7u8; 32],
                data: vec![0xDE, 0xAD, 0xBE, 0xEF],
            },
            FeedEvent::Slot { slot: 2 },
        ];
        let feed = ScriptedFeed::new(script.clone());
        let mut tee = CapturingFeed::new(feed, Vec::new()).unwrap();

        let mut passed = Vec::new();
        while let Some(ev) = tee.next_event() {
            passed.push(ev);
        }
        assert_eq!(passed, script);
        assert_eq!(tee.frames_written(), 3);
        assert_eq!(tee.write_failures(), 0);

        let (mut inner, bytes) = tee.into_parts().unwrap();
        assert!(inner.next_event().is_none(), "inner is fully drained");

        // The captured bytes should replay to the same script.
        let mut replayed = CapturedFeed::open(Cursor::new(bytes)).unwrap();
        let mut got = Vec::new();
        while let Some(ev) = replayed.next_event() {
            got.push(ev);
        }
        assert_eq!(got, script);
    }

    #[test]
    fn empty_inner_yields_no_events() {
        let mut tee = CapturingFeed::new(ScriptedFeed::empty(), Vec::new()).unwrap();
        assert!(tee.next_event().is_none());
        assert_eq!(tee.frames_written(), 0);
    }

    /// A `Write` impl that allows the 12-byte header (8 magic + 4 schema)
    /// but fails on every subsequent write. Confirms the tee logs and counts
    /// failures while still passing events through to the consumer.
    struct FailAfterHeader {
        bytes_written: usize,
    }

    impl Write for FailAfterHeader {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            // 8 (magic) + 4 (schema) = 12 bytes of header. Accept those.
            if self.bytes_written + buf.len() <= 12 {
                self.bytes_written += buf.len();
                return Ok(buf.len());
            }
            Err(std::io::Error::other("synthetic post-header failure"))
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn write_failure_does_not_break_passthrough() {
        let script = vec![FeedEvent::Slot { slot: 1 }, FeedEvent::Slot { slot: 2 }];
        let mut tee = CapturingFeed::new(
            ScriptedFeed::new(script.clone()),
            FailAfterHeader { bytes_written: 0 },
        )
        .unwrap();
        let mut got = Vec::new();
        while let Some(ev) = tee.next_event() {
            got.push(ev);
        }
        assert_eq!(got, script);
        // Header wrote (so 0 frames), then both write_event calls failed.
        assert_eq!(tee.write_failures(), 2);
        assert_eq!(tee.frames_written(), 0);
    }
}
