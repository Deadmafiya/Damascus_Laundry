#!/usr/bin/env bash
# scripts/chaos/kill_rpc_mid_trade.sh
#
# Phase 3 chaos drill #1: drop the RPC connection mid-submit and
# assert the cap is consistent on restart.
#
# What "drop the RPC mid-submit" means in our model:
#   - The real pipeline is `submit_opportunity`, which constructs
#     a fresh `RpcClient` from `cfg.simulate_rpc_url` on every call
#     (see `dl-app/src/live.rs:144`). A dropped RPC manifests as
#     the simulate-gate closure returning `Err(...)` → the
#     pipeline takes the `Rejected("simulate: ...")` branch and
#     refunds the cap charge.
#   - The drill stands in a stub Jito client + stub simulate-fn
#     for that exact failure mode and asserts:
#       (a) the bundle is NOT double-charged (cap was refunded),
#       (b) the cap is consistent after a "restart" (a second
#           process iteration starts with `spent_today == 0`,
#           which is the current in-memory design — see
#           `docs/chaos/README.md` §"Known gaps" for why this is
#           recorded as a known limitation, not a fail).
#
# Both invariants are unit-tested in
# `crates/dl-app/tests/chaos_kill_rpc.rs`. This script is a thin
# harness that runs those tests, parses cargo's red/green output,
# and exits 0 on pass / non-zero on fail.
#
# Acceptance: `bash scripts/chaos/kill_rpc_mid_trade.sh` exits 0
# and prints "[green]".

set -euo pipefail

# Resolve repo root from this script's path so it works from any CWD.
SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" &> /dev/null && pwd)"
REPO_ROOT="$(cd -- "${SCRIPT_DIR}/../.." &> /dev/null && pwd)"

cd "${REPO_ROOT}"

# Run only the chaos_kill_rpc test target. --nocapture so a human
# running this interactively sees the assertion line; CI keeps the
# exit code as the source of truth.
TEST_BIN="chaos_kill_rpc"
echo "[chaos] running cargo test --test ${TEST_BIN} (kill-RPC-mid-trade)"

# `cargo test` returns non-zero on failure; `set -e` will then
# make the script exit non-zero, which is what we want.
cargo test \
    --manifest-path "${REPO_ROOT}/Cargo.toml" \
    --test "${TEST_BIN}" \
    -- --nocapture

# We only get here if the test passed.
echo "[green] kill_rpc_mid_trade: cap consistent, no double-charge"
