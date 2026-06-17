# Designing an Accurate Solana MEV / Arbitrage Paper-Trading & Backtesting Engine

**Research findings report — prepared 2026-06-17**
**Scope:** How to build a simulator that realistically estimates whether a detected Solana MEV/arbitrage opportunity *would have been* profitable, without executing real trades.

**Confidence legend:** 🟢 High (multiple corroborating primary sources) · 🟡 Medium (single credible source or reasoned inference) · 🔴 Low / opinion / unverified.

> **Honest framing up front:** The single most important empirical fact in this entire document is that **~96% of attempted atomic arbitrages on Solana fail** (Umbra Research, 🟢). Any paper-trading model that does not reproduce a comparable failure/loss rate is *optimistic by construction* and will lie to you. Accuracy here means accurately modeling *losing*, not just spotting the gross opportunity.

---

## 0. Key quantified facts grounding the model

| Fact | Value | Source | Conf |
|---|---|---|---|
| Share of attempted atomic arbitrages that fail | ~96% | Umbra, *MEV on Solana* | 🟢 |
| Avg. profit per *successful* arbitrage (2024) | $1.58 | Jito data via Helius MEV Report | 🟢 |
| Successful arb txns (1 yr) / total profit | 90.4M txns / $142.8M | Jito via Helius | 🟢 |
| Cost of a failed tx landing on-chain | 0.000005 SOL (base sig fee) | Umbra | 🟢 |
| Jito relayer "speed bump" / auction window | 200 ms | Umbra; Helius | 🟢 |
| Jito parallel auction tick cadence | 50 ms | Jito Docs | 🟢 |
| Jito minimum tip | 10,000 lamports | Jito Docs; Helius | 🟢 |
| Jito fee on tips | 5% | Helius MEV Report | 🟢 |
| Typical atomic-arb tip as % of MEV available | 20%–50% (rising) | Umbra | 🟡 |
| Priority fee that is **burned** (not to validator) | 50% | Umbra | 🟢 |
| Solana target slot time | ~400 ms | Umbra (continuous production) | 🟢 |

These numbers are the calibration targets your simulator's *aggregate* output must roughly match before you trust its *per-opportunity* output.

---

## 1. Backtesting vs. Forward Paper Trading vs. Shadow/Replay Trading

### Definitions

- **Backtesting** — Run the strategy against *historical* data, simulating past conditions to estimate how it would have performed. It is a special case of cross-validation applied to past time periods (Wikipedia, *Backtesting*, 🟢). Limitations explicitly noted in the literature: it needs detailed historical data, **cannot model strategies that would themselves have affected historic prices**, and is prone to overfitting — "it is often possible to find a strategy that would have worked well in the past, but will not work well in the future" (Wikipedia, 🟢).

- **Forward paper trading** — Run the strategy live against *current* market state in real time, logging the trades it *would* have placed and scoring them after the fact, but never submitting them. Avoids look-ahead bias by construction (you only see what was knowable at decision time), but is slow to accumulate sample and still suffers the "your order never hit the book" problem.

- **Shadow / replay trading** — A hybrid: replay a recorded stream of on-chain state (account updates, slots, shreds) through the *exact same* detection+decision code that production would run, OR run the live detector in "shadow mode" alongside production with execution disabled. The distinguishing feature is that the simulator consumes the *same inputs at the same point in the pipeline* as the real bot, so detection latency and state-staleness are reproduced faithfully.

### Tradeoffs

| Approach | Look-ahead risk | Reproduces latency? | Sample velocity | Cost | Models competition? |
|---|---|---|---|---|---|
| Backtest (historical) | **High** (easy to cheat) | Only if explicitly modeled | Fast (replay months in hours) | Low | Only via assumptions |
| Forward paper | Low | Yes (real-time) | Slow (wall-clock bound) | Low | Partially (you see who won) |
| Shadow/replay | Low–Med | **Yes, faithfully** | Fast (replay recorded streams) | Med–High (must record state) | Yes, if you record what landed |

### Recommendation for MEV (🟡 reasoned, grounded in 🟢 facts)

