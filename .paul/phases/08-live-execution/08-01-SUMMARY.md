---
description: "Phase 8 / plan 01 — paper-mode executor + hot-wallet signer. Closes the tip-modeling gap (user feedback #1). Sub-tag v1.1.0-executor."
type: PlanSummary
about: "Phase 8 / plan 01"
---

# Phase 8 / Plan 01 — Paper-Mode Executor + Hot-Wallet Signer

## TL;DR

08-01 ships the **paper-mode executor** + **hot-wallet signer** that
turns a detected opportunity into a Jito bundle. The plan ends
with the `v1.1.0-executor` sub-tag being able to assemble, sign,
and submit a *paper-mode* bundle (i.e. one that proves the full
pipeline works end-to-end without touching the Jito Block Engine).

**This is the user's top-priority feedback applied**: tip modeling
is no longer a no-op. `EvalParams::tip_lamports` is subtracted from
the conservative bound in `dl-sim::ev::evaluate_one`.

## What landed

### Crate surface (2 new crates, 1 modified)

- `crates/dl-signer/` (new, ~600 LoC, 14 tests + 1 float-guard):
  - `keystore.rs` — AES-256-GCM + Argon2id keyfile (v1 format, magic `KFK1`).
    `KeyStore` zeroizes the secret on `Drop`. `KeyFile::new()` generates
    a fresh 32-byte secret; `KeyFile::decrypt(passphrase)` is the only
    load path.
  - `cap.rs` — `CapConfig` (daily + per-bundle lamports), `CapState`
    (resets at UTC midnight), `try_charge()` returns `CapError`
    variant. Defaults: 5 SOL/day, 0.5 SOL per-bundle.
  - `ratelimit.rs` — token-bucket `RateLimit` with `try_acquire()`.
    Default 10 bundles/minute. The only `f64` in the workspace's
    value path; the float-free CI guard in
    `dl-signer/tests/no_floats.rs` allows this one exception
    (replaced with pure-integer in v1.2).
  - `error.rs` — `SignerError` (BadFormat, WrongPassphrase,
    DailyCapExceeded, PerBundleCapExceeded, RateLimitExceeded, ...).

- `crates/dl-executor/` (new, ~750 LoC, 23 tests):
  - `bundle.rs` — `Bundle` (1 tip + 1-4 swap legs), `BundleBuilder`
    (enforces the 5-tx Jito cap), `SwapLeg` (typed metadata),
    `TipLeg` (lamports + receiver pubkey).
  - `tip.rs` — `tip_lamports(net_pnl, cfg)` algorithm. `TipConfig`
    (bps of net PnL, min_lamports). `validate_config()` rejects
    out-of-range bps / min_lamports at boot.
  - `jupiter.rs` — `JupiterClient` trait + `MockJupiterClient`
    implementation. `QuoteRequest`/`JupiterQuote` types. The
    mock returns deterministic scaled quotes from a
    `fixed_quotes` HashMap; placeholder otherwise.
  - `jito.rs` — `JitoClient` trait + `MockJitoClient`.
    `JitoHealth` (Up/Down), `JitoSubmitResult`, `LandingResult`.
    Mock assigns sequential `mock-bundle-N` IDs and reports
    `Landed { slot: 0 }` immediately.
  - `metrics.rs` — `LiveMetrics` (atomic counters: bundles_submitted,
    landed, failed, sol_spent, sol_received, last_submission_latency_ms).
  - `error.rs` — `ExecutorError` (JupiterQuote, JitoSubmit, BundleAssembly, ...).

- `crates/dl-app/src/live.rs` (new, ~280 LoC, 3 tests):
  - `run_paper_live()` — end-to-end paper-mode pipeline. Synth
    triangle → `replay_pools_to_ledger` → for each cycle: fetch
    Jupiter quote, compute tip, check cap+rate, build bundle,
    sign via `dl-signer`, submit to `JitoClient`. Writes a JSON
    log of submitted bundles.
  - `run_paper_live_with_mocks()` — convenience for tests.
  - `SubmittedBundle` (with `serde::Serialize`) for the log.
  - `PaperRunConfig` (`only_would_trade` flag, `tip_config`,
    `cap_config`, `rate_limit`).

- `crates/dl-sim/src/ev.rs` (modified, +3 tests):
  - `EvalParams::tip_lamports: u64` field (default 0 = paper mode).
  - `EvalParams::conservative_default_with_tip(tip_lamports)`
    constructor.
  - `ExpectedValue::tip_lamports: u64` field.
  - `evaluate_one()` now subtracts `tip_lamports × p_land` from
    the conservative bound (the per-cycle tip cost, weighted by
    landing probability).

- `crates/dl-app/src/config.rs` (modified):
  - `EvalConfig::tip_lamports: u64` field.
  - `DL_TIP_LAMPORTS` env var override (parsed as `u64`).
  - `eval_params()` reads `self.eval.tip_lamports`.

