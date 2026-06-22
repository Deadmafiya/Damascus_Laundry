# Latency Tier Ladder — Phase 4 Research

> **Status:** research only. No execution. Gated on Phase 3 go/no-go
> (24/7 reliability proven; see `plan/damascus_laundry_v2.0.md`).
> **Owner:** BotSRE.
> **Acceptance:** this document (comparison table + per-tier break-even math).
> **Author:** BotSRE. **Date:** 2026-06-21.

## Scope

Compare the four infra tiers in the v2.0 plan's Phase 4:

| Tier | Tier name | Monthly cost (range) | What it buys |
|---|---|---|---|
| 0 | Free public RPC (current) | $0 | baseline we measure against |
| 1 | Paid business RPC | $50–250/mo | reliable WS, higher rate limit, dedicated bandwidth |
| 2 | Jito ShredStream | $800+/mo | sub-ms shred feed, faster mempool-equivalent visibility |
| 3 | Region co-location near a Jito relayer | $2–5k/mo | single-digit-ms RTT to block engine, shared-nothing host |
| 4 | Own Jito-Solana / Firedancer validator | $10k+/mo setup + ongoing | lowest possible landing latency, full control of tip routing |

For each tier 1–4: expected landing-rate improvement over the prior tier,
prerequisites, **time-to-break-even** on the prior tier's profit, and the
metric we use to decide "this tier earned its keep."

## Why this doc is gated on Phase 3

Phases 0–3 must be done first because the whole ladder is funded out of
**realized profit**:

- **Phase 0** locks the atomicity model and kill-switches.
- **Phase 1** lands one real trade end-to-end and proves the bundle path.
- **Phase 2** calibrates the EV model against on-chain truth — this is the
  data that lets us predict how much a latency improvement is worth in $.
- **Phase 3** proves the bot survives a multi-day unattended run with no
  safety violation. Only then is "spend $5k/mo on co-lo" a serious
  question; before Phase 3, infra spend is just amplifying a system that
  has not yet earned the right to be scaled.

If Phase 3 fails, the doc is still useful as a reference but no tier above
the current one should be purchased.

## How to read the break-even math

The math is intentionally rough — these are decision thresholds, not
financial models. The formula in one line:

```
days_to_breakeven  =  tier_monthly_cost  /  (profit_per_day_after_tier - profit_per_day_before_tier)
```

Concretely: if Tier 2 costs $800/mo ($26.67/day) and is expected to lift
realized profit by $X/day over Tier 1, then `days = 26.67 / X`. We use
**days, not months**, because the question is "how long until the tier
pays for itself" — and the answer should be a small number, not a date
that's a year out.

We treat anything > 90 days as "do not buy" and anything > 30 days as
"buy only after a measurable Phase 2/3 baseline."

## The comparison table

| Tier | Monthly cost | Wall-clock to deploy | Expected submit→landed p50 (ms, est.) | Expected landing-rate lift vs prior tier | Profit lift needed to breakeven in 30 days | Profit lift needed to breakeven in 90 days | Prerequisites |
|---|---|---|---|---|---|---|---|
| **0. Free public RPC** (current) | $0 | — | 800–1500 (RTT-bound, often drops @ ~60s) | — (baseline) | — | — | none |
| **1. Paid business RPC** (Helius Business / Triton Project / QuickNode Build+) | $50–250 | <1 day | 100–300 (region-dependent) | ~2–5× over free, mostly from "stays connected" + gRPC rather than raw RTT | $1.67–$8.33/day | $0.56–$2.78/day | Phase 2 calibration data (need the model to convert latency→$) |
| **2. Jito ShredStream** | $800–$1,500 | 1–2 weeks (account, contract, key ceremony) | 20–80 | ~1.5–3× over paid RPC, but **only for mempool-visible arbs**; pure on-graph detection gains little | $26.67–$50/day | $8.89–$16.67/day | Tier 1 already up; Phase 2 niche selection; verifiable landing-rate baseline |
| **3. Region co-lo (Equinix NY4 / LA1, 1–2 Jito-block-engine colos)** | $2,000–$5,000 + setup | 3–6 weeks (colo contract, hardware, network) | 2–10 | ~1.5–3× over ShredStream, but only if TipRouter submission is used | $66.67–$166.67/day | $22.22–$55.56/day | Tier 2 proven net-positive for 30+ days; colocation contract; failover path |
| **4. Own Jito-Solana / Firedancer validator** | $10,000+ setup + $2k+/mo ongoing | 2–6 months (hardware, staking, identity, audit) | <2 (validator→itself) | asymptotic — gives MEV-internal revenue, not just lower searcher latency | $333+/day to breakeven on $10k setup in 30 days | $111+/day ongoing | Tier 3 proven net-positive for 60+ days; capital beyond hot-wallet; staking ops capacity |

