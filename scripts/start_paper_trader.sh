#!/usr/bin/env bash
# Start the damascus_laundry paper trader in the background.
#
# Usage:  ./scripts/start_paper_trader.sh
#
# Reads env from ./.env (if present), then starts dl-app run --feed live
# in background with nohup. Writes a PID file, survives terminal close.
#
# Stop with:  kill $(cat ./trader.pid)

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$REPO_ROOT"

# Load .env if present (allows inline comments, empty lines, quoted values).
if [[ -f .env ]]; then
    set -a
    # shellcheck disable=SC1091
    source .env
    set +a
elif [[ -f .env.example ]]; then
    echo "trader: no .env found, copying .env.example as template"
    cp .env.example .env
    echo "trader: edit ./.env and re-run this script"
    exit 1
fi

: "${DL_LIVE_MODE:?DL_LIVE_MODE is required (devnet | mainnet-paper | mainnet)}"
: "${DL_LIVE_WS_URL:?DL_LIVE_WS_URL is required (e.g. wss://your-quicknode.solana-mainnet.quiknode.pro/<key>/)}"
: "${DL_LIVE_POOL_PUBKEYS:?DL_LIVE_POOL_PUBKEYS is required (comma-separated mainnet pool addresses)}"
: "${DL_LIVE_DURATION_SECS:=28800}"

WALLET_PATH="$REPO_ROOT/wallet.json"
PID_PATH="$REPO_ROOT/trader.pid"
LOG_PATH="$REPO_ROOT/trader.log"

# Refuse to start if a previous instance is still running.
if [[ -f $PID_PATH ]] && kill -0 "$(cat $PID_PATH)" 2>/dev/null; then
    echo "trader: already running (pid $(cat $PID_PATH))"
    exit 1
fi

BINARY="$REPO_ROOT/target/release/dl-app"
if [[ ! -x $BINARY ]]; then
    echo "trader: $BINARY not built; running cargo build --release -p dl-app"
    cargo build --release -p dl-app
fi

# Use setsid + nohup to fully detach from the controlling terminal.
nohup "$BINARY" run --feed live --wallet "$WALLET_PATH" \
    > "$LOG_PATH" 2>&1 &
PID=$!
echo $PID > "$PID_PATH"

echo "trader: started (pid $PID, wallet=$WALLET_PATH, log=$LOG_PATH)"
echo "trader: stop with: kill \$(cat $PID_PATH)"
echo "trader: see status with: ./scripts/status.sh"
