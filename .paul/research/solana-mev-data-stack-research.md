# Solana Trading / MEV Bot — Data & Infrastructure Stack Research

**Purpose:** Project-planning reference for a Solana trading/MEV bot that ingests real-time market data and runs paper-trading simulation.
**Research date:** 2026-06-17. **Scope:** 2025–2026 ecosystem state.
**Sourcing rule:** Every claim links to a primary source where possible. All prices are labeled **APPROX/ESTIMATE** and were observed on the dates noted — verify before budgeting. Confidence: **High** (confirmed from primary doc this session), **Medium** (primary doc but detail thin / inferred), **Low** (could not verify from primary source — flagged).

> **Verification caveats up front (read this):**
> - Several vendor doc/pricing pages are JS-rendered SPAs that returned partial content or 404s on direct fetch. Where that happened it is flagged inline as **UNVERIFIED**.
> - DEX swap math (Raydium/Orca/Meteora) is only partially documented in public docs; the authoritative math lives in each project's SDK/program source. Formulas below are the standard, widely-implemented ones but are marked **Medium/Low** confidence where I could not confirm the exact on-chain field layout from a primary doc this session.
> - I did **not** execute any on-chain calls; account-layout offsets must be confirmed against the live program IDL/source before you hard-code them.

---

## 1. Real-Time Data Ingestion

### 1.1 Decision summary

| Method | Typical latency | Cost profile | Reliability | Best for |
|---|---|---|---|---|
| Standard JSON-RPC WebSocket (`accountSubscribe`/`logsSubscribe`) | 10s–100s ms; non-deterministic under load | Low / included in RPC plan | Medium — drops, missed updates under load | Prototyping, non-latency-critical bots |
| Yellowstone gRPC (Geyser / "Dragon's Mouth") | Low ms, near validator-internal | Medium–High (dedicated/streaming GB) | High | Production market-data ingestion, MEV |
| Jito ShredStream | Lowest (shred level, pre-block) | Free (approval required) + your own infra | High (UDP, no delivery guarantee) | Latency-critical MEV / front-running detection |

**Recommendation for this project:** Yellowstone gRPC as the primary feed for account/transaction streaming (paper trading doesn't need ShredStream's pre-confirmation edge yet), with standard WebSocket as a cheap fallback. Add ShredStream later only if you move to live latency-sensitive execution.

### 1.2 Standard JSON-RPC

**HTTP methods**
- `getAccountInfo` — single account snapshot. Confidence: High (standard).
- `getProgramAccounts` (gPA) — returns all accounts owned by a program, with `filters` (`dataSize`, `memcmp`) and `dataSlice` (offset/length) to reduce payload. **This is expensive and heavily rate-limited / often restricted by commercial providers** because it scans the whole program account set. Source: https://solana.com/docs/rpc/http/getprogramaccounts (accessed 2026-06-17). Confidence: High.
- `getRecentPrioritizationFees` — see §4.

