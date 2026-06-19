#!/usr/bin/env bash
# Run the ArbiNexus paper-trade bridge in a loop, polling
# wallet.cycles.jsonl for new detected cycles.
#
# Usage:  ./scripts/run_arbinexus_bridge.sh
#
# Reads from wallet.cycles.jsonl (next to wallet.json) and writes
# wallet_paper.json with realistic oracle + win-rate + tip modeling.
#
# Requires: pnpm, vendored vendor/arbinexus.

set -uo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$REPO_ROOT"

if ! command -v pnpm >/dev/null 2>&1; then
    echo "bridge: pnpm not installed; install with: sudo npm install -g pnpm"
    exit 1
fi

if [[ ! -d vendor/arbinexus ]]; then
    echo "bridge: vendor/arbinexus not found; run: git submodule update --init"
    exit 1
fi

CYCLES_PATH="${1:-./wallet.cycles.jsonl}"
WALLET_PATH="${2:-./wallet_paper.json}"

# Install deps once (cached after first run).
if [[ ! -d vendor/arbinexus/node_modules ]]; then
    echo "bridge: installing ArbiNexus dependencies (one-time)..."
    (cd vendor/arbinexus && pnpm install --frozen-lockfile)
fi

echo "bridge: polling $CYCLES_PATH, writing $WALLET_PATH"
echo "bridge: Ctrl-C to stop"
echo ""

LAST_LINES=0
while true; do
    if [[ ! -f "$CYCLES_PATH" ]]; then
        sleep 2
        continue
    fi
    CUR_LINES=$(wc -l < "$CYCLES_PATH" 2>/dev/null || echo 0)
    if [[ $CUR_LINES -gt $LAST_LINES ]]; then
        NEW=$((CUR_LINES - LAST_LINES))
        echo "bridge: $NEW new cycles detected, running through ArbiNexus..."
        LAST_LINES=$CUR_LINES
        (cd vendor/arbinexus && \
            pnpm exec tsx scripts/dlx_bridge.ts \
                "$REPO_ROOT/$CYCLES_PATH" \
                "$REPO_ROOT/$WALLET_PATH" \
            || true)
    fi
    sleep 5
done
