#!/usr/bin/env bash
# Show the current state of the damascus_laundry paper trader.
#
# Usage:  ./scripts/status.sh
#
# Prints process state from ./trader.pid, then if wallet.json
# exists, prints balance, trade count, wins/losses, total PnL,
# and the last 10 trades. Safe to run from any directory.
#
# Refreshes the wallet on demand (the bot updates it on every trade).

set -uo pipefail

# Always run from the repo root so ./wallet.json and ./trader.pid
# resolve correctly regardless of the caller's cwd.
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
cd "$SCRIPT_DIR/.."

WALLET="./wallet.json"
PIDFILE="./trader.pid"

# --- Process state ---
if [[ -f "$PIDFILE" ]]; then
    PID=$(cat "$PIDFILE")
    if kill -0 "$PID" 2>/dev/null; then
        echo "trader: RUNNING (PID $PID)"
    else
        echo "trader: STOPPED (stale PID $PID in $PIDFILE)"
    fi
else
    echo "trader: NOT STARTED (no $PIDFILE)"
fi

# --- Wallet state ---
if [[ ! -f "$WALLET" ]]; then
    echo "wallet: $WALLET does not exist (bot hasn't written a trade yet)"
    exit 0
fi

if ! command -v jq >/dev/null 2>&1; then
    echo "error: 'jq' is required. install with: sudo apt install jq" >&2
    exit 1
fi

echo "---"

# Get last-updated timestamp for the wallet file.
WALLET_MTIME=$(stat -c %Y "$WALLET" 2>/dev/null || stat -f %m "$WALLET" 2>/dev/null || echo 0)
NOW=$(date +%s)
AGE=$((NOW - WALLET_MTIME))
echo "wallet: mtime ${AGE}s ago ($(date -d "@$WALLET_MTIME" 2>/dev/null || date -r "$WALLET_MTIME" 2>/dev/null || echo "$WALLET_MTIME"))"

# Pull fields. tolerate empty trades[].
jq -r '
    "balance_lamports:    \(.balance_lamports // 0)",
    "starting_balance:    \(.starting_balance_lamports // .balance_lamports // 0)",
    "updated_at_unix_ms:  \(.updated_at_unix_ms // 0)",
    "trade_count:         \(.trades // [] | length)",
    "wins:                \([(.trades // [])[] | select(.profit_lamports > 0)] | length)",
    "losses:              \([(.trades // [])[] | select(.profit_lamports < 0)] | length)",
    "total_pnl_lamports:  \([(.trades // [])[].profit_lamports] | add // 0)",
    "",
    "last 10 trades:",
    (
        ((.trades // []) | .[-10:] | .[]) as $t
        | "  ts=\($t.ts_unix_ms // 0) pair=\($t.pair // "?") pnl=\($t.profit_lamports // 0) bal=\($t.balance_after_lamports // 0)"
    )
' "$WALLET"
