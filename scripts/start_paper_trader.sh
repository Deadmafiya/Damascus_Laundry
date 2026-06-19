#!/usr/bin/env bash
# Start the damascus_laundry paper trader in the background.
#
# Usage:  ./scripts/start_paper_trader.sh
#
# Pre-conditions:
#   - DL_LIVE_MODE in {devnet, mainnet-paper, mainnet} (refused by default)
#   - ./target/release/dl-app exists (build with: cargo build --release -p dl-app)
#
# Effects:
#   - Starts dl-app run --feed live --wallet <wallet.json> via nohup
#   - Writes PID to ./trader.pid
#   - Logs to ./trader.log
#   - Survives terminal close
#
# Stop with:  kill $(cat trader.pid)

set -euo pipefail

: "${DL_LIVE_MODE:?Set DL_LIVE_MODE (devnet, mainnet-paper, or mainnet)}"

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
cd "$SCRIPT_DIR/.."

WALLET="./wallet.json"
PIDFILE="./trader.pid"
LOGFILE="./trader.log"
BINARY="./target/release/dl-app"

if [[ ! -x "$BINARY" ]]; then
    echo "error: $BINARY not found. Build it first: cargo build --release -p dl-app" >&2
    exit 1
fi

if [[ -f "$PIDFILE" ]]; then
    PID=$(cat "$PIDFILE")
    if kill -0 "$PID" 2>/dev/null; then
        echo "trader already running with PID $PID"
        exit 0
    else
        echo "stale PID file; removing"
        rm -f "$PIDFILE"
    fi
fi

# Seed the wallet if it doesn't exist.
if [[ ! -f "$WALLET" ]]; then
    echo "{}" > "$WALLET"
fi

nohup "$BINARY" run --feed live --wallet "$WALLET" \
    > "$LOGFILE" 2>&1 &
echo $! > "$PIDFILE"

echo "started trader PID $(cat "$PIDFILE")"
echo "  wallet: $WALLET"
echo "  log:    $LOGFILE"
echo
echo "check status with:  ./scripts/status.sh"
echo "stop with:          kill \$(cat trader.pid)"
