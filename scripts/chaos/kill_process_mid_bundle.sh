#!/usr/bin/env bash
# scripts/chaos/kill_process_mid_bundle.sh
#
# Phase 3 chaos drill #2: kill the process mid-bundle (before the
# landing poll completes) and assert no double-submit + cap
# consistency on restart.
#
# What "kill the process mid-bundle" means in our model:
#   - In the real pipeline, the process is a single-threaded loop
#     in `dl-app/src/main.rs` that calls `submit_opportunity` per
#     cycle. `kill -9` between the Jito `submit` returning a
#     `bundle_id` and `poll_landing` returning would orphan the
#     bundle on the Block Engine side.
#   - The on-host safety guarantee we DO have: there is no retry
#     loop and no background queue — the next process iteration
#     is the only thing that can submit again, and it starts with
#     a fresh in-memory `CapState`. So a kill -9 cannot cause
#     double-submission of the SAME bundle (no auto-retry, no
#     in-process queue), and the cap resets on restart.
#   - The drill stands in a stub Jito client that returns
#     `Ok(bundle_id)` on `submit` and then `Err(ExecutorError)`
#     on `poll_landing` (the "process was killed before landing
#     completed" analog), then asserts:
#       (a) the cap is NOT double-charged — once `submit` returns
#           Ok, the cap is NOT refunded even on `Lost`/poll error
#           (because the bundle was actually sent to Jito; the
#           cap correctly accounts for the tip we paid), but the
#           same process cannot re-submit the same bundle (no
#           retry loop), so double-submit is impossible.
#       (b) on "restart" (a fresh `CapState`), the new iteration
#           can submit a new bundle cleanly without the old
#           orphan's tip leaking in.
#
# Both invariants are unit-tested in
# `crates/dl-app/tests/chaos_kill_process.rs`. This script is a
# thin harness that runs those tests, parses cargo's red/green
# output, and exits 0 on pass / non-zero on fail.
#
# Acceptance: `bash scripts/chaos/kill_process_mid_bundle.sh`
# exits 0 and prints "[green]".

set -euo pipefail

# Resolve repo root from this script's path so it works from any CWD.
SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" &> /dev/null && pwd)"
REPO_ROOT="$(cd -- "${SCRIPT_DIR}/../.." &> /dev/null && pwd)"

cd "${REPO_ROOT}"

# Run only the chaos_kill_process test target. --nocapture so a
# human running this interactively sees the assertion line; CI
# keeps the exit code as the source of truth.
TEST_BIN="chaos_kill_process"
echo "[chaos] running cargo test --test ${TEST_BIN} (kill-process-mid-bundle)"

# `cargo test` returns non-zero on failure; `set -e` will then
# make the script exit non-zero, which is what we want.
cargo test \
    --manifest-path "${REPO_ROOT}/Cargo.toml" \
    --test "${TEST_BIN}" \
    -- --nocapture

# We only get here if the test passed.
echo "[green] kill_process_mid_bundle: no double-submit, cap consistent on restart"