> **Why a wide range, not a point estimate.** Landing rate is a function
> of (a) the fraction of cycles that are *contested* in a given block and
> (b) the absolute latency gap to the next-fastest searcher. Both depend
> on niche selection, which is a Phase 2 deliverable. The doc deliberately
> leaves the niche-conditioned number as a range to be filled in once
> Phase 2's calibration report is in.

## Per-tier detail

### Tier 0 — Free public RPC (status quo)

We measure this tier first because everything else is relative to it.

- **Cost:** $0, plus operator time.
- **Behavior observed in this repo:**
  `docs/known-limitations.md:27` notes public `api.mainnet-beta.solana.com`
  disconnects sustained WebSocket after ~60s. That alone disqualifies it
  for any overnight or Phase 2/3 run.
  `docs/v2.0-operator-runbook.md:146` flags "Paid RPC upgrade (recommended
  ~$50–250/mo Helius/Triton/QuickNode)" as the minimum for Phase 2.
- **Expected p50 submit→landed:** 800–1500 ms. The "submit→landing_ms"
  histogram in `dl-latency-probe` is the canonical source; we should run
  it against the free endpoint at least once to anchor Tier 0.
- **Landing-rate baseline (from `plan/damascus_laundry_v2.0.md`):** the
  project already quotes "~96% of attempted atomic arbitrages on Solana
  fail" — most of those failures are on free infra.
- **Decision:** keep Tier 0 only for development and devnet. The first
  action of Phase 1c (mainnet-paper) is to leave Tier 0.

### Tier 1 — Paid business RPC ($50–250/mo)

- **Providers worth comparing** (as of 2026-06, prices and rate limits
  should be re-verified before purchase — all are "moving" numbers):
  - **Helius Business** — $199/mo list, includes Sender (transaction
    landing optimization), gRPC for high-throughput accounts, dedicated
    WS. Best DX for our use case because Sender's landing path is
    designed for the same MEV bundles we're sending.
  - **Triton Project** — similar tier, gRPC-heavy, can be slightly
    cheaper. Rate limits are stricter than Helius.
  - **QuickNode Build+** — best DX, but some tiers restrict WS
    sustained throughput. Verify before signing up.
- **Expected landing-rate lift:** ~2–5× over free, but the lift comes
  mostly from (a) WS not dropping, (b) higher `getMultipleAccounts`
  throughput letting the detector run at full rate, and (c) consistent
  p50 RTT. Pure RTT is usually <300 ms from a us-east VPS.
- **Break-even math:**
  - $50/mo → $1.67/day. If Phase 2 data shows our current net is negative
    or break-even on free, the paid RPC only needs to push us into
    positive territory for a few hours/day to pay for itself.
  - $250/mo → $8.33/day. We need ~0.07 SOL/day net profit uplift
    (at $120/SOL) to pay back in 30 days. This is plausible if Tier 0
    is leaving the bot idle due to WS drops.
- **Prerequisites:**
  - Phase 1c should be runnable on this tier — nothing in Tier 1
    changes the bundle path.
  - The latency probe must be rerun on the new endpoint; the
    `dl_submission_to_landing_ms` histogram is the regression detector
    for "did we just lose the upgrade."
- **Expected deploy wall-clock:** <1 day (account signup, key
  provisioning, env var swap).
- **Recommendation:** **buy this at the start of Phase 2.** The
  v2.0 plan already says this. It's the cheapest step that unlocks
  calibration data and 24/7 runs.

### Tier 2 — Jito ShredStream ($800+/mo)

- **What it actually is:** a paid, authenticated WebSocket feed of
  Solana **shreds** (the lowest-level signed block-parts, before they're
  assembled into blocks). The pitch is sub-millisecond time-to-first-byte
  on a block, so you can compute negative cycles and submit a bundle
  *inside the same slot* the opportunity appears.
