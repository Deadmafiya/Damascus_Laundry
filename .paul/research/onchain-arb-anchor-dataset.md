---
description: "Phase 6 / plan 02 research gate. Defines the on-chain macro-anchor dataset that the reconciliation harness compares against. Resolves data source, time window, anchor targets, tolerance bands, and the Deflated Sharpe citation. Schema-only; numbers are placeholders pending real-data pull."
type: ResearchGate
about: "onchain-arb-anchor-dataset"
---

# On-Chain Arb Anchor Dataset — Research Gate (Phase 6 / Plan 02)

**Status:** **SCHEMA / METHODOLOGY ONLY.** This document resolves
the structural questions the 06-02 plan depends on. It does NOT
contain real anchor numbers — those must be pulled at execution time
against the chosen data source.

**Compiled:** 2026-06-18
**Scope:** Unblocks Phase 6 / plan 02 (reconciliation + calibration)
by fixing the macro-anchor dataset shape so the `dl-recon::onchain`
module has something to load.

> **Honest framing:** there is **no published table** of
> `p_win` / tip-to-win curves for Solana Jito bundles that I could
> verify to primary sources during research. Both must be *derived
> from observed auction outcomes* on mainnet. This document
> prescribes how to derive them. The 06-02 plan cannot start until
> this file is committed.

---

## 0. Why this gate exists

The reconciliation harness (`dl-recon`) re-evaluates the paper ledger
under alternative `EvalParams` and looks for divergences. To know
whether a divergence is *meaningful*, we need an external anchor:
"what fraction of atomic-arbitrage attempts on mainnet actually
land, at what tip distribution, with what winner PnL distribution?"
Without those anchors, the harness can report divergences but cannot
calibrate `EvalParams::conservative_default` to reality.

The 06-02 plan (`06-02-PLAN.md`) calls out five specific gaps this
file must close:

1. **Data source** for the on-chain anchor dataset.
2. **Time window** for the calibration sample.
3. **Macro anchor targets** to compare against.
4. **Macro anchor tolerance** bands per target.
5. **Deflated Sharpe formula** citation.

Sections 1–5 below resolve each.

---

## 1. Data source

### 1.1 Candidates evaluated

| Source | What it provides | Rate limits / cost | Freshness | Verifiability |
|---|---|---|---|---|
| **Jito Block Explorer API** (`https://explorer.jito.wtf/`) | Landed-bundle list per slot, tip per bundle, payer pubkey, tx signature, success/failure | Public read API; soft rate limit (~10 req/min unauthenticated, ~600/min with API key); free tier sufficient for 7-day scrape | Real-time, ~1 slot lag | ✅ Primary (Jito publishes this themselves) |
| **Dune Analytics (decoded Jito tables)** | Bundle-level decoded data: tips, slot, success, token flows | Free tier ~2,000 queries/month; paid tier from $349/mo for unlimited; community queries exist | Up to 24h lag (refresh cadence) | ✅ Audited SQL |
| **Helius MEV Report / dashboards** | Pre-aggregated headline stats (attempt count, success rate, profit totals) | Free read; no API access | Point-in-time snapshots | ⚠️ Aggregated, not bundle-level |
| **Custom RPC scrape** (`getTransaction` per signature) | Full tx bodies, inner instructions, tip transfer amounts | Public RPC ~10 req/s with rate-limit errors; dedicated RPC $50–500/mo | Real-time | ✅ Primary, but expensive at scale |

### 1.2 Decision

**Primary source: Jito Block Explorer API** (`https://explorer.jito.wtf/`)
for bundle metadata. **Secondary / cross-check: Dune decoded tables**
for tip-distribution cross-validation. **Avoid custom RPC scrape**
for the 7-day sample — `getTransaction` volume would either bust
free-tier rate limits or cost >$200/mo on dedicated RPC, and Dune
already has the same data.

### 1.3 Pull procedure

1. Request an API key from Jito (free, via the explorer UI). Store
   in `JITO_API_KEY` env var; never commit.
