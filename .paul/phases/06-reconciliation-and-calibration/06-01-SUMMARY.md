description: "Phase 6 / plan 01 — golden-file replay + DST fault injection + invariants. Recon harness (dl-recon) implemented and applied."
type: PlanSummary
about: "Phase 6 / plan 01"
---

# Phase 6 / Plan 01 — Summary

## What landed

The `dl-recon` crate is now in the workspace. It provides an offline
reconciliation harness that re-reads a captured Solana account stream,
re-runs detection → sizing → EV → ledger under alternative `EvalParams`,
and returns a structured `ReconReport` with divergences.

### Crate surface (7 source files)

- `src/lib.rs` — module map, public re-exports, integer-only invariant docs.
- `src/error.rs` — `ReconError` with named variants (Decode, Detect, Sim,
  Ledger, Math, Capture, UnknownAccountSize).
- `src/pipeline.rs` — `replay_pools_to_ledger`, `replay_capture_to_ledger`,
  `pools_from_feed`, `CycleRecord`, `ReconReport`, `Divergence`, `ReplayParams`.
- `src/fault.rs` — `JitterRng`, `BoundedDrop`, `BoundedCorrupt`,
  `JitteredSlot`, `Reorder` (with `ReorderMode::{Reverse,Permute}`),
  `Capped`, `FaultConfig`.
- `src/invariants.rs` — `check_i1..check_i6` property assertions.
- `src/fixture.rs` — `SynthPoolSpec`, `synthesize_pools`,
  `synthesize_small_capture`, `synthesize_small_ledger`, `ReconFixture`.

### Tests added (5 new files, 42 new tests)

- `src/pipeline.rs::tests` — 5 tests: determinism, empty/universe/single/triangle cases.
- `src/fault.rs::tests` — 9 tests: each middleware + RNG determinism.
- `src/invariants.rs::tests` — 6 tests: I-1, I-3, I-4, I-5, I-6 across scenarios.
- `src/fixture.rs::tests` — 3 tests: pool synthesis, capture round-trip, fixture bundle.
- `tests/golden_replay.rs` — 5 tests: golden hash check + fixture round-trip.
- `tests/dst_faults.rs` — 11 tests: per-fault determinism + combined stack + capture-path tests.
- `tests/floats.rs` — 3 tests: integer-only guard + bare-token scanner + lib.rs doc check.

Total workspace tests: **253** (was 211). All pass.

### Test fixtures

- `tests/fixtures/golden_triangle.hash` — committed FNV-1a 64 hash
  (`9565092578115491832`) for the canonical triangle pool universe.
- Inline synthetic pool builders in `fixture.rs` produce pools,
  captures, and ledgers without external data.

### Invariants enforced

- **I-1 Determinism**: same `(pools, params)` → byte-identical report.
  Verified via `check_i1_determinism` and `replay_with` in DST tests.
- **I-2 Integer-only**: no `f32` / `f64` in recon source. Verified at
  test time by `tests/floats.rs::no_floats_in_recon_sources`.
- **I-3 Schema enforcement**: opening a v2+1 ledger returns
  `LedgerError::SchemaMismatch`. Verified by
  `check_i3_schema_enforcement`.
- **I-4 No silent skips**: `ReconReport::divergences` is the complete
  set of mismatches. In 06-01 it is always empty; 06-02 will populate
  it with source-ledger diffs.
- **I-5 EOF terminates**: ledger reader returns `None` after N
  entries; no terminator frame is required. Verified by
  `check_i5_eof_terminates`.
- **I-6 Hash match**: the recorded `report_hash` matches the
  recomputed FNV-1a 64 walk. Verified by `check_i6_hash_match`.

### Workspace changes

- `Cargo.toml` (workspace): added `bincode = "1"` and `thiserror = "1"`
  to `[workspace.dependencies]`; added `dl-recon` to members.
- `crates/dl-ledger/Cargo.toml`: switched from `bincode = "1"` to
  `bincode.workspace = true`.

## What did NOT land

- **Real capture path**: `replay_capture_to_ledger` works, but the
  AmmInfo+vault assembly is driven by synthetic blobs in fixtures. A
  full mainnet `.dlf` round-trip is deferred to 06-02 once the on-chain
  anchor dataset is available.
- **Divergence population**: `ReconReport::divergences` is empty in
  06-01 by design. The 06-02 plan will populate it from a real
  source-ledger comparison.
- **CLI mode for the recon harness**: the harness is library-only
  today; a `dl-app recon` mode is a Phase 7 concern.

## Build status

```
cargo build --workspace          → green
cargo test --workspace           → 253 passed, 0 failed
cargo clippy -p dl-recon --all-targets → 11 cosmetic warnings, 0 errors
cargo fmt --all                  → no diff
```

## Verification commands

```
cd /home/deadmafia/Documents/damascus_laundry
cargo test -p dl-recon --lib              # 23 tests, lib
cargo test -p dl-recon --test golden_replay  # 5 tests, golden hash
cargo test -p dl-recon --test dst_faults     # 11 tests, DST
cargo test -p dl-recon --test floats         # 3 tests, I-2 guard
```

## Next plan

06-02 is BLOCKED on writing `.paul/research/onchain-arb-anchor-dataset.md`.
That research doc defines the on-chain anchor dataset that the
reconciliation harness will compare against. Until it lands, 06-02
cannot proceed.