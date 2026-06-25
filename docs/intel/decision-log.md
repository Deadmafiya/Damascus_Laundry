# Decision Log — Damascus Laundry

> **Status:** v1, bootstrapped 2026-06-21 by Project Archivist (DAM-114).
> **Rule:** append-only. New entries go at the top. Never edit an old
> entry; if it was wrong, add a correction entry dated the day you
> noticed, with a link to the old entry.
> **Format:** date · decision · decider · alternatives rejected + why ·
> discussion locus (issue/PR/board id).

This is the doc that answers "why is it this way?" six months from now.
The vision doc answers "what are we building?"; this one answers
"why didn't we build the other thing?" When the two contradict, the
vision doc wins — but the contradiction itself gets a new entry here.

---

## 2026-06-21 — v1.0 series lock

### 2026-06-21 · Manager reorg: CEO→CTO→Manager→IC is universal
- **Decider:** CEO
- **Locus:** DAM-100 (issue thread, rev 3)
- **Decision:** Full manager layer — EngManager, OpsManager, ProductManager, SecurityManager, OpsCoordinator — under the CTO. 14 ICs re-routed through managers. BotSRE kill-switch authority and Security→CEO dotted line preserved.
- **Rejected:** Flat IC reporting directly to the CEO (rev 1, rev 2). Failed because CEO context budget was being eaten by 14 simultaneous direct reports; managers were the delegation primitive that restored CEO scope.
- **Why this matters:** Any future org-change proposal must justify itself against this. The CEO is the bottleneck, and the manager layer is what makes the bottleneck survivable.

### 2026-06-21 · No private keys in the value path (architectural lock)
- **Decider:** CEO + CTO (early v1.1)
- **Locus:** `docs/architecture.md`, `crates/dl-app/src/live.rs`
- **Decision:** `dl-app run --feed live` never loads a keyfile. The `dl-signer` crate *can* derive keys from an encrypted keyfile for testing, but the production live path stays keyless. Real execution is out of scope for the v1.1 series; the executor module is the only thing that needs to change to go live.
- **Rejected:** "Just put a keyfile in the repo for testing." Failed because the moment a keyfile is in the value path, it becomes loadable in production by a config flip. Separation-by-crate is the only kind of separation that survives pressure.
- **Why this matters:** Any future PR that touches `dl-signer::keystore` needs a re-justification of why the keyless invariant still holds. The CEO is the final sign-off; the CTO can defer.

### 2026-06-21 · Cap-state persistence is a JSON snapshot, not a database
- **Decider:** CTO (DAM-67)
- **Locus:** DAM-67, commits `a91a971` + `a96434f`
- **Decision:** `CapState` persists across restarts as a JSON snapshot on disk. `dl-app --submit-live` calls `CapState::load_or_init`.
- **Rejected:** SQLite. Failed because (a) the cap is a single-process artifact for the v2.0 era, (b) JSON is human-readable and operator-debuggable, (c) introducing a DB dependency for one integer per day is over-engineering.
- **Why this matters:** When a future ticket proposes "we should put cap-state in Postgres," the answer is "no, because the JSON snapshot is the audit log; we want it readable."

### 2026-06-21 · v3 backtest: spec gate before code
- **Decider:** Quant (self-deferred)
- **Locus:** DAM-39, DAM-90, comment `71d9743e-…` on DAM-90
- **Decision:** DAM-90 (re-run v3 backtest on 30 captures) is *blocked* until (a) DAM-44 (Backend) ships the integration DAM-90 depends on, and (b) a v3 spec with §5.7 gates exists on disk. The ticket body says "DAM-40" but DAM-40 is Product; the real upstream is DAM-44. The Quant's v3 spec is not yet on disk.
- **Rejected:** Just re-run the v2 backtest on 30 captures and call it v3. Failed because "more data" is not a v3; the v3 spec is what defines the new EV model, and a re-run on more data is just a re-run.
- **Why this matters:** v3 is a strategy change, not a data change. Without a spec on disk, the ticket is uncargoable.

