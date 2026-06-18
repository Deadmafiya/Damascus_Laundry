---
phase: 05-simulation-core-paper-ledger
plan: 02
type: Summary
about: "damascus_laundry"
description: "APPLY results for Phase 5 / plan 02: append-only paper ledger (schema v2, magic DLD-LDG1) in a new dl-ledger crate. 39 new tests. No new core deps (bincode + serde-derive only; no rusqlite, no parking_lot)."

# Phase 5 / Plan 02 — Paper Ledger: APPLY Results

## What landed

- New crate `dl-ledger` (was a placeholder). 7 source files, 3 test files.
- File format `DLD-LDG1` / schema v2: 8-byte magic + 4-byte LE schema version + N frames of `[u32 LE payload_len | bincode(LedgerEntry)]`. No terminator (EOF signals end).
- `LedgerEntry` (struct, derives Serialize+Deserialize): seq, entry_id, cycle_hash (FNV-1a 64 over cycle legs), net (Phase 4 NetProfit), optimistic (EV), conservative (EV), decision (WouldTrade iff conservative.e_pnl > 0).
- `LedgerWriter<W: Write>` — `new()` writes header, `write_entry()` writes one frame, `into_inner()` flushes.
- `LedgerReader<R: Read>` — `open()` validates header, `read_entry()` returns `Option<Entry>` (None on clean EOF), `Err(Truncated)` on partial frames, `Err(Bincode)` on decode failure.
- `LedgerSummary::from_entries(&[LedgerEntry])` — integer-only aggregates: total / would_trade / would_not_trade / sum_optimistic_e_pnl / sum_conservative_e_pnl / sum_conservative_p_land. Returns `LedgerError::Math` on integer overflow (defensive; unreachable in v1.0 magnitudes).
- `LedgerHash` — deterministic FNV-1a 64 of a Cycle's leg sequence (pool pubkey + direction discriminator). Provides `Display` as 16-char lowercase hex.
- `LedgerError` (thiserror) — Io, BadMagic, SchemaMismatch { found, expected }, Bincode, Truncated, Math.

## Files created

- `crates/dl-ledger/Cargo.toml` (added bincode + serde workspace deps)
- `crates/dl-ledger/src/lib.rs`
- `crates/dl-ledger/src/format.rs` — magic / schema / `format_spec()`
- `crates/dl-ledger/src/error.rs`
- `crates/dl-ledger/src/hash.rs` — FNV-1a 64 over `Cycle`
- `crates/dl-ledger/src/entry.rs` — `LedgerEntry`, `Decision`
- `crates/dl-ledger/src/writer.rs` — `LedgerWriter<W: Write>`
- `crates/dl-ledger/src/reader.rs` — `LedgerReader<R: Read>`
- `crates/dl-ledger/src/summary.rs` — `LedgerSummary`
- `crates/dl-ledger/tests/ledger_roundtrip.rs` — round-trip + format lock + corruption
- `crates/dl-ledger/tests/ledger_props.rs` — proptest (round-trip, decision, summary associativity)
- `crates/dl-ledger/tests/int_only_no_fractional.rs` — CI guard

## Files modified

- `Cargo.toml` — added `serde = { version = "1", features = ["derive"] }` to workspace.dependencies
- `crates/dl-core/Cargo.toml` — switched to `serde.workspace = true`
- `crates/dl-sim/Cargo.toml` — added `serde.workspace = true`
- `crates/dl-state/Cargo.toml` — added `serde.workspace = true`
- `crates/dl-state/src/cycle.rs` — `Cycle, Leg, Direction` got `Serialize, Deserialize`
- `crates/dl-state/src/pool.rs` — `Pubkey` got `Serialize, Deserialize`
- `crates/dl-sim/src/ev.rs` — `Prob, CompetitionParams, LatencyBudget, LandingParams, FailedCostModel, ExpectedValue, EvalParams, EvalOutcome` got `Serialize, Deserialize`
- `crates/dl-sim/src/net_profit.rs` — `NetProfit` got `Serialize, Deserialize`
- `crates/dl-sim/src/cost.rs` — `CostBreakdown` got `Serialize, Deserialize`

## CI gates

- `cargo fmt --all -- --check` — clean
- `cargo clippy --workspace --all-targets -- -D warnings` — clean
- `cargo test --workspace` — **211 tests pass** (was 170 end of plan 01 → +41 new)
  - `dl-ledger --lib` — 28 tests (format, error, hash, entry, writer, reader, summary)
  - `dl-ledger --test ledger_roundtrip` — 7 tests
  - `dl-ledger --test ledger_props` — 4 proptest cases × 64 cases
  - `dl-ledger --test int_only_no_fractional` — 1 guard test
- Float-free guard now active for **5 crates** (dl-feed, dl-state, dl-detect, dl-sim, **dl-ledger**)

## Plan deviations

