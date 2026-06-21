# Cap-Raise Protocol — Phase 4

> **Status:** Specification (no execution). Gated on Phase 3 (DAM-31) being
> delivered to production and observing ≥1 mainnet cycle at the current cap.
>
> **Owner:** Quant
> **Source issue:** DAM-71
> **Related:** DAM-58 (paper cap floor), DAM-61 (mainnet cap floors),
> `crates/dl-signer/src/cap.rs` (the runtime cap that this protocol governs).

## 1. Purpose

The current cap is pinned at **0.5 SOL/day** for `DL_LIVE_MODE=mainnet`
(DAM-61 / Phase 1d). At that size the bot is a probe: it cannot discover
genuine edge, but it can demonstrate that the runtime, signing path, and
risk controls hold under real adversarial conditions. Once those hold, we
want to grow the cap — *carefully*. The cap is the primary security control
in the hot-wallet model (see `docs/v1.1.md` §5.1); raising it is a one-way
door with capital exposure as the price of admission.

This protocol specifies the conditions under which the cap is raised,
and the conditions under which it is dropped back down. It is a written
contract between the operator and the bot: every cap change references
this document and the gating metrics it names.

## 2. Cap Tiers

| Tier | Daily cap | Per-bundle cap | Phase | Notes |
|------|-----------|----------------|-------|-------|
| **T0 (floor)** | 0.5 SOL | 0.05 SOL | Phase 1d (DAM-61) | Pinned in source. **Cannot be raised by this protocol.** Source change + rebuild required. |
| **T1** | 5 SOL | 0.5 SOL | Phase 4 entry | First cap the protocol can grant. 10× the floor. |
| **T2** | 50 SOL | 5 SOL | Phase 4 mid | 10× T1. |
| **T3** | 500 SOL | 50 SOL | Phase 4 long-horizon | 10× T2. Out of scope for the first 90 days of operation. Revisit at the T2 → T3 gate. |

Why 10× per step: at the current per-bundle cap of 0.05 SOL, the PnL
variance of a single bundle is dominated by microstructural noise. A
10× step is the smallest step that makes per-tier Sharpe ratios
*distinguishable* in a window of ~200 trades. Smaller steps dilute the
signal; larger steps amplify the downside if the gate is wrong.

## 3. Promotion Gates (raise)

All four conditions must hold over the **evaluation window** at the
current cap tier before promotion to the next tier.

### 3.1 Sample size (G1)

> **N ≥ 200 landed bundles at the current cap.**

A promotion decision is a hypothesis test on the Sharpe ratio (§3.2) and
on the loss-tail behavior (§3.4). 200 trades is the smallest sample
where a Sharpe estimate has roughly ±0.4 standard error at the 1.0 level
(assumes IID; the autocorrelation check in §3.5 weakens this assumption
in a controllable way). Below 200 trades, the test is too noisy to
justify raising the cap.

Trades counted: any bundle that landed on-chain and has a recorded
realized PnL entry in `wallet.cycles.jsonl` (or the v1 successor;
`cycle.v1` per DAM-47). Bundles that errored, dropped, or were
rejected by Jito do **not** count.

### 3.2 Sharpe (G2)

> **Sharpe ratio over the window > 1.0, computed on per-trade returns
> in basis points relative to per-bundle cap.**

Definition:

```
r_i = realized_pnl_lamports_i / per_bundle_cap_lamports_at_trade_i
Sharpe_window = mean(r_i) / stddev(r_i) * sqrt(N_window)
```

The Sharpe is normalized to *per-bundle-cap* units, not absolute SOL.
This makes the threshold tier-invariant: a Sharpe of 1.0 at T0 means
the same thing as a Sharpe of 1.0 at T1. The scaling matters because
absolute PnL variance scales with the cap, but the question we are
asking — "is this strategy's edge real?" — is scale-invariant.

> **Note (open question §8.Q1):** the per-bundle-cap normalization
> assumes the PnL distribution is homoscedastic in cap, which is only
> approximately true. We may need a stratified Sharpe. Defer to the
> first re-cut after T1 is granted.

### 3.3 Drawdown (G3)

> **Maximum intra-window drawdown < 20% of the *cumulative* realized
> PnL at the current cap, AND no single 24h period with realized
> PnL < −0.1 × cap (the kill condition from §4).**

This is the "no blow-up" gate. A high-Sharpe strategy with a 50% peak-
to-trough drawdown is not safe to scale — the 50% drawdown scales with
the cap, and at T2 a 50% drawdown is 25 SOL. 20% is a tight gate but
matches the risk budget in `docs/observability/slos.md` v1.1.

The 24h-loss sub-clause is the "kill condition" from §4 surfaced as a
promotion gate: any 24h period that *would have* dropped us back a
tier disqualifies the window. This closes the loophole where a smooth
60-day average hides a near-miss.

### 3.4 Loss-tail (G4)

> **No more than 2% of landed bundles have realized PnL
> < −0.5 × per-bundle cap.**

A "fat tail" in the loss distribution means the strategy occasionally
loses an amount that is no longer bounded by the per-bundle cap (e.g.,
adverse selection, sandwich attacks, oracle staleness). At T0, a loss
of 0.5 × per-bundle cap is 0.025 SOL — survivable. At T2, the same
*event* (not the same multiple) is 2.5 SOL. The 2% threshold is the
"this is still a real-arbitrage loss, not a black-swan loss" check.

### 3.5 Autocorrelation (G5, advisory)

> **Lag-1 autocorrelation of per-trade returns < 0.2, OR a documented
> adjustment to the Sharpe threshold is applied.**

