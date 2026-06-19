# Known limitations

Things that are **intentionally out of scope** at the current version. Listed
here so you don't waste time hunting for them in the code.

## v1.1.7-realistic-mode (current)

### Live trading not implemented

- **No real transactions.** The `dl-executor` crate builds bundles, but the clients (`JupiterClient`, `JitoClient`) are mocks that return deterministic fake quotes. **No private key is loaded in the value path.**
- **Real execution requires v1.2+** (out of scope per the v1.1 series plan; the executor module is the only thing that needs to change).

### Streaming detector limitations

- **Vault subscriptions are Raydium-only.** Orca Whirlpool and Meteora DLMM pool reserves do not flow in (those pools register but their reserves stay 0). Graph edges for non-Raydium pools have no weight → no cycles through them.
- **Reserves from AmmInfo only give mints + fee.** The actual base/quote reserves live in the SPL-token vault accounts, which the bot must subscribe to separately. Without vault subscriptions, the graph is empty of weights.
- **New pools added on the fly** but their vault subscriptions race the next cycle detection. The very first `find_negative_cycles` after a new pool arrives may not include its reserves.

### ArbiNexus bridge limitations

- **Bridge is downstream of `dl-app`.** If `dl-app` isn't running, `wallet.cycles.jsonl` doesn't grow, and the bridge has nothing to process.
- **Win rate is uniform random.** Real signal/strategy calibration (win rate proportional to confidence score) is a v1.2+ task.
- **OraclePrice is hardcoded `1.0`.** Real Pyth oracle integration is v1.2+.

### Public RPC rate limits

- **Public `api.mainnet-beta.solana.com` disconnects sustained WebSocket after ~60s.** Must use a paid RPC (Helius/Triton/QuickNode) for overnight runs.

### `dl-app run --feed live` edge cases

- **No reconnection logic.** If the WS drops mid-run, the bot exits. The user must restart via `start_paper_trader.sh`.
- **No back-pressure on slow disk.** If `wallet.save` blocks, cycles queue up. Not a problem at 100 trades/min.
- **Cycle fill math is a placeholder** (`legs.len() * 100_000` lamports per leg). Real `dl_sim::fill_constant_product` math is wired in but the live trader doesn't use it yet (v1.2+).

## Things you might mistake for bugs

| Symptom | Why it's correct |
|---------|------------------|
| `wallet: NOT STARTED (no ./trader.pid)` after `stop_paper_trader.sh` | correct — bot stopped, PID file removed |
| `trades_written=0` with `cycles_evaluated > 1000` | conservative bound is rejecting sub-bp cycles; switch to `DL_PAPER_MODE=optimistic` for visualization |
| `Win rate: 100.0%` | optimistic mode; use `realistic` for honest 30% |
| `Win rate: 30%` | realistic mode; this is the random-loss injection, not a bug |
| Single pool subscription shows 0 trades | one pool alone cannot form a 2+ DEX cycle; subscribe to ≥2 pools |

## Documentation debt

- `docs/v1.0.md` is referenced from README but doesn't exist yet. The v1.0 release notes are in `CHANGELOG.md`.
- `docs/v1.1.md` is also referenced but doesn't exist. The v1.1 series is documented across the 7 sub-tag commit messages.
- These will be written as a separate doc-cleanup task in v1.2.

## Out of scope per the locked decisions (v1.0)

These were explicitly rejected by the user during v1.0 design:

- **Not using sidecar / HSM / session keys for hot wallet custody.** Plain encrypted keyfile is the v1.0 path.
- **Not using BAM (block assembly marketplace).** Jito bundles only.
- **Not using hand-rolled swap transactions.** Jupiter Aggregator v6 only.
- **Not building a UI/dashboard.** Terminal + `wallet.json` only.

If you see a PR adding these, reject it; it bypasses a locked decision.
