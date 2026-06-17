---
phase: 02-ingestion-pool-state
plan: 01
type: Summary
about: "damascus_laundry"
description: "APPLY results for Phase 2 / Plan 01: WebSocket feed + capture/replay"
---

# SUMMARY — 02-01 Ingestion: WS Feed + Capture/Replay

**Status:** APPLY complete. 7 of 7 tasks DONE. All 5 acceptance criteria PASS.
**Date:** 2026-06-18
**Commits:** 197e92a, d90e570, 25bb71e, b17ac3b, 72ec64d, b6e0f23, 89f748a (7 on top of Phase 1's b094547)

## What was built

A live WebSocket feed (`WsFeed`) and a deterministic capture/replay pipeline
(`CaptureWriter` + `CapturedFeed` + `CapturingFeed<W, F>` tee) that the rest of
the engine consumes through Phase 1's pluggable `Feed` trait. Live data and
replay data flow through the *same* `next_event()` API, so Phase 3+ detection
code can't tell the difference.

### dl-feed — full source

| File | Lines | Role |
|------|-------|------|
| `src/error.rs` | 41 | `FeedError` enum (thiserror): `Io`, `BadMagic`, `SchemaMismatch`, `WebSocket`, `JsonRpc { code, message }`, `SubscribeFailed`, `ChannelClosed` |
| `src/capture.rs` | 289 | `CaptureWriter<W>`, `CapturedFeed<R>`, `format_spec()`, `CAPTURE_MAGIC = b"DLF-CAP1"`, `CAPTURE_SCHEMA_VERSION = 1` |
| `src/capturing.rs` | 145 | `CapturingFeed<W, F>` — write-through tee: forwards `next_event()` to inner feed, copies every event to the capture writer, exposes `frames_written` / `write_failures` counters |
| `src/ws_feed.rs` | 470 | `WsFeed` (gated on `ws` feature): background `std::thread` running a tokio runtime that owns the tungstenite stream; bridges async JSON-RPC notifications to a **sync** `std::sync::mpsc::Receiver<FeedEvent>`. Global `Mutex<HashMap<u64, oneshot::Sender<…>>>` for request-id → reply routing. `Command` enum (`Account`, `Program`, `Slots`) with oneshot replies |
| `src/lib.rs` | 27 | Module surface + crate doc |

### Capture format (the spec, by the code)

```
+------------------------------------------------+
| 8 bytes   | MAGIC      | b"DLF-CAP1"           |  file header
| 4 bytes   | u32 LE     | schema_version (== 1) |  one-time, validated at open
+------------------------------------------------+
| ... frame 0, frame 1, ...                     |
+------------------------------------------------+
```

```
+------------------------------------------------+
| 4 bytes   | u32 LE     | payload_len (bytes)   |
| N bytes   | bincode    | serialized FeedEvent  |
+------------------------------------------------+
```

Frames are length-prefixed bincode, written back-to-back. No padding. No
terminator — EOF on the reader signals end of stream. bincode → bit-identical
across machines (AC-1 determinism). A 752-byte Raydium `AmmInfo` serializes in
~1.1 KB; the 60-second sample capture is 2396 bytes for 149 slot events.

### Tests added

| Test | What it proves |
|------|----------------|
| `dl-feed::capture::round_trip_via_cursor` | Writer → reader yields events byte-identical |
| `dl-feed::capture::format_spec_mentions_key_fields` | Spec text mentions every layout field (anti-drift) |
| `dl-feed::capturing::*` (3 inline) | `CapturingFeed` forwards events, counts frames, surfaces write failures |
| `dl-feed::ws_feed::error_mapping` (2 inline) | `WsFeedError` → `FeedError` conversion covers all 5 variants |
| `dl-feed::tests::capture_roundtrip` (9) | Scripted → write → read == scripted; mismatched magic/schema rejected |
| `dl-feed::tests::determinism` (4) | Two runs of `SeededRng` from same seed → byte-identical capture (AC-1) |
| `dl-feed::tests::ws_feed_live` (1, **#[ignore]**) | End-to-end against mainnet wss://api.mainnet-beta.solana.com/ |

### dl-app — wired through the new surface

`crates/dl-app/src/main.rs` now has three modes:

1. **No env vars** — Phase 1 placeholder, prints "foundations ready" (AC-4 preserved).
2. **`DL_RPC_URL` + `DL_CAPTURE_PATH` set** — connects to the WS RPC, builds a
   `CapturingFeed<WsFeed, File>`, drains for `DL_CAPTURE_SECS` (default 60s),
   prints a summary with `frames_written` + `capture_write_failures` counters.
3. **`DL_DRY_RUN=1`** — opens the sample capture and replays it (see 02-02).

### Live evidence (commit 89f748a)

- 60 seconds against `wss://api.mainnet-beta.solana.com/`
- **149 slot events** captured to `crates/dl-feed/tests/fixtures/sample_capture.bincode` (2396 bytes)
- Slot range 427123909 → 427124057 (~2.5 slots/sec, matches Solana cadence)
- Fixture is checked into git; subsequent replays are bit-identical

## Acceptance criteria

| AC | Result | Evidence |
|----|--------|----------|
| AC-1 deterministic capture / replay | PASS | `tests/determinism.rs` (4 proptests), 60s live capture fixture checked in |
| AC-2 float-free value path in dl-feed | PASS | `tests/fixed_point_no_floats.rs` greps `src/` for `f32`/`f64` — 0 hits |
| AC-3 WS feed (live JSON-RPC) behind `Feed` trait | PASS | `WsFeed` compiles only with `--features ws`; sync `next_event()` via mpsc; 1 live test, manually verified during 02-01-07 |
| AC-4 `cargo run -p dl-app` always exits 0 | PASS | Default run still prints "foundations ready" and exits 0 |
| AC-5 capture file format documented, versioned, validated at open | PASS | `format_spec()` text + `BadMagic` / `SchemaMismatch` errors exercised in `capture_roundtrip.rs` |

## Deviations from plan

- **Plan spec was `dl_core::Fixed` + `.ratio(quote, base)` for `mid_price`**. The actual
  Phase 1 API is the free function `dl_core::fixed::mul_div_floor(value, num, denom)`,
  returning `Result<u128, MathError>`. Adapted `Pool::mid_price_scaled_1e9` to use it
  with the same semantics; test assertion adjusted to match. (Caught at 02-02-01.)
- **Added `dl-feed` as a `[features] ws`-gated dep on dl-app**, not unconditional.
  Keeps the default `cargo run -p dl-app` path (which doesn't need tokio-tungstenite)
  free of async/TLS deps. The live-capture path explicitly enables `ws` via
  `dl-feed = { workspace = true, features = ["ws"] }` in `dl-app/Cargo.toml`.
- **Added rustls ring crypto provider** (`rustls = { version = "0.23", features = ["ring"] }`)
  to `dl-feed/Cargo.toml`. The public Solana RPC uses TLS; `tokio-tungstenite`'s
  `rustls-tls-webpki-roots` feature pulls in rustls but rustls 0.23 requires exactly
  one of `aws-lc-rs` / `ring` enabled. `ring` is the default and ships in our
  toolchain. Without this fix the live-capture path panics at runtime with
  "no process-level CryptoProvider available".
- **No `pub fn Fixed::ratio`** in the codebase; the spec text mentions it. Used
  `mul_div_floor` directly. The plan's `mid_price_scaled_1e9` semantics still hold:
  result is a 1e-9-scaled integer of `quote / base` in raw base units.

## Concerns / notes for UNIFY

- `CapturedFeed::next_event` returns `None` on bincode-decoding error or truncated
  frame. `Feed::next_event() -> Option<FeedEvent>` can't surface `Err`, so the
  failure path is logged via `tracing::error!` and `events_returned` lets the
  consumer detect the shortfall. Good enough for v1.0; a `Result` variant of the
  `Feed` trait could be considered in v1.1 if the silent-truncation footgun bites.
- The `ws_feed` global `Mutex<HashMap<u64, oneshot::Sender<…>>>` is single-task
  contention, so the lock is uncontended in practice. If we ever spawn multiple
  WsFeeds or move to per-task routing, switch to `DashMap` or a per-instance map
  threaded through the state machine.

## Deferred to Phase 2 / Plan 02

- Decoders that turn `AccountUpdate.data` into normalized `Pool` state.
- The "fresh quote" venue leg for arb pairing (an order book venue).
- Pool registry update path into `dl-app` (added in 02-02-07, not in this sub-plan).