If consecutive trade returns are correlated (e.g., position carryover,
adverse-selection clustering), the effective sample size is smaller
than 200 and the Sharpe estimate is overstated. The default correction
is to inflate the Sharpe threshold by `1 / sqrt(1 − ρ₁)`. If we
cannot bring ρ₁ below 0.2 *or* apply the correction, the promotion
is deferred.

This is advisory: a high ρ₁ does not block promotion if the
correction is applied and documented. It blocks promotion if neither is
done.

## 4. Demotion Gates (drop)

Demotion is *faster* than promotion. The protocol's asymmetry is
deliberate: we want to be slow to scale up (good months can be lucky)
and fast to scale down (a bad day at T2 is real money).

### 4.1 Hard kill (D1)

> **If any 24h period has realized PnL < −0.1 × cap, drop one tier
> immediately. No operator override. No 24h grace period.**

This is the kill condition from the issue body. It is enforced in
`crates/dl-signer/src/cap.rs` (or its successor) at the runtime layer,
not as an operator decision: the cap should drop *before* a human
notices the loss. Implementation: the live counter in `dl-app`
(`dl_daily_pnl_sol`, per DAM-81) feeds a 24h-rolling check; on breach
the cap is rewritten to the prior tier and the operator console
advisor surfaces the event (DAM-106 advisor IA, "State" region).

### 4.2 Soft kill (D2)

> **If any 7-day window has realized PnL < 0, OR the rolling 30-day
> Sharpe drops below 0.5, drop one tier after a 24h operator
> review window.**

Softer than D1 because the trigger is slow (a 7-day losing streak is
not catastrophic, but it is informative). The 24h review window is for
the operator to decide whether to override (e.g., "we know about the
Jito tip bump, ignore the 7-day loss"). Override is logged in the
issue thread; auto-drop proceeds if no override is posted.

### 4.3 Hard stop (D3)

> **If two demotions in a 30-day window, or realized PnL < −cap
> in any single 24h period, stop the bot and require an explicit
> restart from the board.**

The D3 condition is "the strategy is no longer working at this size
and we do not know why." Restarting requires a postmortem and a
re-derivation of the promotion gates, not just a button press.

## 5. Evaluation window

> **The evaluation window is 7 days at the current cap, with rolling
> re-evaluation.**

A promotion requires G1–G4 to hold for the *trailing 7 days*, not for
the lifetime at the current cap. This means a tier-1 strategy that was
great for 60 days and then degrades gets caught within 7 days, not 60.

The 7-day window is also the minimum re-evaluation interval: a
promotion is *not* re-evaluated more often than once per 7 days, even
if all the gates pass. This is to prevent promotion-chasing on a
lucky streak.

## 6. Operating procedure

1. **State check (daily, automated):** the live-status JSON
   (`live_status.json` per DAM-82) carries the current cap tier and
   the trailing-7d Sharpe / drawdown / loss-tail metrics. The operator
   console displays them in the "Next action" region.
2. **Gate check (every 7 days, automated):** a scheduled job evaluates
   G1–G5 over the trailing 7 days. If all pass, the advisor emits a
   "promote to T_n+1" recommendation; if any fails, the advisor emits
   a "hold at T_n" or "demote to T_n-1" recommendation per §4.
3. **Operator action (manual):** promotions and soft demotions (D2)
   require a one-line operator comment on the DAM-71 thread ("promote
   to T1, G1–G4 met, 2026-MM-DD") before the cap is changed. Hard
   demotions (D1) and hard stops (D3) are automatic.
4. **Audit (per change):** every cap change writes a row to
   `cap_changes.jsonl` with the from-tier, to-tier, triggering gate
   set, and a SHA-256 of the metrics snapshot that triggered the
   change. The audit log is the source of truth for post-incident
   review.

## 7. Out of scope

- **T3 (500 SOL/day).** The first three tiers (T0, T1, T2) cover the
  first 90 days of live operation. T3 requires a re-derivation of the
  gates — the loss-tail threshold and the drawdown cap almost
  certainly do not scale linearly past 50 SOL.
- **Per-DEX or per-route caps.** This protocol governs the *global*
  daily cap. Per-route caps are a separate concern (orthogonal to
  tier) and may be layered on top later.
- **Dynamic caps.** This is a step-function protocol, not a continuous
  one. Continuous caps invite micromanagement and obscure the
  threshold behavior. If we want continuous, we re-derive from scratch.

## 8. Open questions for the implementer

- **Q1:** Is the per-bundle-cap normalization in G2 the right scale-
  invariance, or do we need a stratified Sharpe? (Defer to first re-cut
  after T1 is granted.)
- **Q2:** The 7-day evaluation window is a default. Should T2 → T3
  require a longer window (e.g., 30 days) given the larger absolute
  exposure? (Re-derive at the T2 → T3 gate.)
- **Q3:** The D1 hard-kill threshold of −0.1 × cap is from the issue
  body. Does this need to scale with the *number of bundles landed*
  in the 24h window? (E.g., a 24h period with 5 bundles and a 0.05-SOL
  loss is qualitatively different from a 24h period with 200 bundles
  and a 0.05-SOL loss. Same absolute PnL, very different signal.)
  Defer until we have T1 data.
- **Q4:** The audit log (`cap_changes.jsonl`) is named but not yet
  specced. Owner: Backend. Filed as a follow-up to DAM-71.

## 9. Change log

| Rev | Date | Author | Notes |
|-----|------|--------|-------|
| 1   | 2026-06-21 | Quant (CTO sign-off pending) | Initial spec. Tier ladder T0/T1/T2/T3, gates G1–G5, demotions D1–D3, 7-day rolling window. |