**WebSocket subscriptions** (source: https://solana.com/docs/rpc/websocket/accountsubscribe, accessed 2026-06-17; Confidence: High):
- `accountSubscribe` — push on account data change; supports `commitment` and `encoding`.
- `logsSubscribe` — push on transaction logs (filter by mentions of an account/program). Useful to catch swaps but logs are truncated/unreliable for full decoding.
- `programSubscribe`, `slotSubscribe`, `signatureSubscribe` also exist.

**Tradeoffs / gotchas:**
- Commitment levels: `processed` (fastest, may be rolled back), `confirmed` (typical), `finalized` (safest, slowest). Confidence: High.
- WebSockets on shared public/commercial RPC can silently miss updates under load and reconnect lossy — **no replay guarantee**. Treat as best-effort.
- `getProgramAccounts` for hot programs (Token program, large DEXes) is often disabled or rate-limited; providers steer you to gRPC instead.

### 1.3 Yellowstone gRPC / Geyser ("Dragon's Mouth")

Open-source Geyser-plugin-based gRPC streaming interface, maintained by Triton (`rpcpool/yellowstone-grpc`). Source: https://github.com/rpcpool/yellowstone-grpc (accessed 2026-06-17). Confidence: High.

**Subscribe filters** (from the proto / docs):
- `commitment`: `processed` / `confirmed` / `finalized`.
- `accounts` — subscribe to specific accounts or by owner/filters (`dataSize`, `memcmp`).
- `accounts_data_slice` — `{offset, length}` to receive only needed bytes (big bandwidth saver).
- `transactions` / `transactionsStatus` — full or filtered (include/exclude `vote`, `failed`, by `account_include`/`account_exclude`, by `signature`).
- `slots`, `blocks`, `blocksMeta`.
- `ping` — send a subscribe request with `ping: true` to keep the stream alive; server sends a `Ping` every 15s (works around Cloudflare/Fly.io idle disconnects). Confidence: High.

Filter semantics (confirmed via Helius gRPC docs, https://www.helius.dev/docs/grpc, accessed 2026-06-17): multiple filter *types* AND together; values *within* an array OR together. Confidence: High.

**Who offers it:**
- **Triton One** — origin/maintainer of Yellowstone. Confidence: High.
- **Helius** — managed as **LaserStream gRPC** (their branded Yellowstone-compatible service). Source: https://www.helius.dev/docs/grpc + pricing page. Confidence: High.
- **Shyft** — managed Yellowstone gRPC; notable: streams up to **150 slots of lookback** from current head; advertises **no rate limits** on subscribe requests (connection-limited per tier instead); hot programs (e.g. Token program) restricted to **Dedicated Nodes**. Source: https://docs.shyft.to/solana-yellowstone-grpc/grpc-docs (accessed 2026-06-17). Confidence: High.
- **QuickNode** — offers "Metered gRPC data" (Yellowstone) as an add-on. Confidence: Medium (listed on pricing page; metering detail UNVERIFIED).

### 1.4 Jito ShredStream

Distributes **shreds** (the lowest-level block fragments) from Jito's Block Engine to your machines *before* a block is fully assembled/confirmed — the lowest-latency view of the chain. Source: https://docs.jito.wtf/lowlatencytxnfeed/ and https://docs.jito.wtf/lowlatencytxnsend/ (accessed 2026-06-17). Confidence: High.

- Run the **ShredStream Proxy** (`jito-labs/shredstream-proxy`): authenticates to the Block Engine with an **approved keypair** (public key must be allow-listed via Jito Discord), sends a heartbeat, and forwards shreds via **UDP** to your `DEST_IP_PORTS`.
- Config: `BLOCK_ENGINE_URL=https://mainnet.block-engine.jito.wtf`, `DESIRED_REGIONS` (**max 2**), default incoming shred port `20000/udp`. **NAT not supported.** Run one proxy per region where you host RPCs.
- Block Engine regions (Block Engine URL / Shred Receiver IP): Amsterdam, Dublin, Frankfurt, London, New York, Salt Lake City, Singapore, Tokyo. NTP servers per region provided for clock sync. Confidence: High.
- Cost: the service is **free** but gated by approval; you pay for your own colocated infra. Confidence: High (free), Medium (approval process detail).
- Reliability: UDP best-effort, no replay/ordering guarantee; you must dedupe/reassemble. This is an *edge* feed, not a system of record.

### 1.5 Commercial providers — offerings & APPROX pricing

> **All pricing below is APPROX/ESTIMATE, observed 2026-06-17, monthly unless noted. Verify on the vendor page before budgeting.** Credit-cost-per-call varies by method, so "credits" ≠ "requests".

**Helius** — source: https://www.helius.dev/pricing (accessed 2026-06-17). Confidence: High (page rendered fully).

| Plan | APPROX price/mo | Credits/mo | RPC RPS | sendTransaction/s | sendBundle/s | gPA/s | LaserStream gRPC |
|---|---|---|---|---|---|---|---|
| Free | $0 | 1M | 10 | 1 | — | 5 | Devnet only* |
| Developer | ~$49 (~$24.50 annual) | 10M | 50 | 5 | — | 25 | yes |
| Business | ~$499 | 100M | 200 | 50 | 5 | 50 | yes |
| Professional | ~$999 | 200M | 500 | 100 | 5 | 75 | yes |
| Enterprise | Custom | 1B+ | Custom | Custom | Custom | Custom | yes |

Add'l credits ~$5/M. *Free-tier LaserStream noted as devnet/trial; a **2-day LaserStream trial** is offered before buying a dedicated node. Staked Connections + Enhanced WebSockets on paid tiers. Confidence: High.

**Triton One** — source: https://triton.one/pricing/ (accessed 2026-06-17). Confidence: High (model), Medium (exact rates — calculator is interactive).
- Model: prepaid PAYG, **min deposit $125**, valid 12 months, non-refundable. Every product included on every plan (no tier-gating). Also offers **Dedicated Nodes**.
- APPROX rates seen: RPC ~**$10 / million calls**; streaming/Titan Prime ~**$0.08/GB bandwidth**; Metaplex/Photon ~$0.08/GB; Metis API ~$0.08/GB + $80/M calls. Confidence: Medium (verify in calculator).

**QuickNode** — source: https://www.quicknode.com/pricing (accessed 2026-06-17). Confidence: High (table rendered).

| Plan | APPROX price/mo | API credits | RPS | Endpoints | Archive/logs |
|---|---|---|---|---|---|
| Free trial | $0 | 10M | 15 | 1 | — |
| Build | ~$49 (~$42 annual) | 80M | 50 | 10 | 1 hour |
| Accelerate | ~$249 (~$212) | 450M | 125 | 20 | 1 day |
| Scale | ~$499 (~$424) | 950M | 250 | 50 | 1 day |
| Business | ~$999 (~$849) | 2B | 500 | 50 | 1 day |
| Enterprise | Custom | Custom | Custom | Unlimited | 14 days |

Add'l credits ~$0.50–$0.62/M depending on tier. Metered gRPC (Yellowstone) available as add-on. Confidence: High (table), Medium (gRPC metering).

**Shyft** — source: https://docs.shyft.to/solana-yellowstone-grpc/grpc-docs (accessed 2026-06-17); pricing page https://shyft.to/solana-rpc-grpc-pricing was **not fetched** this session → **pricing UNVERIFIED**. Offering confirmed: managed Yellowstone gRPC, tiered by connection limits (no per-request rate limit), 150-slot lookback, IDL-based transaction parsing, Dedicated Nodes for hot programs. Confidence: High (offering), Low (pricing).

---

## 2. Decoding AMM/DEX Pool State & Simulating Swaps

**Two strategies:**
- **A) On-chain account decoding + local math** — lowest latency, no per-quote API dependency, required for MEV. You stream pool accounts (via gRPC) and compute quotes yourself. Higher engineering cost; you must track every pool type's layout and math precisely.
- **B) SDKs / quote APIs (Jupiter, Raydium SDK, Orca SDK)** — fast to build, accurate, but adds network round-trips and rate limits; unsuitable for the hot path of latency-sensitive MEV but excellent for paper-trading validation and as a correctness oracle.

