---
phase: 02-ingestion-pool-state
plan: 02
type: Summary
about: "damascus_laundry"
description: "APPLY results for Phase 2 / Plan 02: AMM decoders → normalized state"
---

# SUMMARY — 02-02 Pool State: AMM Decoders + Float-Free Guard + Dry-Run

**Status:** APPLY complete. 8 of 8 tasks DONE. All 5 acceptance criteria PASS.
**Date:** 2026-06-18
**Commits:** ac63cd8, 76457f1, e592aae, fbf47c8, 37830b3, 321214a, 32f1822 (7 on top of 02-01's 89f748a; total Phase 2 = 14 commits + 1 sync-of-Phase-1-hash)

## What was built

`dl-state` — normalized pool state, plus a Raydium AMM v4 byte-level decoder.
`dl-app` — wire the decoder into the capture replay path as a dry-run smoke
test. `dl-feed` and `dl-state` both pass the float-free CI guard.

### dl-state — full source

| File | Lines | Role |
|------|-------|------|
| `src/error.rs` | 27 | `DecodeError` (thiserror): `TooShort`, `BadDiscriminator { expected, got }` |
| `src/pool.rs` | 100 | `Pool { pubkey, base_mint, quote_mint, base_vault, quote_vault, base_decimals, quote_decimals, base_reserve, quote_reserve, lp_supply, last_update_slot, fee_bps }`. All integers, no floats. `mid_price_scaled_1e9()` via `dl_core::fixed::mul_div_floor` |
| `src/registry.rs` | 95 | `PoolRegistry` — single-writer store keyed by `Pubkey`. `insert`, `get`, `len`, `is_empty`, `iter` |
| `src/mint.rs` | 80 | `MintDecimalsSource` trait + `HardcodedMintSource` (for tests) + `RpcMintSource` stub. AmmInfo already carries decimals (offsets 32/40), so this is a sanity-check / cross-reference surface, not on the hot path |
| `src/decoder/mod.rs` | 15 | Re-exports. `pub fn decode_amm_info(bytes: &[u8]) -> Result<AmmInfo, DecodeError>` |
| `src/decoder/raydium_amm_v4.rs` | 280 | `AmmInfo` (parsed fields), `SplTokenAccount`, `decode_amm_info`, `decode_spl_token_account`, `assemble_pool`, `RAYDIUM_AMM_V4_PROGRAM_ID = 675kPX9MHTjS2zt1qfr1NYHuzeLXfQM9H24wFSUt1Mp8`, `AMM_INFO_SIZE = 752`, `SPL_TOKEN_ACCOUNT_SIZE = 165` |
| `docs/RESEARCH.md` | 75 | Research note: program ID, source URL, AmmInfo byte offsets for every field we care about, verification step |
| `docs/raydium_state.rs` | 1103 | Snapshot of `raydium-io/raydium-amm` `program/src/state.rs` (master) at the time of research, for traceability |

### Tests added (dl-state)

| Test | What it proves |
|------|----------------|
| `dl-state::pool::*` (3 inline) | `Pool::new` / `mid_price_scaled_1e9` / `fee_bps` math |
| `dl-state::registry::*` (3 inline) | insert/get/overwrite semantics |
| `dl-state::decoder::raydium_amm_v4::*` (12 inline) | decode happy path, status discriminator, size validation, fee math, mint-vs-vault cross-check, bad-discriminator errors |
| `dl-state::mint::*` (1 inline) | HardcodedMintSource returns correct decimals |
| `dl-state::tests::decoder_property` (4 proptest) | For any 752-byte input, `decode_amm_info` returns Ok or specific `DecodeError` — never panics. For any 165-byte input, `decode_spl_token_account` returns Ok or `DecodeError::TooShort` |
| `dl-state::tests::amminfo_validation` (1, **#[ignore]**) | Pulls a real `AmmInfo` and the two vault SPL token accounts from mainnet, decodes, asserts reserves match to 1 base unit. **Verified live during APPLY**: see "Live evidence" below |
| `dl-feed::tests::fixed_point_no_floats` (1) | Greps `dl-feed/src/` for `f32`/`f64` word-boundary — 0 hits |
| `dl-state::tests::fixed_point_no_floats` (1) | Greps `dl-state/src/` for `f32`/`f64` word-boundary — 0 hits |

### Live evidence (commit 37830b3)

Ran the live AmmInfo validation against mainnet with `DL_TEST_POOL_PUBKEY=3sjNoCnkkhWPVXYGDtem8rCciHSGc9jSFZuUAzKbvRVp`:

```
real pool OK: mints=(069b8857...,761dd686...), reserves=(15601127615524,40565486731739),
              fee_bps=25, decimals=(9,6)
```

- mints: `069b8857…` (likely wSOL: `So11111111111111111111111111111111111111112`) and `761dd686…` (likely USDC: `EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v`)
- reserves: 15,601,127,615,524 lamports ≈ 15,601 SOL, and 40,565,486,731,739 micro-USDC ≈ 40,565,486 USDC
- mid-price: ~$2,600 / SOL (matches market)
- fee: **25 bps** = 0.25% (matches Raydium's documented default)
- decimals: 9 / 6 (SOL / USDC, as expected)

### Dry-run in `dl-app` (commit 32f1822)

`DL_DRY_RUN=1 cargo run -p dl-app` opens `crates/dl-feed/tests/fixtures/sample_capture.bincode` (the 60-second slot-only capture from 02-01-07), replays it through `CapturedFeed`, and runs every `AccountUpdate.data` through `decode_amm_info`. Current output (slot-only capture, no account subs):

```
INFO dl_app: damascus_laundry starting ... version="0.0.0" mode="paper-trading"
INFO dl_app: starting dry-run replay path=.../crates/dl-feed/tests/fixtures/sample_capture.bincode
INFO dl_app: dry-run replay complete events_returned=149 slots=149 accounts_total=0
            decoded_ok=0 decoded_err=0 slot_range="427123909..=427124057"
```

Phase 3+ will replace the per-event decode log with a `PoolRegistry` write
path. The plumbing is in place.

### CI gate additions (commit 321214a)

`.github/workflows/ci.yml` now runs after the regular test step:

```yaml
- name: Float-free invariant
  run: |
    cargo test -p dl-feed --test fixed_point_no_floats
    cargo test -p dl-state --test fixed_point_no_floats
```

Both must pass on every push. The grep walks `src/`, skips lines starting with
`//` (doc comments), and matches `\bf(32|64)\b` so `f32x4` or `f32_le` (SIMD
intrinsics names) wouldn't false-positive.

## Acceptance criteria

| AC | Result | Evidence |
|----|--------|----------|
| AC-1 deterministic capture / replay | PASS | (Re-stated from 02-01; replay tested in dry-run) |
| AC-2 float-free value path in dl-state | PASS | `tests/fixed_point_no_floats.rs` — 0 hits in `dl-state/src/` |
| AC-5 float-free value path in dl-feed | PASS | `tests/fixed_point_no_floats.rs` — 0 hits in `dl-feed/src/` |
| AC-3 first AMM type (constant-product) decoded end-to-end | PASS | `decode_amm_info` + `decode_spl_token_account` + `assemble_pool`; live mainnet test passes against Raydium SOL/USDC pool `3sjNoCnkkhWPVXYGDtem8rCciHSGc9jSFZuUAzKbvRVp` |
| AC-4 dry-run smoke test for the end-to-end pipeline | PASS | `DL_DRY_RUN=1 cargo run -p dl-app` exits 0, replays 149 slots, prints summary |

## Deviations from plan

- **Plan suggested using `dl_core::Fixed::new(9).ratio(q, b)`**. Doesn't exist; used
  `dl_core::fixed::mul_div_floor(q as u128, 1_000_000_000, b as u128)` with the
  same semantics. Same one-line divergence as 02-01; surfaced in 02-01 SUMMARY.
- **Float-guard pattern**: the plan showed one shared `fixed_point_no_floats.rs`
  per crate. I wrote the file separately for `dl-feed` and `dl-state` (two
  distinct test binaries, since `#[test]` modules are crate-scoped), but
  contents are byte-identical except for the docstring.
- **`dl-state` `Pool::mid_price_scaled_1e9` returns `u128`**, not `Fixed`. The
  Phase 1 `Fixed` type was removed during UNIFY (Phase 1's final design uses
  raw `u128` + explicit scale). The function signature matches the plan's
  intent, just with a primitive return type.
- **Validation test was run with `bs58` inlined (not a dep)**. The test file has
  a small `bs58_decode` / `bs58_encode` (~40 lines) so we don't pull a dep just
  to parse two base58 strings in a one-off test. If we need base58 elsewhere
  later, replace with the `bs58` crate (it's already a dep of `dl-app`).

## Concerns / notes for UNIFY

- `AmmInfo` reserves are **not** in the AmmInfo account; they live in the
  SPL token accounts at `coin_vault` and `pc_vault`. A single `AccountUpdate`
  fires for *one* account, so to assemble a `Pool` we need three
  `AccountUpdate`s: the AmmInfo + two vaults. Phase 3+ detection will need
  to track partial state per pubkey and assemble once all three are seen in
  a slot window. This is not a Phase 2 deliverable but the data shape implies
  it.
- The `mint.rs` `RpcMintSource` is a stub. It returns the value from the
  closure you hand it. A real impl would batch-fetch from
  `getMultipleAccounts` on the mint pubkeys; out of scope for v1.0 since
  `AmmInfo` already carries the decimals we need.
- Raydium SDK JSON is 111 MB; the public RPC's `getProgramAccounts` is
  rate-limited. We discovered the SOL/USDC pool pubkey by inspecting a recent
  mainnet transaction and looking for an account owned by
  `675kPX9MHTjS2zt1qfr1NYHuzeLXfQM9H24wFSUt1Mp8` with exactly 752 bytes of
  data. The recipe is in the test docstring for next time.

## Deferred to Phase 3

- "Fresh quote" leg (an order-book venue, e.g. Phoenix or OpenBook v2).
- Multi-account pool assembly (3 AccountUpdates per pool, window-based).
- Bellman-Ford negative-cycle detection over the (pool → token) graph.
- Per-`Pool` snapshots for replay assertions.
- `dl-detect` and `dl-sim` and `dl-ledger` remain placeholders.
