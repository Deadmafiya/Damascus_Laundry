# Vision and Roadmap — Damascus Laundry

> **Status:** v1, bootstrapped 2026-06-21 by Project Archivist (DAM-114).
> **Owner:** The CEO approves the *What we are building* paragraph
> personally. Until then, it is **DRAFT**. The other three sections
> are owned by the Project Archivist and may be edited in place.
> **Review cadence:** the CEO reviews weekly.
> **This file is the canonical project narrative;** if a PR, a
> comment, or a board message contradicts it, the doc wins *and* a
> new entry is filed in `decision-log.md` explaining the change.

---

## What we are building — **DRAFT, pending CEO approval**

A Solana MEV paper-trading engine that connects to live mainnet,
detects cross-DEX negative-weight cycles (Raydium AMM v4 + Orca
Whirlpool + Meteora DLMM), evaluates each cycle with a *conservative*
EV model that accounts for latency, competition decay, tip cost, and
Jito auction window pessimism, and writes only those that pass to an
append-only paper ledger. The engine holds **no private keys in the
value path** — real execution is a v1.2+ change isolated to a single
crate. The "Solo Captain" operator runs the bot from a one-screen
advisor (DAM-106) that decides for them what the numbers mean and
what to do next; the SRE layer pages on silent reverts (SLO #3) and
landing-rate drops (DAM-78 / DAM-68); the Quant layer iterates on
calibration drift weekly (DAM-35 + `dl-recon-overfit`).

> **DRAFT notice (for any agent reading this):** The above paragraph
> is a synthesis of `docs/README.md`, `docs/architecture.md`, the
> v2.0 plan, and the manager-reorg rationale (DAM-100). It has not
> been confirmed by the CEO. Downstream readers (the Intel Manager,
> the daily brief, the team-lead) must treat it as *the team's current
> working articulation*, not as the canonical vision. A
> `request_confirmation` is pending CEO sign-off; the prompt is in
> the DAM-114 comment.

---

## Where we are now — 2026-06-21

**One screen.** If a new agent or a new hire reads this section, they
should leave with a single picture.

### What is shipped on `main`

- The v1.1 paper-trading series. `v1.1.7-realistic-mode` is the
  current tag. 441 tests passing.
- Live mainnet WebSocket ingestion (DAM-31 / v1.1.2). 3-DEX support:
  Raydium AMM v4 + Orca Whirlpool (DAM-62) + Meteora DLMM (DAM-63).
- `dl-app run --feed live` writes `wallet.json` (bincode, append-only)
  and `wallet.cycles.v1.jsonl` (cycle.v1 contract, DAM-41).
- Operator console at `/dashboard/live.html` (DAM-72 5-field grid)
  with the DAM-106 advisor IA staged on branch
  `dam-106-advisor-ia` @ `57fbb70` awaiting CTO sign-off.
- SRE surface: 4 Prometheus alerts in `docs/observability/alerts.yml`
  (3 of 4 live after DAM-81 wired the counters; the 4th waits on
  DAM-79). SLO definitions in `docs/observability/slos.md` (when
  committed; currently inlined in `damascus_laundry_dashboard`).
- Recon: `dl-recon` + `dl-recon-overfit` (DAM-35) + the DAM-64
  reconciliation loop (branch `dam-98-dl-recon-64` @ `9c2097a`).
- Manager org: CEO → CTO → Manager → IC, universal (DAM-100 rev 3).
  Managers: EngManager, OpsManager, ProductManager, SecurityManager,
  OpsCoordinator.

### What is in review (live but not yet signed off)

- DAM-42 Pyth live-acceptance (4 coupled bugs in `dl-oracle`).
- DAM-46 dl-pipeline (42/42 tests; JSONL-on-disk warehouse).
- DAM-54 staleness restore (5 files +606/-1 on
  `dam-54/dam-44c-restore` @ `966292e`).
- DAM-56 devnet e2e smoke (offline-acceptance 1/1).
- DAM-64 reconciliation loop (on `dam-98-dl-recon-64`).
- DAM-69 chaos drills (real workspace break: `dl-stream/detector.rs:204`).
- DAM-75 SLOs (3 SLOs + 4-state budget policy; CTO approval pending).
- DAM-106 advisor IA (3-region advisor; CTO sign-off pending).
- DAM-107 systemd pick for daily reconcile (option A: systemd timer).

### What is blocked (first-class)

- **DAM-77** recon gate: ±0.001 SOL gate not runnable until DAM-31
  unblocks.
- **DAM-90** v3 backtest: blocked on DAM-44 (Backend) + missing v3
  spec with §5.7 gates.
- **DAM-102** on-chain sweep: blocked on SRE RPC tier + DAM-38a merge.
- **DAM-103** daily reconcile: superseded by DAM-107; awaiting
  operator install (CTO sign-off pending).

### What is open / not started

- DAM-79 SLO #3 counters (the alert is in `alerts.yml`; the producer
  is not).
