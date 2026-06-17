# Solana MEV Landscape — Research Findings (2025–2026)

> **Purpose:** Domain map for building a paper-trading MEV bot on Solana.
> **Compiled:** 2026-06-17. **Author:** research pass via primary sources (Jito docs, Helius, Umbra Research) + recent news verification.
>
> **Confidence legend:** **High** = directly stated in a primary/reputable source and stable; **Medium** = stated but dated, single-source, or partially inferred; **Low** = inference, fast-moving, or could not fully verify.
>
> **Reading note:** Several headline numbers come from the Helius "Solana MEV Report" (published 2025, by Lostin) and Umbra Research pieces. Many dollar figures are *point-in-time* and should be treated as **estimates / illustrative**, not current. I did not independently reproduce any on-chain numbers.

---

## 0. Source inventory (with what each is good for)

| Source | URL | Date | Used for | Confidence in source |
|---|---|---|---|---|
| Helius — *Solana MEV: An Introduction* (Ryan Chern) | https://www.helius.dev/blog/solana-mev-an-introduction | Jun 1, 2024 | Supply chain, relayer role, mempool removal | High |
| Helius — *Solana MEV Report* (Lostin) | https://www.helius.dev/blog/solana-mev-report | 2025 | Timeline, strategy taxonomy, profit figures, mitigation | High (but figures are point-in-time) |
| Umbra Research — *MEV on Solana* | https://www.umbraresearch.xyz/writings/mev-on-solana | 2023 (foundational) | Strategy mechanics, colocation dynamics | High (but pre-dates mempool removal) |
| Umbra Research — *Lifecycle of a Solana Transaction* | https://www.umbraresearch.xyz/writings/lifecycle-of-a-solana-transaction | 2023 | Scheduler, no-mempool, fee mechanics | High |
| Jito Docs — *Low Latency Txn Send / Bundles* | https://docs.jito.wtf/lowlatencytxnsend/ | living doc | Bundles, tips, revert protection, `jitodontfront` | High |
| Jito Docs — *ShredStream* | https://docs.jito.wtf/lowlatencytxnfeed/ | living doc | Low-latency shred feed | High |
| Helius — *Priority Fees* (0xIchigo) | https://www.helius.dev/blog/priority-fees-understanding-solanas-transaction-fee-mechanics | living doc | CU pricing, local/global fee markets | High |
| Jump — *Firedancer* | https://jumpcrypto.com/firedancer/ | living | Firedancer overview | High |
| CoinDesk / Helius / Blockworks — *Jito BAM* | see §1.6 | Jul–Sep 2025 | BAM launch, TEEs, economics | High |
| Solana Compass / BlockEden / CryptoSlate — *Firedancer stake* | see §1.7 | Oct 2025 – early 2026 | Firedancer/Frankendancer stake share | Medium (figures fluctuate daily) |

**Could NOT verify / blocked during research:** Jito's own blog index (`jito.network/blog`, HTTP 403), Blockworks sandwich article (403), a dedicated Umbra "sandwich attacks" page (404). The Jito *internal* sandwich attribution numbers are reported second-hand via Helius and are not independently checkable.

---

## 1. How MEV works on Solana vs Ethereum

### 1.1 The core structural difference: no public mempool — **High**
- On Ethereum, pending txs sit in a **distributed p2p mempool**; builders/searchers observe them and bid for ordering in discrete ~12s block auctions (mev-boost). (Umbra, Helius Intro)
- On Solana there is **no public mempool**. Pending transactions are **forwarded directly to the current leader and the next few leaders** (via Gulf Stream–style forwarding). You cannot "watch the mempool" to front-run — there is no shared queue of pending txs to observe. (Umbra *Lifecycle*; Helius Intro) — **High**
- Consequence: classic Ethereum-style **front-running on unconfirmed public txs is structurally much harder**. The surface area for front-running is reduced by design. (Helius Intro) — **High**

### 1.2 Continuous block-building model — **High**
- Solana's default (Agave) client uses **continuous block production**: transactions continuously stream into the validator → execution → block production → propagation. There is no discrete "hold then build" window like Ethereum's 12s slots. (Umbra *Lifecycle*) — **High**
- **Key implication for a bot:** priority fees do **not guarantee position** within a block. Because building is continuous and not a clean auction, **latency matters more than on Ethereum** for competitive trades. (Umbra) — **High**

