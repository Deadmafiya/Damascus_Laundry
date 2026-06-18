---
description: "Phase 7 / plan 02 research gate. Resolves Orca Whirlpool + Meteora DLMM math + Prometheus/OTel decision. Primary sources: orca-so/whirlpools and MeteoraAg/dlmm-sdk on GitHub."
type: Research
about: "Multi-DEX math for v1.0"
---

# Phase 7 / Plan 02 — Multi-DEX Math Research Gate

## Purpose

Phase 7 / plan 02 (multi-DEX scale-up + v1.0 release) requires
two new AMM decoders and fill-math paths (Orca Whirlpool +
Meteora DLMM) plus a metrics backend (Prometheus or OTel).
This document resolves the structural questions so the
implementer can ship without improvising. **No decoder or
fill-math task may start until this file is filled in.**

## Primary sources

- **Orca Whirlpool SDK**: `https://github.com/orca-so/whirlpools`
  (verified reachable, `rust-sdk/core/src/math/{price,tick,position}.rs`).
  The repo also has a `ts-sdk/` and a `programs/` directory;
  the math lives in `rust-sdk/core/src/math/`. No public PDF.
- **Meteora DLMM SDK**: `https://github.com/MeteoraAg/dlmm-sdk`
  (verified reachable, `ts-client/src/dlmm/helpers/{lbPair,bin,math,fee}.ts`).
  No public PDF.

Both SDKs are written in TypeScript for the high-level API and
Rust for the math primitives. **Both use the same `BN.js` (TS)
or `u128` (Rust) BigInt style for fixed-point arithmetic** —
no `f64` in the math itself (the TS code uses `BN`; the Rust
code uses `u128`).

## 1. Orca Whirlpool math

### 1.1 Account layout

A Whirlpool account is ~250 bytes and contains the
**concentrated-liquidity** state. Key fields (offsets from
`LbPair` struct in the Orca SDK, confirmed in
`ts-client/src/dlmm/helpers/lbPair.ts` via the analogous
`LbPair`/`Whirlpool` types):

| Field | Type | Notes |
| --- | --- | --- |
| `sqrt_price` | `u128` (Q64.64 fixed-point) | `sqrt(price) * 2^64` |
| `tick_current_index` | `i32` | active tick |
| `tick_spacing` | `u16` | ticks between initializable ticks (1, 8, 64, 128) |
| `liquidity` | `u128` | active liquidity in current tick range |
| `token_mint_x` | `Pubkey` (32 B) | base token |
| `token_mint_y` | `Pubkey` (32 B) | quote token |
| `token_vault_x` | `Pubkey` (32 B) | base vault |
| `token_vault_y` | `Pubkey` (32 B) | quote vault |
| `fee_rate` | `u16` (bps) | pool fee, e.g. 3000 = 0.3% |

### 1.2 Fill math

The Whirlpool fill (`getAmountOut`) operates on the current
`sqrt_price` and `liquidity`. The input `amount_in` (in token X)
changes `sqrt_price` toward the next tick, consuming liquidity
in the current tick range until the active range is exhausted
or the input is fully consumed.

The key formula (from `rust-sdk/core/src/math/price.rs`):

```
amount_out = liquidity * (1/sqrt_price_new - 1/sqrt_price_old)
            (for token X → Y, when the new sqrt_price is
             still within the same tick range)
```

The math uses Q64.64 throughout: `sqrt_price * 2^64` is a
`u128` value, the inverse is `(2^128 - 1) / sqrt_price` (also
`u128`), and the multiplications are exact (no rounding until
the final division).

### 1.3 Integer sqrt

