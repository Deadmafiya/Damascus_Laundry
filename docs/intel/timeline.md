# Project Timeline — Damascus Laundry

> **Status:** v1, bootstrapped 2026-06-21 by Project Archivist (DAM-114).
> **Scope:** Major project milestones, dated, with the issue/PR/commit that
> anchored them and a one-line "what changed and why."
> **Source of truth:** `git log` on `main`, DAM-xx issue stream, the prior
> `docs/STATE` (deleted) and `docs/ROADMAP` (deleted) history in commit
> messages.

This is a **narrative doc**, not a changelog. The changelog is `git log`.
What goes here is the *why* behind the *what* — what decision the commit
was an artifact of, what it unlocked, and what it cost.

---

## 2026 — the v1.1 paper-trading series

The v1.1 series took the engine from "research code" to "runs on a
laptop, writes a wallet.json, doesn't touch real money." Every release
in this series is a tag on `main`; the sub-tags (`v1.1.0-executor` etc.)
are the atomic commits within each release.

| Date       | Milestone | Anchor | What changed and why |
|------------|-----------|--------|----------------------|
| 2026-06-21 | **v1.1.7-realistic-mode** shipped | commit `abc5747` on `main`; 441 tests passing | The ArbiNexus bridge landed and a 30% win-rate loss model was wired in. Before this, paper PnL was nonsense (100% wins); after, it means something. |
| 2026-06-21 | ArbiNexus bridge merged | commit `ae0024f` (`feat(09-07): bridge to ArbiNexus paper-trade simulator`) | Bridge reads `wallet.cycles.jsonl`, applies an oracle confidence gate, writes `wallet_paper.json`. The bridge is downstream of `dl-app`; if `dl-app` isn't running, the bridge has nothing to process. |
| 2026-06-21 | Optimistic paper mode | commit `8252c60` (`fix(09-06): paper mode uses optimistic bound so wallet records every cycle`) | Before, `conservative_default()` rejected most sub-bp cycles and the wallet never grew. Switched to `EvalParams::optimistic()` in paper mode so the operator *sees* every cycle the detector finds. |
| 2026-06-21 | v1.1.4 — vault subscriptions | commit `08e1975` (`feat(09-05): vault subscriptions + dynamic pool addition`) | Pool reserves come from SPL-token vault accounts, not from `AmmInfo`. Subscribing to vaults is the only way the graph has edge weights; without it, no cycles are detected at all. |
| 2026-06-21 | v1.1.3 — conservative detector gate | (per docs) | The detector now runs `conservative_default()` before writing a trade. The optimizer's *optimistic* answer is kept for diagnostics; the *conservative* answer is what gates the trade. |
| 2026-06-21 | v1.1.2 — live mainnet wire | commit `7406682` (`feat(09-02): wire --feed live to mainnet WS, support DL_LIVE_POOL_PUBKEYS`) | AccountUpdate stream from a paid RPC. Gated by `LiveMode` (DAM-31) so devnet and mainnet can't be confused. |
| 2026-06-21 | v1.1.1 — live paper trader | commit `86ea882` (`feat(09-01): live paper trader + dl-paper + start/status scripts`) | `start_paper_trader.sh` + `status.sh` ship with the repo. Status script shows SOL+USD, beginner-friendly layout. |
| 2026-06-21 | v1.1.0 — LiveMode gate | commit `2f36d13` (`feat(08-03): LiveMode gate, dl-signer CLI, dl-app run live path`) | The first release that could touch a private key in *testing*. The `dl-signer` keystore was extracted into its own crate so the value path stays keyless. |
| 2026-06-21 | v1.1.0-streaming | commit `57d5ce6` (`feat(08-02): streaming detector + latency benchmark`) | The detector went from batch to streaming: it now consumes AccountUpdate events and runs Bellman-Ford on every pool update instead of over a static snapshot. |
| 2026-06-21 | v1.1.0-executor | commit `a93f291` (`feat(08-01): paper-mode executor + hot-wallet signer`) | The `dl-executor` crate was extracted from `dl-app`. It is the only thing that needs to change when the project moves from paper to real execution. |

## 2026 — the v1.0 multi-DEX series

