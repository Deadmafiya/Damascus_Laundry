---
phase: 01-foundations
plan: 01
type: Summary
about: "damascus_laundry"
description: "APPLY results for Phase 1 Foundations"
---

# SUMMARY — 01-01 Foundations

**Status:** APPLY complete. 4 of 4 tasks DONE. All 4 acceptance criteria PASS.
**Date:** 2026-06-17

## What was built

A deterministic-by-construction Rust workspace for damascus_laundry.

### Crates created (7-crate virtual workspace)

| Crate | Role | Phase-1 content |
|-------|------|-----------------|
| `dl-core` | Foundations | Fixed-point math + Clock/Rng/Feed traits (fully implemented) |
| `dl-feed` | Ingestion | Placeholder (Phase 2) |
| `dl-state` | Pool state | Placeholder (Phase 2) |
| `dl-detect` | Detection | Placeholder (Phase 3) |
| `dl-sim` | Profit/cost + simulation | Placeholder (Phase 4/5) |
| `dl-ledger` | Paper ledger/metrics | Placeholder (Phase 5/7) |
| `dl-app` | Binary | `tracing` init + structured startup logs |

Root: `Cargo.toml` (workspace, shared deps), `rust-toolchain.toml` (pinned 1.94.1 +
rustfmt/clippy), `.gitignore` (target, secrets, replay artifacts), `README.md`,
`.github/workflows/ci.yml`.

### Fixed-point math API surface (`dl-core`)

- `fixed.rs`:
  - `MathError { Overflow, DivByZero, ScaleMismatch }` — `std::error::Error`.
  - `checked_add`, `checked_sub` → `Result<u128, MathError>`.
  - `mul_div_floor(value, numerator, denominator) -> Result<u128, MathError>` — the core
    primitive. Fast path uses native `u128`; wide path uses a **128×128→256-bit full
    multiply + 256/128 bitwise long division**, so it never overflows mid-computation and
    returns `Err(Overflow)` only when the true quotient exceeds `u128`.
  - `pow10(exp)`, plus internal `full_mul` / `div_256_by_128`.
- `amount.rs`:
  - `Amount { raw: u128, decimals: u8 }` — `from_base_units`, `zero`, `raw`, `decimals`,
    `checked_add`, `checked_sub`, `mul_div`, `to_scale`, `from_scale`, exact integer
    `Display`. Down-scaling that would lose nonzero low digits returns
    `Err(ScaleMismatch)` (lossless or loud).
- `display.rs`: **the only module permitted floats** — `amount_to_f64` for display/metrics.

### Injectable nondeterministic deps (`dl-core`)

- `clock.rs`: `Clock` trait (`now_millis`, `slot`); `SystemClock` (real) and `MockClock`
  (deterministic: `advance_millis`, `tick_slot`, `set_slot`). `Slot = u64`.
- `rng.rs`: `Rng` trait (`next_u64`, `next_below`); `SeededRng` = SplitMix64, sequence
  fully determined by seed (no system entropy in the deterministic path).
- `feed.rs`: `Feed` trait (`next_event`); `FeedEvent { Slot, AccountUpdate }` (minimal,
  extended in Phase 2); `ScriptedFeed` deterministic replay source + `empty()`.

## Acceptance criteria

| AC | Result | Evidence |
|----|--------|----------|
| AC-1 build + fmt + clippy + test clean | PASS | all four exit 0 |
| AC-2 u128 fixed-point, overflow-safe, lossless decimals, float-free value path | PASS | 6 property tests + float-grep clean (only doc-comment mentions) |
| AC-3 Clock/Rng/Feed injectable, deterministic under seed | PASS | `tests/determinism.rs` — two seeded runs byte-identical |
| AC-4 structured logging + CI | PASS | `tracing` in `dl-app`; `.github/workflows/ci.yml` runs fmt/clippy/test |

**Tests:** 17 passing — 9 `dl-core` unit, 2 determinism integration, 6 fixed-point property.

## Deviations from plan

- **Added `crates/dl-core/src/display.rs`** (not in the plan's file list). Reason: the
  plan's float helper `to_f64_display` lived in `amount.rs`, which would fail the AC-2
  float-grep ("no f32/f64 outside a display/format module"). Moved it to a dedicated,
  clearly-named display module — satisfies the AC's own escape clause. Net positive; no
  scope change.
- Logging was wired into `dl-app` during Task 1 (plan put it in Task 4) to avoid churn;
  Task 4 then added CI. No functional difference.

## Concerns / notes for UNIFY

- None blocking. `SystemClock::slot()` derives slot from elapsed wall-time at the nominal
  400 ms cadence — fine for live use but **never** used in replay assertions (replay uses
  `MockClock`). Worth a glance in UNIFY but not a defect.
- `next_below` uses multiply-shift (negligible bias for our ranges); documented inline.

## Deferred to Phase 2

- `FeedEvent` gains decoded pool/transaction variants.
- Real network `Feed` impl (JSON-RPC WebSocket, gRPC-ready).
- Raw feed capture-to-disk + replay-from-disk (Phase-1 `ScriptedFeed` is in-memory only).

## Files modified

```
Cargo.toml, rust-toolchain.toml, .gitignore, README.md, .github/workflows/ci.yml
crates/dl-core/{Cargo.toml, src/{lib,fixed,amount,display,clock,rng,feed}.rs,
                tests/{fixed_props,determinism}.rs}
crates/dl-feed/{Cargo.toml, src/lib.rs}
crates/dl-state/{Cargo.toml, src/lib.rs}
crates/dl-detect/{Cargo.toml, src/lib.rs}
crates/dl-sim/{Cargo.toml, src/lib.rs}
crates/dl-ledger/{Cargo.toml, src/lib.rs}
crates/dl-app/{Cargo.toml, src/main.rs}
```
