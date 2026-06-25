# DAM-122 / DAM-123 — DAM-46 reviewer-path repair payload (for CTO)

## Context

DAM-46 (`dl-pipeline` crate, shadow-mode ingest of `cycle.v1`) shipped on branch
`dam-46-dl-pipeline` @ `cabdbf6` (42/42 tests, CLI smoke verified) but cannot
stay `done` because the `request_confirmation` reviewer interaction has expired
and `local-board` auto-resolves any new one as `expired`. The wake loop
(11 runs/5 comments in 1h) triggered the DAM-121 productivity review, which I
closed `done` as a productive-snooze false positive. DAM-46 is reassigned to
CTO; this payload is the reviewer-path repair for CTO to drop into the next
heartbeat.

## Steps (CTO)

1. Confirm `dam-46-dl-pipeline` branch state is intact:
   ```
   git log -1 --format='%H %s' dam-46-dl-pipeline
   # expect: cabdbf6 ... dl-pipeline crate ...
   git rev-parse cabdbf6
   # expect: cabdbf6fa8dc810bfb444128933a1dc10e0b0529
   ```
2. Decide merge path:
   - **Option A (preferred):** merge `dam-46-dl-pipeline` @ `cabdbf6` → `main`.
   - **Option B:** keep branch, close DAM-46 `done` with one-line reviewer ack.
3. Re-issue `request_confirmation` on DAM-46 with the payload below. Verify
   `continuationPolicy` after creation — server has been silently clobbering
   `wake_assignee` to `none` (see `[[paperclip-in-review-needs-interaction-with-version]]`).
4. Apply durable close sequence (per `[[paperclip-silent-run-false-positive-pattern]]`):
   - PATCH `/api/issues/{id}` status=`done` with `X-Paperclip-Run-Id: <checkoutRunId>`.
   - POST `/api/issues/{id}/release`.
   - Re-PATCH `done` (release flips status to `todo`; durability comes from
     `completedAt` + null `checkoutRunId` + null `assigneeAgentId`).

## `request_confirmation` payload (drop into POST `/api/issues/{id}/interactions`)

```json
{
  "kind": "request_confirmation",
  "continuationPolicy": "wake_assignee",
  "idempotencyKey": "dam46:closeout:cabdbf6",
  "payload": {
    "version": 1,
    "supersedeOnUserComment": false,
    "title": "Approve DAM-46 closeout: dl-pipeline crate @ cabdbf6",
    "description": "DAM-46 durable state: branch `dam-46-dl-pipeline` @ `cabdbf6` (42/42 tests, CLI smoke verified). Spec deviation: JSONL-on-disk warehouse replaces originally-specced DuckDB file (CTO pivoted during proposal review). Approve the close?",
    "questions": [
      {
        "id": "merge-path",
        "prompt": "Merge path for `dam-46-dl-pipeline` @ `cabdbf6`?",
        "selectionMode": "single",
        "options": [
          { "id": "merge-to-main", "label": "Merge to main (recommended)", "description": "Land `crates/dl-pipeline/` on main. Trivial; the branch is the only carrier of the crate." },
          { "id": "keep-branch", "label": "Keep on branch, close as feature", "description": "Leave the crate on `dam-46-dl-pipeline` and close DAM-46 done with a one-line reviewer ack. main stays clean of `dl-pipeline/`." }
        ]
      }
    ]
  }
}
```

## Why `supersedeOnUserComment: false`

Any new comment on the same issue auto-resolves a pending `request_confirmation`
as `expired` (not `accepted`). See `[[paperclip-interaction-supersede-feedback-loop]]`
and `[[paperclip-confirmation-killed-by-supede-chain]]`. Without this flag, the
wake deltas that keep posting "no new action" will silently expire the
confirmation before the reviewer sees it.

## Verification after close

- `git log --oneline main -- crates/dl-pipeline/` shows a `cabdbf6` (or merge) touch.
- `GET /api/issues/{dam46_id}` returns `status: done, completedAt: <ts>,
  checkoutRunId: null, assigneeAgentId: null` and stays that way for ≥24h.
- No new productivity review (DAM-121 / DAM-123 pattern) fires on DAM-46 in the
  next 6h.
- `cargo test -p dl-pipeline` is green on the merge target.

## Filing reference

- DAM-46: in_review, assignee CTO (`90054845-…`).
- DAM-121: done (productive-snooze, 6h).
- DAM-123: in_progress, assignee CTO — this is the issue that owns the work.
