# Testing

This project uses **integration tests as architecture documentation**. The test
suite doubles as worked examples for how the crates fit together.

## Running the suite

```bash
# Full workspace (441 tests as of v1.1.7)
cargo test --workspace

# Just one crate
cargo test -p dl-detect

# Just one test (by name substring)
cargo test -p dl-detect -- triangle

# With output for failing tests
cargo test --workspace -- --nocapture

# Release mode (much slower compile, faster test)
cargo test --workspace --release
```

## Test layout per crate

| Crate | What to read in `tests/` |
|-------|--------------------------|
| `dl-core` | `fixed_point_no_fractional.rs` — float-free guard |
| `dl-feed` | `determinism.rs`, `capture_roundtrip.rs` — same input → same output |
| `dl-state` | decoder tests per DEX (raydium/whirlpool/dlmm), plus `no_floats.rs` |
| `dl-detect` | `graph_build.rs`, `cycle_detection.rs` — algorithm correctness |
| `dl-sim` | `ev_integration.rs`, `ev_props.rs` — eval gate semantics |
| `dl-ledger` | `ledger_roundtrip.rs` — schema stability |
| `dl-recon` | `golden_replay.rs` — end-to-end with golden hash |
| `dl-recon-overfit` | `dsr_pbo_props.rs` — overfit defense math |
| `dl-signer` | `no_floats.rs` — float-free guard |
| `dl-executor` | `bundle_construction.rs`, `tip_math.rs` |
| `dl-stream` | `latency_props.rs`, `e2e_latency.rs` |
| `dl-paper` | `roundtrip.rs`, `stats.rs` |
| `dl-app` | `dl_ledger_path.rs`, `dry_run_e2e.rs`, `recon_cli.rs` |

## Adding a test

Pick the smallest failing test that reproduces your bug. Put it in the
**same crate as the code under test**, not in `dl-app`.

### Pattern

```rust
// crates/dl-detect/tests/cycle_three_legs.rs
use dl_detect::graph::build_from_pools;
use dl_state::pool::{AmmKind, Pool};
use dl_state::Pubkey;

#[test]
fn triangle_three_pools_produces_one_cycle() {
    let pools = vec![/* triangle of 3 pools */];
    let graph = build_from_pools(&pools).unwrap();
    let cycles = graph.find_negative_cycles(3);
    assert_eq!(cycles.len(), 1, "triangle should yield exactly one cycle");
}
```

### Float-free guards

Each crate has a `tests/no_floats.rs`:

```rust
// crates/dl-detect/tests/no_floats.rs
#[test]
fn value_path_is_float_free() {
    let src = std::fs::read_to_string("src/lib.rs").unwrap();
    let forbidden = ["f64", "f32"];
    for kw in forbidden {
        assert!(
            !src.contains(&format!("use f{kw}")),
            "{kw} found in src/lib.rs; float-free invariant violated"
        );
    }
}
```

If your test fails on this, **don't disable the guard**. Either:
- move the float math to a `whitelisted_floats!` block (none exist; add if justified), or
- switch to fixed-point (`u128` with `ONE_E18 = 1_000_000_000_000_000_000`).

## Test conventions

1. **Use `u128` / `i128` everywhere.** Even in test fixtures. Mixing `f64` in test data masks overflow bugs.
2. **Determinism first.** Tests must pass in any order, on any machine. Avoid `std::time::now()` or `rand::random()` without a seeded RNG.
3. **Property tests for invariants.** `proptest` is in workspace deps; use it for roundtrips and arithmetic bounds.
4. **One assertion concept per test.** If you assert 5 things, split into 5 tests or use a `Result`-returning helper.

## Coverage gaps (intentional, see `known-limitations.md`)

- `dl-executor`: real Jupiter/Jito HTTP clients are mocked. Live execution untested.
- `dl-feed::ws_feed`: requires a live WS endpoint; CI uses a scripted feed.
- `dl-signer`: keyfile round-trip is unit-tested; Argon2id memory-hard params are *not* benchmarked.