- **Where the lift comes from:**
  - For **mempool-equivalent** races (e.g. a pending swap on Raydium that
    creates a temporary mispricing we can arb against) — ShredStream
    is the difference between seeing the trade and not seeing it.
  - For **purely on-graph detection** (price differences that exist
    across public state with no mempool trigger) — Tier 2 helps less,
    because the data is already in the accounts we subscribe to. The
    real lift is in the submission path, not the detection path.
- **Where it doesn't help:**
  - Jito bundles are already on the Jito path. ShredStream doesn't
    change the bundle submission or the tip — only the *signal* into
    our detector. If our strategy is purely cross-DEX on public state,
    the marginal value of ShredStream is small.
- **Expected landing-rate lift:** ~1.5–3× over Tier 1, **conditional on
  niche**. If Phase 2's niche selection lands on mempool-triggered
  opportunities, the lift can be 3×+. If we end up in the on-graph niche,
  it may be 1.2×.
- **Break-even math:**
  - $800/mo → $26.67/day. To pay back in 30 days we need ~$0.22/day
    net profit uplift per dollar of tier cost, or ~$0.67/SOL at
    $120/SOL. This is plausible only if Tier 1 already runs net
    positive *and* we have a niche where ShredStream is the limiting
    factor.
  - $1,500/mo → $50/day. Needs ~$1.67/SOL/day uplift. Plausible only
    for a high-volume niche.
- **Prerequisites:**
  - Tier 1 already deployed and the latency probe is healthy.
  - Phase 2 calibration has identified a niche where mempool visibility
    is the bottleneck, not on-graph detection.
  - The detector has a code path that consumes shred-level data. Today
    `dl-feed` consumes `programSubscribe` and `accountSubscribe`
    JSON-RPC streams — a ShredStream integration is a new feed
    adapter, not a config change. Estimate: 1–2 weeks of code + test.
- **Recommendation:** **only buy once Tier 1 has produced a positive
  net-PnL baseline AND Phase 2's niche selection has flagged
  mempool-triggered arbs as the winning niche.** Otherwise this is a
  $800+/mo tax on a system that wasn't bottlenecked on detection.

### Tier 3 — Region co-location near a Jito relayer ($2–5k/mo)

- **What it actually is:** renting a 1U or fractional rack in a colo
  facility that is on the same switch fabric as a Jito Block Engine
  (e.g. NY4 for the NY block engine, LAX1 for the west-coast one).
  The bundle submission RTT drops from 20–80 ms (internet path) to
  2–10 ms (same-AZ, often same-switch).
- **Where the lift comes from:**
  - This is the tier that matters for *contested* races — when two
    searchers see the same opportunity, the one whose bundle lands at
    the block engine first wins. 10 ms vs 50 ms is the difference
    between "we got the slot" and "we didn't."
  - Also helps the `getBundleStatuses` poll loop land faster, which
    reduces the false-negative rate on "Landed" and tightens our
    reconciliation.
- **Where it doesn't help:**
  - Detection latency. If we never see the opportunity, no amount of
    colo speed helps.
