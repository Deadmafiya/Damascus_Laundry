//! Round-trip tests for the capture format. If any of these break, the
//! format spec and the impl have drifted; re-read the spec at
//! `dl_feed::capture::format_spec()` and fix one or the other.

use std::io::Cursor;

use dl_core::Feed;
use dl_core::FeedEvent;
use dl_feed::capture::{CaptureWriter, CapturedFeed, CAPTURE_MAGIC, CAPTURE_SCHEMA_VERSION};
use dl_feed::error::FeedError;

fn make_script(n: usize) -> Vec<FeedEvent> {
    (0..n)
        .map(|i| FeedEvent::AccountUpdate {
            slot: i as u64,
            pubkey: [i as u8; 32],
            data: vec![(i & 0xFF) as u8; (i % 32) + 1],
        })
        .collect()
}

#[test]
fn round_trip_simple_two_events() {
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
    assert_eq!(w.frames_written(), 2);
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
fn round_trip_larger_script() {
    let script = make_script(1000);
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
}

#[test]
fn empty_capture_yields_no_events() {
    let w = CaptureWriter::new(Vec::new()).unwrap();
    let bytes = w.into_inner().unwrap();
    let mut feed = CapturedFeed::open(Cursor::new(bytes)).unwrap();
    assert!(feed.next_event().is_none());
    assert_eq!(feed.events_returned(), 0);
}

#[test]
fn next_event_after_exhausted_keeps_returning_none() {
    let ev = FeedEvent::Slot { slot: 1 };
    let mut w = CaptureWriter::new(Vec::new()).unwrap();
    w.write_event(&ev).unwrap();
    let bytes = w.into_inner().unwrap();
    let mut feed = CapturedFeed::open(Cursor::new(bytes)).unwrap();
    assert!(feed.next_event().is_some());
    assert!(feed.next_event().is_none());
    assert!(feed.next_event().is_none(), "sticky None after EOF");
    assert!(feed.next_event().is_none());
}

#[test]
fn schema_mismatch_rejected() {
    let mut buf = Vec::new();
    buf.extend_from_slice(CAPTURE_MAGIC);
    buf.extend_from_slice(&999u32.to_le_bytes()); // wrong schema
    let res = CapturedFeed::open(Cursor::new(buf));
    match res {
        Err(FeedError::SchemaMismatch { file, build }) => {
            assert_eq!(file, 999);
            assert_eq!(build, CAPTURE_SCHEMA_VERSION);
        }
        other => panic!("expected SchemaMismatch, got {other:?}"),
    }
}

#[test]
fn bad_magic_rejected() {
    let buf = vec![0u8; 64];
    let res = CapturedFeed::open(Cursor::new(buf));
    assert!(matches!(res, Err(FeedError::BadMagic)));
}

#[test]
fn truncated_payload_surfaces_error_or_clean_eof() {
    // A length prefix that claims 100 bytes followed by only 50 bytes is
    // truncation. The Feed trait can't return Err, so it returns None and
    // logs an error. The test asserts the consumer sees a clean None at the
    // end (the only thing the trait can do).
    let mut buf = Vec::new();
    buf.extend_from_slice(CAPTURE_MAGIC);
    buf.extend_from_slice(&CAPTURE_SCHEMA_VERSION.to_le_bytes());
    buf.extend_from_slice(&100u32.to_le_bytes()); // claim 100-byte payload
    buf.extend_from_slice(&[0u8; 50]); // only 50 follow
    let mut feed = CapturedFeed::open(Cursor::new(buf)).unwrap();
    assert!(feed.next_event().is_none());
}

#[test]
fn schema_field_written_first() {
    // First 8 bytes: magic. Next 4: schema version. Confirms the file
    // header matches the spec (magic + schema, not schema + magic).
    let mut w = CaptureWriter::new(Vec::new()).unwrap();
    w.write_event(&FeedEvent::Slot { slot: 1 }).unwrap();
    let bytes = w.into_inner().unwrap();
    assert_eq!(&bytes[..8], CAPTURE_MAGIC);
    let got_schema = u32::from_le_bytes(bytes[8..12].try_into().unwrap());
    assert_eq!(got_schema, CAPTURE_SCHEMA_VERSION);
}

#[test]
fn partial_length_header_rejected_as_truncated() {
    // Less than 8 bytes total: not even the header. Cleaner: just truncated
    // length header.
    let mut buf = Vec::new();
    buf.extend_from_slice(CAPTURE_MAGIC);
    buf.extend_from_slice(&CAPTURE_SCHEMA_VERSION.to_le_bytes());
    buf.extend_from_slice(&[0u8, 0u8]); // only 2 bytes of the 4-byte length prefix
    let mut feed = CapturedFeed::open(Cursor::new(buf)).unwrap();
    assert!(feed.next_event().is_none());
}
