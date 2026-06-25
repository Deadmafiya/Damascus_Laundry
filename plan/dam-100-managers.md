# DAM-100 — Manager layer under the CTO

> Status: **rev 3, 2026-06-21.** Board corrected rev 2 at
> 2026-06-21T09:21:32Z: "every other except CEO and CTO will be
> report to their field managers. and manager will report to CTO
> and CTO will report to you/CEO." Rev 3 implements the full
> manager layer (3 new managers + 12 IC re-routes); the plan
> now records the actual final org.

> **rev 2, 2026-06-21.** Board approved rev 1 at 2026-06-21T09:12:35Z
> ("hire those managers as per plan"). Rev 2 records what was actually
> hired, the role-enum deviation from rev 1, and the handoff to the CTO.

## 0. Why this memo exists

The user opened DAM-100 with the ask: hire managers under the CTO so the
CTO is not the single point of coordination across the whole engineering
team. The literal ask names two candidate slots — "programmer manager"
and "code reviewer manager." The deeper ask is: **reduce the bus
factor on the CTO and the integration tax on every multi-agent
work item.**

The right answer is not "yes, hire both" or "no, never." It is
"here is the actual coordinator bottleneck, here are the
slots that would relieve it, here are the slots that would just
add a layer, and here are the triggers that should fire before we
hire any of them."

## 1. The org as it actually is, 2026-06-21

Counts pulled from the Paperclip board this heartbeat.

| Layer | Agents | Headcount |
|---|---|---|
| CEO | CEO | 1 |
| CTO direct reports | CTO + Backend + Frontend + Data + Quant + SRE + BotSRE + Security + Product + UIUX + PRReviewer + QA | 12 |
| Managers | (none) | 0 |
| **Total** | | **13** |

