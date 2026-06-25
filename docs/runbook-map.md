# Runbook Map

> **Status:** v1.0, 2026-06-21. Author: Product / Docs Lead.
> **Created from:** DAM-40 P0-1 (runbook collision).
>
> There are two runbooks in `docs/`. They cover overlapping
> territory and use different naming. This file is the single
> source of truth for "which file owns which step."

---

## Tier ↔ Phase mapping

| Phase (v2.0 plan) | Tier (live-runbook) | Runbook in this repo | Operator surface |
|---|---|---|---|
| Phase 0 — Pre-flight | — | `live-runbook.md` §1 | one-time checklist before any real SOL |
| Phase 1a — Real clients (devnet) | Tier 1 | `live-runbook.md` §4 Tier 1 | first real Jito bundle attempt |
| Phase 1b — Connect real detector | Tier 1 (cont.) | `live-runbook.md` §4 Tier 1 | replaces `synth_triangle_pools()` |
| Phase 1c — mainnet-paper (0.001 SOL/day) | Tier 2 | `v2.0-operator-runbook.md` Phase 1c | first real-SOL bundle |
| Phase 1d — tiny mainnet (<0.5 SOL/day) | Tier 3 | `v2.0-operator-runbook.md` Phase 1d | first sustained run |
| Phase 2 — calibration | — | `v2.0-operator-runbook.md` Phase 2 | weekly `dl-recon-overfit` review |
| Phase 3 — 24/7 reliability | — | `v2.0-operator-runbook.md` Phase 3 | systemd unit + alert rules |
| Phase 4 — scale | Tier 4 | `v2.0-operator-runbook.md` Phase 4 | profit-funded infra ladder |

## What lives where

- **`docs/live-runbook.md`** — operator SOPs (hot-wallet
  funding, kill-switch recovery, devnet airdrop, daily
  recon). Read top-to-bottom once before the first run.
- **`docs/v2.0-operator-runbook.md`** — phase-gated checklist
  of manual steps per phase. Read section-by-section as you
  enter that phase.
- **`docs/runbook.md`** — legacy paper-trader runbook.
  Superseded by `live-runbook.md` for live-mode operations
  only. Keep around for the paper workflow until that's also
  retired.
- **`docs/personas.md`** — the operator persona ("Solo
  Captain"). Both runbooks should drift with this file; if
  they don't, one of them is stale.
- **`docs/v2.0-operator-runbook-review.md`** — the review
  pass that produced this map. Closes DAM-40.
- **`plan/damascus_laundry_v2.0.md`** — the v2.0 plan. Both
  runbooks are checklists extracted from it; this is the
  source of truth for phase structure and cap values.
- **`plan/atomicity-decision.md`** — locked decision
  (option a: custom BPF assert program). Read once before
  the first mainnet-paper run; not a runbook.

## Disambiguation rules (when the two runbooks disagree)

1. **For phase structure and cap values** — `plan/damascus_laundry_v2.0.md`.
2. **For daily SOPs (funding, keyfile, recon)** —
   `live-runbook.md`.
3. **For phase-gated checklists (build, deploy, gate
   conditions)** — `v2.0-operator-runbook.md`.
4. **For the persona and IA framing** — `docs/personas.md` and
   the "IA proposal" section of `v2.0-operator-runbook-review.md`.
5. **If two runbooks give different commands for the same
   step** — prefer the command in the runbook that is the
   **owner of that step** in the table above. If both claim
   ownership, prefer the more recent file's command and file
   a docs ticket to reconcile.

## Environment variable ↔ flag

Some commands appear in both runbooks with the hot-wallet
specified two ways. They are equivalent (the env var resolves
to the flag), but the convention is:

- **In `live-runbook.md`** — prefer `DL_MAINNET_KEYFILE` (and
  `DL_DEVNET_KEYFILE`) env vars. They are set once in
  `~/.damascus/dl-app.env` and not repeated in every command.
- **In `v2.0-operator-runbook.md`** — prefer `--keyfile` flag
  on the `dl-app run` line. The env var is implicit through
  the `EnvironmentFile=` systemd directive (Phase 3) and
  through the operator's shell env (Phases 1c/1d).

Both work. Do not change one to match the other — change
`docs/runbook-map.md` if the convention needs to evolve.

## Adding a new runbook

If you write a new runbook (a `docs/X-runbook.md`), update
this file in the same PR. Untracked runbooks become
load-bearing without anyone noticing, which is exactly the
bug P0-1 was filed to prevent.
