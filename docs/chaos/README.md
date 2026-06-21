# Phase 3 chaos drills

Two scripted drills that exercise the v2.0 live-submit pipeline
under failure modes that the on-host safety controls are
specifically designed to survive:

1. **RPC drops mid-submit** — `scripts/chaos/kill_rpc_mid_trade.sh`
2. **Process is killed mid-bundle** — `scripts/chaos/kill_process_mid_bundle.sh`

Both drills use **devnet** (no real money) and assert invariants
on the in-process `CapState`, the Jito client call count, and
the pipeline outcome. They are unit-test-driven: each script
runs a single `cargo test` target and propagates the pass/fail
status through `set -e`.

## Running the drills

```bash
# drill 1: drop the RPC connection mid-submit
bash scripts/chaos/kill_rpc_mid_trade.sh

# drill 2: kill the process mid-bundle
bash scripts/chaos/kill_process_mid_bundle.sh
```

Expected output on pass:

```
[chaos] running cargo test --test chaos_kill_rpc (kill-RPC-mid-trade)
...
test chaos_kill_rpc_does_not_double_charge_and_cap_is_consistent_on_restart ... ok
...
[green] kill_rpc_mid_trade: cap consistent, no double-charge
```

Both scripts exit `0` on pass and non-zero on fail. CI can wire
either script to its chaos-test job without further glue.

## What each drill exercises

### Drill 1 — `kill_rpc_mid_trade.sh`

**Failure mode:** the RPC connection drops while
`simulateTransaction` is in flight.

**How the pipeline is supposed to behave:**

1. The cap is charged at the top of `submit_opportunity_with_simulate`
   (line 310 of `dl-app/src/live.rs`), before the simulate gate.
2. The simulate-fn returns `Err(ExecutorError::SimulateFailed(...))`
   — the gate is fail-closed.
3. The pipeline takes the `Rejected("simulate: ...")` branch
   (line 362) and **refunds** the cap charge. The bundle is
   never sent to Jito.
4. On restart (a fresh `CapState`), a new bundle can charge
   cleanly. No tip leaks across the process boundary.

**Invariants asserted by `crates/dl-app/tests/chaos_kill_rpc.rs`:**

- The pipeline returns `OpportunityOutcome::Rejected` with
  reason containing `"simulate"` (not `Landed`).
- `cap.spent_today() == 0` after the failure (refund landed).
- `cap.remaining() == daily_lamports` (cap at full).
- On a fresh process iteration, a new bundle lands cleanly and
  `cap.spent_today() == expected_tip` (not `2 * expected_tip`).
  This is the no-double-charge invariant.

### Drill 2 — `kill_process_mid_bundle.sh`

**Failure mode:** the process is `kill -9`'d between
`jito.submit` returning `Ok(bundle_id)` and
`jito.poll_landing` returning. The bundle is orphaned on the
Block Engine.

**How the pipeline is supposed to behave:**

1. The cap is charged once at the top of
   `submit_opportunity_with_simulate` (line 310), before the
   Jito `submit` call.
2. `jito.submit` returns `Ok(bundle_id)`. The bundle is on the
   Block Engine. The tip was paid.
3. The process is killed before `poll_landing` returns.
4. The cap is **not** refunded (we DID send the bundle — the
   tip is real).
5. There is no in-process retry path: the next process
   iteration is the only thing that can submit a NEW bundle.
   The previous bundle cannot be re-submitted by the same
   process (it doesn't exist anymore — `kill -9`).

**Invariants asserted by `crates/dl-app/tests/chaos_kill_process.rs`:**

- The pipeline returns `OpportunityOutcome::NotSubmitted` with
  reason containing `"poll"` (the poll error), not `Landed`.
- `cap.spent_today() == expected_tip` after the failure
  (charged once, not refunded — the tip is real).
- `jito.submit` was called exactly once (no auto-retry → no
  double-submit of the same bundle).
- On a fresh process iteration, a new bundle lands cleanly and
  `cap.spent_today() == expected_tip` (the orphaned bundle's
  tip does not leak in).

## Why unit tests, not a live `kill -9` harness?

The drills drive `submit_opportunity_with_simulate` directly,
with stub Jito + Jupiter + simulate-fn clients. The trade-off:

| | Unit-test-driven drill | Live `kill -9` harness |
|--|--|--|
| Reproducibility | Deterministic (no network) | Network-dependent |
| Speed | Seconds | Minutes (per drill) |
| What it exercises | The cap + retry invariants | The actual process death |
| What it does NOT exercise | Real Solana RPC, real Jito Block Engine, OS-level `kill -9` | N/A |

The unit-test-driven approach exercises **the invariants the
drills are designed to prove**: cap consistency, no-double-submit,
refund on simulate-gate failure. It does NOT exercise the OS
signal path, which is a separate concern (and is exercised by
the existing `dl-app` unit tests for the simulate gate — see
`crates/dl-app/src/live.rs:1353`).

A live `kill -9` harness would require a running devnet bot
on a real host. That's the SRE on-call's job during the dry-run
window, not the unit-test job. The unit tests prove the
**invariants**; live chaos proves the **runtime**.

## Known gaps

These are recorded here so the SRE on-call knows what the
drills do NOT prove, and so the gaps are visible in the
runbook during a dry-run:

1. **Cap state is in-memory only.** `CapState` does not
   persist across processes. A kill + restart resets
   `spent_today` to 0. The drills assert that this is
   consistent (no leak across processes), but they do
   NOT assert that the cap is "global across the day" —
   it isn't. A persistent cap (e.g. to a small JSON file
   or a sidecar DB) is a Phase 4 item. Implication: a
   determined attacker who can kill + restart the process
   can spend N × `daily_cap` in one day. The kill-switch
   and the per-bundle cap (500M lamports) are the
   compensating controls.

2. **No idempotency key on bundles.** A bundle submitted
   to Jito is identified by `bundle_id`, which is assigned
   by the Block Engine, not by us. A kill mid-bundle leaves
   a bundle_id we never recorded. On restart we cannot
   tell the Block Engine "this is a duplicate, drop it."
   The drills assert that we do not auto-retry in the
   same process (so the duplicate is impossible from our
   side), but they do NOT assert that a restart is
   idempotent. Jito's `getBundleStatuses` is the
   cross-process dedup mechanism if needed; not yet wired.

3. **No persistent queue / WAL.** A killed bundle is
   orphaned; on restart we don't know it ever existed. If
   the bundle DID land, we have no record of it (no PnL
   entry, no calibration capture). This is the
   `wallet.cycles.jsonl` ↔ Jito on-chain reconciliation
   gap (DAM-21, in scope for SRE). The drills do not
   exercise the reconciliation path.

## Relationship to other docs

- `docs/runbook.md` — the on-call runbook. The drills are
  the "verify the runbook is right" check.
- `docs/v2.0-operator-runbook.md` — the operator-facing
  runbook. The chaos drills are part of the dry-run
  acceptance (DAM-21).
- `docs/live-runbook.md` — the mainnet mainnet deploy
  runbook. The drills are pre-flight for mainnet; they
  should pass cleanly on devnet before mainnet.
- `docs/known-limitations.md` — the project-wide list of
  known gaps. The "Known gaps" section above is the
  chaos-specific subset.