| Date       | Milestone | Anchor | What changed and why |
|------------|-----------|--------|----------------------|
| 2026-06-21 | v1.0 — multi-DEX triangle (AC-4) | commit `5b5629a` (`feat(07-02): per-DEX edge labeling + AC-4 multi-DEX triangle`) | The detector learned to label edges by DEX. A 3-leg cycle across Raydium + Orca Whirlpool + Meteora DLMM is the new maximum-profit shape on mainnet. |
| 2026-06-21 | Orca Whirlpool + Meteora DLMM decoders | commit `a27281a` (`feat(07-02): Orca Whirlpool + Meteora DLMM decoders (AC-1, AC-2)`) | Per-DEX decode paths. Each DEX has a different binary layout for pool state; the decoders turn it into a common `dl_state::pool::Pool`. |
| 2026-06-21 | On-chain reconciliation + dl-recon | commit `eff6ff7` (`feat(06-recon): onchain reconciliation + overfit metrics + recon CLI`) | A backtest harness that compares simulated trades against on-chain fills. Surfaces calibration drift. |
| 2026-06-21 | Paper ledger (DLD-LDG1) | commit `4084ebd` (`feat(05-sim): paper ledger (DLD-LDG1, append-only bincode frames)`) | The wallet became append-only bincode frames. Crash-safety: a torn write is detected on next read. |
| 2026-06-21 | CostModel + CostBreakdown | commit `f7451d6` (`feat(04-sim): CostModel + CostBreakdown`) | The simulator learned the real cost: base sig fee + priority fee + Jito tip + 5% Jito fee. NetProfit is now a boundary object, not an afterthought. |
| 2026-06-21 | fill_constant_product | commit `9b556c7` (`feat(04-sim): fill_constant_product primitive + SimError + module skeleton`) | The atomic math primitive. Every DEX's fill is built on top of it. |

## 2026 — the v2.0 / SRE era

The v2.0 plan reorganizes the work around the *phases* in
`plan/damascus_laundry_v2.0.md` (read that doc; it is the source of
truth for phase structure). The DAM-xx ticket series is the executable
backlog of the plan.