**Recommended for this project:** Build local decoders for the pools you target (A) **and** cross-check your computed quotes against Jupiter `/order` and the native SDKs (B) during paper trading to validate your math. This gives you the MEV-ready path plus a ground-truth check.

### 2.1 Raydium

Three pool programs (confirm program IDs against live source before use):
- **AMM v4** — classic constant-product (`x * y = k`). Reserves held in two vault token accounts referenced by the pool (AMM) state account. Price = `reserve_quote / reserve_base` (adjust for decimals). Swap-out (exact-in) with fee `f`: `dy = (y * dx * (1-f)) / (x + dx * (1-f))`. Confidence: High (standard CPMM math), Medium (exact field offsets — verify from IDL).
- **CLMM** — concentrated liquidity (Uniswap-v3-style): per-tick liquidity, `sqrt_price`, tick arrays. Quote requires walking tick arrays. Confidence: Medium.
- **CPMM** — newer standard constant-product program (Token-2022 compatible), distinct program ID from AMM v4. Confidence: Medium.
- SDK: **`raydium-sdk-V2`** (TypeScript), source https://github.com/raydium-io/raydium-sdk-V2 (accessed 2026-06-17). Provides `api.fetchPoolById`, `fetchPoolByMints`, pool lists, and compute/trade modules. Confidence: High (SDK exists & methods confirmed), Medium (swap-compute API surface).
- **FLAG (Low):** I could **not** fetch `docs.raydium.io/.../addresses` (404 this session) — **program IDs and exact account layouts are UNVERIFIED**; pull them from the on-chain IDL / SDK constants.

### 2.2 Orca Whirlpools

