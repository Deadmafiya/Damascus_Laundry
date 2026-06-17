# damascus_laundry

A Solana MEV **paper-trading** engine. `damascus_laundry_v1.0` ingests real-time Solana
market state, detects **atomic DEX-DEX arbitrage** opportunities, and *simulates* whether
each opportunity would have been profitable — **without ever submitting a real
transaction**.

> **Paper trading only.** v1.0 holds **no private keys**, signs nothing, and submits
> nothing to the network. There is no funded wallet in the value path. Live execution is
> explicitly out of scope for v1.0 (deferred to a later phase that swaps in only the
> executor module).

## Why "accurate" is the whole point

~96% of attempted atomic arbitrages on Solana fail. A simulator is only honest if it
reproduces *losing* at that rate — not just spotting gross opportunities. Profit is
counted only after latency, competition, landing probability, and fees are modeled
pessimistically by default.

## Workspace layout

| Crate | Responsibility | Status |
|-------|----------------|--------|
| `dl-core` | Fixed-point (`u128`) math + injectable `Clock`/`Rng`/`Feed` traits + shared types | **Phase 1** |
| `dl-feed` | Real-time data ingestion (JSON-RPC WS first, gRPC-ready) | Phase 2 |
| `dl-state` | In-memory pool/account state, decimals-normalized | Phase 2 |
| `dl-detect` | Opportunity detection (price graph + Bellman-Ford negative cycle) | Phase 3 |
| `dl-sim` | Profit/cost estimation + pessimistic simulation core | Phase 4/5 |
| `dl-ledger` | Paper portfolio, PnL attribution, metrics | Phase 5/7 |
| `dl-app` | Binary wiring the pipeline together | all phases |

## Build & test

```shell
cargo build --workspace
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo run -p dl-app
```

## Engineering constraints

- **No floating-point in any value/balance/PnL path** — `u128` fixed-point base units,
  overflow-checked, explicit scale tracking. Floats only at display edges.
- **All nondeterministic dependencies are injectable** (`Clock`, `Rng`, `Feed`) so runs
  are reproducible under a seed — the foundation for golden-file replay.
- **Abstract read-state / submit layers** — the Solana block-building stack is in flux
  (mempool removal → Jito BAM → Firedancer); no single ordering model is hard-coded.

## Reference material

Deep-research findings (sourced, confidence-tagged) live in `.paul/research/`:

- `solana-mev-landscape.md` — domain map (no mempool, Jito/BAM/Firedancer, strategies, DEXs, risks)
- `solana-mev-data-stack-research.md` — ingestion, DEX decoding math, `simulateTransaction`, fees, SDKs
- `solana-mev-paper-trading-research.md` — accurate-simulation principles, metrics, overfitting defense
- `solana-mev-paper-bot-research.md` — architecture, language, repos, precision, phased build order

Key external reference:
[`jito-foundation/jito-solana`](https://github.com/jito-foundation/jito-solana) — Jito's
MEV fork of the Agave Solana validator (Apache-2.0). Used as the authoritative spec for
the bundle/tip/relayer/auction mechanics the simulation core models, and the node we'd run
for the eventual live/ShredStream path.

## License

Apache-2.0.