### 1.3 The scheduler (how ordering actually works) — **High**
- The leader validates signatures, then schedules txs for execution. The native scheduler is **NOT first-come-first-serve**. (Umbra) — **High**
- **Agave v1.18 (May 2024)** introduced a new **central scheduler** building a dependency graph ("prio-graph") to prioritize higher-fee txs deterministically across threads. (Helius MEV Report) — **High**
  - Effect: reduced the old stochastic "jitter" that previously rewarded **spamming** the leader with duplicate txs. More deterministic = somewhat less raw-spam-driven, more fee/latency driven. — **High**
- **Local fee markets:** priority fee = `compute_units_requested × compute_unit_price (micro-lamports)`. Fees are assessed against **hot accounts** (write-locked accounts), so contention is localized to the accounts your tx touches. (Helius Priority Fees) — **High**
- Base fee: **0.000005 SOL per signature** (~1 signature/tx typical). Priority fee is optional and additive. (Umbra) — **High**

### 1.4 How transactions reach validators & how state is read — **High**
- **Writing state (landing a tx):** send to current/upcoming leader's TPU (QUIC), often via RPC, or via Jito's transaction send endpoint. **Stake-weighted QoS** means high-stake connections get prioritized ingress — another reason searchers colocate with / buy access from large validators. (Helius Intro; Umbra) — **High**
- **Reading state (seeing opportunities):** state propagates via **Turbine** in **shreds**, first to a **stake-weighted shuffle** of validators. To see new state fastest, you want to **run your own node and/or colocate with high-stake validators**, or consume a low-latency shred feed. (Umbra; Helius Intro) — **High**
- **Jito ShredStream:** delivers the **lowest-latency shreds** from leaders to subscribers, giving a head-start on seeing block state vs waiting for normal Turbine propagation. Directly relevant to reducing "time-to-see-opportunity." (Jito ShredStream docs) — **High**

### 1.5 Jito's role: block engine, bundles, tips, relayer — **High**
- **Adoption:** Jito-Solana became the **de facto default MEV client**; reported at **>97% of the network** running Jito's validator client around the 2025 BAM launch. (web search / Blockworks via summary) — **Medium** (high-level true; exact % moves)
- **Relayer:** routes incoming txs, does limited TPU work (dedup, sigverify), forwards to **both block engine and validator**. Open-source; anyone can run one. (Helius Intro) — **High**
- **Block engine + bundles:** searchers submit **bundles** — sequences of txs executed **atomically and in order, all-or-nothing** — with a **tip** (in SOL) to win an **off-chain auction**. Only the winning bundle is posted on-chain → reduces spam. (Helius MEV Report) — **High**
- **The 200ms delay:** Jito's relayer historically held incoming txs **~200ms** before forwarding to the leader, creating the auction window. (Helius MEV Report) — **High**
- **Tips & fees:**
  - Jito takes a **5% fee on tips** (per Helius MEV Report). — **High** (note: BAM-era DAO governance changed fee *destination* — see §1.6)
  - **Minimum tip: 1,000 lamports for bundles** (current Jito docs). *Note:* the Helius MEV Report cites a 10,000-lamport historical minimum — treat the **1,000-lamport figure (Jito docs) as current**. — **High / discrepancy noted**
  - During high demand the minimum is insufficient; you must set **both a priority fee and a Jito tip**. (Jito docs) — **High**
  - **Tips are paid to tip accounts**; tip is what you bid in the auction. — **High**
- **Revert protection:** `bundleOnly=true` sends the tx as a single-tx bundle; `sendTransaction` proxy always sets `skip_preflight=true` (no RPC simulation). (Jito docs) — **High**
- **Anti-sandwich primitive — `jitodontfront`:** include any pubkey starting with `jitodontfront...` in an instruction; the block engine **rejects any bundle** containing that tx **unless that tx is first (index 0)**. Lets a program/tx forbid being front-run inside a bundle. (Jito docs) — **High**