- Model: concentrated liquidity (CLMM). State stored as a `Whirlpool` account holding `sqrtPrice` (Q64.64), `tickCurrentIndex`, `liquidity`, `tickSpacing`, fee rate, token vaults; tick data in `TickArray` accounts. Confidence: Medium (model standard; exact layout verify against SDK).
- Price from sqrtPrice: `price = (sqrtPrice / 2^64)^2`, then adjust for token decimals. A swap crossing ticks consumes liquidity per tick; you must walk `TickArray`s. Confidence: Medium (standard Orca/UniV3 math), **Low** that I confirmed exact field names from a primary doc — the dev site (https://dev.orca.so/developers/overview) rendered as a thin SPA shell this session.
- SDK: `@orca-so/whirlpools-sdk` (TypeScript) + a Rust SDK. Quick-start confirms `WhirlpoolContext` / `buildWhirlpoolClient` / `client.getPool`. Source: https://dev.orca.so/ (accessed 2026-06-17). Confidence: High (SDK exists), Low (deep math docs not retrieved).

### 2.3 Meteora DLMM

- Model: **discrete liquidity bins**. Each **bin** holds liquidity at a fixed price; price is constant within a bin (zero slippage intra-bin). Source: https://docs.meteora.ag/ ("What is DLMM?") (accessed 2026-06-17). Confidence: High.
- Program: `lb_clmm`. Key account fields confirmed from https://docs.meteora.ag/developer-guides/dlmm/program/accounts.md (accessed 2026-06-17), Confidence: High:
  - `LbPair`: `token_x_mint`/`token_y_mint`, `reserve_x`/`reserve_y` (program vaults), **`active_id`** (current price bin), **`bin_step`** (price increment per bin, bp-style), plus PDA seeds.
  - `BinArray` accounts: **`MAX_BIN_PER_ARRAY = 70`** bins each; `BIN_ARRAY_BITMAP_SIZE = 512` covers bin indexes −512…511; oracle observation default length 100; `NUM_REWARDS = 2`.
- Price formula: bin price = `(1 + bin_step/10_000)^active_id` (then decimals-adjusted). Confidence: Medium (standard DLMM formula; `bin_step` semantics confirmed, exact exponent base verify in SDK).
- SDK: `@meteora-ag/dlmm` (TS) + Rust library (exact-in/exact-out, account-state inputs). Confidence: Medium.

### 2.4 Jupiter Aggregator API

Source: https://dev.jup.ag/docs/ and https://developers.jup.ag/docs/swap/order-and-execute.md and https://developers.jup.ag/docs/price/index.md (accessed 2026-06-17). Confidence: High.

- **Swap API V2** — base URL `https://api.jup.ag/swap/v2`.
  - `GET /order` — returns a best-price quote **and an assembled base64 transaction**. Required params: `inputMint`, `outputMint`, `amount`, `taker`. Optional: `slippageBps` (or `rtse` for Real-Time Slippage Estimator), `referralAccount`, gasless payer. Without optional params, all routers compete (Metis on-chain routing, JupiterZ RFQ, DFlow, OKX); adding fee/modification params can restrict routing to Metis only.
  - `POST /execute` — managed landing (Jupiter's own landing pipeline + retry/confirmation polling).
  - **Circular arbitrage routes are NOT supported via the API** (requires the Metis binary). Confidence: High — important for an arb/MEV bot: you cannot get self-loop arb routes from the public API.
- **Price API V3** — single authoritative USD-ish price per token (V3 collapsed V2's multiple price fields into one heuristic-cleaned price). For V2-style detail, derive from Swap `/quote`. Confidence: High.
- **Rate limits / tiers** (source: developers.jup.ag/docs/portal/*, accessed 2026-06-17). APPROX pricing:

| Tier | APPROX price/mo | RPS (general) | Credits |
|---|---|---|---|
| Keyless (no signup) | $0 | 0.5 | — |
| Free | $0 | 1 | — |
| Developer | ~$25 | 10 | 25M |
| Launch | ~$100 | 50 | 100M |
| Pro | ~$500 | 150 | 500M |

  60-second sliding window; limits per **account** (not per key); `429` on exceed. `/swap/v2/execute` and `/tx/v1/submit` have **separate, higher** rate buckets (Keyless 20 RPS, Free 50, Paid 100). Auth via `x-api-key` header. Confidence: High. **Implication:** the public Jupiter API is fine as a paper-trading oracle and for non-HFT execution, but 1–150 RPS is far too low for the hot path of a competitive MEV bot — decode pools locally for that.

---

## 3. Transaction Simulation (`simulateTransaction`)

Source: https://solana.com/docs/rpc/http/simulatetransaction (accessed 2026-06-17). Confidence: High.

**Returns** (`value` object): `err`, `logs[]`, `accounts` (post-sim account states when requested via `accounts.addresses`), `unitsConsumed` (compute units used — key for fee/CU budgeting), `returnData`, `loadedAccountsDataSize`, `innerInstructions` (when requested), and (with `replaceRecentBlockhash`) a `replacementBlockhash`.

**Key options:**
- `sigVerify` — verify signatures (mutually exclusive with `replaceRecentBlockhash`).
- `replaceRecentBlockhash` — replace the tx blockhash with a recent one so you can simulate without a fresh blockhash (great for repeated profit checks).
- `accounts.addresses` + `encoding` — return specified post-execution account states.
- `commitment` — simulate against `processed`/`confirmed`/`finalized` bank state.

**How searchers use it for profit estimation:** simulate the candidate arb/swap bundle against current bank state, read the returned post-state **token account balances** (or `returnData`) to compute realized output, subtract fees + priority fee + Jito tip, and only submit if net > 0. `unitsConsumed` feeds the compute-unit-limit and price-per-CU calc (§4).

**Accuracy caveats (Medium confidence, important):**
- Simulation runs against a **snapshot** of bank state at the chosen commitment — by the time your tx lands, competing txs may have moved pool reserves, so simulated profit is an **upper bound**, not a guarantee. This is the core risk MEV bots manage.
- Simulation does **not** account for your transaction's position in the block vs. competitors.
- For true execution realism, validator-side / banking-stage simulation (or local SVM via `solana-program-test` / LiteSVM) is more faithful than a remote `simulateTransaction`. Flagged: I did not verify a specific LiteSVM doc this session — **Low** confidence on that tool's current API.

---

## 4. Priority Fees & Compute Unit Pricing

**Compute Budget Program** (Confidence: High, standard):
- `SetComputeUnitLimit(units)` — cap CUs for the tx (default ~200k/instruction; max 1.4M per tx).
- `SetComputeUnitPrice(micro_lamports_per_CU)` — sets the **priority fee**. Total priority fee ≈ `compute_unit_limit × price_micro_lamports / 1_000_000` lamports.
- Best practice: set the CU limit tight using `unitsConsumed` from `simulateTransaction` (+ small headroom), so you don't overpay or hit a CU ceiling.

**`getRecentPrioritizationFees`** — source: https://solana.com/docs/rpc/http/getrecentprioritizationfees (accessed 2026-06-17). Confidence: High.
- Returns an array of `{ slot, prioritizationFee }` samples (the fee is in **micro-lamports per CU** observed in recent slots). Optionally pass account pubkeys to scope to slots that wrote those accounts (useful: fee level for a contended pool).
- Use percentiles of recent samples (e.g. p75/p90) to pick a competitive price; many bots also blend in provider-specific fee APIs (Helius/Triton priority-fee endpoints) and Jito tip floors.

**Jito tips vs priority fees** (Confidence: High): On Jito's bundle path, landing is driven by the **tip** (a transfer to a Jito tip account), separate from the Compute Budget priority fee. Tip floor reference: `GET https://bundles.jito.wtf/api/v1/bundles/tip_floor` returns landed-tip percentiles (25/50/75/95/99th) + EMA. Source: https://docs.jito.wtf/lowlatencytxnsend/ (accessed 2026-06-17).

**Profitability impact:** For an MEV/arb tx, net profit = simulated_output − input − base_fee − (CU_limit × CU_price/1e6) − jito_tip − rent. Priority fee/tip is the bid in the landing auction: too low → you don't land (lose the opportunity); too high → you erode or erase profit. Size it from the recent-fee percentiles + tip floor, and gate submission on simulated net being positive after fees.

---

## 5. Recommended Libraries / SDKs by Language

### Rust — **recommended for the latency-critical / live path.** Confidence: High.
- **`solana-sdk`** / `solana-client` — core types, RPC client. Mature, canonical.
- **`anchor` / `anchor-lang`** — IDL-based account (de)serialization; most DEX programs ship Anchor IDLs (Meteora `lb_clmm`, etc.) → use generated structs to decode pool state.
- **`yellowstone-grpc-client`** (from `rpcpool/yellowstone-grpc`) — native gRPC streaming client.
- **Jito Rust crates / `searcher-client`** — bundle submission; the ShredStream proxy is also Rust.
- **DEX Rust SDKs:** Orca Whirlpools Rust SDK, Meteora DLMM Rust library — for exact on-chain-faithful quote math.
- Fit for HFT: **Best.** No GC; fits colocated proxy + gRPC + local SVM simulation.

### TypeScript — **recommended for orchestration, paper-trading harness, and prototyping.** Confidence: High.
- **`@solana/kit`** (formerly `@solana/web3.js` 2.x) — the renamed, current 2.x line (source: https://github.com/anza-xyz/kit, accessed 2026-06-17). Tree-shakable, modern. **`@solana/web3.js` 1.x is in maintenance** (`solana-labs/solana-web3.js` `maintenance/v1.x` branch); `@solana/compat` bridges 1.x↔Kit types. Use Kit for new code.
- **`jito-ts`** — Jito TS SDK for bundles/searcher (source: https://github.com/jito-labs/jito-ts, accessed 2026-06-17). Confidence: High.
- **DEX SDKs:** `raydium-sdk-V2`, `@orca-so/whirlpools-sdk`, `@meteora-ag/dlmm`. Confidence: High (existence).
- Yellowstone gRPC: `@triton-one/yellowstone-grpc` (TS client). Confidence: Medium (package name — verify).
- Fit for HFT: orchestration/glue and correctness oracle, **not** the sub-ms hot path (GC + event-loop jitter). Good for the paper-trading layer.

### Python — **prototyping, research, backtesting only. Not for HFT hot path.** Confidence: High.
- **`solders`** — Rust-backed (PyO3) primitives (Keypair, Pubkey, Transaction) — fast, the modern base. (Could not load https://kevinheavey.github.io/solders/ this session — **docs link UNVERIFIED**, but the library is the established standard.)
- **`solana-py`** (`solana`) — RPC client, builds on solders.
- **`anchorpy`** — Anchor IDL client for Python (account decode + program calls).
- Fit for HFT: **Not suitable** for latency-critical execution; fine for strategy research, data analysis, and paper-trading simulation logic.

**Stack recommendation for this project (paper trading first):**
- Ingest: Yellowstone gRPC (Helius LaserStream or Triton) for accounts/txns; standard WS fallback.
- Decode: Anchor IDL structs (Rust) or `anchorpy`/SDKs (Python/TS) per DEX.
- Quote/sim: local constant-product / CLMM / DLMM math, validated against Jupiter `/order` + native SDKs; use `simulateTransaction` for execution realism.
- Language: Rust for the eventual live engine; TS or Python for the paper-trading harness and research.

---

## Open items / could not verify this session (flagged)
1. **Raydium program IDs & exact account offsets** — `docs.raydium.io/.../addresses` 404'd. UNVERIFIED → pull from on-chain IDL/SDK. (Low)
2. **Orca Whirlpool exact field layout & tick math** — dev site rendered as thin SPA. Formulas given are standard CLMM; confirm against `@orca-so/whirlpools-sdk` source. (Low)
3. **Meteora DLMM bin-price exponent base** — `bin_step`/`active_id` confirmed; exact formula `(1 + bin_step/1e4)^active_id` is standard but verify in SDK. (Medium)
4. **Shyft pricing** — pricing page not fetched. UNVERIFIED. (Low)
5. **QuickNode gRPC (Yellowstone) metering details** — listed as add-on; metering UNVERIFIED. (Medium)
6. **Triton exact PAYG rates** — calculator is interactive; rates are approximate. (Medium)
7. **`solders` docs** — host DNS failed this session; library is established but doc link UNVERIFIED. (Medium)
8. **All prices** — APPROX, observed 2026-06-17. Re-verify before budgeting.

## Primary sources (accessed 2026-06-17)
- Solana RPC: https://solana.com/docs/rpc/websocket/accountsubscribe · https://solana.com/docs/rpc/http/getprogramaccounts · https://solana.com/docs/rpc/http/simulatetransaction · https://solana.com/docs/rpc/http/getrecentprioritizationfees
- Yellowstone gRPC: https://github.com/rpcpool/yellowstone-grpc
- Helius: https://www.helius.dev/docs/grpc · https://www.helius.dev/pricing
- Triton: https://triton.one/pricing/
- QuickNode: https://www.quicknode.com/pricing
- Shyft: https://docs.shyft.to/solana-yellowstone-grpc/grpc-docs
- Jito: https://docs.jito.wtf/lowlatencytxnsend/ · https://docs.jito.wtf/lowlatencytxnfeed/
- Jupiter: https://dev.jup.ag/docs/ · https://developers.jup.ag/docs/swap/order-and-execute.md · https://developers.jup.ag/docs/price/index.md
- Raydium SDK: https://github.com/raydium-io/raydium-sdk-V2
- Orca: https://dev.orca.so/
- Meteora: https://docs.meteora.ag/ · https://docs.meteora.ag/developer-guides/dlmm/program/accounts.md
- Solana Kit: https://github.com/anza-xyz/kit · Jito TS: https://github.com/jito-labs/jito-ts