| Date       | Milestone | Anchor | What changed and why |
|------------|-----------|--------|----------------------|
| 2026-06-21 | **DAM-106: advisor IA shipped** | branch `dam-106-advisor-ia` @ `57fbb70`; 3-region advisor layout (State/Recent/Next action) supersedes DAM-72's 5-field grid | The "Solo Captain" persona's worst day is "I can't tell if the numbers are bad or the bot is down." The advisor renders one screen, three regions, no tabs. The `next_action` region is the only field the console cannot fake — until Backend Programmer wires DAM-40 P0-3, the console shows an honest "Pending: …" sentence. |
| 2026-06-21 | **DAM-76: devnet e2e golden path** | commit `f5b96be` on `main`; 7-stage test, `DL_E2E_DEVNET=1` gated, ~122s budget | First end-to-end test that doesn't lie. Previous "smoke tests" passed offline but never touched a real RPC. DAM-87 owns the live nightly. |
| 2026-06-21 | **DAM-42: Pyth live-acceptance fix** | commit `ff50470` on `main`; 4 coupled bugs in `dl-oracle` | URL form (`?ids=` rejected by live hermes.pyth.network), pubkey hex vs bs58, parser shape, stale SOL/USD constant. Skew tolerance bumped 5s → 120s. |
| 2026-06-21 | **DAM-81: wire live counters** | commit `d070fd4` on `main` | The metrics counters for live PnL/cap/bundles are now actually emitted by `dl-app`'s `MetricsRegistry`. Until this commit, DAM-68 alerts were 3 of 4 silent. |
| 2026-06-21 | **DAM-82: live_status.json writer** | commit `7717a76` + `19d8c5e` on `main`; contract v1 | `dl-app` writes a 1Hz JSON snapshot to `live_status.json`. The DAM-72 console and the DAM-106 advisor both consume it. |
| 2026-06-21 | **DAM-56: devnet e2e smoke** | commit `633264c` on `main` | Offline-acceptance test. CTO-fixed `ReconError::Json` + dl-signer serde deps + removed phantom `dl-pipeline` workspace entry. |
| 2026-06-21 | **DAM-98: DAM-64 reconciliation loop** | branch `dam-98-dl-recon-64` @ `9c2097a` (unmerged) | 5 reverted edits re-applied in worktree `/tmp/dam64-b`. 38/38 dl-recon + 12/12 dl-calibration (`--features dam64`) green. |
| 2026-06-21 | **DAM-97: whirlpool landed on main** | commit `29dc52a` on `main` | `dl-feed::whirlpool` module was on `dam-52` and `dam-89` branches but missing from main. Backend Programmer (DAM-97) copied it across to unblock the DAM-64 build. |
| 2026-06-21 | **DAM-62: Orca Whirlpool vault subscription** | commit `3df04ee` on `main`; `crates/dl-app/src/live.rs::cycles_from_capture` | First non-Raydium DEX in the live detector. Vault subscriptions for Whirlpool are 256-byte simplified layout only. |
| 2026-06-21 | **DAM-63: Meteora DLMM capture-replay tests** | commit `053a5d2` on `main` | Two green tests: full 9-frame wire-format round-trip + partial-capture guard. Closes the Meteora half of the 3-DEX surface. |
| 2026-06-21 | **DAM-57: 11-test hermetic integration suite** | commit `d567ca9` on `main` | Jupiter /quote parse, /swap base64, Jito sendBundle, getBundleStatuses Landed/Failed/Pending, HTTP timeout/5xx retry. Test-local `send_with_retry` is NOT wired into production clients — DAM-92 owns that. |
| 2026-06-21 | **DAM-31.D Phase 3 (dl-feed)** | commit `3f75e71` on `main`; 56/56 tests | Auto-reconnect + registry + staleness + metrics_hook + ws_feed integration. `dl-app::metrics_prom` e2e deferred to DAM-79. |
| 2026-06-21 | **DAM-67: cap state persistence** | commits `a91a971` + `a96434f` on `main` | `CapState::load_or_init` wired into `dl-app --submit-live`. State persists across restarts via JSON snapshot. |
| 2026-06-21 | **DAM-61: mainnet cap floors** | commit `ed3469d` on `main` | 0.5 SOL/day and 0.05 SOL/bundle mainnet cap floors. Distinct from DAM-58 (0.001 SOL/day paper floor). |
| 2026-06-21 | **DAM-59: dl_assert_program deploy verification** | commit `9406f9b` on `main` | Mainnet deploy verification script. |
| 2026-06-21 | **DAM-70: Phase 4 latency tier ladder** | commit `7814e33` on `main`; `docs/research/latency-ladder.md` | Compares 4 infra tiers (paid RPC → Jito ShredStream → co-location → own validator) with break-even math. |
| 2026-06-21 | **DAM-58: mainnet-paper cap floor** | branch `dam-58-cap-floor` @ `dd9d8f6` | 0.001 SOL/day floor in `dl-signer::cap` + `dl-app verify-mainnet-paper-cap` CLI. |
| 2026-06-21 | **DAM-100: manager reorg (rev 3)** | DAM-100 (issue thread) | CEO→CTO→Manager→IC is universal. EngManager + OpsManager + ProductManager + SecurityManager + OpsCoordinator under CTO. BotSRE kill-switch authority + Security→CEO dotted line preserved. 14 ICs re-routed. |
| 2026-06-21 | **DAM-41: data architecture delivered** | DAM-41 (issue thread) | Cycle.v1 contract + DAM-43 (Quant) + DAM-46 (dl-pipeline) + DAM-47 (Data gate). DAM-41 in_review with `request_confirmation` `86969a5a` pending CTO answer. |
| 2026-06-21 | **DAM-78: landing-rate targets v1** | DAM-78 (issue thread); `docs/observability/landing-rate-targets.md` | N1=0.70 / N2=0.55 / X1=0.40 + min-sample + recompute procedure. SRE re-cut on metric names (issue said `dl_bundles_*`, canonical is `dl_jito_*`). |
| 2026-06-21 | **DAM-68: alerts + runbook v1** | DAM-68 (issue thread); `docs/observability/alerts.yml` | 4 alerts shipped; 3 of 4 silent until DAM-81 wired the counters. The 4th (SLO #3) needed DAM-79 to land. |
| 2026-06-21 | **DAM-75: SLOs and budget policy v1.1 (rev 2)** | DAM-75 (issue thread) | 3 SLOs + 4-state budget policy. `request_confirmation` pending CTO approval. Note: `docs/observability/slos.md` is not on disk in this worktree (see [[dam-80-observability-docs-missing]]); constants are inlined in `damascus_laundry_dashboard`. |

## Out of scope for this doc

- The pre-v1.0 series (Phases 0–5). See `git log` for the full pre-2026
  history; that is research-engineering, not the product.
- Per-PR-level detail. The unit of this doc is the milestone, not the
  commit. The commit hash is the *anchor*, not the *content*.
- Anti-milestones (decisions *not* made). Those go in
  `decision-log.md` under "rejected" lines.
