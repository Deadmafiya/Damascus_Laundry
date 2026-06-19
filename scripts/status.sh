#!/usr/bin/env bash
# Show the damascus_laundry paper trader's current state.
#
# Usage:  ./scripts/status.sh
#
# Beginner-friendly dashboard. Prints:
#   - Trader process status
#   - Wallet balance in SOL and approximate USD
#   - Profit & Loss (P&L) in SOL and approximate USD
#   - Trade summary (wins, losses, win rate)
#   - Recent trades table
#
# Reads wallet.json next to the trader.
#
# Run from anywhere: ./scripts/status.sh

set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
cd "$SCRIPT_DIR/.."

WALLET="./wallet.json"
PIDFILE="./trader.pid"

# --- Live SOL/USD price (best-effort) ---
SOL_USD_PRICE=""
SOL_USD_DISPLAY="(no internet)"
SOL_USD=$(curl -sf --max-time 5 "https://api.coinbase.com/v2/prices/SOL-USD/spot" 2>/dev/null \
    | jq -r '.data.amount // empty' 2>/dev/null \
    | head -1)
if [[ -n "$SOL_USD" && "$SOL_USD" != "null" && "$SOL_USD" != "" ]]; then
    SOL_USD_PRICE=$(printf "%.2f" "$SOL_USD")
    SOL_USD_DISPLAY="USD \$$SOL_USD_PRICE"
fi

lamports_to_sol() {
    awk -v l="$1" 'BEGIN { printf "%.9f", l / 1000000000 }'
}

lamports_to_usd() {
    local sol
    sol=$(lamports_to_sol "$1")
    if [[ -z "$SOL_USD_PRICE" ]]; then
        echo "(no price)"
    else
        awk -v s="$sol" -v p="$SOL_USD_PRICE" 'BEGIN { printf "%.4f", s * p }'
    fi
}

print_section() {
    echo ""
    echo "════════════════════════════════════════════════════════════════════"
    echo "  $1"
    echo "════════════════════════════════════════════════════════════════════"
}

# --- Process status ---
if [[ -f "$PIDFILE" ]]; then
    PID=$(cat "$PIDFILE")
    if kill -0 "$PID" 2>/dev/null; then
        RUNNING="YES  (PID $PID)"
    else
        RUNNING="NO   (stale PID $PID in $PIDFILE)"
    fi
else
    RUNNING="NO   (no $PIDFILE)"
fi

print_section "BOT STATUS"
echo "  Trader running:  $RUNNING"
echo "  Wallet file:     $WALLET"
echo "  SOL/USD price:   $SOL_USD_DISPLAY"

# --- Wallet state ---
if [[ ! -f "$WALLET" ]]; then
    print_section "WALLET"
    echo "  (no trades yet — the bot has not written any paper trade)"
    echo "  Starting balance:  10.000000000 SOL"
    echo ""
    echo "════════════════════════════════════════════════════════════════════"
    echo "  The bot connects to mainnet, detects cycles, and only writes"
    echo "  trades to wallet.json when a cycle's conservative bound says"
    echo "  WouldTrade. For new overnight runs, expect 0 trades on launch"
    echo "  and the first fill in seconds-to-minutes."
    echo "════════════════════════════════════════════════════════════════════"
    exit 0
fi

if ! command -v jq >/dev/null 2>&1; then
    echo "error: 'jq' is required. install with: sudo apt install jq" >&2
    exit 1
fi

# --- Read core fields ---
BAL=$(jq -r '.balance_lamports // 0' "$WALLET")
START=$(jq -r '.starting_balance_lamports // .balance_lamports // 0' "$WALLET")
UPDATED=$(jq -r '.updated_at_unix_ms // 0' "$WALLET")
TRADES=$(jq -r '.trades // [] | length' "$WALLET")
WINS=$(jq -r '[.trades // [] | .[] | select(.profit_lamports > 0)] | length' "$WALLET")
LOSSES=$(jq -r '[.trades // [] | .[] | select(.profit_lamports <= 0)] | length' "$WALLET")
PNL=$((BAL - START))

# Wallet freshness
WALLET_MTIME=$(stat -c %Y "$WALLET" 2>/dev/null || stat -f %m "$WALLET" 2>/dev/null || echo 0)
NOW=$(date +%s)
AGE=$((NOW - WALLET_MTIME))
WALLET_MTIME_FMT=$(date -d "@$WALLET_MTIME" 2>/dev/null || date -r "$WALLET_MTIME" 2>/dev/null || echo "unknown")

# Win-rate (handle divide-by-zero)
if [[ $TRADES -gt 0 ]]; then
    WIN_RATE_PCT=$(awk -v w="$WINS" -v t="$TRADES" 'BEGIN { printf "%.1f", (w * 100.0) / t }')
else
    WIN_RATE_PCT="n/a"
fi

BAL_SOL=$(lamports_to_sol "$BAL")
START_SOL=$(lamports_to_sol "$START")
PNL_SOL=$(lamports_to_sol "$PNL")
BAL_USD=$(lamports_to_usd "$BAL")
PNL_USD=$(lamports_to_usd "$PNL")
START_USD=$(lamports_to_usd "$START")

# Sign formatting for P&L
if [[ $PNL -ge 0 ]]; then
    PNL_SIGN="+"
else
    PNL_SIGN=""
fi

print_section "WALLET"
printf "  Balance:        %15s SOL  ≈  USD %-10s\n" "$BAL_SOL" "$BAL_USD"
printf "  Starting:       %15s SOL  ≈  USD %-10s\n" "$START_SOL" "$START_USD"
printf "  P&L:            %s%13s SOL  ≈  USD %s%-10s\n" "$PNL_SIGN" "$PNL_SOL" "$PNL_SIGN" "$PNL_USD"
echo "  Last trade:     $AGE seconds ago  ($WALLET_MTIME_FMT)"

print_section "TRADES"
printf "  Total trades:   %d\n" "$TRADES"
printf "  Wins:            %d\n" "$WINS"
printf "  Losses:         %d\n" "$LOSSES"
printf "  Win rate:       %s%%\n" "$WIN_RATE_PCT"

# --- Last trades table ---
print_section "LAST 10 TRADES"
echo "  TIMESTAMP            PAIR            P&L (lamports)    BALANCE (SOL)"
echo "  ────────────────────  ──────────────  ───────────────   ──────────────"
jq -r '
    (
        (.trades // []) as $t
        | ($t | .[-10:] | .[]) as $tr
        | "  \(($tr.ts_unix_ms / 1000) | floor | strftime("%Y-%m-%d %H:%M:%S"))  \(($tr.pair // "?") | .[0:14])  \(($tr.profit_lamports // 0) | tostring | (if .[0:1] == "-" then . else " " + . end) | .[-14:])  \(($tr.balance_after_lamports // 0) / 1000000000 | tostring | .[0:13])"
    )
' "$WALLET" 2>/dev/null || echo "  (trades unavailable)"

# --- Actionable hint ---
echo ""
echo "════════════════════════════════════════════════════════════════════"
echo "  ACTIONS"
echo "════════════════════════════════════════════════════════════════════"
if [[ "$RUNNING" == NO* ]]; then
    echo "  Start the bot:        ./scripts/start_paper_trader.sh"
else
    echo "  Stop the bot:         ./scripts/stop_paper_trader.sh"
    echo "  Watch the log:        tail -f trader.log"
fi
echo "  Show trades JSON:     cat $WALLET | jq ."
echo "  Realistic PnL (30%):  ./scripts/run_arbinexus_bridge.sh"
echo "════════════════════════════════════════════════════════════════════"