1. **`bincode` added (workspace not yet present)** — the plan said "bincode workspace dep already there". I added `bincode = "1"` to dl-ledger's `dependencies`. The workspace does not yet have a top-level `[workspace.dependencies] bincode`. Future plans (Phase 6 reconciliation) should hoist this to the workspace.
2. **`rusqlite` / `parking_lot` NOT added** — the plan listed them as "if needed for testing". The in-memory buffer is sufficient for round-trip + corruption tests; tests pass in <0.01s without them. Adding a SQLite dep would require a `bundled` build of the C library; deferred until the Phase 6 reconciliation plan, where actual SQL queries are needed.
3. **`serde.workspace = true` added to dl-core** — dl-core already had `serde = "1"` directly, not via workspace. Standardized to workspace during this commit.
4. **`format_spec()` updated to include the literal magic string** — the spec text used to say `magic(8)`; the roundtrip test asserted `DLD-LDG1` in the spec. Both are now present (`magic(8 bytes, "DLD-LDG1")`).
5. **No `dl-app` wiring this plan** — the plan called for an end-to-end demo (build a Cycle → simulate → evaluate → ledger entry) before this plan, but no integration in `dl-app`. The integration test lives in `tests/ledger_roundtrip.rs::eval_outcome_constructor_integration`. `dl-app` wiring is deferred to plan 03 or Phase 6.

## AC checklist

- AC-1 — no floats: ✓ (5th guard in dl-ledger, scanning `src/` for `f32/f64/f16/bf16`, skipping doc comments so the guard can describe itself)
- AC-2 — magic prefix: ✓ (rejected via `BadMagic` in 7 round-trip tests)
- AC-3 — schema version 2: ✓ (rejected via `SchemaMismatch` when reading schema 1)
- AC-4 — bincode round-trip: ✓ (entry -> write -> read -> entry, 7 round-trip tests, 64 proptest cases)
- AC-5 — truncated frames: ✓ (`read_entry` returns `Err(Truncated)` for partial payload, partial length)
- AC-6 — decision driven by conservative only: ✓ (Decision::from_ev uses only `conservative.e_pnl > 0`; proptest covers positive/zero/negative cases)
- AC-7 — deterministic hash: ✓ (FNV-1a 64, 4 in-file tests + display hex format)
- AC-8 — overflow-detecting summary: ✓ (`LedgerError::Math` on `checked_add` overflow, defensive)
- AC-9 — replay: ✓ (LedgerReader is the replay API; 7 round-trip + corruption tests)
- AC-10 — end-to-end demo: DEFERRED (test in `tests/ledger_roundtrip.rs` covers the data path; `dl-app` integration is out of scope for plan 02)

## Key design decisions

- **Magic prefix `DLD-LDG1`** (8 bytes) — distinct from `DLF-CAP1` (capture) so a single tool can disambiguate by magic. Explicitly tested.
- **Schema v2** — leaves room for a v1 (in-memory only) if Phase 6 needs it; v2 is the persisted format. v0 was "no ledger".
- **`LedgerEntry` stores `LedgerHash`, not the full `Cycle`** — keeps the entry small (~200 bytes) and stable across formats. The full cycle is re-derivable from a capture replay. Trade-off: a reader without the capture cannot reconstruct the cycle. For v1.0 paper, this is fine.
- **`serde::Serialize/Deserialize` on domain types** — bincode needs them. All Phase 4 + Phase 5 domain types now have them. Cost: 1 workspace dep (serde, already transitively present via bincode). No runtime cost (bincode writes raw bytes).
- **FNV-1a 64 hand-rolled, not `DefaultHasher`** — `DefaultHasher` is randomized per process (RandomState seed), which would break AC-1 determinism across runs. FNV-1a is ~10 lines of code.
- **No terminator frame** — EOF on the file signals end. Adding a terminator would require either (a) writing it from `into_inner()`, or (b) padding the schema. Both add complexity for no benefit; a short read returns None and the reader stops.
- **`bincode` at the dl-ledger boundary only** — domain types stay clean (no bincode types leak). The dependency is a one-line `bincode::serialize(entry)` call.
- **`Decision::from_ev` is in dl-ledger, not in dl-sim** — the gate semantics ("is this an opportunity we'd actually paper-trade?") are ledger-level. dl-sim just produces numbers; the ledger interprets them.

## What this unblocks

- Phase 5 plan 03 (if planned) — `dl-app` can now persist every evaluated opportunity to a `*.dld` file. The full pipeline (capture → detect → simulate → evaluate → ledger) is one writer call away.
- Phase 6 reconciliation — given a `*.dld` ledger, the replayer can rebuild the entry stream and compare against on-chain outcomes.
- Calibration (Phase 6) — the `optimistic` and `conservative` EV bounds are recorded for every entry, so the calibration pass can re-score old entries under new defaults without needing a fresh capture.

## What remains

- `dl-app` integration (open a ledger, write an entry per opportunity, close on shutdown) — small change to `crates/dl-app/src/main.rs`. Estimated ~50 lines.
- Phase 6 plan — calibration + reconciliation against on-chain data, fed by these `*.dld` files.
- Phase 7 plan — observability, rate limiting, runtime safeguards.

## Velocity

- Plans completed in Phase 5: 2/2.
- Total plans completed: 6 (01-01, 02-01, 02-02, 04-01, 05-01, 05-02).
- Total commits on main: 4 (Phase 5), 5 (Phase 4), 2 (Phase 3), 1 (Phase 2), 1 (Phase 1).
- Tests at end of Phase 5 plan 02: 211 (was 0 at start of Phase 1).