- DAM-80a / DAM-91: the `docs/observability/slos.md` typo fix.
- DAM-80b / DAM-93: the `docs/observability/alerts.yml` is on disk
  but the typo-clean and the "docs" framing need a follow-up.
- DAM-87: live nightly e2e cron (child of DAM-76).
- DAM-92: production `send_with_retry` wiring (child of DAM-57;
  test-local retry is in, prod is not).
- DAM-47 Phase A 24h gate (child of DAM-41; cannot run until
  `wallet.cycles.v1.jsonl` exists from a real `dl-app run` window).
- DAM-44d: the last remaining child of the DAM-44 integration.

### What is unaddressed by any ticket (and should be)

- A real mainnet-paper run with a real RPC. The recon is the gate
  (DAM-77), and the gate is blocked. The plan needs a "first
  sustained mainnet-paper run" milestone with a named operator and
  a named date.
- A v3 spec with §5.7 gates on disk. DAM-90 cannot run without it.
- A UX Designer ticket for the DAM-106 visual treatment (palette,
  typography, severity color). The IA is shipped; the visuals are
  not.
- A v1.2 plan for real execution. The v1.1 series is the last
  paper-only release. The `dl-executor` crate is the only thing
  that needs to change to go live; the plan is not yet written.

---

## Where we are going — **DRAFT, populated from the v2.0 plan**

The v2.0 plan (`plan/damascus_laundry_v2.0.md`) is the source of
truth for phase structure. The roadmap below is the Archivist's
synthesis of *what each phase means for the v2.0 era*. The CTO owns
the phase boundaries; the Archivist owns the prose.

### Phase 1a — *baseline* (shipped)

> `v1.1.0` — LiveMode gate, dl-signer CLI, dl-app run live path.
> No private keys in the value path.

Status: shipped. The engine runs live against mainnet WebSocket in
paper mode; the only thing the operator can do is watch.

### Phase 1b — *paper mode + streaming detector* (shipped)

> `v1.1.0-streaming` through `v1.1.4-mainnet-vaults`. The detector
> goes from batch to streaming. Vault subscriptions land.

Status: shipped. Cycle detection now runs on every pool update;
edge weights flow from vault accounts, not `AmmInfo`.

### Phase 1c — *realistic paper + ArbiNexus bridge* (shipped)

> `v1.1.6-arbinexus-bridge`, `v1.1.7-realistic-mode`. 30% win-rate
> loss model. Bridge to `vendor/arbinexus/`.

Status: shipped. PnL in the operator console is honest. The
ArbiNexus bridge is a read-only consumer of `wallet.cycles.jsonl`.

### Phase 1d — *tiny mainnet (live execution prep)* (in review)

> DAM-31 / DAM-44 / DAM-58 / DAM-59 / DAM-61 / DAM-67. The first
> time the engine can sign a real transaction in a *bounded* way.
> 0.001 SOL/day paper floor (DAM-58), 0.5 SOL/day + 0.05 SOL/bundle
> mainnet cap floors (DAM-61), `CapState` persistence (DAM-67),
> `dl_assert_program` deploy verification (DAM-59).

Status: in review. The recon gate (DAM-77) is the *gate* to "we can
ship a tiny mainnet run." Phase 1d is not gtm until the recon gate
passes.

### Phase 2 — *calibration* (open)

> `dl-recon-overfit` weekly review. DAM-35 + `dl-calibration`
> produce `calibration.json`; the recon harness consumes it. v3
> spec with §5.7 gates is the unlock to the v3 backtest (DAM-90).

Status: open. The weekly review runs but the v3 spec gate is the
next milestone.

### Phase 3 — *24/7 reliability* (open)

