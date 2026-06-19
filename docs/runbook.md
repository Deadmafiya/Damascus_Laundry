# Runbook

Detailed operator guide. See `README.md` for the quick-reference.

## 1. First-time setup

```bash
cd /home/deadmafia/Documents/damascus_laundry
cp .env.example .env
# Edit .env (gitignored) — set DL_LIVE_MODE, DL_LIVE_WS_URL,
# DL_LIVE_POOL_PUBKEYS. Leave DL_PAPER_MODE as 'realistic'.
cargo build --release -p dl-app
```

`.env` is gitignored — never commit it.

## 2. Operating modes

| Mode | Win rate | Use case |
|------|----------|----------|
| `DL_PAPER_MODE=optimistic` (default) | 100% | visualization; every detected cycle wins |
| `DL_PAPER_MODE=realistic` | 30% random | honest paper PnL tracking |

Both modes use the **optimistic bound** for the eval step (so sub-bp cycles still pass), then apply the win rate at the trade-write step.

## 3. Start / stop / inspect

```bash
./scripts/start_paper_trader.sh              # background, nohup
./scripts/status.sh                         # one-shot dashboard
./scripts/stop_paper_trader.sh              # SIGTERM, then SIGKILL after 10s
./scripts/seed_wallet.sh 1.0                # reset wallet to 1 SOL
tail -f trader.log                           # raw event stream
cat wallet.json | jq .                       # full wallet JSON
```

## 4. Realistic PnL via ArbiNexus bridge

The ArbiNexus bridge is **optional**. Run it only after `start_paper_trader.sh` is running:

```bash
# Terminal 2:
./scripts/run_arbinexus_bridge.sh
# Watches wallet.cycles.jsonl, runs ArbiNexus oracle model,
# writes wallet_paper.json.

cat wallet_paper.json | jq '{balance:.balance_lamports,trades:(.trades|length),wins:[.trades[]|select(.profit_lamports>0)]|length,losses:[.trades[]|select(.profit_lamports<=0)]|length,pnl:([.trades[].profit_lamports]|add//0)}'
```

## 5. Pool addresses

`DL_LIVE_POOL_PUBKEYS` accepts comma-separated base58 pubkeys. The `.env.example` ships three mainnet pools as a starting point:

| DEX | Pool | Address |
|-----|------|---------|
| Raydium AMM v4 | SOL/USDC | `58oQChx4yWmvKdwLLZzBi4ChoCc2fqCUWBkwMihLYQo2` |
| Orca Whirlpool | SOL/USDC | `Czfq3xZZDmsdGdUyrNLtRhGc47cXcZtLG4crryfu44zE` |
| Meteora DLMM | SOL/USDC | `HTvjzsfX3yU6BUodCjZ5vZkUrAxMDTrBs3CJaq43ashR` |

To find more: query `https://api.dexscreener.com/latest/dex/tokens/SOL` for the SOL mint.

## 6. Public-RPC limitations

The Solana Labs public RPC (`wss://api.mainnet-beta.solana.com`) rate-limits sustained WebSocket connections to ~60s before disconnecting. For overnight runs use a paid RPC:

- **Helius** free tier — WebSocket + Jito bundles + gRPC. Sign up at https://helius.dev.
- **Triton** free tier — WebSocket + gRPC. https://triton.one.
- **QuickNode** Build plan — 15 RPS sustained. https://quicknode.com.

Format: `wss://your-endpoint.example.com/<api-key>/`.

## 7. Wallet JSON schema

```json
{
  "starting_balance_lamports": 1000000000,
  "balance_lamports": 1057162000,
  "updated_at_unix_ms": 1781894309697,
  "trades": [
    {
      "ts_unix_ms": 1781894309697,
      "pair": "btq-qtb",
      "profit_lamports": 184000,
      "balance_after_lamports": 1057162000
    }
  ]
}
```

All amounts are integer lamports (1 SOL = 1,000,000,000 lamports). To convert: `lamports / 1e9`.

## 8. Troubleshooting

| Symptom | Likely cause | Fix |
|---------|---------------|-----|
| `wallet: NOT STARTED` | bot crashed; check `trader.log` | look for panic or connection error |
| 0 trades after 1 hour | conservative bound rejecting sub-bp cycles | use `DL_PAPER_MODE=optimistic` for visualization |
| `ws event channel disconnected` | public RPC rate limit | use paid RPC (Helius/Triton/QuickNode) |
| `vault subscribe failed` | mainnet RPC doesn't allow arbitrary programSubscribe | set `DL_LIVE_POOL_PUBKEYS` for accountSubscribe |
| `status.sh` shows stale PID | bot died but PID file not cleaned | run `./scripts/stop_paper_trader.sh` (handles stale PID) |

## 9. Overnight run checklist

```bash
# Pre-flight
./scripts/stop_paper_trader.sh 2>/dev/null || true
./scripts/seed_wallet.sh 1.0

# Confirm env
cat .env | grep -v '^#' | grep -v '^$'

# Build (once)
cargo build --release -p dl-app

# Start
./scripts/start_paper_trader.sh

# Verify it's connected
sleep 10
./scripts/status.sh
tail -20 trader.log

# Go to bed.
# (Optional) Terminal 2: ./scripts/run_arbinexus_bridge.sh

# In the morning:
./scripts/status.sh
# or stop with:
./scripts/stop_paper_trader.sh
```

## 10. Cost estimate (per hour, paper trading)

- Compute: ~0.5 CPU core, <100MB RAM (single-threaded with periodic bursts).
- RPC: included in your free tier (Helius free handles ~100k WS messages/day, this bot does ~10k/hour).
- Storage: `wallet.json` grows ~10KB per 100 trades. Negligible.

Total cost: **$0/hour** in paper mode.