### 2026-06-21 · DAM-78 landing-rate targets: N1=0.70 / N2=0.55 / X1=0.40
- **Decider:** SRE (DAM-78), Quant in review
- **Locus:** DAM-78, `docs/observability/landing-rate-targets.md` v1
- **Decision:** Landing-rate targets per phase. Min-sample + recompute procedure on disk.
- **Rejected:** A single global "we want 60% landing rate" target. Failed because the rate is conditional on phase (1d tiny mainnet behaves differently from Phase 3 24/7) and DEX mix (Meteora DLMM lands differently from Raydium). Phase-conditional targets let the operator read the dashboard without a manual phase lookup.
- **Why this matters:** When the SLO breach hits (and it will, in early Phase 1d), the target is a defensible number, not a vibes number.

### 2026-06-21 · DAM-68: 4 alerts shipped, 3 of 4 silent until DAM-81
- **Decider:** SRE (DAM-68)
- **Locus:** DAM-68, `docs/observability/alerts.yml`
- **Decision:** 4 alerts shipped. 3 of 4 (the "live counters" alerts) are dormant until Backend Programmer wires the counters — that gate is DAM-81. The 4th (SLO #3: silent reverts) is dormant until DAM-79 lands the SLO #3 counters.
- **Rejected:** Wait until all 4 alerts can fire before shipping the YAML. Failed because (a) the YAML is the design surface, easier to review in place than in flight, (b) the operator needs the runbook URL even when the alert can't fire yet, (c) shipping one is cheaper than shipping four later.
- **Why this matters:** The pattern (ship the runbook + YAML together; wire the producer later) is the default for any future observability ticket.

### 2026-06-21 · DAM-75: 3 SLOs + 4-state budget policy
- **Decider:** SRE (DAM-75), CTO pending approval
- **Locus:** DAM-75, `docs/observability/slos.md` v1.1 (rev 2)
- **Decision:** 3 SLOs. (1) Submission-gate calibration (consistency with backtest). (2) Landing rate per DEX (N1/N2/X1 targets). (3) Zero silent reverts in a 30-day rolling window — pages on-call + CTO + Security. 4-state budget policy: green → yellow → red → page.
- **Rejected:** A flat SLO "% of bundles that land" as the only number. Failed because silent reverts (gate approves, no outcome observed) are the *catastrophic* failure mode for paper-trade confidence; they need their own SLO with their own page path.
- **Why this matters:** SLO #3 is the *correctness* SLO; the other two are *performance* SLOs. When something looks fine on the dashboard but the recon says "1 silent revert," SLO #3 fires and the operator trusts the page over the dashboard.

### 2026-06-21 · DAM-41 data architecture: cycle.v1 contract
- **Decider:** Quant + Data + CTO
- **Locus:** DAM-41, `docs/contracts/cycle.v1.md` + `docs/contracts/cycle.v1.schema.json`
- **Decision:** `cycle.v1` is a JSONL contract. One cycle per line. `cycle_id` = `blake3(sorted_legs_json || detected_at_slot)` as 64 lowercase hex chars. File: `wallet.cycles.v1.jsonl` next to the wallet. The ArbiNexus bridge continues to consume the legacy `wallet.cycles.jsonl` (v0 ad-hoc shape) for one release; the shim keeps the bridge unchanged.
- **Rejected:** Avro / Parquet / a real warehouse. Failed because (a) the v2.0 era is single-process, JSONL is human-readable, (b) the Data pipeline (DAM-43) reads JSONL natively, (c) the recon join (DAM-79) joins two JSONL files on `bundle_id` and that join is the audit trail. The DAM-46 `dl-pipeline` crate (in review) replaces the spec's originally-specced DuckDB file with the JSONL-on-disk warehouse, which is a strict improvement (operator-debuggable, no native deps).
- **Why this matters:** When a future ticket proposes "let's put the cycle warehouse in DuckDB," the answer is "we moved away from DuckDB on purpose — see DAM-46."

### 2026-06-21 · DAM-100 / DAM-44 / DAM-67 / DAM-76 / DAM-82 / DAM-89 / DAM-97 / DAM-98 → shipped

These are the 2026-06-21 tickets that landed on `main` or in_review
on this date. Each one is a *delivery* (code + tests + request_confirmation),
not a decision. They are listed here so the audit trail is complete; the
*why* of each is in the corresponding DAM issue thread, not in this log.

| Issue    | What shipped |
|----------|--------------|
| DAM-100  | Manager reorg rev 3 (CEO→CTO→Manager→IC) |
| DAM-76   | devnet e2e golden path (7 stages, `DL_E2E_DEVNET=1` gated, ~122s) |
| DAM-42   | Pyth live-acceptance fix (4 coupled bugs in `dl-oracle`) |
| DAM-81   | Wire live counters into `dl-app` MetricsRegistry |
| DAM-82   | `live_status.json` writer from `dl-app` (contract v1) |
| DAM-56   | devnet e2e smoke test passes offline |
| DAM-98   | DAM-64 reconciliation loop (branch `dam-98-dl-recon-64` @ `9c2097a`) |
| DAM-97   | `dl-feed::whirlpool` module landed on main to unblock DAM-64 build |
| DAM-62   | Orca Whirlpool vault subscription in live detector |
| DAM-63   | Captured-replay tests for Meteora DLMM vault subscriptions |
| DAM-57   | 11-test hermetic integration suite for `HttpJupiterClient` + `HttpJitoClient` |
| DAM-31.D | Phase 3 scaffolding — `dl-feed` auto-reconnect + staleness guard |
| DAM-67   | Cap state persists across restarts via JSON snapshot |
| DAM-61   | 0.5 SOL/day and 0.05 SOL/bundle mainnet cap floors |
| DAM-58   | 0.001 SOL/day mainnet-paper cap floor + `dl-app verify-mainnet-paper-cap` CLI |
| DAM-59   | Mainnet deploy verification script for `dl_assert_program` |
| DAM-70   | Phase 4 latency tier ladder research (`docs/research/latency-ladder.md`) |

---

## 2026 — pre-2026-06-21

The pre-v1.0 series (Phases 0–5) was research-engineering, not product
decisions. The decisions worth keeping from that era:

| Date | Decision | Decider | Locus | Rejected alternative |
|------|----------|---------|-------|----------------------|
| 2026 | Detector: Bellman-Ford, not linear search | Eng lead | `crates/dl-detect/src/bellman_ford.rs` | A naive pair/triple enumeration. Bellman-Ford finds *any* negative-weight cycle, not just 2/3-leg ones. |
| 2026 | Multi-DEX support from v1.0, not later | CEO + CTO | DAM-31 sub-phases | "Just Raydium first, add Orca and Meteora after v1.0." The 3-DEX triangle is where the profit lives; a Raydium-only detector has no edge. |
| 2026 | CostModel includes Jito tip + 5% Jito fee | Quant | `crates/dl-sim/src/cost.rs` | "Just subtract the base sig fee." Underestimates real cost by 10–100x; NetProfit becomes a fantasy. |
| 2026 | NetProfit is the boundary object, not gross spread | CEO | `docs/contracts/cycle.v1.md` | A "high gross-spread count" headline. PnL is what the operator actually pays attention to. |
| 2026 | Bincode paper ledger, not JSON | CTO | `crates/dl-ledger/` | A JSON wallet. Bincode is half the size, half the parse time; human-readability is provided by the cycle.v1 JSONL record next to the wallet. |
| 2026 | Conservative bound gates the trade; optimistic is diagnostic | CEO | `crates/dl-sim/src/ev.rs` | "Trade on optimistic." The whole point of the simulation is honesty; the conservative side is what tells you whether the trade is actually worth making. |
| 2026 | ArbiNexus as a downstream bridge, not a dependency | CEO | `vendor/arbinexus/` (submodule) | Forking ArbiNexus into `crates/`. The bridge is a read-only consumer of `wallet.cycles.jsonl`; vendoring as a submodule keeps the contract clean. |
| 2026 | 30% win rate is honest; uniform random is honest | Quant | `vendor/arbinexus/` | A 50% win rate with no data to back it. The 30% is documented as the random-loss injection, not a strategy claim. |

---

## Out of scope for this doc

- Per-PR rationale. That lives in the PR review thread and (when
  important) the commit message body.
- Process decisions (who-routes-what, which interaction kind to use).
  Those go in `AGENTS.md` for the relevant agent, not here.
- Architectural proposals that have not been decided. Those are
  *open*; route them to the CEO via the Intel Manager and don't write
  them down here until they are decided.