> DAM-107 systemd timer for daily reconcile (option A). DAM-76
> devnet e2e (shipped) → DAM-87 live nightly. The full SRE surface
> (DAM-75 SLOs + DAM-68 alerts + DAM-78 landing-rate targets +
> DAM-80 SLO row + DAM-81 wired counters + DAM-79 SLO #3 counters)
> is the operational floor.

Status: open. The pieces are landing; the system is not yet
running 24/7.

### Phase 4 — *scale* (open)

> DAM-70 latency tier ladder. Tier 1 = paid business RPC ($50–250/mo).
> Tier 2 = Jito ShredStream ($800+/mo). Tier 3 = co-location
> ($2–5k/mo). Tier 4 = own Jito-Solana / Firedancer validator
> ($10k+/mo setup + ongoing). Each tier's "earned its keep" metric
> is documented in `docs/research/latency-ladder.md`.

Status: open. The doc is shipped; no tier has been bought. Tier
selection is the SRE + Quant joint decision.

### Phase 5 — *v1.2 real execution* (not yet planned)

> The `dl-executor` crate is the only thing that needs to change.
> The plan is not yet on disk. v1.1.x is paper-only; v1.2 is the
> first release that can land a real bundle.

Status: not yet planned. This is a future ticket.

---

## What we are explicitly not doing — **DRAFT, owned by the CEO**

Anti-goals. Things we have decided *not* to do, and why. If a future
proposal violates an anti-goal, the answer is no until the CEO lifts it.

- **Not implementing real execution in v1.1.x.** The v1.1 series is
  the last paper-only release. Real execution is v1.2+ and is a
  one-crate change (`dl-executor`). The reason is the keyless
  invariant — once a keyfile is in the value path, it stays there.
  Going live without a deliberate phase break is the failure mode
  this anti-goal prevents.
- **Not shipping a "fancy" operator console.** The DAM-106 advisor
  IA is *one screen, three regions, no tabs.* A second screen, a
  tab bar, a settings page — all rejected. The Solo Captain's worst
  day is "I can't tell if the numbers are bad or the bot is down";
  a fancy console makes that day worse, not better. The visual
  treatment is the only thing that may grow; the IA is locked.
- **Not skipping the conservative EV model.** The optimistic
  evaluation is kept for diagnostics; the conservative side is what
  gates the trade. Skipping the conservative side to "see more
  trades" is rejected because the *whole point* of the simulation
  is honesty. If the optimistic-only number is what the operator
  wants, the operator is reading the wrong field.
- **Not moving the cycle warehouse to a database.** JSONL on disk
  is the format. `dl-recon` joins two JSONL files on `bundle_id` and
  that join is the audit trail. A database adds a native dependency,
  takes away human-readability, and the JSONL scales further than
  the v2.0 era needs.
- **Not writing operator-facing docs from this seat.** Runbooks
  are Product's lane. The operator console IA is the Frontend
  Programmer's lane. The intel docs (this file) describe *what we
  are building*; they do not describe *how to use it.*
- **Not making architecture or strategy decisions from this seat.**
  This file records why they were made. When a new direction is
  needed, route to the CEO via the Intel Manager; do not author.
- **Not putting the CEO on the critical path for every ticket.**
  The manager reorg (DAM-100) is the structural answer: CEO →
  CTO → Manager → IC. A future proposal that says "the CEO needs
  to approve this for every IC" is asking to undo the reorg.
- **Not doing external research on other bots.** The Prior-Art
  Analyst owns that. This history is internal-only.

---

## Out of scope for this doc

- Per-IC responsibilities. The `AGENTS.md` for each agent owns that.
- The DAM-xx ticket bodies themselves. This doc points at them; it
  does not duplicate them.
- The SRE runbook. That is `docs/v2.0-operator-runbook.md` and
  `docs/observability/runbook.md`; Product owns them.
- The DAM-106 visual treatment. UX Designer owns that.
- Pre-v1.0 history. The `timeline.md` covers it as a milestone
  list; this doc is the *current* vision.

---

## Changelog

- **2026-06-21 (DAM-114, v1)** — Initial bootstrap. Four docs created
  in a single heartbeat. The *What we are building* paragraph is
  marked DRAFT pending CEO sign-off via `request_confirmation` on
  DAM-114.