- **Expected landing-rate lift:** ~1.5–3× over Tier 2 in contested
  races. If our niche is un-contested (e.g. smaller pools that pros
  don't watch), the lift is much smaller.
- **Break-even math:**
  - $2,000/mo → $66.67/day. To pay back in 30 days we need $2.22/SOL/day
    uplift at $120/SOL. This is plausible for a high-volume niche,
    implausible for a slow one.
  - $5,000/mo → $166.67/day. Needs $5.56/SOL/day uplift. This is the
    threshold where the math only works if we're processing a serious
    fraction of a SOL per day in landed arbs.
- **Prerequisites:**
  - Tier 2 proven net-positive for ≥30 days. The decision is
    "spend $2–5k/mo" and we don't make that decision on speculation.
  - A colo contract (1U reservation, 1 Gbit/s, power, remote hands).
    The big-line-item question is "which Jito relayer are we
    co-locating to" — there are usually 2–3 main block engines and
    the choice is decided by where the winning bundles from the
    prior tier went. (This is itself a data point from Phase 2.)
  - A **failover path** to Tier 1/2 if the colo host dies, fails
    auth, or the Jito relayer has an outage. Without a failover, a
    colo outage means zero revenue; with failover, it means
    degraded-tier revenue.
- **Recommendation:** **buy only after Tier 2 has shown net-positive
  for a sustained window AND the niche selection has identified a
  contested-race strategy.** The risk of buying this tier too early
  is high: it's a big fixed cost on a system whose strategy isn't
  yet proven.

### Tier 4 — Own Jito-Solana / Firedancer validator ($10k+/mo setup + ongoing)

- **What it actually is:** running our own Solana validator (either
  the Jito-Solana fork, which participates in the MEV auction as both
  a searcher and a block producer, or Firedancer for raw throughput)
  on colocated hardware. We become our own block producer.
- **Where the lift comes from:**
  - **Validator→itself latency** is sub-millisecond. We can land
    bundles we authored with effectively zero submission cost in time.
  - **MEV-internal revenue**: as a block producer, we get the MEV
    rewards (tips) on bundles that *other* searchers land in our
    blocks. This is a separate revenue stream from our own search.
  - **Tip routing control**: with our own validator, we can route
    tips to ourselves via a tip program we control. The economics
    shift from "we pay tips to win races" to "we collect tips from
    races that land in our blocks."
- **Where it doesn't help:**
  - The detection problem. Owning a validator doesn't tell us about
    more opportunities; it just changes the economics of landing
    the ones we find.
  - Capital intensity: $10k+ setup is a *capex*, not opex. The
    break-even math below treats it as opex for simplicity, which
    is generous to the tier.
- **Expected landing-rate lift:** asymptotic — at the limit, every
  bundle we author lands in our own block. The real metric is
  **MEV revenue per day**, not landing rate.
- **Break-even math (capex treated as opex for simplicity):**
  - $10k setup + $2k/mo ongoing. Setup only: 30-day payback
    needs $333/day MEV uplift; 90-day payback needs $111/day.
  - Ongoing $2k/mo: $66.67/day just to cover operations. Needs
    *both* the search-side uplift and the validator-side MEV
    revenue to clear.
- **Prerequisites:**
  - **Tier 3 proven net-positive for ≥60 days** (we don't run a
    validator to fix a niche problem we haven't yet proved).
  - Capital beyond the hot wallet — staking a validator requires
    bonded SOL (or delegated stake), not just opex.
  - Staking ops capacity: someone has to monitor the validator
    24/7, run upgrade cycles, handle slashing conditions. This
    is a real headcount/ops problem, not a "we'll get to it" item.
  - Legal/entity questions about validator rewards, taxes, and
    custody of the staked SOL.
- **Recommendation:** **do not buy this in 2026.** This is the
  tier we write a doc about so that, in 12+ months, when the
  searcher side is proven and the strategy is well-defined, we
  have a reference to come back to. It's also the tier where the
  *company's* risk profile changes (we go from running a bot to
  running infrastructure that other people stake to), so the
  decision belongs to CEO + Security + Legal, not to BotSRE.

## Decision criteria: when to buy each tier

This is the section a future operator will reference at the moment of
truth. Each tier is gated on **measured, durable** prior-tier evidence,
not on vibes.

| Decision | Trigger | Evidence to gather | Approver |
|---|---|---|---|
| Free → Tier 1 | Start of Phase 2; or any incident where WS drop cost us a run | `dl-latency-probe` on free RPC shows p99 > 5s OR WS uptime < 50% over 24h | BotSRE + CTO |
| Tier 1 → Tier 2 | Tier 1 net-positive for ≥30 days AND niche flagged as mempool-triggered | `dl-recon` report + niche doc with winning-bundle block-engine IDs | BotSRE + Quant |
| Tier 2 → Tier 3 | Tier 2 net-positive for ≥30 days AND niche flagged as contested | Same as above, plus 30-day variance: daily PnL σ < X (need to define X) | BotSRE + CEO |
| Tier 3 → Tier 4 | Tier 3 net-positive for ≥60 days AND we have staking ops capacity | Capex approval + legal entity capable of receiving validator rewards | CEO + Security + Legal |

The "variance < X" condition for Tier 3 is important. A daily PnL of
+$5 with σ = $50 means we're not really earning $5/day — we're
occasionally lucky. We don't scale infra on luck.

## What we measure to know if a tier is earning its keep

These are the on-chain + on-host signals, all already plumbed in
`dl-app` Prometheus metrics:

- `dl_submission_to_landing_ms` histogram — the headline latency
  metric. Tier 0 → 1 should show p50 drop of 3–10×. Tier 1 → 2
  should show p50 drop of ~3–5×. Tier 2 → 3 should show p50 drop
  of ~3–8×. Tier 3 → 4 should show p50 drop of ~5–10× **and**
  validator-side MEV revenue stream.
- `dl_landed_bundles_total` counter — landing count per slot/window.
  This is the "are we winning more races" question, the most
  decision-relevant metric for tiers 1–3.
- `dl_jito_tip_lamports_total` — what we paid in tips. Used to
  compute net PnL after Jito costs. Tier 4 (own validator)
  decouples this from external tip markets.
- `dl_consensus_reconnect_total` — how many times we lost the WS.
  Tier 0 → 1 should make this number go to ~0.
- Reconciliation delta from `dl-recon` (per the v2.0 plan's daily
  review) — sim vs. live PnL. A sudden divergence after a tier
  upgrade means the tier changed something we didn't model.

## What could go wrong

These are the failure modes the ladder can introduce, named so the
operator watches for them:

- **Tier 1 outage during calibration.** If the paid RPC has an
  incident during Phase 2's calibration window, the calibration
  data is contaminated. We should run calibration only after the
  new endpoint has a 7-day uptime baseline.
- **Tier 2 channel auth failure mid-flight.** ShredStream requires
  a key; if the key rotates and our bot doesn't get the new one,
  we silently lose the feed. Treat the feed as a hard dependency
  in the kill-switch logic.
- **Tier 3 colo's Jito relayer has an outage.** Without a failover
  path, a 4-hour outage at the colo = 4 hours of zero revenue and
  *still* paying $2–5k/mo. Failover to Tier 2 is non-negotiable.
- **Tier 4 slashing / downtime.** A validator we run is a validator
  we can be slashed on. Pre-Tier 4 we need: a separate
  ops-on-call rotation, an alerting path that's not "the same
  Prometheus we're already running," and a written incident
  playbook for downtime.
- **Buying the next tier before the prior one is measured.** The
  easiest way to burn $5k/mo is to buy co-lo while the prior tier
  is still showing negative net. Gating each tier on the prior
  tier's measured net-positive window is the single most important
  rule in this doc.

## Open questions for the next heartbeat

These are the things this doc *cannot* answer because they require
data we don't have yet:

1. **What fraction of our cycles are contested?** Without a
   reproducible contested-race signal in our captured data, the
   "1.5–3×" landing-rate lift is a guess. Answer belongs in the
   Phase 2 niche-selection deliverable.
2. **What is the cross-tier landing-rate delta on *our* hot path?**
   The "expected lift" numbers in the table are from public
   sources; ours may differ. We need to run the latency probe
   at each tier as we adopt it and update this doc with the
   actual delta.
3. **What's the Jito block-engine distribution for our winning
   bundles?** This decides which colo to buy at Tier 3. Phase 2
   reconciliation should record block-engine IDs.
4. **What is the variance of our daily PnL at each tier?** The
   "σ < X" condition for Tier 3 needs a real number. Belongs in
   the Phase 2/3 reconciliation report.

## Cross-references

- `plan/damascus_laundry_v2.0.md` — the source of the four-tier
  structure and the Phase 3 go/no-go gate.
- `docs/v2.0-operator-runbook.md` — the operator checklist for
  Phase 1c/1d that Tier 1 enables.
- `docs/known-limitations.md:27` — the WS-drop limitation that
  disqualifies Tier 0 for any overnight or Phase 2/3 run.
- `docs/live-runbook.md` — the kill-switch + cap rules that gate
  every tier's spend.
- (Pending) `docs/research/onchain-arb-anchor-dataset.md` and
  the rest of the `.paul/research/` corpus — currently
  un-staged in the working tree, but contains the Jito/BAM/
  Firedancer background this doc summarizes. Re-stage and link
  from here when those land.

## Change log

- **2026-06-21** — initial draft (BotSRE, DAM-70).