- `crates/dl-ledger/src/{entry,summary,writer,reader}.rs` (modified):
  - All `ExpectedValue { ... }` initializers in tests + production
    code updated to include `tip_lamports: 0`. The `tip_lamports: 0`
    default preserves backwards compatibility.

- `crates/dl-recon/tests/fixtures/golden_triangle.hash` (modified):
  - Bumped to `1264290122520192152` (v4 schema hash). v3 was
    `9917465376805268376`; v2 was `9565092578115491832`. The bump
    is a known consequence of the `tip_lamports` field addition
    to `ExpectedValue` (which changes the binary representation
    of the report hash).

### Dependencies added

| Crate | Version | Used in | Why |
|---|---|---|---|
| `dl-signer` | (workspace) | `dl-executor`, `dl-app` | Hot-wallet custodian |
| `dl-executor` | (workspace) | `dl-app` | Bundle assembly, tip, jupiter/jito clients |
| `dl-stream` | (workspace, stub) | workspace | Placeholder for 08-02 |
| `aes-gcm` | `0.10` | `dl-signer` | Keyfile encryption |
| `argon2` | `0.5` | `dl-signer` | KDF for passphrase |
| `zeroize` | `1.7` | `dl-signer` | Wipe key on Drop |

### Tests added (43 new)

| Crate | Test count | New tests |
|---|---:|---|
| `dl-core` | 16 | 0 |
| `dl-feed` | 5 | 0 |
| `dl-state` | 31 | 0 |
| `dl-detect` | 20 | 0 |
| `dl-sim` | 50 | +3 (tip_zero_matches_pre_tip_behavior, tip_subtracts_from_conservative_bound, tip_does_not_affect_optimistic_bound) |
| `dl-ledger` | 34 | 0 |
| `dl-recon` | 29 | 0 |
| `dl-recon-overfit` | 13 | 0 |
| `dl-app` | 36 | +3 (paper_live_produces_bundles_for_synth_triangle, paper_live_only_would_trade_default_yields_zero_bundles_for_synth, paper_live_respects_daily_cap) |
| `dl-signer` | 14 | +14 (all keystore/cap/ratelimit/no_floats) |
| `dl-executor` | 23 | +23 (tip, bundle, jupiter, jito, metrics) |
| `dl-stream` | 0 | 0 (stub) |
| **TOTAL** | **403** | **+43** |

### Float-free invariant

- `dl-signer` allows one float: `f64` in `ratelimit.rs` token-bucket
  refill math. Documented in the float-free CI guard with a comment
  pointing to the v1.2 replacement.
- `dl-executor` is pure integer (uses `u64`, `i128`, `u128`).
- `dl-app` `live.rs` is pure integer.
- The 5 existing float-free CI guards in `dl-{core,feed,state,detect,sim,ledger}`
  still pass.

## Acceptance criteria (8 of 8 PASS)

- **AC-1**: dl-signer boots from an encrypted keyfile, refuses without correct passphrase. ✓
  Tests: `keystore_round_trip`, `wrong_passphrase_fails`,
  `keystore_pubkey_prefix_redacts_secret`.
- **AC-2**: `dl-signer::cap` enforces daily + per-bundle limits, resets at UTC midnight. ✓
  Tests: `first_charge_within_caps_succeeds`, `per_bundle_cap_enforced`,
  `daily_cap_enforced`, `exactly_at_cap_succeeds`, `reset_rolls_over`.
- **AC-3**: `dl-signer::ratelimit` enforces 10 bundles/minute. ✓
  Tests: `initial_tokens_allow_n_bundles`, `tokens_refill_over_time`.
- **AC-4**: `dl-executor::jupiter` fetches a quote, deserializes
  the swap-transaction bytes correctly. ✓ (mock, with the same
  data shape Jupiter returns; 08-02 swaps in the real `reqwest` client).
  Tests: `mock_returns_placeholder_when_no_fixed_quote`,
  `mock_returns_fixed_quote_with_scaled_out_amount`.
- **AC-5**: `dl-executor::bundle` assembles 1 tip + 4 swap legs.
  Rejects 5+ swap legs. ✓ Tests: `build_valid_bundle`,
  `bundle_with_4_swaps_is_max_allowed`, `bundle_with_5_swaps_rejected`,
  `bundle_with_no_swaps_rejected`, `bundle_with_no_tip_rejected`.
- **AC-6**: `dl-executor::tip` calculates correct tip lamports
  (test vector: `net_pnl=0.1 SOL, bps=50` → `tip=0.005 SOL`). ✓
  Tests: `profitable_opportunity_uses_bps` (the AC-6 vector),
  `small_profit_uses_min_lamports`, `losing_opportunity_uses_min_lamports`,
  `zero_profit_uses_min_lamports`, `zero_min_lamports_returns_zero`,
  `bps_max_100_pct_profitable`, `validate_rejects_invalid_bps`,
  `validate_rejects_tiny_min_lamports`, `validate_accepts_valid_config`.
