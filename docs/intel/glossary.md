# Glossary — Damascus Laundry

> **Status:** v1, bootstrapped 2026-06-21 by Project Archivist (DAM-114).
> **Scope:** Internal shorthand, ticket history, and module references
> so terms don't drift in meaning across agents over time.
> **Rule:** Living doc. New entries are dated; old entries are not
> silently rewritten — corrections go in with a date and a backref.

If you encounter a term in a PR review or a comment and you can't find
it here, add it. The glossary is the place where "what does the team
mean by X?" lives.

---

## Module references

The Rust workspace has 17 crates. Each one has a single primary
responsibility; cross-crate dependencies are listed below.

| Crate | Responsibility | Key types | Notes |
|-------|----------------|-----------|-------|
| `dl-app` | The binary. Wires every other crate together; reads CLI flags; emits `live_status.json` (DAM-82) and `wallet.cycles.v1.jsonl` (cycle.v1 contract). | `submit_opportunity_with_simulate` (DAM-84), `live.rs::cycles_from_capture` (DAM-62), `MetricsRegistry` (DAM-81), `cycle_writer`, `gate_writer` | The only crate with a `main.rs`; the operator-facing surface. |
| `dl-detect` | Bellman-Ford negative-cycle detection on the in-memory price graph. Owns `Graph`, `Cycle`, and the staleness guard (`prune_stale_edges`, DAM-55 home). | `find_negative_cycles`, `prune_stale_edges`, `staleness` | The staleness module is *here*, not in `dl-state` (per DAM-55, dep-cycle constraint). |
| `dl-state` | Shared types: `Pool`, `Mint`, `Cycle`, `LegKey`, `Pubkey`, the per-DEX decoders. | `dl_state::pool::AmmKind`, `dl_state::cycle::{Cycle,Direction}` | No I/O; pure data + pure decode functions. |
| `dl-feed` | WebSocket pool subscriptions. `ws_feed`, `reconnect`, `registry`, `staleness`, `metrics_hook`, `whirlpool` (DAM-52), `capture` (DAM-39). | `FeedEvent::Pool`, `FeedEvent::StalePoolHalt` (DAM-84) | Auto-reconnect + per-DEX dispatch. |
| `dl-stream` | Streaming detector wrapper. Bridges `dl-feed` events into `dl-detect`'s graph. Owns `StreamingDetector`. | `crates/dl-stream/src/detector.rs` | The `:204` line is the known workspace break (DAM-69). |
| `dl-sim` | Per-cycle evaluation. `EvalParams`, `CostModel`, `simulate_cycle`, `EvalOutcome`, `Decision` (WouldTrade / WouldNotTrade). | `EvalParams::conservative_default()`, `EvalParams::optimistic()` | The conservative side gates the trade; the optimistic side is diagnostic. |
| `dl-executor` | Paper-mode executor + hot-wallet signer. The *only* crate that needs to change to go live (v1.2+). | (n/a — to be expanded) | The separation-by-crate is the safety. |
| `dl-signer` | Keyfile management + cap enforcement. `keystore`, `cap`, `livemode`, `ratelimit`. | `CapState::load_or_init` (DAM-67), `dl-app verify-mainnet-paper-cap` (DAM-58) | Cap state is a JSON snapshot, not a DB. |
| `dl-recon` | Backtest + on-chain reconciliation harness. `bundles`, `reconcile`, `invariants`, `onchain`, `pipeline`, `fixture`, `fault`. | `dl-recon replay-bundles` (SLO #3 source) | The SLO #3 "silent revert" counter comes from this crate. |
| `dl-recon-overfit` | Calibration-drift detector. The weekly review runs this. | (n/a) | Lower frequency than `dl-recon`. |
| `dl-calibration` | Fits p_detect / p_win / p_land from a `.dlf` capture. `calibrate --from-capture` (DAM-35). | `calibration.json` output | The output is consumed by the v3 spec (not yet shipped; DAM-90). |
| `dl-oracle` | Pyth price feed integration. `HttpPythClient`. | skew tolerance 5s → 120s (DAM-42) | Live-acceptance fixed in DAM-42. |
| `dl-core` | Tiny shared crate: `cycle_id_hex`, `LegKey`. | `cycle_id_hex(legs, slot)` | The single source of truth for the cycle_id hash. |
| `dl-ledger` | Append-only bincode paper ledger (DLD-LDG1). | (n/a) | Crash-safety: torn-write detection on next read. |
| `dl-assert-program` | On-chain BPF assert program. Deployed via `dl-assert-sdk` (DAM-59). | `dl_assert_program` | The locked atomicity decision is custom BPF (option a), not a third-party CPI. |
| `dl-assert-sdk` | CLI for operator pre-flight (`dl-assert-sdk …`). | (n/a) | The operator's "did the deploy succeed?" tool. |
| `dl-paper` | Paper-trade mode glue. | (n/a) | Wraps `dl-executor` in paper-only paths. |
| `dl-pipeline` | **(NOT YET ON DISK)** | The DAM-46 work is staged on branch `dam-46-dl-pipeline` @ `cabdbf6` but the crate is not in the workspace. | Status: 42/42 tests green on the branch; CTO `request_confirmation` pending. |

---

## Internal shorthand

### A–E

- **AC-1, AC-2, AC-3, AC-4** — Acceptance Criteria from the v1.0 plan.
  AC-1 = per-DEX decoders. AC-2 = decoder round-trip tests. AC-3 = fill
  math for each DEX. AC-4 = multi-DEX triangle (Raydium + Orca +
  Meteora). All four are now shipped (commits `a27281a`, `5b5629a`).
- **advisor (or advisor IA)** — The DAM-106 layout for the operator
  console. Three regions: **State** (top, big numbers), **Recent
  history** (middle, last 50 bundles), **Next action** (bottom, a
  single opinionated sentence). Supersedes the DAM-72 "5-field grid"
  layout. The console *trusts* the producer's `next_action`; the
  producer is Backend Programmer's DAM-40 P0-3.
- **AmmKind** — `dl_state::pool::AmmKind` enum: `RaydiumAmmV4`,
  `OrcaWhirlpool`, `MeteoraDlmm`. Three variants today; the cycle.v1
  contract schema has a closed enum `[raydium, orca, meteora]`.
- **bp (basis point)** — 1/100th of a percent. The conservative
  detector gate uses 10bp decay in `conservative_default()`.
- **bundle** — A Jito transaction bundle. Identified by `bundle_id`
  (a Jito-assigned UUID). The DAM-79 recon join uses `bundle_id` to
  match `gate_events.jsonl` (gate approvals) against `outcomes.jsonl`
  (landed/failed-cleanly).

### F–J

- **`feed`** — CLI flag: `dl-app run --feed live | capture`. `live`
  connects to `DL_LIVE_WS_URL`; `capture` reads a `.dlf` file.
- **`FeedEvent::Pool` / `FeedEvent::StalePoolHalt`** — Variants of
  `dl_feed::FeedEvent`. `Pool` is a normal pool update;
  `StalePoolHalt` is emitted by the dl-feed staleness guard when a
  pool's vault age exceeds the configured threshold (DAM-84).
- **gate_approved** — The kind tag in `wallet.gate_events.jsonl`
  (DAM-79 / SLO #3). A row = one simulate-gate approval.
- **`gate_writer`** — `crates/dl-app/src/gate_writer.rs`. Companion
  to `cycle_writer`; the two are joined on `bundle_id` to surface
  silent reverts.
- **gtm (go-to-mainnet)** — The state of being ready to submit a real
  bundle. v2.0 is *not* gtm; the engine is paper-trade only. The
  gtm date is not set; DAM-77 is the recon gate, DAM-103/DAM-107
  are the scheduler.
- **Jito ShredStream** — Tier 2 in the DAM-70 latency ladder.
  Sub-ms shred feed. $800+/mo.
- **`jl` (jetson-lamports)** — Informal abbreviation for lamports,
  the SOL sub-unit. 1 SOL = 1_000_000_000 lamports.

### K–O

- **lamports** — The SOL sub-unit. 10⁹ per SOL. All on-chain amounts
  in `dl-app` are in lamports (integer math; no floats).
- **`live_status.json`** — The DAM-82 wire contract. 1Hz snapshot
  written by `dl-app` next to the wallet. The DAM-72 console and the
  DAM-106 advisor both consume it. The DAM-82 v1 contract
  (cap/kill/last_landed/pnl/sol_usd) is preserved; the DAM-106
  advisor fields are *additive*. The contract spec is in
  `docs/console/advisor-contract-v1.md`.
- **LiveMode** — The v1.1.0 gate (commit `2f36d13`). Three states:
  `devnet`, `mainnet-paper`, `mainnet`. Empty = refused. The
  paper mode is the only one that can write to a wallet; `mainnet`
  requires DAM-67 cap + DAM-58 floor + DAM-61 floors.
- **manager reorg** — The DAM-100 CEO decision: CEO → CTO →
  Manager → IC. Managers: EngManager, OpsManager, ProductManager,
  SecurityManager, OpsCoordinator.

### P–T

- **paper ledger** — The `dl-ledger` crate. Append-only bincode
  frames (DLD-LDG1). The human-readable view is the cycle.v1 JSONL
  next to the bincode file.
- **paper mode** — `DL_PAPER_MODE=optimistic | realistic`. Optimistic
  = 100% wins, no decay (use for cycle *detection* visualization).
  Realistic = 30% wins, 10bp decay (use for honest PnL).
- **phase** — The v2.0 plan is structured in phases. Phase 1a/b/c/d
  → Phase 2 (calibration) → Phase 3 (24/7 reliability) → Phase 4
  (scale). Each phase has a tier from the latency ladder.
- **PnL (Profit and Loss)** — Reported in SOL and lamports. The
  `realized_pnl_today_sol` / `realized_pnl_today_lamports` fields
  in `live_status.json` are the day's total. The ArbiNexus bridge
  has its own PnL (`wallet_paper.json`); the two are different
  number spaces and must not be conflated.
- **recon (reconciliation)** — `dl-recon` is the harness. The
  weekly review uses `dl-recon-overfit`. The daily `dl-recon
  replay-bundles` is the SLO #3 source.
- **SLO #1** — Submission-gate calibration (DAM-75). A bundle the
  gate approves must, in backtest, have a positive expected net.
- **SLO #2** — Landing rate per DEX (DAM-75). Targets from
  DAM-78: N1=0.70, N2=0.55, X1=0.40.
- **SLO #3** — Zero silent reverts in a 30-day rolling window
  (DAM-75). Pages on-call + CTO + Security. The `DlSilentRevert`
  alert in `alerts.yml` fires on any silent revert; the
  `DlSimulateGateHighRejection` alert fires when the 5m rejection
  rate is above 50% (a leading indicator).
- **Solo Captain** — The DAM-106 persona. An operator who runs the
  bot as a side activity; their worst day is "I can't tell if the
  numbers are bad or the bot is down." The advisor IA is designed
  for them.
- **StalePoolHalt** — See `FeedEvent::StalePoolHalt`. The dl-feed
  pattern (DAM-84) that emits a halt event when a pool's reserves
  age past the configured threshold. The intent is to *fail closed*
  on stale reserves rather than trade on phantom weights.

### U–Z

- **vault subscription** — A WebSocket subscription to a pool's
  SPL-token vault accounts. The reserves for the constant-product
  math live in the vaults, not in `AmmInfo`. DAM-62 ships Whirlpool
  vault subscriptions; DAM-89 (DLMM) is the Meteora half.
- **v0 / v1 / v2 / v3** — Versioning for the cycle record and the
  detector:
  - **v0** = the legacy `wallet.cycles.jsonl` (ad-hoc shape, base_mint
    = "unknown", dex = "raydium"). The ArbiNexus bridge consumes v0
    until DAM-44 lands. The shim in `cycle_writer.rs` writes both.
  - **v1** = the `cycle.v1` JSONL contract (DAM-41 / DAM-43).
    `cycle_id` is blake3. The Data pipeline reads v1.
  - **v2** = the simulation engine version (the v2.0 plan).
  - **v3** = the next-gen EV model. Spec is not yet on disk;
    DAM-90 is blocked on the spec gate.
- **v1.1.x** — The paper-trading release series. v1.1.0 is the
  LiveMode gate; v1.1.7 is the realistic-mode. v1.2+ is real
  execution (out of scope for the v1.1 series; only `dl-executor`
  needs to change).
- **wallet.json / wallet.cycles.jsonl / wallet.cycles.v1.jsonl /
  wallet.gate_events.jsonl / wallet_paper.json** — The five files
  the operator cares about. `wallet.json` is the bincode paper
  ledger. `wallet.cycles.jsonl` is the v0 cycle record. `wallet.
  cycles.v1.jsonl` is the v1 contract record. `wallet.gate_events
  .jsonl` is the DAM-79 SLO #3 source. `wallet_paper.json` is the
  ArbiNexus bridge output (30% win rate). All five land next to
  the wallet via `append_jsonl_path`.

---

## DAM-xx ticket history (selected, last 7 days)

The DAM series is the executable backlog of the v2.0 plan. The
*full* ticket history lives in the Paperclip issue tracker; this
is the curated index for context that recurs.

| ID | Title | Status | Anchor | One-line |
|----|-------|--------|--------|----------|
| DAM-22 | Reconciliation spec | shipped | (BotSRE) | sim-vs-live spec, the parent of DAM-38. |
| DAM-31 | Live mainnet wire (umbrella) | shipped | commit `7406682` | The umbrella for the LiveMode series; sub-phases A/B/C/D. |
| DAM-31.D | Phase 3 (dl-feed auto-reconnect) | shipped | commit `3f75e71` | 56/56 tests; `dl-app::metrics_prom` e2e deferred to DAM-79. |
| DAM-33 | Phase 1b state (closure-variant refactor) | done | branch `dam-84-sim-fn` @ `db00b93` | `submit_opportunity_with_simulate` + 3 unit tests. |
| DAM-35 | dl-calibration end-to-end | shipped | branch `dam-35/dl-calibration-end-to-end` @ `3e3b739` | 12/12 tests. |
| DAM-38 | Reconciliation spec (replacement for DAM-22) | in review | comment `b4292ee4-…` | 4 child issues DAM-38a/b/c/d. |
| DAM-39 | v2 baseline backtest | done | (Quant) | 183 trades, +0.009144 SOL, 6024 cycles. v3 deferred. |
| DAM-40 | Multi-DEX backend integration | in review | (Backend) | The umbrella for DAM-41/44/89/97; DAM-40 P0-3 is the named unblock for the advisor `next_action` producer. |
| DAM-41 | Data architecture | in review | `request_confirmation` `86969a5a` | cycle.v1 contract + DAM-43/46/47 children. |
| DAM-42 | Pyth live-acceptance fix | in review | `request_confirmation` `adee4657` | 4 coupled bugs in `dl-oracle`. |
| DAM-43 | Quant consumer of cycle.v1 | (open) | child of DAM-41 | The Data pipeline reads v1. |
| DAM-44 | Backend integration | in review | (Backend) | 4 children: 44a/b decoder+tests on main, 44c on `dam-54/dam-44c-restore` @ `966292e`, 44d blocked. |
| DAM-46 | dl-pipeline | in review | branch `dam-46-dl-pipeline` @ `cabdbf6` | 42/42 tests; JSONL-on-disk warehouse replaces the DuckDB spec. |
| DAM-47 | Phase A 24h gate | (open) | child of DAM-41 | Cannot run until `wallet.cycles.v1.jsonl` exists. |
| DAM-53 | Meteora DLMM feed test | done | commit `394ca04` on `main` | 6/6 dlmm_lb_pair_feed tests. |
| DAM-54 | staleness restore | in review | branch `dam-54/dam-44c-restore` @ `966292e` | 5 files restored +606/-1; the prior wake's "shipped" claim was a wake-vs-disk mismatch. |
| DAM-56 | devnet e2e smoke | in review | commit `633264c` | 1/1 on `cargo test -p dl-app devnet_smoke`. |
| DAM-57 | HTTP clients hermetic tests | shipped | commit `d567ca9` | 11 tests; production wiring (DAM-92) is separate. |
| DAM-58 | mainnet-paper cap floor | shipped | branch `dam-58-cap-floor` @ `dd9d8f6` | 0.001 SOL/day floor. |
| DAM-59 | dl_assert_program deploy | shipped | commit `9406f9b` | Mainnet deploy verification script. |
| DAM-61 | mainnet cap floors | shipped | commit `ed3469d` | 0.5 SOL/day, 0.05 SOL/bundle. |
| DAM-62 | Orca Whirlpool vault subscription | done | commit `3df04ee` on `main` | First non-Raydium DEX in the live detector. |
| DAM-63 | Meteora DLMM capture-replay | done | commit `053a5d2` on `main` | Closes the Meteora half. |
| DAM-64 | dl-recon warehouse | in review | branch `dam-98-dl-recon-64` @ `9c2097a` | Recon join (gate approvals vs outcomes) on `bundle_id`. |
| DAM-65 | Niche selection | shipped | (Quant) | 4 niches ranked + 4 disabled. |
| DAM-67 | Cap state persistence | shipped | commits `a91a971` + `a96434f` | `CapState::load_or_init`. |
| DAM-68 | alerts + runbook | shipped | `docs/observability/alerts.yml` | 4 alerts, 3 of 4 silent until DAM-81. |
| DAM-69 | chaos drills | in review | (BotSRE) | Real workspace break: `dl-stream/detector.rs:204` stray `}` + `dl_detect::staleness` not declared in `lib.rs`. |
| DAM-70 | Phase 4 latency ladder | shipped | `docs/research/latency-ladder.md` | 4 tiers, break-even math. |
| DAM-72 | Live operator console (5-field) | done | `/live.html` + `/api/live` at 1Hz | Superseded by DAM-106 (3-region advisor). |
| DAM-75 | SLOs and budget policy | in review | `request_confirmation` (pending) | 3 SLOs + 4-state budget policy. |
| DAM-76 | devnet e2e golden path | shipped | commit `f5b96be` on `main` | 7 stages, `DL_E2E_DEVNET=1` gated, ~122s budget. |
| DAM-77 | Recon gate | blocked | (SRE) | Blocked on DAM-31; ±0.001 SOL gate not yet runnable. |
| DAM-78 | Landing-rate targets | shipped | `docs/observability/landing-rate-targets.md` | N1=0.70 / N2=0.55 / X1=0.40. |
| DAM-79 | SLO #3 counters | (open) | (SRE) | The counters that feed `dl_simulate_silent_revert_total`. |
| DAM-80 | SLO row on dashboard | shipped | branch `dam-80/slo-row` @ `9ca6675` | /api/slos + 3-card row in `dashboard/index.html`. |
| DAM-81 | Wire live counters | shipped | commit `d070fd4` on `main` | The 3 of 4 DAM-68 alerts come alive. |
| DAM-82 | live_status.json writer | shipped | commits `7717a76` + `19d8c5e` on `main` | Contract v1. |
| DAM-87 | Live nightly | (open) | child of DAM-76 | The cron that runs the e2e against live mainnet. |
| DAM-89 | DAM-40 (Orca + Meteora) | shipped | branch `dam-89/dam-40-ormea-meteora` @ `5416d73` | 18 files +2435/-12; live WS dispatch. |
| DAM-90 | v3 backtest on 30 captures | blocked | comment `71d9743e-…` | Blocked on DAM-44 + missing v3 spec. |
| DAM-91 | DAM-80a (slos.md typo) | (open) | child of DAM-80 | 43.2→432 min typo fix; the slos.md file itself lands here. |
| DAM-92 | Production `send_with_retry` wiring | (open) | child of DAM-57 | The test-local retry is not in prod yet. |
| DAM-93 | alerts.yml (observability docs) | (open) | child of DAM-80 | The alerts.yml file lands here. |
| DAM-97 | whirlpool on main | done | commit `29dc52a` on `main` | DAM-64.a done. |
| DAM-98 | DAM-64 reconciliation loop | in review | branch `dam-98-dl-recon-64` @ `9c2097a` | 5 reverted edits re-applied. |
| DAM-100 | Manager reorg (rev 3) | shipped | (issue thread) | CEO→CTO→Manager→IC universal. |
| DAM-102 | On-chain sweep | blocked | branch `dam-102/dam-38b-onchain-sweep` @ `1d85965` | First-class blocked on SRE RPC tier + DAM-38a merge. |
| DAM-103 | Daily reconcile | blocked | branch `dam-103-daily-reconcile` @ `b779dd1` | 4 shell scripts + 1 hermetic test. Superseded by DAM-107. |
| DAM-106 | advisor IA | in review | branch `dam-106-advisor-ia` @ `57fbb70` | 3-region advisor; `request_confirmation` `4eef38b8-…`. |
| DAM-107 | systemd pick (scheduler) | in review | branch `dam-107-daily-reconcile-scheduler` @ `115cead` | Option A: systemd timer @ 23:55 UTC + Persistent=true. |
| DAM-114 | **Project Archivist bootstrap** | **in progress** | (this doc) | The four docs/intel/ files. |

---

## Status conventions

- **DRAFT** — Written by an agent, not yet approved by the named
  decider. Must be marked as such. Downstream readers should not
  treat as canonical.
- **shipped** — Code + tests + commit on a branch; not yet
  approved for merge. The branch HEAD is the truth, not the
  branch name.
- **in_review** — A `request_confirmation` exists; the named
  reviewer (CTO/CEO) has not yet responded. The branch is
  preserved at `.claude/worktrees/<branch-name>`.
- **done** — Approved + merged + `request_confirmation` accepted.
- **blocked** — A first-class blocker exists; the unblock owner
  is named in the issue body. `re-mark blocked` is the
  default re-wake action until the unblock fires.

---

## Cross-references

- `docs/architecture.md` — Runtime architecture (for code modifiers).
- `docs/v2.0-operator-runbook.md` — Operator SOPs (hot-wallet, kill
  switch, daily recon). Read top-to-bottom once before the first
  mainnet-paper run.
- `docs/console/advisor-contract-v1.md` — The on-wire shape of
  `live_status.json` (DAM-82 + DAM-106 additive).
- `docs/observability/alerts.yml` — Prometheus alert rules.
- `docs/observability/slos.md` — SLO definitions (when committed;
  currently inlined in `damascus_laundry_dashboard`).
- `docs/research/latency-ladder.md` — Phase 4 tier comparison.
- `plan/damascus_laundry_v2.0.md` — The v2.0 plan; the source of
  truth for phase structure and cap values.
- `plan/atomicity-decision.md` — Locked decision (option a: custom
  BPF assert program). Read once before the first mainnet-paper run.
- The `MEMORY.md` file at `~/.claude/projects/-home-deadmafia-Documents-damascus-laundry/memory/`
  is the per-agent memory index; it points to topic files for
  cross-issue context.