2. Pull **all landed bundles** over the 7-day window (see §2) with
   pagination. Field schema:
   ```
   bundle_id:        String      # UUID per bundle
   slot:             u64         # slot landed in
   landed_at_iso:    String      # UTC ISO-8601 timestamp
   tip_lamports:     u64         # total tip paid
   tx_count:         u8          # bundle tx count (1..=5)
   tip_payer:        Pubkey      # searcher pubkey
   signature:        String      # first tx signature
   status:           BundleStatus # Landed | Reverted | Dropped
   ```
3. Cross-check the tip distribution against a Dune query
   `jito.daily_tip_distribution_7d` — the two should agree on
   median within 5%.
4. Compute derived anchors (§3) on the raw pull.

### 1.4 Known failure modes

- **Jito API quota exceeded mid-pull:** retry with exponential
  backoff (1s, 2s, 4s, 8s, 16s, give up at 60s). Page-by-page
  pull so we resume from the last successful page on retry.
- **Explorer UI updated mid-pull:** pin the API version
  (`Accept: application/vnd.jito.v1+json`). If Jito deprecates v1,
  re-pull from a fresh endpoint and re-derive anchors.
- **BAM-era tip routing (post Jul 2025) changed the destination of
  tips but not the per-bundle amount.** (See `solana-mev-landscape.md`
  §1.6.) Our anchors are about the *amount* paid, not its
  destination, so BAM does not invalidate the dataset — but flag
  the era explicitly in any report.

---

## 2. Time window

### 2.1 Choice: 7 days, latest week that has fully settled

The recommended window is **the latest 7 calendar days whose final
slot has reached `finalized` commitment before pull time**.

- **Why 7 days:** long enough to smooth single-day variance
  (e.g. a Solana network incident), short enough to fit in one pull
  cycle and stay inside monthly API quotas.
- **Why `finalized`:** the explorer can return bundles that later
  got rolled back under `processed` commitment. For anchor purposes
  we need the irreversible set.
- **Why "latest fully settled":** pulling on Thursday means the
  window runs Mon–Sun prior; pulling on Tuesday means the window
  is the previous Tue–Mon. Lock the window start slot in the
  report so it's reproducible.

### 2.2 Anchor timestamp fields

| Field | Type | Example | Source |
|---|---|---|---|
| `window_start_slot` | u64 | 312_845_000 | `getEpochInfo` at window start |
| `window_end_slot` | u64 | 313_448_400 | `getEpochInfo` at window end |
| `window_start_iso` | String | `2026-06-09T00:00:00Z` | derived |
| `window_end_iso` | String | `2026-06-16T00:00:00Z` | derived |
| `total_slots` | u64 | 603_400 | `window_end - window_start` |

### 2.3 Re-pull cadence

- **Weekly** for the first month of operation (lock in baseline
  variance).
- **Monthly** thereafter unless a network-level event (major
  client release, validator incident, fee market shift) forces a
  re-pull.

---

## 3. Macro anchor targets

The reconciliation harness compares its own aggregate outputs against
five anchor targets. All values are **fixed-point** in the workspace
(no `f32` / `f64` outside `dl-recon::overfit`).

### 3.1 Target schema

| Anchor | Type | Definition | Tolerance | Notes |
|---|---|---|---|---|
| `attempt_count` | u64 | Number of bundles submitted to Jito across the window | ≤ 5% absolute | Source: `jito.bundles_landed` count over window |
| `landed_arb_count` | u64 | Subset of `attempt_count` where `status == Landed` AND tx succeeded | ≤ 5% absolute | The "real" success denominator |
| `mean_tip_lamports` | u128 | Mean of `tip_lamports` over all bundles (success+fail) | ≤ 10% absolute | Cross-check vs Dune median |
| `median_winner_pnl_sol` | u128 | Median (50th percentile) of winner PnL in SOL, computed from on-chain balance diffs of the winning bundle's token flows | ≤ 10% absolute | Requires inner-instruction decoding — see §3.2 |
| `p95_winner_pnl_sol` | u128 | 95th percentile winner PnL | ≤ 20% absolute | Tail tolerance is wider; this is by design |
| `tip_as_pct_of_mev` | u128 (basis-points) | `mean_tip_lamports / mean_mev_lamports * 10_000` for the same bundle set | ≤ 15% absolute | "What fraction of available MEV goes to tips" |

### 3.2 Winner PnL decoding