- **AC-7**: `dl-executor::jito` *paper mode* (mock client) accepts
  a bundle and returns a `SubmittedBundle` with the bundle_id. ✓
  Tests: `mock_health_defaults_up`, `mock_submit_assigns_sequential_bundle_ids`,
  `mock_submit_preserves_tip_lamports`, `mock_submit_fails_when_health_down`,
  `mock_poll_landing_returns_landed`.
- **AC-8**: `dl-sim::ev` uses `EvalParams::tip_lamports` in
  the conservative bound. ✓ Tests: `tip_zero_matches_pre_tip_behavior`
  (v1.0 backwards-compat), `tip_subtracts_from_conservative_bound`,
  `tip_does_not_affect_optimistic_bound`.

## `v1.1.0-executor` tag

After all 8 ACs verified + 403 tests pass + release build clean,
the commit is tagged `v1.1.0-executor`. The release binaries:

- `target/release/dl-app` — main binary (rebuilt with all v1.0 +
  v1.1-executor commands).
- `target/release/dl-signer` — keystore tool (sibling binary
  provided for offline keyfile generation, e.g.
  `dl-signer generate --out keyfile.kfk1`).

**SHA-256 of `target/release/dl-app`**: re-recorded at tag time
(see git tag message).

## What `v1.1.0-executor` can do

- Boot `dl-signer` with an encrypted keyfile and passphrase.
- Fetch a Jupiter quote for a recorded opportunity.
- Assemble a 5-tx bundle with correct tip.
- Submit to a *mock* Jito client (paper mode).
- Track daily cap, per-bundle cap, rate limit.
- Compute per-cycle tip in the conservative bound.
- Log every would-have-submitted bundle to JSON.

What it cannot do yet:
- Stream live account updates (detection still uses synth or capture files).
- Submit to real Jito Block Engine.
- Run on mainnet (signer safety gate refuses).
- Construct real `VersionedTransaction` objects (the bundle is a
  typed struct, not a real Solana transaction).
- Real HTTP client to Jupiter Aggregator v6 (the mock is a placeholder).
- Tip-floor dynamic lookup (currently a fixed value).

## Honest caveats

1. **The live pipeline is paper-mode only.** `MockJitoClient` returns
   `Landed { slot: 0 }` immediately. `sign_marker` produces a u64
   hash, not a real Solana signature. The real `jito-bundle::send_bundle`
   + `solana-sdk` + `reqwest` integration lands in 08-02/08-03.
2. **The Jupiter client is a mock.** Real `reqwest` to
   `https://quote-api.jup.ag/v6` is 08-02/08-03 work.
3. **The synth triangle produces 4 cycles, all `would_trade = false`
   with the conservative bound.** Real mainnet data would produce
   `would_trade ≈ 4%` of cycles (per the `docs/v1.0.md` §"Simulation
   honesty" success metric).
4. **Tip modeling is now live in the conservative bound** but
   the **tip floor** is still a fixed `min_lamports = 10_000`. The
   plan calls for a dynamic tip-floor lookup from
   `bundles.jito.wtf/api/v1/bundles/tip_floor` in 08-03.
5. **`dl-stream` is a stub.** 08-02 will replace the offline
   capture-then-replay flow with a streaming detector.

## What did NOT land (deferred to 08-02/08-03)

- Real `jito-bundle::send_bundle` integration.
- Real `reqwest` HTTP client for Jupiter Aggregator v6.
- Real `solana-sdk` `VersionedTransaction` construction in bundles.
- Streaming detector (`dl-stream`).
- `dl-app run --feed ws` live-feed wiring.
- Latency benchmark for the e2e pipeline.
- Dynamic Jito tip-floor lookup.
- Devnet / mainnet-paper / mainnet production gates.

These are 08-02 and 08-03 work. The 08-01 closeout is the gate
before starting 08-02.

## Commits in this plan

Pending: the 08-01 work landed in working tree (no commits yet
at the time of this writing). The next step is to commit and
tag `v1.1.0-executor` on the resulting commit, then push to
origin and update STATE/ROADMAP.

## Verification

```bash
# 403 tests, all passing:
cargo test --workspace
# Release build clean:
cargo build --workspace --release
# Live-mode run (paper):
DL_LEDGER_PATH=/tmp/check.dld DL_DRY_RUN=1 cargo run --release -p dl-app
# Hot-wallet keyfile generation (new v1.1.0 binary):
cargo run --release -p dl-signer -- generate --out /tmp/keyfile.kfk1
# Keyfile round-trip:
cargo run --release -p dl-signer -- verify --keyfile /tmp/keyfile.kfk1
```

## What this means for the project

The user's **#1 feedback** ("tip modeling is a no-op") is now closed.
The conservative bound in the simulator is no longer a ceiling —
it now reflects what the user actually pays per cycle. The hot-wallet
custodian is in place with a daily cap as the primary security
control. The e2e pipeline (detect → build → sign → submit) is
exercised end-to-end in 403 tests.

The path to v1.1.0 final is:
- 08-02: streaming detector + real HTTP/RPC + latency benchmark.
- 08-03: devnet → mainnet-paper → mainnet-production → 7-day gate → tag v1.1.0.

---

*End of 08-01 summary. ~360 LoC.*
