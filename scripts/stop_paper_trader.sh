#!/usr/bin/env bash
# Stop the damascus_laundry paper trader started by start_paper_trader.sh.
#
# Usage:  ./scripts/stop_paper_trader.sh
#
# Reads ./trader.pid, sends SIGTERM, waits up to 10s for graceful exit,
# then SIGKILL if it's still alive. Also handles the case where the
# process is already gone.

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
PID_PATH="$REPO_ROOT/trader.pid"

if [[ ! -f $PID_PATH ]]; then
    echo "trader: no pid file at $PID_PATH; nothing to stop"
    exit 0
fi

PID="$(cat $PID_PATH)"
if ! kill -0 "$PID" 2>/dev/null; then
    echo "trader: pid $PID not running; cleaning up stale pid file"
    rm -f "$PID_PATH"
    exit 0
fi

echo "trader: sending SIGTERM to pid $PID"
kill -TERM "$PID"

# Wait up to 10s for graceful exit.
for i in 1 2 3 4 5 6 7 8 9 10; do
    if ! kill -0 "$PID" 2>/dev/null; then
        echo "trader: stopped (pid $PID exited after ${i}s)"
        rm -f "$PID_PATH"
        exit 0
    fi
    sleep 1
done

# Still alive after 10s; force kill.
echo "trader: pid $PID did not exit gracefully; sending SIGKILL"
kill -KILL "$PID" 2>/dev/null || true
rm -f "$PID_PATH"
echo "trader: force-killed"