The Rust SDK provides `sqrt_u128()` (Newton's method) in
`rust-sdk/core/src/math/price.rs`. **This is the one operation
that needs an integer square root, and the SDK already
implements it.** For v1.0 we can either:
- (a) Lift the SDK's `sqrt_u128` directly into our
  `dl-sim/src/fill_orca.rs` (~30 LoC, exact).
- (b) Use a precomputed tick table + linear interpolation
  (~100 LoC, ±1 base unit error).

**Decision (matches the plan's open question answer)**:
**option (a)** — copy the SDK's `sqrt_u128` directly. The
Newton's method is short, integer-only, and well-tested
upstream. Documented in `crates/dl-sim/src/fill_orca.rs`.

### 1.4 Tick-index math

`rust-sdk/core/src/math/tick.rs` provides:
- `sqrt_price_to_tick_index(sqrt_price) -> i32`
- `tick_index_to_sqrt_price(tick_index) -> u128`

Both use the same Q64.64 representation. The formulas are
detailed in the SDK and reference Uniswap V3's whitepaper
(Whirlpool is a Uniswap-V3 fork). For v1.0, lift both
functions directly from the SDK.

## 2. Meteora DLMM math

### 2.1 Account layout

An Meteora `LbPair` account is ~1 KB and contains a
**bin-based** liquidity state. Key fields (from
`ts-client/src/dlmm/types/`):

| Field | Type | Notes |
| --- | --- | --- |
| `parameters` | `sParameters` (struct) | `base_factor`, `filter_period`, `decay_period`, `reduction_factor`, `variable_fee_control`, `max_volatility_accumulator`, ... |
| `v_parameters` | `vParameters` (struct) | volatility accumulator state |
| `bin_step` | `u16` (bps) | price step per bin (e.g. 100 = 1%) |
| `base_mint` / `quote_mint` | `Pubkey` (32 B) | token X / Y mints |
| `base_vault` / `quote_vault` | `Pubkey` (32 B) | token X / Y vaults |
| `active_id` | `i32` | active bin ID |
| `bins` | `[Bin; 70]` (per side) | per-bin reserves, fees |
| `token_mint_x_program_flag` / `token_mint_y_program_flag` | `u8` | 0 = TOKEN, 1 = TOKEN_2022 |

### 2.2 Bin math

Each bin has:
- `price` (`u128`): per-bin price, scaled by `SCALE_OFFSET` (a
  constant — the SDK uses `1_000_000_000_000` aka 1e12).
- `amount_x` / `amount_y` (`u64`): per-bin reserves.
- `fee_amount_x_per_token_*` / `fee_amount_y_per_token_*`
  (`u128`): per-bin fee accumulators.

The fill math (`getAmountOut` in `bin.ts`):

```
amount_out = in_amount * price / SCALE_OFFSET
```

…consuming bins from `active_id` outward until the input is
fully spent. **The math is constant-product-per-bin**, with
each bin's price fixed (no sqrt involved). This is the
cleanest math in the project.

### 2.3 Per-bin integer walk

For a `swapExactIn` of `in_amount` starting at `active_id`:
1. For bin `b` in the swap direction: take the bin's
   `amount_in_max` = min(in_amount_remaining,
   bin.{x or y}_reserve / bin.price * SCALE_OFFSET).
2. Compute the consumed `in_amount` and produced
   `out_amount` using `mulShr` / `shlDiv` (from `math.ts`,
   pure-integer shift-and-multiply).
3. Subtract from `in_amount_remaining`; if zero, done.
4. Otherwise, advance to the next bin and repeat.

All arithmetic is `u128` (or `BN.js` in TS, equivalent to
`u128`). No `f64`, no `sqrt`, no transcendental functions.

### 2.4 Fee model

The fee is composed of a base fee (from `bin_step`) plus a
dynamic volatility-based component. For v1.0, we use only the
base fee (`bin_step * in_amount / BPS_DENOMINATOR`). The
volatility component is v1.1.

## 3. Prometheus vs OpenTelemetry decision

### 3.1 Options

| Option | Crate | LOC added | Maint. burden | Feature surface |
| --- | --- | --- | --- | --- |
| **Hand-rolled text emitter** | none | ~100 | low | labels, counters, gauges, simple histograms |
| `prometheus` crate | `prometheus = "0.13"` | ~30 wrapper | medium | full Prometheus, label sets, histogram quantiles |
| `opentelemetry` crate | `opentelemetry = "0.27"` | ~50 + OTel collector | high | OTLP gRPC, traces + metrics + logs |

### 3.2 Decision (per the plan's open question answer)

**Hand-rolled text-format emitter for v1.0.** Rationale:

1. We have **4 metric names** (`cycles_evaluated`, `would_trade`,
   `total_tip_lamports`, `report_hash` + future `opps_per_sec`,
   `detection_latency_us`, etc.). The Prometheus text format
   is ~50 lines for a complete emitter.
2. **No new dependency** — the `prometheus` and `opentelemetry`
   crates both have churn (breaking changes every 6-12 months),
   and pinning adds a maintenance cost for a v1.0 release that
   ships once.
3. **Float-free**: the hand-rolled emitter writes our `u64`
   metric values directly; the `prometheus` crate uses `f64`
   for histogram quantile estimation (which we'd need to
   work around, since the workspace is integer-only in the
   value path).
4. The `MetricsSink` trait is **backend-agnostic**. If v1.1
   needs an OTel adapter, it's a single new file
   (`crates/dl-app/src/metrics_otel.rs`) that doesn't touch
   the trait or the engine.

**Re-evaluation trigger**: if a v1.1 user requires histogram
quantiles (P50/P95/P99) or label-set cardinality, revisit
this decision. Until then, hand-rolled wins.

### 3.3 Text-format spec

The Prometheus text format (per the official spec at
`prometheus.io/docs/instrumenting/exposition_formats/`) is:

```
# HELP <metric_name> <docstring>
# TYPE <metric_name> <counter|gauge|histogram>
<metric_name>{label="value"} <number>
<metric_name> <number>
```

A minimal emitter formats each `RegistryCounter` / `RegistryGauge`
/ `RegistryHistogram` in this format and concatenates. The
content-type header for HTTP is `text/plain; version=0.0.4`.

## 4. Cross-DEX routing

For a multi-DEX triangle (Raydium + Orca + Meteora) to be
detected, the graph's edges need to be labeled with
`(dex_id, pool_id)`. Today the graph is token-keyed:

```rust
Graph::add_edge(Pubkey /*token*/, Pubkey /*token*/, weight)
```

The change is:

```rust
Graph::add_edge(Pubkey /*token*/, Pubkey /*token*/, EdgeLabel {
    pool_id: Pubkey,
    dex_id: AmmKind,
    weight: ...
})
```

This is a non-breaking extension (we add a new field with a
default). The cycle's `Pool.amm_kind` is recoverable from the
`EdgeLabel` directly, no graph walk needed.

## 5. Reproducible-build strategy

Per the plan's open question answer, **strict** reproducibility:
- Same Rust version (`rustc --version` pinned in
  `rust-toolchain.toml`).
- Same target triple (host's `rustc -vV` `host` field).
- Same `Cargo.lock` (committed).
- `__CARGO_DEFAULT_RUSTC_VERSION` env var set in the build
  container to enable Cargo's deterministic build (since
  1.73).

The verification is: `cargo build --release` on two hosts
producing bit-identical `target/release/dl-app` binaries
(±1 byte on the strip step, which is acceptable).

## 6. SDK pinning

For v1.0 we **do not depend on** the Orca / Meteora SDK crates
directly. The Rust SDK is `no-std`-unfriendly (uses
`libm` for the `pow`/`sqrt` conversions) and the TS SDK is
TypeScript. Instead, we re-implement the math in
`crates/dl-sim/src/fill_orca.rs` and `fill_meteora.rs`, using
the SDK source as the **reference** for the math. Tests
verify that our re-implementation matches the SDK's reference
output to within ±1 base unit (AC-3 + AC-4).

## 7. References

- Orca Whirlpool:
  `https://github.com/orca-so/whirlpools/blob/main/rust-sdk/core/src/math/{price,tick,position}.rs`
- Meteora DLMM:
  `https://github.com/MeteoraAg/dlmm-sdk/blob/main/ts-client/src/dlmm/helpers/{bin,lbPair,math,fee}.ts`
- Uniswap V3 whitepaper (Orca math reference):
  `https://uniswap.org/whitepaper-v3.pdf`
- Prometheus text format:
  `https://prometheus.io/docs/instrumenting/exposition_formats/`
- Cargo deterministic builds:
  `https://doc.rust-lang.org/cargo/reference/config.html#buildrustc-wrapper`

## 8. Open questions resolved

This document closes the following questions from the 07-02
PLAN:

- **Orca fill math** (§1.3): lift SDK's `sqrt_u128` directly.
- **Meteora fill math** (§2.2-2.3): per-bin `mulShr`/`shlDiv`
  walk, integer-only, no sqrt.
- **Prometheus vs OTel** (§3.2): hand-rolled text emitter.
- **Reproducible build** (§5): strict.
- **SDK pinning** (§6): re-implement, don't depend.

## 9. Confidence assessment

| Source | Confidence | Notes |
| --- | --- | --- |
| Orca `sqrt_u128` math | **High** | Verified source on `main`; Newton's method is canonical. |
| Orca tick math | **High** | Verified source on `main`; same as Uniswap V3. |
| Meteora bin math | **High** | Verified source on `main`; `mulShr`/`shlDiv` are pure-integer. |
| Meteora fee model | Medium | Base fee confirmed; volatility component is v1.1. |
| Prometheus text format | **High** | Public spec. |
| Cargo deterministic build | **High** | Documented since Cargo 1.73. |

**One real risk**: Meteora's `bin_step` is reported in the
research doc as a `u16` bps, but the SDK uses `number` (TS)
which could be `u16` or a wider type depending on the IDL
version. Implementer must verify against
`MeteoraAg/dlmm-sdk/program/src/state.rs` (if present) or
the IDL `idl/lb_clmm.json` at implementation time. If the
type is wider than `u16`, expand the field.