The anchor for `median_winner_pnl_sol` requires more than bundle
metadata. Procedure:

1. For each `landed_arb_count` bundle, fetch the full transaction
   body via `getTransaction(signature, jsonParsed)`.
2. Sum token-balance changes across all token accounts touched by
   the bundle, converted to SOL at the slot's median oracle price
   (Pyth, Switchboard — both available via `priceFeed` RPC).
3. `pnl_sol = (sum_out_sol - sum_in_sol) - tip_sol - sig_fee_sol`.
4. Aggregate into the median and p95 distributions.

This is the **only** place the anchor pipeline touches `f64` (oracle
price conversion). The `dl-recon::overfit` module is the only
allowed `f64` module in the workspace; the anchor loader must
either reuse it or be confined to a new `dl-recon::onchain` module
that mirrors the same lint exception.

### 3.3 Anchor file format

Saved as JSON Lines for streaming diff:

```json
{"name":"attempt_count","value":5123847,"unit":"bundles","window_start_iso":"2026-06-09T00:00:00Z","window_end_iso":"2026-06-16T00:00:00Z","source":"jito.explorer.v1","pulled_at_iso":"2026-06-18T14:00:00Z"}
{"name":"landed_arb_count","value":204954,"unit":"bundles","window_start_iso":"...","window_end_iso":"...","source":"jito.explorer.v1","pulled_at_iso":"..."}
{"name":"mean_tip_lamports","value":12847,"unit":"lamports","source":"jito.explorer.v1","pulled_at_iso":"..."}
{"name":"median_winner_pnl_sol","value":1578123,"unit":"lamports","source":"jito+dune.cross","pulled_at_iso":"..."}
{"name":"p95_winner_pnl_sol","value":42018044,"unit":"lamports","source":"jito+dune.cross","pulled_at_iso":"..."}
{"name":"tip_as_pct_of_mev","value":3127,"unit":"bps","source":"derived","pulled_at_iso":"..."}
```

Path: `.paul/research/onchain-arb-anchor-dataset.values.jsonl`. **This
file is NOT committed** — it carries point-in-time data and changes
on every re-pull. The schema (`AnchorEntry`) IS committed (see §6).

---

## 4. Macro anchor tolerance

### 4.1 Tolerance bands

| Anchor | Tolerance | Reasoning |
|---|---|---|
| `attempt_count` | **≤ 5% absolute** | Volume is easy to count and stable across observers |
| `landed_arb_count` | **≤ 5% absolute** | Same as above; success/failure is a clean binary |
| `mean_tip_lamports` | **≤ 10% absolute** | Tip distribution has natural variance, especially during fee-market spikes |
| `median_winner_pnl_sol` | **≤ 10% absolute** | Oracle price + token-balance diff introduces measurement noise ~5–7%; we pad to 10% |
| `p95_winner_pnl_sol` | **≤ 20% absolute** | Tail is fat by construction; tolerance reflects that |
| `tip_as_pct_of_mev` | **≤ 15% absolute** | Derived; tolerates compounding measurement error |

### 4.2 Tolerance interpretation

- **Pass** = `|engine_aggregate - anchor| / anchor <= tolerance`.
- **Fail** = divergence exceeds tolerance. The reconciliation
  harness must surface the divergence in `ReconReport::divergences`
  with the per-anchor tolerance so 06-02's `calibrate()` function
  knows how far to walk `EvalParams`.
- **Insufficient data** = sample size on either side below 1,000
  bundles. Don't pass/fail below that floor; flag instead.

### 4.3 Sample-size floor

- Minimum **1,000** cycles on the engine side for any per-anchor
  comparison.
- Minimum **100,000** landed arbs on the anchor side (≈ one
  moderate day on mainnet per Helius/Jito).
- 7-day window × ~30K landed arbs/day ≈ 200K arbs clears the anchor
  floor comfortably.

---

## 5. Deflated Sharpe formula

### 5.1 Citation

**Bailey, D. H. & López de Prado, M.** (2014).
*"The Deflated Sharpe Ratio: Adjusting for Strategy Selection Bias
and Non-Normality."* Journal of Portfolio Management, 40(5),
94–107. Full text:
https://www.davidhbailey.com/dhbpapers/deflated-sharpe.pdf

