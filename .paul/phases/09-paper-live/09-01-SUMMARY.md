---
description: "Phase 9 / plan 01 — live paper trader (dl-paper crate, dl-app run --feed live, start/status scripts). Sub-tag v1.1.1-paper-live."
type: PlanSummary
about: "Phase 9 / plan 01"
---

# Phase 9 / Plan 01 — Live Paper Trader

## What shipped

- `dl-paper` crate (new): `PaperWallet`, `Trade`, `TradeFill`, `WalletStats`, atomic save/load.
- `dl-app run --feed live --wallet <path>`: paper-trader subcommand with refused-by-default LiveMode gate.
- `scripts/start_paper_trader.sh`: background-launches the trader via nohup, writes PID.
- `scripts/status.sh`: prints balance, trade count, win/loss, total PnL, last 10 trades.

## What you can do today

```bash
# Start before bed
DL_LIVE_MODE=devnet ./scripts/start_paper_trader.sh

# Check in the morning
./scripts/status.sh
```

## Test count

441 passing (was 428, +13 from dl-paper). Build clean. Wallet persists across restarts.

## Tag

`v1.1.1-paper-live`.
