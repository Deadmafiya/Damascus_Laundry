# Deferred: `tests/cycle_writer_schema.rs` (moved alongside)

This integration test was moved here during the `dam-69/chaos-drills`
sync (commit fixing the build to be test-clean) because upstream
glue it depends on has not landed yet:

- `crates/dl-app/src/cycle_writer.rs` exists and is well-written,
  but `crates/dl-app/src/lib.rs` cannot declare it as `pub mod`
  until:
  - the `uuid` crate is added to `dl-app/Cargo.toml` deps;
  - `dl_core::cycle_id_hex` and `dl_core::LegKey` are exported
    (currently the references resolve nowhere);
  - `dl_state::cycle::Pubkey` either becomes an actual re-export
    or the import is corrected to `dl_state::pool::Pubkey`.

The same shape applies to `crates/dl-app/src/gate_writer.rs`
(DAM-79 / SLO #3).

The test file itself (`720+ lines of contract-driven schema checks
for the v1 cycle JSONL schema, per DAM-43`) needs to be moved back
to `tests/cycle_writer_schema.rs` once those four upstream pieces
land.

Cargo only auto-discovers integration tests in the top-level
`tests/` directory, so this file currently has no compile-time
impact on `cargo test --workspace` or `make build`.

Tracked under DAM-43 (cycle.v1 contract) and DAM-79 (bundle events).
Follow-up on `dam-69/chaos-drills`.