### 1.6 What changed recently — Jito mempool removal & BAM
- **March 2024 — Jito suspended its public mempool feature.** This had given searchers a 200ms preview of incoming txs and was heavily used for **sandwich attacks**. Suspension sacrificed significant revenue and **immediately reduced harmful MEV**, but pushed activity toward **private/permissioned alternative mempools** (e.g., DeezNode) that lack transparency. (Helius MEV Report) — **High**
- **BAM — Block Assembly Marketplace (2025):** Jito's next-gen block-building architecture.
  - Announced **July 21, 2025**; **early mainnet Sept 25, 2025**. (CoinDesk; PRNewswire; Blockworks) — **High**
  - Off-chain layer of **block builders running in Trusted Execution Environments (TEEs)** that encrypt/simulate bundles → transparent, verifiable sequencing; aims to **mitigate harmful MEV**. Draws on Flashbots' BuilderNet design. No base-layer protocol change. (CoinDesk; Helius BAM blog) — **High**
  - **Plugin framework / "application-controlled execution" (ACE):** apps (CLOBs, derivatives venues like Drift) can define custom ordering logic. (Blockworks) — **Medium**
  - **Economics:** BAM validator client **open-sourced**; JTO holders voted to redirect Jito Labs' share of engine + BAM fees to the DAO; Jito estimated **~$15M/yr** additional revenue. (Blockworks; CoinDesk) — **Medium** (estimate)
  - Initial alpha validators: Triton One, SOL Strategies, Figment, Helius. — **Medium**
  - **Sources:** [CoinDesk](https://www.coindesk.com/tech/2025/07/21/jito-launches-bam-to-reshape-solanas-blockspace-economy), [Helius BAM](https://www.helius.dev/blog/block-assembly-marketplace-bam), [Blockworks](https://blockworks.co/news/jito-bam-solana-mainnet), [PRNewswire](https://www.prnewswire.com/news-releases/bam-launches-to-redefine-block-building-on-solana-302508999.html), [Jito Sept roundup](https://www.jito.network/blog/september-monthly-roundup-2025/)
- **Competition:** Raiku announced a **$15M seed** as a competing block-building/infra challenger (Sept 2025). — **Medium**

### 1.7 Firedancer implications — **Medium**
- **Firedancer** (Jump Crypto) is an independent, high-performance Solana validator client built from the ground up for throughput. **Frankendancer** = hybrid (Firedancer networking/components + Agave runtime). (Jump; Solana Compass) — **High**
- **Stake trajectory (figures fluctuate daily — treat as Medium):**
  - June 2025 ~8% → Oct 2025 **~21%** (≈207 validators on Frankendancer) → early 2026 **~26%** (≈165 validators). Agave/Jito client remained dominant (~72% in Oct 2025). Full standalone Firedancer reached mainnet **Dec 2025** but held **<1%** of stake. (Solana Compass; BlockEden; CryptoSlate) — **Medium**
  - Adoption intentionally throttled to limit ecosystem risk while gathering production data. — **Medium**
- **MEV implications:**
  - **Higher throughput / lower latency** raises the bar — competition compresses, **latency edge matters even more**, and the gap between colocated/optimized searchers and everyone else widens. — **Low/Medium (inference)**
  - Early Frankendancer **did not yet integrate Jito's bundle auction**, so those validators were "leaving MEV on the table." As Firedancer matures and integrates (or as BAM becomes the assembly layer), the **block-building stack is in flux** — a real risk for any infra assumption you bake in now. — **Medium**
  - A multi-client world (Agave/Jito + Firedancer + BAM builders) means **no single, stable transaction-ordering model** — your bot should not hard-assume one client's scheduling behavior. — **Medium (inference)**
  - **Sources:** [Solana Compass / Firedancer](https://solanacompass.com/projects/firedancer), [BlockEden deep-dive](https://blockeden.xyz/forum/t/firedancer-at-21-stake-on-solana-mainnet-a-technical-deep-dive-into-the-architecture-that-could-reshape-validator-infrastructure/619), [CryptoSlate](https://cryptoslate.com/firedancer-is-live-but-solana-is-violating-the-one-safety-rule-ethereum-treats-as-non-negotiable/)

---

## 2. MEV strategy types — edge, data, latency, competition

> Profit figures below are **point-in-time, from the Helius MEV Report (2025) / Umbra (2023)** and are **illustrative estimates**, not current market rates.

### 2.1 Atomic on-chain arbitrage (DEX–DEX) — **dominant form** — **High**
- **What:** exploit price discrepancies for the same pair across venues, **both legs in one atomic Solana tx** (no inventory risk; if it doesn't profit, it reverts). Classic pattern: pick off a **stale AMM quote** (xy=k constant-product) and offset on an **on-chain order book** whose market makers already moved. (Umbra; Helius MEV Report) — **High**
- **Reported scale (point-in-time estimate):** Jito's arb detection identified **~90.4M successful arbitrage txs over a year**, **avg profit ~$1.58 each**, **~$142.8M total**, single best **~$3.7M**. → *most arbs are tiny; a fat tail carries the value.* — **Medium (single-source, point-in-time)**
- **Edge needed:** fast state reads, fast/atomic landing, good route-finding across many pools, tight execution costs. — **High**
- **Data:** real-time pool/orderbook state across all major DEXs; ideally a **shred feed (ShredStream)** + own node. — **High**
- **Latency tolerance:** **very low** — directly latency-competitive. — **High**
- **Competition:** **very high / saturated**; thin per-trade margins → wins decided by latency + cost. — **High**
- **Risk:** paying fees on **reverted/failed** attempts that still land on-chain. — **High**

### 2.2 CEX–DEX arbitrage — **High concept, harder to verify edge**
- **What:** off-chain (CEX) price moves first; the on-chain AMM is stale → trade on-chain and hedge on CEX. **Not atomic** → carries **inventory + execution/trust risk** across venues. (Helius MEV Report) — **High**
- **Edge needed:** CEX connectivity + inventory on both sides, hedging infra, and a latency edge to act on CEX moves before the on-chain quote corrects. This is closer to a **market-making/HFT operation** than pure on-chain searching. — **Medium (inference)**
- **Competition:** high and **capital/infra-intensive**; this is where colocation near CEXs (Binance/Coinbase) is theorized to matter. (Umbra) — **Medium**
- **Latency tolerance:** very low. **Note for paper trading:** hard to simulate honestly because it depends on off-chain fills you don't control.

### 2.3 Sandwich attacks — **feasible but contested; mostly via private mempools** — **High**
- **Feasibility now:** Since there's no public mempool and Jito removed its public mempool (Mar 2024), sandwiching mainly persists via **private/permissioned mempools** (notably **DeezNode**). (Helius MEV Report) — **High**
- **Reported scale (point-in-time estimate):** the **Vpe** program (DeezNode) reportedly did **~1.55M sandwich txs in 30 days (Dec 7–Jan 5)**, ~51,600/day, **88.9% success**, **~65,880 SOL (~$13.43M)** profit; Jito *internal* analysis attributed ~half of all Solana sandwiches to this one program. (Helius MEV Report, citing Jito internal + Flipside) — **Medium (second-hand; not independently verifiable)**
- **Targets:** **memecoin traders** with high slippage tolerance on illiquid pairs; relatively insensitive to being front-run. — **High**
- **Ethics / project stance:** widely regarded as **harmful/extractive**; Jito explicitly de-platformed it and offers **anti-sandwich tooling** (`jitodontfront`). **Recommendation for this project: exclude sandwiching from the bot's strategy set** — it requires running/buying into an opaque private mempool, is reputationally toxic, and is being actively designed out (BAM TEEs). — **High (recommendation)**
- **Latency/competition:** requires privileged orderflow access; not a level playing field.

### 2.4 JIT (just-in-time) liquidity — **Medium**
- **What:** an LP adds **concentrated liquidity right before** a large swap to capture its fees, then withdraws — effectively front-running passive LPs for fee share. Most relevant on **concentrated-liquidity / DLMM** venues (Orca Whirlpools, Meteora DLMM). — **Medium (mechanics general; Solana specifics thinner in sources)**
- **Related ecosystem direction:** DFlow's **Conditional Liquidity** + **Segmenters** and **RFQ** systems are explicitly about expressing **JIT preferences / order-flow segmentation** on Solana. (Helius MEV Report — Conditional Liquidity, https://pond.dflow.net/blog/2024-12-19-intro-cl) — **Medium**
- **Edge:** detect large incoming swaps early; precise CLMM/DLMM position math; fast add/remove.
- **Latency tolerance:** low. **Competition:** moderate, more specialized than vanilla arb.
- **Could NOT verify:** hard, current Solana-specific JIT profitability numbers. **Label as under-documented.**

### 2.5 Liquidations (lending protocols) — **"good MEV"** — **High**
- **What:** when a borrower's position falls below required collateralization (per **oracle** price), liquidators **permissionlessly** repay debt and receive collateral **at a discount**. Keeps protocols solvent. (Umbra; Helius MEV Report) — **High**
- **Protocols:** **Kamino** (reported as Solana's largest lending protocol by liquidity/users in the report era), **MarginFi**, **Solend**, plus others (Drift for perps). — **High** (Kamino's "largest" status is point-in-time — **Medium**)
- **Example (point-in-time):** a Kamino liquidation netting only **~$0.049** after fees — *margins on small positions are razor-thin.* — **Medium**
- **Edge needed:** **oracle-aware monitoring** of at-risk positions, fast reaction to price updates, often **flash-loan** capital to size up without holding inventory, atomic execution. — **High (mechanics) / Medium (flash-loan specifics)**
- **Data:** full position/collateral state per protocol + oracle feeds (Pyth/Switchboard). — **High**
- **Latency tolerance:** **low at the moment of an oracle update** (everyone races the same liquidatable position); otherwise event-driven. — **Medium**
- **Competition:** high on large/profitable positions; bots cluster on big liquidations. — **Medium**
- **Why attractive for a paper bot:** deterministic trigger (health factor crosses threshold), simulatable from on-chain state + oracle, ethically clean. **Good candidate for an initial strategy.** — **recommendation**

### 2.6 Backrunning / new-pool sniping — **High (concept)**
- **Backrunning:** place a tx **right after** a large/poorly-routed swap to **re-equalize prices** across pools and capture the imbalance. On Solana this is typically done **inside a Jito bundle** (your backrun bundled after the target tx, with a tip). (Helius MEV Report) — **High**
  - **Famous example (point-in-time):** Jan 10, 2024 WIF — a searcher backran an $8.9M Jupiter buy that wicked WIF to $3, via a Jito bundle with an **890.42 SOL (~$91.6k) tip**, netting **~17,442 SOL (~$1.79M)** in one tx. Illustrates the **fat-tail** payoff and that **tips scale with opportunity size**. — **Medium (single example)**
- **New-pool sniping:** detect new pool/launch (e.g., **pump.fun / PumpSwap**, new Raydium pools) and trade the initial inefficiency. Extremely competitive, bot-dominated, high revert/failure rate. — **Low/Medium (under-documented in primary sources I gathered)**
- **Edge:** earliest possible detection of the triggering event (shred feed), bundle construction, tip sizing.
- **Latency tolerance:** very low. **Competition:** very high on attractive launches.

### 2.7 Strategy comparison (synthesized — treat qualitative ratings as Medium)

| Strategy | Profit potential | Frequency | Latency need | Competition | Capital | Ethics | Paper-trade-ability |
|---|---|---|---|---|---|---|---|
| Atomic DEX–DEX arb | Low/trade, fat tail | Very high | Very low | Very high | Low (atomic) | Neutral/good | Good (on-chain state) |
| CEX–DEX arb | Med–High | High | Very low | High | High (inventory) | Neutral | Hard (off-chain legs) |
| Sandwich | Med–High | High | Low | Gated (private MP) | Med | **Harmful — exclude** | N/A (recommend skip) |
| JIT liquidity | Med | Med | Low | Moderate | Med | Debated | Medium |
| Liquidations | Low–High (event) | Event-driven | Low (at trigger) | High on big ones | Med (flash loans help) | Good | **Good** |
| Backrun / re-equalize | Low, fat tail | High | Very low | Very high | Low | Neutral | Medium |
| New-pool sniping | High variance | High | Very low | Very high | Low–Med | Gray | Hard |

---

## 3. DEXs / AMMs where MEV happens

- **Raydium** — AMM **v4** (constant product), **CLMM** (concentrated liquidity), **CPMM**. Major venue; appears as a leg in large arbs/backruns (the WIF backrun used Raydium CLMM + V4). High arb relevance. (Helius MEV Report) — **High**
- **Orca Whirlpools** — concentrated-liquidity AMM; the canonical "stale AMM" leg in Umbra/Helius arb examples. Core arb + JIT venue. (Umbra; Orca dev docs) — **High**
- **Meteora** — **DLMM** (discretized bins) + **dynamic pools**; prominent for memecoin liquidity and JIT-style strategies. — **Medium**
- **Phoenix** — on-chain **central limit order book**; the canonical "fresh quote" leg (market makers move quotes to off-chain price → arb vs stale AMM). High arb relevance. (Umbra; Helius MEV Report) — **High**
- **OpenBook v2** — on-chain CLOB (Serum successor); order-book leg for arb. — **Medium**
- **Lifinity** — proactive market maker (oracle-based pricing, low/concentrated liquidity); designed to *reduce* its own adverse selection — relevant as a venue whose quotes behave differently from xy=k. — **Medium**
- **pump.fun / PumpSwap** — memecoin launch + AMM; epicenter of **new-pool sniping** and (historically) **sandwich** targeting due to high-slippage memecoin flow. — **Medium**

**Which matter most for arbitrage:** the highest-value atomic arbs pair a **stale constant-product AMM** (Raydium v4/CPMM, Orca) against a **fresh on-chain order book** (Phoenix, OpenBook v2), plus **CLMM/DLMM** pools (Raydium CLMM, Orca Whirlpools, Meteora DLMM) for routing depth. **Jupiter** (aggregator) is not a venue but **shapes flow** — large Jupiter swaps create the imbalances backrunners target, and **JupiterZ RFQ** (default since ~Dec 2024) routes some flow off-chain to market makers, **removing MEV from those trades** and reducing on-chain composability. (Helius MEV Report) — **High**

---

## 4. Profitability expectations & dominant players

### 4.1 Realistic expectations — **Medium**
- **Per-trade margins on vanilla atomic arb are tiny** (avg ~$1.58 in the Jito-detected dataset). The business is **high-volume, low-margin** with a **fat tail** of rare large wins (backruns of huge swaps, liquidations of big positions). — **Medium (point-in-time)**
- **You will not win the median competitive arb without a latency edge.** Most opportunities are claimed by the fastest, lowest-cost participant. — **High (inference from continuous-build + scheduler dynamics)**
- **Failed-tx cost is a real, recurring drag.** Reverted attempts that still land cost base + priority fees. Cost discipline (skip-preflight risk, CU budgeting) directly affects net P&L. — **High**

### 4.2 What separates winners from losers — **High**
1. **Latency to read state** — own node + colocation with high-stake validators + low-latency shred feed (**ShredStream**). The sooner you *see* the opportunity, the more likely you win it. (Umbra; Jito) — **High**
2. **Latency to land** — proximity to leaders, stake-weighted QoS, Jito bundles for atomic inclusion + tips. (Umbra; Helius) — **High**
3. **Colocation / verticalization** — searchers colocate in the same data centers as large validators (a centralizing force). With Firedancer raising throughput, this edge intensifies. (Umbra) — **High**
4. **Signal quality** — route-finding across many pools, accurate stale-quote detection, oracle-aware liquidation monitoring. — **Medium**
5. **Capital** — matters most for CEX–DEX (inventory) and large liquidations; **atomic arb needs little capital** (that's its appeal). Flash loans substitute for capital in liquidations. — **Medium**
6. **Cost control & tip strategy** — sizing Jito tips to the opportunity (the WIF backrunner paid an ~$92k tip on a ~$1.9M win), and minimizing failed-tx spend. — **High**

### 4.3 Dominant players — **Medium**
- **Infra layer dominated by Jito** (validator client reported >97% around BAM launch; block engine/bundles are the standard rails). — **Medium**
- **Sandwich flow concentrated in DeezNode/Vpe** (private mempool) — reportedly ~half of all sandwiches. — **Medium (second-hand)**
- **Searchers are largely anonymous/private**; the report frames a verticalized, colocated, latency-optimized field. **Ecosystem investors** (e.g., Multicoin) expect MEV value-capture to grow and **shift toward apps/protocols** (RFQ, conditional liquidity, BAM ACE). — **Medium**

---

## 5. Key risks & failure modes for searchers

1. **Reverted/failed transactions** — the primary direct cost of atomic strategies; you pay fees even when you lose the race. — **High**
2. **Latency loss** — being out-competed on the same opportunity; structural, not fixable by paying more priority fee alone (continuous build). — **High**
3. **Tip mispricing** — overbidding erodes the edge; underbidding loses the auction. — **High**
4. **Infra dependence & change risk** — the block-building stack is **actively shifting** (mempool removal 2024 → BAM TEEs 2025 → Firedancer/full client 2025–26 → competitors like Raiku). Assumptions about ordering, the 200ms window, or bundle behavior can break. — **High**
5. **Private-mempool gatekeeping** — some profitable flow (and sandwiching) is only accessible via permissioned mempools you may not (and arguably should not) access. — **High**
6. **Oracle/price-feed risk (liquidations)** — stale or manipulated oracle inputs, race conditions on the same liquidatable position. — **Medium**
7. **Smart-contract / integration risk** — DEX/lending program upgrades, account layout changes, CU limits; a wrong assumption silently reverts or mis-executes. — **Medium**
8. **Regulatory / reputational risk** — sandwiching especially; ecosystem actively de-platforms it. — **Medium**
9. **Centralization arms race** — colocation/stake requirements mean a well-capitalized incumbent can structurally out-position you. — **High (inference)**
10. **Skip-preflight blind spots** — Jito send sets `skip_preflight=true`; no RPC simulation safety net, so malformed/unprofitable txs aren't caught before landing. — **High**

---

## 6. Recommendations for a paper-trading MEV bot

- **Start with strategies that are simulatable from on-chain state and ethically clean:** **(a) liquidations** (deterministic trigger via oracle + position health) and **(b) atomic DEX–DEX arbitrage** (stale AMM vs fresh CLOB; fully reconstructable from pool/orderbook state). — **recommendation**
- **Model the things that decide real P&L**, even in paper mode: latency-to-see (assume you are *not* the fastest), failed-tx cost, Jito tip cost (incl. 5% fee), and the local fee market on the accounts you touch. A paper bot that ignores latency/revert cost will wildly overstate profit. — **High (inference)**
- **Do NOT build sandwiching.** Toxic, gated behind private mempools, and being engineered out (BAM TEEs, `jitodontfront`). — **recommendation**
- **Treat CEX–DEX and new-pool sniping as "advanced / later"** — they depend on off-chain fills or extreme latency you can't honestly paper-simulate at first. — **recommendation**
- **Design for infra change** — abstract the "submit" and "read-state" layers (RPC vs ShredStream vs BAM) so the Firedancer/BAM transition doesn't force a rewrite. — **recommendation**
- **Verify all numbers fresh before relying on them.** Every dollar/percentage in this doc is point-in-time and should be re-pulled from on-chain data / current dashboards (e.g., Jito explorer https://explorer.jito.wtf/) before being used in planning math. — **High**

---

## 7. Explicit "could not verify" list

- Current (mid-2026) **per-strategy profitability** — all figures here are 2023–2025 point-in-time estimates. **Not current.**
- **Jito internal** sandwich attribution (~half from Vpe) — second-hand via Helius; not independently checkable.
- Exact **current Jito client / Firedancer stake split** — fluctuates daily; sources disagree by a few points.
- **Current Jito tip fee routing** post-BAM DAO vote — governance changed fee destination; exact current split not re-verified here.
- Solana-specific **JIT liquidity profitability** and **new-pool sniping** economics — under-documented in the primary sources gathered.
- Whether the **200ms relayer delay** still applies unchanged under BAM — **not verified for current state.**