**For Solana MEV, shadow/replay trading is the appropriate primary method, with forward paper trading as the validation gate and historical backtesting used only for coarse strategy screening.**

Rationale, tied to Solana mechanics:
- Opportunities are *fleeting and latency-bound*. Umbra: "Latency is more important for searchers on Solana because state updates are so frequent" and "the sooner a competitive trade is identified and sent to the network, the more likely it is to succeed" (🟢). A naive historical backtest that evaluates an opportunity at the price it *first appeared* implicitly assumes zero detection-to-decision latency and zero competition — both fatal.
- Solana has **no public mempool**; pending txns are forwarded directly to the current and next few leaders (Umbra, 🟢). So you cannot backtest against a recorded mempool the way Ethereum researchers do — your "what was knowable" set is the *account-state stream you were actually subscribed to*. This makes faithful **state-stream replay** the only way to reproduce the information set the bot really had.
- Because state is propagated via Turbine in stake-weighted order, *which* validator/RPC you're connected to changes what you see and when (Umbra, 🟢). The simulator must therefore replay *your* vantage point's stream, not an idealized global one.

---

## 2. Major Sources of Simulation Inaccuracy (and how to model them honestly)

### 2.1 Slippage & price impact 🟢
Atomic arbitrage on Solana typically exploits a **stale quote on a constant-product (xy=k) AMM** offset against a venue (often an order book like Phoenix) whose quote already moved (Helius MEV Report; Umbra, 🟢). The worked example in the literature: buy 2.11513 SOL for 45 USDC on Orca, sell 2.115 SOL for 45.0045 USDC on Phoenix → ~0.00013 SOL (~$0.026) profit (Helius, 🟢). Margins are *razor-thin*; mid-price assumptions destroy accuracy.

**Honest modeling:**
- **Never use mid-price.** Fill each leg against the *actual pool reserves / curve at the simulated slot*. For xy=k, the output for input `dx` into reserves `(x, y)` with fee `f` is `dy = y · (1 − x / (x + (1−f)·dx))`. Use the *real* fee tier and the *real* reserves recorded at that slot, not a snapshot from minutes earlier.
- For concentrated-liquidity / CLMM pools (Orca Whirlpools, Raydium CLMM) you must walk the active tick liquidity, not assume a single curve.
- For order-book legs (Phoenix, OpenBook), fill against the *recorded book depth*, consuming levels — a large size eats through levels and the marginal price worsens.
- Model the **two-leg interaction**: your own first leg moves the pool, changing the price available for the second leg. Simulate sequentially against mutated state.
- The cleanest way to get all of this right at once is **`simulateTransaction`** (see §3.2), which runs the *actual* swap instructions against real account state and returns real token-balance deltas — capturing curve, ticks, fees, and rounding exactly.

### 2.2 Latency: detection-to-execution & "would it still be there?" 🟢
This is the dominant error source for MEV specifically.

Solana facts that constrain the model:
- **Continuous block production**: txns stream continuously into the leader; "priority fees do not guarantee position within a block… latency is more important for competitive trades" (Umbra, 🟢).
- **No mempool**: you must send to the *current and next few* leaders (Umbra, 🟢).
- **Some atomic opportunities persist for multiple blocks before capture** (Umbra, 🟢) — so latency tolerance is opportunity-specific, not uniform.