The CTO currently owns 12 direct reports. Recent done-list (last 10
items, 2026-06-18 → 2026-06-21) is dominated by integrator work
("Drop DAM-25 plan awareness into DAM-7/8/10/13/14", "Propagate
jito-solana pin into DAM-7 and DAM-8", "Hire Frontend Programmer /
PR Reviewer / Backend / QA"). The CTO is the integrator-of-last-resort
for every multi-agent work item.

CTO's current active load:

- 5 **blocked** (3 of which are Paperclip ops: "Review silent active
  run for CTO" — the stale-run false-positive loop is generating
  ~3 of these a day). Real work: DAM-60a (live mainnet recon run,
  high priority) and DAM-44 (ArbiNexus bridge to cycle.v1).
- 3 **in_review** (all 3 are P0/P1 cross-functional approvals the
  Backend Programmer is waiting on).
- 1 **in_progress** (DAM-76 Phase 1a devnet e2e).

## 2. What is *actually* generating CTO load

Three failure modes show up repeatedly in the auto-memory
(`[[multi-agent-file-contention]]`, `[[dam-46-dl-pipeline-blocked-by-concurrent-clobber]]`,
`[[dam-64-shipped-on-main-016917c]]`, `[[paperclip-stale-run-false-positive]]`):

1. **The integration tax.** Multi-agent work (DAM-31, DAM-46, DAM-64,
   DAM-89) gets clobbered 2–4x by peer heartbeats reverting each
   other's edits. Someone has to recognize the revert, re-apply, and
   commit within the same window. Today that is the CTO.
2. **The review-of-last-resort.** Backend Programmer ships a PR;
   PRReviewer reviews for style; but the cross-crate or cross-DEX
   invariants get checked by the CTO (DAM-53 decoder-end test,
   DAM-62 Whirlpool routing, DAM-63 DLMM). The CTO is functioning
   as a second review layer.
3. **The Paperclip-ops tax.** 3 of the 5 currently-blocked CTO issues
   are literally `Review silent active run for CTO` — Paperclip
   system events, not engineering work. They are bookkeeping and
   they outnumber real engineering blockers on the CTO's plate.

These are the three things a manager would, in principle, absorb.

## 3. What "manager" should mean in this org

To avoid the common failure mode (manager becomes a status-meeting
tax with no operational authority), every manager slot must answer
**yes** to all four:

- **Has a defined domain** (a specific cluster of issues, not "all
  things engineering").
- **Has ship authority** (can PATCH `in_progress` → `in_review` →
  `done` on issues in the domain, and can hire/fire direct reports).
- **Has escalation rules** (knows what the CTO still owns, e.g.
  cross-domain merges, capital/custody, anything touching mainnet
  signing).
- **Is itself a Paperclip agent** with a clear role prompt — not a
  human process we invent and forget to maintain.

A role that is just "approves PRs" or "tracks tickets" without
domain authority is a coordinator, not a manager. Coordinators are
fine, but they should be called that.

## 4. Candidate manager slots, ranked

### 4.1 Eng Manager (Backend + Frontend + Quant + Data) — **HIRE**

This is the strongest case. These four roles produce the *core
deliverable* (the bot code, the strategies, the data plane) and they
have the highest rate of cross-cutting work:

- DAM-31 / DAM-62 / DAM-63 / DAM-89 (multi-DEX feed wiring) all
  required Backend + Quant + Data alignment.
- DAM-46 dl-pipeline was Backend + Data; clobbered twice.
- DAM-64 reconciliation was Backend + Data + Quant; clobbered
  three times.
- DAM-90 v3 backtest is Quant + Backend + Data and is currently
  blocked on DAM-44 (Backend).

The CTO today is the only person who can see all four queues and
resolve a contention in 30 seconds. A manager with `in_progress`
authority on this domain and a clear escalation rule
("anything touching mainnet signing, custody, or kill-switch still
goes to CTO") would absorb 60–80% of the integration tax on this
cluster.

Escalation to CTO: cross-domain merges, anything that touches
`crates/dl-signer`, anything that touches the kill-switch, anything
that involves the board directly.

### 4.2 QA & Reliability Manager (QA + SRE + BotSRE) — **HIRE, conditional**

Second strongest case. These three roles are already
operations-shaped and already collaborate heavily:

- DAM-75 SLOs + DAM-68 alerts are SRE work the CTO is currently
  accountable for end-to-end.
- DAM-69 chaos drills are blocked on dl-stream + dl-detect
  (SRE-flavored); DAM-79 SLO instrumentation is a SRE+Backend child.
- DAM-21 mainnet dry-run operationalization is BotSRE-owned but
  the readiness review is CTO-signed.

The reason this is **conditional**: BotSRE is a named owner of
DAM-21 with a *standing kill-switch authority* that the CTO
explicitly delegates. A manager over BotSRE must not erode that
authority. The manager's role here is "drives SLO + chaos
readiness, owns the test infrastructure, and removes blockers
between QA/SRE/BotSRE." It is **not** an extra approval layer on
kill-switch decisions.

Trigger: hire this one when DAM-75 SLO instrumentation (DAM-79)
ships and there is a real SLO dashboard for the manager to point
at. Before that, this slot is a coordinator and a coordinator
without a dashboard is a status-meeting generator.

### 4.3 PR / Code Review Lead — **DO NOT HIRE**

The user's literal second example ("code reviewer manager") is
the weakest case. The PR Reviewer role is *already* dedicated to
that function and ships well (`dam-57-shipped-on-main-d567ca9`,
`dam-83-shipped-via-dam-84`). What is missing is not review
capacity; it is **post-review integration ownership** — who
watches the 3-day-old "approved but not merged" PRs and chases
the unblocker. That is a coordinator, not a manager. Hire it as
a 1-quarter contractor (BotSRE or a new part-time role) or absorb
it into the Eng Manager's scope, but do not dedicate a slot.

### 4.4 Product & UX Manager (Product + UIUX) — **DEFER**

Product and UIUX are currently low-volume (1 issue each in the
last 30 days). They do not generate the integration tax that the
engineering cluster does. Re-evaluate when the operator console
hits v2 and there are real cross-cutting UX questions
(information architecture, design system, accessibility audit)
that need daily ownership. Not before.

### 4.5 Security Manager — **DEFER, with a dotted line**

Security already has a clear single-issue pipeline and reports
to CTO with a dotted line to the CEO on capital/custody. The
correct next step is *not* a manager but a written escalation
path for security issues that the manager layer would route.
The current Security prompt already names this; we just need
to write the path into a runbook.

### 4.6 Paperclip Ops Coordinator — **HIRE, but call it a coordinator**

The 3-of-5-blocked is "Review silent active run" tells you the
real cost. A coordinator (not a manager) with `in_progress`
authority on Paperclip ops issues only — stale-run cleanup,
checkout hygiene, status discipline — would clear the CTO's
plate of mechanical work and let the CTO concentrate on
engineering integration. This is the cheapest slot to fill
and the highest immediate-return.

## 5. What stays at the CTO after the layer lands

The CTO should *still* own:

1. **Architecture and cross-domain decisions.** Anything that
   changes the crate graph, the data contract, the signing path,
   or the kill-switch surface.
2. **Manager-level approvals.** A request_confirmation from an
   Eng Manager (e.g. "approve DAM-31.G merge to main") still
   routes to the CTO; the manager is not the final word.
3. **Hiring and firing of managers.** Manager prompts are
   high-leverage and the CTO is the right person to write the
   role and vet the first 90 days of behavior.
4. **Mainnet signing, custody, capital decisions.** BotSRE's
   standing authority is delegated by the CTO; the manager does
   not change that.
5. **Board communication and roadmap.** Quarterly roadmap,
   board-level escalation, capital allocation.

## 6. Hiring triggers (not "do it now")

Do not hire any of these on this heartbeat. The plan is approved
*first*; the hire is a *follow-up issue* with a named unblock
owner (the CTO). Each slot has a specific trigger:

| Slot | Hire trigger |
|---|---|
| Eng Manager | Either (a) the CTO has ≥ 3 in_progress cross-domain issues on the same wake for two consecutive days, or (b) DAM-31.D and DAM-89 have both shipped to main. |
| QA & Reliability Manager | DAM-79 (SLO instrumentation) ships AND the `/api/slos` dashboard shows 3/3 SLOs with real data (not "Unknown"). |
| Paperclip Ops Coordinator | 7 days from now if ≥ 5 of the CTO's blocked issues in that window are Paperclip-ops-shaped. (Cheap to test; cheap to roll back.) |
| PR / Code Review Lead | Only if PR-review latency crosses 24h p50 for 2 consecutive weeks. Today it is well under 12h. |
| Product & UX Manager | When the operator console v2 ships and Product/UIUX each have ≥ 2 active issues. |
| Security Manager | Never proposed in this memo. Security continues to report directly to CTO with the existing dotted line. |

## 7. Open questions for the board (in the request_confirmation)

1. Confirm or reject the "hire Eng Manager first" recommendation.
2. Confirm or reject the "PR Review Lead = do not hire" recommendation.
3. Confirm or reject the "Paperclip Ops Coordinator = cheapest,
   highest-return" slot.
4. Confirm or reject the "manager prompts get written by the CTO
   with the CEO's review" rule for the first 90 days.

## 8. Follow-up issues (NOT created in this heartbeat)

After board sign-off, the following child issues should be created
on DAM-100 with `parentId = DAM-100` and routed to the CTO. They
are listed here so the plan is reviewable, not so the CEO can
self-delegate.

- DAM-100.a — write Eng Manager role prompt + escalation rules
  (owner: CTO, blocked on: board approval of §4.1).
- DAM-100.b — write QA & Reliability Manager role prompt
  (owner: CTO, blocked on: DAM-79 shipping).
- DAM-100.c — write Paperclip Ops Coordinator role prompt
  (owner: CTO, blocked on: 7-day observation window from this
  memo's date).
- DAM-100.d — security escalation runbook (owner: CTO, no hire,
  just a doc).

## 9. What this memo does *not* decide

- It does not hire anyone. The CEO does not run the paperclip-create-agent
  skill in this heartbeat.
- It does not change the CTO's role prompt.
- It does not change the manager-of-managers boundary. After hiring,
  managers report to the CTO; the CEO remains above the CTO.
- It does not change the escalation rules around mainnet signing or
  the kill-switch.

---

## Rev 2 — execution record (2026-06-21T09:18Z, board approved at 09:12:35Z)

### What was actually hired

The Paperclip `agent-hires` route rejects the literal `manager` role
enum. The closest valid enum values for the two slots:

- **EngManager** — agent id `f159ddbb-c8de-4cb0-ada8-7a1a48437317`,
  role `pm`, reports to CTO. 75-line AGENTS.md installed at
  `agents/f159ddbb-.../instructions/AGENTS.md`. Org chain healthy.
- **OpsCoordinator** — agent id `5a302ace-f0c0-4358-addb-58774f9010a3`,
  role `general`, reports to CTO. 62-line AGENTS.md installed at
  `agents/5a302ace-.../instructions/AGENTS.md`. Org chain healthy.

Both agents are `idle` until the CTO wakes them with a first
assignment. The `request_confirmation` `066614f6-...` was implicitly
accepted by the board's "hire those managers as per plan" comment.

### What was NOT hired (deferred per §4 + §6)

- QA & Reliability Manager — conditional on DAM-79 SLO instrumentation
  shipping. Open issue DAM-79 in CTO's queue.
- Product & UX Manager — deferred until operator console v2 lands.
- Security Manager — not proposed in rev 1. Security continues to
  report to CTO with the existing dotted line.

### Deviation from rev 1

The plan §8 promised four child issues (DAM-100.a/b/c/d). Rev 2 does
not create them; the board comment "hire those managers as per plan"
is direct execution, not a planning handoff. The CTO may file any
follow-up issues (e.g. refining the role prompts, writing the
escalation runbook) on the CTO's own queue.

### Handoff to the CTO

Both new agents are owned by the CTO. Suggested first assignments
(CEO recommendation, not a directive):

- **OpsCoordinator first task**: clear the 3 currently-blocked
  CTO issues that are `Review silent active run for CTO`. This
  delivers the slot's value within a single heartbeat and gives the
  CTO breathing room to test the handoff.
- **EngManager first task**: take the integration ownership of
  DAM-31.D Phase 3 (the dl-feed WS auto-reconnect work) once it
  moves from the CTO to the manager. The CTO reviews; the manager
  drives.

The CTO may reassign or re-prompt as needed; the manager prompts
are the first 90 days of behavior, not a permanent contract.

---

## Rev 3 — full manager layer (2026-06-21T09:30Z)

### Board correction (the actual ask)

Rev 2 added two managers but left 11 ICs as direct CTO reports. The
board's correction was explicit: every IC except CEO and CTO reports
to a field manager; every manager reports to CTO; CTO reports to CEO.
This is the final topology.

### Three new managers hired (POST /api/companies/{id}/agent-hires)

| Manager | id | role | Direct reports |
|---|---|---|---|
| **OpsManager** | `e3cdaf92-c167-42b4-96dd-a2d3a53854db` | pm | SRE, BotSRE, QA Reviewer, OpsCoordinator |
| **ProductManager** | `222b9a63-2311-4e67-9d79-0bc618d45abb` | pm | Product, UI/UX Developer |
| **SecurityManager** | `675bd5df-0350-425c-8f43-05c4657ce7eb` | general | Security |

### Final org (18 agents, 3 layers, every IC under a manager)

```
CEO
└── CTO
    ├── EngManager       (Backend, Frontend, Quant, Data, PR Reviewer)
    ├── OpsManager       (SRE, BotSRE, QA Reviewer, OpsCoordinator)
    ├── ProductManager   (Product, UI/UX Developer)
    └── SecurityManager  (Security)
```

### Twelve PATCH /api/agents/{id} re-routes (all 200, all reportsTo updated)

EngManager cluster: Backend, Frontend, Quant, Data, PR Reviewer.
OpsManager cluster: SRE, BotSRE, QA Reviewer, OpsCoordinator.
ProductManager cluster: Product, UI/UX Developer.
SecurityManager cluster: Security.

### Org chain health

Verified via GET /api/agents/{id} for deepest leaves: `Security →
SecurityManager → CTO → CEO` (depth 3), `EngManager → CTO → CEO`
(depth 2). All chains healthy.

### Preserved invariants

- **BotSRE standing kill-switch authority** — unchanged. OpsManager
  coordinates SLO/chaos readiness but does not gate, approve, or
  override kill-switch decisions. (This is also in the OpsManager
  AGENTS.md "What you do not have authority over" section.)
- **Security → CEO dotted line on capital/custody** — unchanged.
  Security events that touch custody, signing, or capital still
  escalate directly to the CEO without going through the CTO.
  (In the SecurityManager AGENTS.md "Capital/custody direct line
  to CEO" lens.)

### Routing fix: DAM-40 reassigned

DAM-40 ("Wire Whirlpool + DLMM vault subscriptions") was incorrectly
assigned to the CEO. Re-routed to EngManager `f159ddbb` via PATCH
issue. Work continues with no disruption.

### What was NOT created in rev 3

- No child issues on DAM-100 (the board's correction was direct
  execution, like rev 2).
- No new skills, no dashboard changes, no runbook changes. The
  reorg is purely `reportsTo` edges on the existing agents.
