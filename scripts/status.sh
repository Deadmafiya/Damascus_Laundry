#!/usr/bin/env bash
# Show the current state of the damascus_laundry paper trader.
#
# Usage:  ./scripts/status.sh
#
# Reads wallet.json and prints balance, trade count,
# win/loss breakdown, total PnL, and the last 10 trades.
# Also reports whether the trader process is running.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
cd "$SCRIPT_DIR/.."

WALLET="./wallet.json"
PIDFILE="./trader.pid"

# Process state
if [[ -f "$PIDFILE" ]]; then
    PID=$(cat "$PIDFILE")
    if kill -0 "$PID" 2>/dev/null; then
        echo "trader: RUNNING (PID $PID)"
    else
        echo "trader: STOPPED (stale PID $PID)"
    fi
else
    echo "trader: NOT STARTED (no $PIDFILE)"
fi

# Wallet state
if [[ ! -f "$WALLET" ]]; then
    echo "wallet: $WALLET does not exist"
    exit 0
fi

if ! command -v jq >/dev/null 2>&1; then
    echo "error: 'jq' is required. install with: sudo apt install jq" >&2
    exit 1
fi

echo "---"

jq -r '
    "balance_lamports:    \(.balance_lamports)",
    "starting_balance:    \(.starting_balance_lamports)",
    "updated_at_unix_ms:  \(.updated_at_unix_ms)",
    "trade_count:         \(.trades | length)",
    "wins:                \([.trades[] | select(.profit_lamports > 0)] | length)",
    "losses:              \([.trades[] | select(.profit_lamports < 0)] | length)",
    "total_pnl_lamports:  \([.trades[].profit_lamports] | add // 0)",
    "",
    "last 10 trades:",
    ((.trades | .[-10:] | .[]) as $t |
        "  ts=\($t.ts_unix_ms) pair=\($t.pair) pnl=\($t.profit_lamports) bal=\($t.balance_after_lamports)")
' "$WALLET"