**Honest modeling — introduce an explicit latency budget and re-check viability at the *projected landing slot*, not the detection slot:**
1. Stamp every detected opportunity with the slot/time it was *first observable in your stream*.
2. Add a modeled `Δt = t_detect + t_decide + t_build + t_network + t_auction`. Anchor `t_auction` to the **200 ms Jito relayer speed bump** if routing through Jito, and remember the **50 ms parallel auction ticks** (Jito Docs, 🟢).
3. **Re-evaluate the opportunity against the state at the *projected* landing slot** (advance the replayed state by `Δt`). If the stale quote has already been corrected (by you-can't-see-it competition or organic flow), the opportunity is **gone** — score it as not-taken or as a *loss* if you'd have still sent and reverted.
4. Treat `Δt` as a *distribution*, not a constant — network and scheduler jitter are real (Umbra notes jitter can cause a later copy of a tx to land before an earlier one, 🟢). Run the sim across a latency distribution and report the profit distribution.

### 2.3 Competition / adverse selection / winner's curse 🟢 (concept) / 🟡 (parameterization)
This is the subtlest and most-cheated-on dimension. Foundational framing: arbitrage bots "like high-frequency traders on Wall Street… optimize network latency to frontrun" (Flash Boys 2.0, Daian et al., arXiv:1904.05234, 2019, 🟢). On Solana, "the trades described above are profitable, so MEV searchers compete to win them" (Umbra, 🟢).

The **winner's curse** in MEV: the opportunities you *win* are disproportionately the ones *no faster searcher wanted* — i.e., the marginal, lower-EV, or already-decaying ones. A backtest that credits you with *every* opportunity you detected systematically overstates PnL, because in reality faster competitors skim the best ones. (🟡 — well-established as a concept; the exact haircut is strategy- and pair-specific.)

**Honest modeling:**
- **Do not assume you win every detected opportunity.** Assign a **fill/win probability** `p_win` that *decreases with opportunity richness* (the fatter the spread, the more competitors chase it, the lower your odds unless you outbid them). This deliberately inverts the naive optimism.
- Calibrate `p_win` against ground truth: for opportunities that *did* get captured on-chain by *someone*, check whether your bot would have detected them in time and what tip would have been needed to win (see §5).
- Model **adverse selection on the venue side too**: DEXs/segmenters increasingly price order-flow toxicity; sophisticated takers face widening effective spreads (Helius MEV Report on conditional liquidity / segmenters, 🟢). Your effective fills may be worse precisely because you are toxic flow.
- Conservative default: treat detected gross edge as an *upper bound* and apply a competition haircut; never let the simulator's expected win rate exceed what on-chain data says is achievable.

### 2.4 Transaction landing probability & failed-tx costs 🟢
- **~96% of attempted atomic arbitrages fail** (Umbra, 🟢). The failures are cheap (0.000005 SOL each via the spam path) but they are *not free*, and they are *frequent*. A simulator that only books the 4% of winners is fraudulent.
- **Optimistic MEV**: searchers send txns *assuming* state updated and revert if it hasn't, because failure is cheap (Umbra, 🟢). This is a deliberate spam-and-pray pattern. If your strategy is optimistic, model the full stream of attempts and their fees, not just the lands.
- **Two landing regimes to model separately:**
  - **Priority-fee / spam path:** `priorityFee = computeBudget × computeUnitPrice` (micro-lamports) (Helius Priority Fees, 🟢). Failed txns *land on-chain and still cost the base 0.000005 SOL signature fee* (priority fee is generally only charged on execution, but the signature fee is paid regardless once included). Half of priority fees are burned (Umbra, 🟢).
  - **Jito bundle path:** bundles are *all-or-nothing*; **failed bundles do NOT land on-chain** (Umbra; Helius, 🟢), so you pay no tip on a losing bundle — but you also paid the off-chain auction in latency (200 ms) and you only win if your **tip/CU-efficiency** beats competitors (Jito Docs: "Bundle orderings… prioritized… based on requested tip/cus-requested efficiency", 🟢).

**Honest modeling:** make landing probability `p_land` an explicit function of (path, tip, CU price, competition, latency). For the Jito path, `p_land` ≈ probability your tip wins its local-state auction; for the priority-fee path, `p_land` ≈ probability you're scheduled before a competitor given jitter. Charge fees on the correct events: signature fee on every *included* tx (win or revert) on the spam path; tip *only on won bundles* on the Jito path.

### 2.5 Gas / priority fee / Jito tip costs eating margin 🟢
With average successful-arb profit of **$1.58** (Helius/Jito, 🟢) and tips running **20–50% of available MEV** (Umbra, 🟢), fee modeling is not a rounding error — it is often the difference between +EV and −EV.

**Honest modeling — net every opportunity:**
`PnL = gross_edge − tip − priority_fee − base_sig_fee − (DEX swap fees, already in fills) − failed_attempt_costs`
- Tip must be set high enough to plausibly win (calibrate to observed winning tips for similar opportunities), which directly compresses the modeled margin. Do not assume the *minimum* 10,000-lamport tip wins a contested opportunity.
- Account for the **5% Jito fee on tips** (Helius, 🟢).
- Include the *amortized cost of all the failed attempts* that accompany each win, not just the winning tx's fees.

### 2.6 Look-ahead & survivorship bias 🟢 (general) / 🟡 (MEV-specific)
- **Look-ahead bias** — using information not available at decision time. In MEV the classic violations are: (a) evaluating fills at the *settled* post-trade price instead of the pre-trade pool state; (b) using the *final* block contents to decide what you'd have done, when you couldn't have seen the block before it was produced; (c) using a global state view when your node only saw a delayed Turbine-propagated view. **Mitigation: replay only the state stream as your vantage point received it, time-ordered, and freeze the decision input at detection time.** Forward/shadow paper trading is structurally immune; historical backtests must be engineered carefully to avoid it.
- **Survivorship bias** — only studying opportunities/pools/tokens that still exist or that *did* get captured. If you calibrate `p_win` only on arbitrages that *landed*, you ignore all the opportunities that decayed uncaptured or that everyone lost on. Pull the *full* population (all detectable price dislocations), not just the realized-profit subset.

---

## 3. Concrete Techniques to Make the Simulation Accurate

### 3.1 Replay historical on-chain state 🟢
Record and replay the **account-update / slot / (ideally) shred stream** that your production bot consumes (e.g., via Geyser plugin, Yellowstone gRPC, or a self-run validator). Reconstruct pool reserves, tick liquidity, and order books at each slot. Drive your *unmodified* detection code from this replayed stream so that detection latency and state staleness are reproduced. This is the backbone of faithful shadow/replay simulation and the only way to honor the "no mempool / Turbine vantage point" constraints (Umbra, 🟢).

### 3.2 `simulateTransaction` against live/historical state 🟢 — highest-fidelity fill model
Solana's RPC `simulateTransaction` runs the actual transaction against real account state and returns logs, compute units consumed, and (via the `accounts` config) post-execution account data — i.e., the *real* token balances your swap would produce (Solana RPC docs, 🟢). Relevant config fields: `replaceRecentBlockhash` (lets you simulate unsigned/forward txns), `sigVerify`, `minContextSlot`, `innerInstructions`, and `accounts` (Solana docs, 🟢).

Use it two ways:
- **Forward/shadow mode:** build the *exact* arbitrage transaction you would have sent and `simulateTransaction` it against *live* state at the moment of detection. The returned balance deltas are your ground-truth gross fill (curve, ticks, fees, rounding all exact). This is dramatically more accurate than re-deriving AMM math yourself.
- **Replay mode:** simulate against a *pinned* historical slot (`minContextSlot`) to evaluate "would this have filled as I modeled at slot N."

**Caveats (🟡):** `simulateTransaction` reflects state *at the node's current / pinned slot*, not the *contested landing slot after competitors act* — it tells you the fill *if you were alone*. It does **not** model the auction, competition, or whether you'd actually land. Treat it as the **gross-edge oracle**, then layer `p_win`, `p_land`, latency re-check, tips, and failure costs on top.

### 3.3 Model fill / win / landing probability explicitly 🟡
Decompose realized PnL multiplicatively:
`E[PnL] = p_detect_in_time × p_win_auction × p_land × (gross_edge − costs)  −  E[failed_attempt_costs]`
Each probability is a *modeled, calibrated* quantity, not 1.0. Fit them against on-chain ground truth (§5). The default posture should make these *pessimistic*: if you can't justify a high `p_win`, don't assume one.

### 3.4 Conservative vs. optimistic assumption sets 🟡 (best-practice, opinion-informed)
Run the simulator in (at least) two explicit modes and **report both**:
- **Optimistic bound:** you detect instantly, win every auction, land at minimum tip, fill at the pre-trade curve. (This is the naive backtest — useful only as a *ceiling*.)
- **Conservative/realistic:** full latency distribution, competition haircut on `p_win`, calibrated winning tips, fills via `simulateTransaction`, full failed-attempt cost amortization, ~96%-fail reality check.
A strategy is only interesting if it's still +EV under the conservative set. The *gap* between the two bounds is itself a useful risk signal.

### 3.5 Account for the inclusion auction 🟢
Model the path you'll actually use:
- **Jito bundle path:** all-or-nothing; failed bundles don't land; winner chosen by **tip/CU efficiency** in **parallel 50 ms auctions** grouped by **account lock-intersection** (write/write, read/write, write/read collide; read/read run separately) (Jito Docs, 🟢). Practical consequence for the sim: opportunities touching the *same hot account* compete in the *same* local auction — model contention at the account level, not globally. A 200 ms speed-bump latency penalty applies (Umbra, 🟢).
- **Priority-fee path:** continuous, latency-and-jitter dominated, no inclusion guarantee, 50% of priority fee burned (Umbra, 🟢).

---

## 4. Metrics & Evaluation

### 4.1 Per-opportunity and aggregate metrics 🟢/🟡
- **Expected value per opportunity** `E[PnL]` (the §3.3 decomposition) — the core decision metric.
- **Hit rate** — fraction of *attempted* opportunities that are net-profitable after all costs. Sanity-check it against the on-chain reality that ~96% of atomic-arb *attempts* fail (Umbra, 🟢); a hit rate wildly above the achievable range signals a modeling leak.
- **PnL attribution** — decompose realized/simulated PnL into: gross edge, − slippage vs. mid, − tips, − priority fees, − failed-attempt fees, − competition losses. This tells you *where* margin is lost and which assumption your result is most sensitive to.
- **Sharpe ratio** — excess return over risk-free, divided by return standard deviation (Sharpe 1966; Wikipedia, 🟢). Useful as a *risk-adjusted* summary, but for MEV (many tiny, fat-tailed, autocorrelated bets) raw Sharpe is fragile — prefer reporting the full PnL distribution, max drawdown, and tail behavior alongside it.
- **Per-trade economics reality anchor:** average winning arb ≈ $1.58 (🟢). If your sim's average *winner* is much larger, you are likely mis-modeling fills, competition, or survivorship.

### 4.2 Statistical significance, sample size, and overfitting 🟢
The quant-finance literature is unambiguous that backtests are *easy to overfit*: "it is often possible to find a strategy that would have worked well in the past, but will not work well in the future" (Wikipedia, *Backtesting*, 🟢). Standard defenses (Bailey & López de Prado line of work; QuantPedia overview, 🟢/🟡):
- **Deflated Sharpe Ratio (DSR)** — adjusts the observed Sharpe for the *number of strategy variants/trials* you tested and for non-normal (skewed, fat-tailed) returns, giving the probability the true Sharpe exceeds zero. Essential because trying many parameter sets inflates the best one's apparent Sharpe. *(Bailey & López de Prado, "The Deflated Sharpe Ratio", J. Portfolio Management 2014. The full-text PDF at davidhbailey.com/dhbpapers/deflated-sharpe.pdf exists but I could not parse it inline — 🟡 on exact formula, 🟢 on the concept.)*
- **Probability of Backtest Overfitting (PBO)** via **Combinatorial Symmetric Cross-Validation (CSCV)** — splits the trial matrix into many in-sample/out-of-sample combinations and measures how often the in-sample best underperforms out-of-sample. (López de Prado et al.; QuantPedia, 🟡.)
- **Minimum Backtest Length / Minimum Track Record Length** — the more trials you run, the longer the sample needed to avoid a false positive; running N strategy configs requires materially more data than running one. (López de Prado line, 🟡.)
- **Purged / walk-forward cross-validation** — strict separation of in-sample (tuning) and out-of-sample (evaluation) windows, with a *purge/embargo* gap so that information doesn't leak across the boundary. Backtesting is itself "a special type of cross-validation applied to previous time periods" (Wikipedia, 🟢).
- **MEV-specific sample sizing (🟡):** because the *win* rate is ~4%, you need a *large* number of *opportunities* (not trades) before per-strategy EV is statistically distinguishable from noise — back-of-envelope, thousands of detected opportunities minimum to bound a small edge. Treat any conclusion from a few dozen "wins" as anecdotal.

**Overfitting hygiene checklist:** hold out a final untouched out-of-sample period; cap and *log every* parameter trial; report DSR/PBO, not just the best Sharpe; prefer fewer, economically-motivated parameters; re-validate forward (paper) before trusting backtest.

---

## 5. Validating the Paper-Trading Model Against Reality 🟢 (method) / 🟡 (thresholds)

The gold standard for an MEV simulator is **predicted-vs-actual reconciliation against trades that DID land on-chain** — your own *and* competitors'.

1. **Self-reconciliation (when you eventually go live, or in a tiny live canary):** for each tx you actually send, compare the simulator's *predicted* token-balance deltas, fees, and land/fail outcome to the *realized* on-chain result. The simulator is only trustworthy when predicted ≈ actual within tight tolerance. `simulateTransaction` immediately before sending makes this comparison nearly exact for the *fill* component (🟢).
2. **Competitor back-test (no live trading needed):** harvest the population of arbitrages that *others* landed on-chain (Jito's arbitrage explorer / Dune queries cited in the Helius report expose this, 🟢). For each:
   - Did your detector *see* the opportunity, and *when* relative to when it was captured? (Calibrates `p_detect_in_time`.)
   - What tip did the winner pay vs. what your model would have bid? (Calibrates `p_win` and tip model.)
   - Does your fill model reproduce the *winner's actual realized profit* when fed the same pre-trade state? (Calibrates the slippage/curve model.)
3. **Aggregate calibration:** does your simulator, run across a representative window, reproduce the *known macro facts* — ~96% attempt-failure rate, ~$1.58 average winner, tip-as-%-of-MEV in the 20–50% band? If not, find which assumption is off *before* trusting per-opportunity output (🟢 anchors).
4. **Shadow-mode A/B:** run the simulator live in shadow alongside a *small* real execution path; the divergence between shadow-predicted and real PnL is your model error, monitored continuously.

---

## Principles for an Accurate Solana MEV Paper-Trading Simulator

1. **Model losing, not just winning.** The ~96% failure rate (🟢) is the headline. If your aggregate simulated failure/loss rate isn't in that neighborhood, the model is optimistic and wrong.
2. **Replay your vantage point's state stream — never a global, instantaneous, post-settlement view.** No mempool, Turbine-delayed propagation, and continuous block production mean "what you knew" = "what your node saw, when it saw it" (🟢). This kills look-ahead bias structurally.
3. **Re-check viability at the projected landing slot, not the detection slot.** Apply a latency *distribution* (`t_detect+t_decide+t_build+t_network+t_auction`, anchored to the 200 ms Jito window / 50 ms ticks) and advance state before scoring (🟢).
4. **Use `simulateTransaction` as the gross-edge oracle, then haircut everything on top.** It gives exact fills against real state but assumes you're alone and ignores competition, the auction, and landing — those are separate, *pessimistic-by-default* multipliers (🟢/🟡).
5. **Decompose PnL multiplicatively:** `E[PnL] = p_detect × p_win × p_land × (gross − costs) − E[failed_costs]`. Each factor is calibrated, none is 1.0.
6. **Make `p_win` *decrease* with opportunity richness** to encode the winner's curse / adverse selection — invert naive optimism (🟡 concept / 🟢 grounding).
7. **Charge fees on the correct events per path.** Spam path: base sig fee on every *included* tx incl. reverts, 50% priority-fee burn. Jito path: tip *only on won bundles*, plus 5% Jito fee, plus 200 ms latency penalty (🟢).
8. **Model the auction at the account-lock level**, not globally — colliding writes to a hot pool account compete in the same 50 ms local auction (Jito Docs, 🟢).
9. **Always report an optimistic *and* a conservative bound; only act on the conservative one.** The gap is a risk signal (🟡).
10. **Defend against overfitting explicitly:** log every trial, report Deflated Sharpe / PBO, use purged walk-forward CV, hold out a final untouched window, and require thousands of *opportunities* before trusting a small edge (🟢/🟡).
11. **Validate predicted-vs-actual against on-chain ground truth** — competitors' landed arbs for calibration, your own (canary/live) txns for reconciliation. Reproduce the macro anchors ($1.58 avg, ~96% fail, 20–50% tip) before trusting micro output (🟢).

---

## Sources

**Primary / high-confidence:**
- Umbra Research — *MEV on Solana* — https://www.umbraresearch.xyz/writings/mev-on-solana (no-mempool, ~96% fail rate, 200 ms speed bump, optimistic MEV, tip 20–50%, latency primacy). 🟢
- Umbra Research — *Lifecycle of a Solana Transaction* — https://www.umbraresearch.xyz/writings/lifecycle-of-a-solana-transaction (continuous block production, scheduler not FCFS, fee structure). 🟢
- Helius — *Solana MEV Report (2025)* — https://www.helius.dev/blog/solana-mev-report ($1.58 avg arb, 90.4M arbs / $142.8M, Jito 200 ms relayer, 5% fee, 10k lamport min tip, segmenters/adverse selection). 🟢
- Helius — *Solana MEV: An Introduction* — https://www.helius.dev/blog/solana-mev-an-introduction (Jito bundles all-or-nothing, out-of-protocol auction). 🟢
- Helius — *Priority Fees: Understanding Solana's Transaction Fee Mechanics* — https://www.helius.dev/blog/priority-fees-understanding-solanas-transaction-fee-mechanics (`priorityFee = computeBudget × computeUnitPrice`). 🟢
- Jito Labs Docs — *Low Latency Txn Send / Auction* — https://docs.jito.wtf/lowlatencytxnsend/ (parallel 50 ms auctions, account-lock grouping, tip/CU-efficiency ordering, highest-paying-combo selection). 🟢
- Solana RPC Docs — *simulateTransaction* — https://solana.com/docs/rpc/http/simulatetransaction (config: replaceRecentBlockhash, sigVerify, minContextSlot, accounts, innerInstructions). 🟢
- Daian, Goldfeder, Kell, et al. — *Flash Boys 2.0: Frontrunning, Transaction Reordering, and Consensus Instability in DEXes* — arXiv:1904.05234 (2019) — https://arxiv.org/abs/1904.05234 (arb bots as HFT, latency optimization, priority gas auctions). 🟢
- Paradigm (Robinson & Konstantopoulos) — *Ethereum is a Dark Forest* (2020) — https://www.paradigm.xyz/2020/08/ethereum-is-a-dark-forest (generalized frontrunning bots; adversarial environment framing). 🟢
- Wikipedia — *Backtesting* — https://en.wikipedia.org/wiki/Backtesting (definition, cross-validation framing, overfitting/look-ahead limitations). 🟢
- Wikipedia — *Sharpe ratio* — https://en.wikipedia.org/wiki/Sharpe_ratio (definition, Sharpe 1966). 🟢

**Methodology / medium-confidence:**
- Bailey & López de Prado — *The Deflated Sharpe Ratio* (J. Portfolio Management, 2014) — PDF at https://www.davidhbailey.com/dhbpapers/deflated-sharpe.pdf — **could not parse inline (raw PDF); concept high-confidence, exact formula unverified here.** 🟡
- QuantPedia — *How to Deal with Backtest Overfitting* — https://quantpedia.com/how-to-deal-with-backtest-overfitting/ (PBO/CSCV, minimum backtest length overview). 🟡

**Could not verify / blocked during research (flagged for follow-up):**
- SSRN abstracts 2308659 (Bailey et al., *Pseudo-Mathematics & Financial Charlatanism*) and 2326253 (*The Probability of Backtest Overfitting*) returned HTTP 403 — obtain via institutional access or author PDFs. 🔴 unverified inline.
- davidhbailey.com/dhbpapers/pseudo-math.pdf returned 404; locate current URL. 🔴
- jito.network/blog and a chorus.one artificial-latency article returned 403/404 — re-source for latest tip-economics figures. 🔴
- Quantitative `p_win` / tip-to-win curves and exact landing-probability-vs-tip relationships are **strategy- and time-specific and were not found as published constants** — these must be empirically calibrated from your own on-chain data (§5). 🟡/🔴