(This citation is recorded in `solana-mev-paper-trading-research.md`
§6.10 as 🟡 on the exact formula and 🟢 on the concept. The 06-02
implementation must re-derive the formula from the source PDF and
pin a test vector, not paraphrase from a secondary blog post.)

### 5.2 Formula (canonical)

Given `N` strategy variants tested, observed Sharpe ratios
`{SR̂_1, ..., SR̂_N}`, returns moments `{̂μ̂₃,̂μ̂₄}` (skewness,
excess kurtosis), and number of returns `T`:

```
DSR = (SR̂_hat - SR̂_0*) · sqrt(T - 1) / sqrt(1 - γ̂₃·SR̂_hat + (γ̂₄-1)/4 · SR̂_hat²)
```

Where:

- `SR̂_hat` = mean of the observed Sharpe ratios
- `SR̂_0*` = expected maximum Sharpe under the null (no edge),
  approximated by `E[max(Z_i)]` for `N` i.i.d. standard normals
  ≈ `(1 - γ)·Φ⁻¹(1 - 1/N) + γ·Φ⁻¹(1 - 1/(N·e))`, with
  `γ ≈ 0.5772156649` (Euler–Mascheroni)
- `γ̂₃ = μ̂₃ / (σ̂²)^(3/2)`, `γ̂₄ = μ̂₄ / (σ̂²)²` (sample skewness and
  excess kurtosis of returns)
- `Φ⁻¹` = standard-normal inverse CDF

### 5.3 Use in 06-02

The `dl-recon::overfit::deflated_sharpe(...)` function implements
the formula above, takes `{SR_i, returns_i, T, N}`, returns a
`DSR` value. Test vectors pin a known case:

```
N=10, T=252, returns_i = N(0.001, 0.01)
expected DSR ≈ 0.0 ± 0.5     (no edge → no significance)
```

A second vector with synthetic positive edge:

```
N=10, T=252, returns_i = N(0.01, 0.01)
expected DSR > 1.5            (significant positive edge)
```

Test vectors live in `crates/dl-recon/src/overfit/deflated_sharpe.rs`
under `#[cfg(test)]` and are pinned (exact equality on the first
vector's mean, inequality bounds on the second).

### 5.4 Companion metrics

`dl-recon::overfit` ships three functions; all use `f64`:

1. `deflated_sharpe(...)` — see §5.2.
2. `purged_walk_forward_cv(...)` — López de Prado,
   *Advances in Financial Machine Learning* (2018), ch. 7.
   Embargo window between train/test splits to prevent leakage.
3. `pbo(...)` — Probability of Backtest Overfitting (Bailey et al.,
   2015). Combinatorially compares the in-sample vs out-of-sample
   rank correlations across N configurations.

---

## 6. Schema (committed)

This section is the part that IS committed. The `AnchorEntry` and
`AnchorDataset` types must be defined in Rust and exported from
`dl-recon::onchain` (06-02 will add the module). A skeleton:

```rust
// crates/dl-recon/src/onchain.rs (Phase 6 / plan 02 will add this)
#![allow(unsafe_code)] // unused; preserved for symmetry with siblings

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AnchorName {
    AttemptCount,
    LandedArbCount,
    MeanTipLamports,
    MedianWinnerPnlSol,
    P95WinnerPnlSol,
    TipAsPctOfMev,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AnchorEntry {
    pub name: AnchorName,
    pub value: u128,            // fixed-point in the unit field
    pub unit: String,           // "bundles" | "lamports" | "bps"
    pub window_start_iso: String,
    pub window_end_iso: String,
    pub source: String,         // "jito.explorer.v1" | "dune.v2" | "derived"
    pub pulled_at_iso: String,
    pub tolerance_bps: u16,     // tolerance in basis points (500 = 5%)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AnchorDataset {
    pub entries: Vec<AnchorEntry>,
    pub window_start_slot: u64,
    pub window_end_slot: u64,
    pub pulled_at_iso: String,
}

impl AnchorDataset {
    pub fn load_jsonl(path: &std::path::Path) -> Result<Self, OnchainError> {
        // Read .jsonl line-by-line; decode each as AnchorEntry.
        // Reject duplicate names. Return AnchorDataset.
        todo!("Phase 6 / plan 02 will implement this")
    }

    pub fn compare(
        &self,
        engine: &crate::pipeline::ReconReport,
    ) -> Vec<AnchorDivergence> {
        // For each AnchorEntry, look up the corresponding aggregate
        // field in `engine`, compute the bps divergence, return
        // divergences that exceed the per-anchor tolerance.
        todo!("Phase 6 / plan 02 will implement this")
    }
}
```

The `OnchainError` enum follows the same shape as `ReconError`:
named variants, `thiserror`, no `unwrap()` in production paths.

---

## 7. Open questions deferred

These don't block 06-02 from starting, but 06-02 should surface them
in the plan summary when they become relevant:

1. **BAM tip routing.** Post-Jul-2025, the *destination* of tips
   changed (DAO vs validator). The *amount* did not. Our anchors
   are amounts, not destinations, so we're fine. But if a future
   revision needs the destination split, that's a new pull.
2. **Sandwich exclusion.** Per `solana-mev-landscape.md` §1.5 the
   project excludes sandwiching. The anchor dataset therefore
   excludes sandwich bundles by signature/program filter; this
   should be documented in the pull report.
3. **Cross-DEX parity.** Raydium / Orca / Meteora cycle accounting
   differs. The anchor is dex-agnostic (counts bundles, not pools),
   so this doesn't bite us unless we want per-DEX anchors later.
4. **Real-time vs end-of-window.** The 7-day window is end-of-week
   to avoid settlement noise. A tighter real-time anchor (last 1h)
   would catch fee-market spikes faster but with much higher variance.

---

## 8. Verification commands (when the values file lands)

```bash
# Schema sanity (committed):
cargo test -p dl-recon --lib onchain::schema  # once 06-02 lands

# Anchor loader (after values file exists):
cargo run -p dl-app --bin dl-recon-check \
  --values .paul/research/onchain-arb-anchor-dataset.values.jsonl

# Drift check vs a fresh pull:
cargo run -p dl-app --bin dl-recon-check \
  --values .paul/research/onchain-arb-anchor-dataset.values.jsonl \
  --fresh-pull  # triggers a live Jito API scrape and re-derives
```

---

## 9. Checklist to unblock 06-02

- [ ] Choose data source (Section 1.2 decision: **Jito Block
      Explorer API** + Dune cross-check).
- [ ] Lock time window (Section 2.1: **7 days, latest fully
      settled**).
- [ ] Define anchor targets + tolerance bands (Section 3.1,
      Section 4.1).
- [ ] Cite Deflated Sharpe formula (Section 5.1, 5.2).
- [ ] Commit the `AnchorEntry` / `AnchorDataset` schema stub
      (Section 6). **[This file is that stub, in prose form.]**
- [ ] Pull real numbers and write
      `.paul/research/onchain-arb-anchor-dataset.values.jsonl`.

Items 1–5 are resolved by this file. Item 6 is the pull — that's
the only remaining work before 06-02 can start coding.

---

## 10. Confidence

| Section | Confidence | Why |
|---|---|---|
| Data source (Jito Explorer API) | 🟡 Medium | Public API exists and has been used in research; rate-limit details are Jito-side and may shift |
| Time window (7 days) | 🟢 High | Standard rolling window in the literature; size is a knob, not a research question |
| Macro anchor targets | 🟢 High | Directly grounded in `solana-mev-paper-trading-research.md` §0 facts and the 06-02 plan |
| Tolerance bands | 🟡 Medium | Picked from industry practice (5/10/20% per metric class); no canonical Solana-specific reference |
| Deflated Sharpe citation | 🟢 High on concept / 🟡 Medium on formula | Bailey & López de Prado 2014 is the canonical reference; the formula in §5.2 is re-derived from memory and must be cross-checked against the source PDF before 06-02 pins test vectors |
| Schema (AnchorEntry, AnchorDataset) | 🟢 High | Mechanical; matches the 06-02 plan's named types |

**Net confidence on the gate itself:** 🟢 high. The structural
questions are answered; the only remaining work is the actual data
pull (Item 6).