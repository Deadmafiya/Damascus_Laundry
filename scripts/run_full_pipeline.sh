#!/usr/bin/env bash
# Run the full damascus_laundry + ArbiNexus paper-trading pipeline.
#
# Usage:  ./scripts/run_full_pipeline.sh
#
# This is the **complete** paper-trading loop:
#   1. dl-app subscribes to mainnet pools + vaults, runs the
#      streaming detector, writes wallet.json + wallet.cycles.jsonl.
#   2. dlx_bridge reads wallet.cycles.jsonl, feeds each cycle
#      through ArbiNexus's computeOpportunity() with realistic
#      oracle confidence gating + 30% win rate + 10k lamport tip,
#      writes wallet_paper.json with realistic paper PnL.
#
# The bridge step requires Node.js + pnpm + the vendored ArbiNexus
# packages built. If those are missing, dl-app still runs on its own.
#
# Stop with:  ./scripts/stop_paper_trader.sh

set -uo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$REPO_ROOT"

# 1. Start the Rust pipeline in background.
./scripts/start_paper_trader.sh

# 2. Optionally run the ArbiNexus bridge in parallel.
#    This polls wallet.cycles.jsonl every 5 seconds.
if command -v pnpm >/dev/null 2>&1 && [[ -d vendor/arbinexus ]]; then
    echo "trader: ArbiNexus bridge available; tail wallet.cycles.jsonl with: ./scripts/run_arbinexus_bridge.sh"
else
    echo "trader: pnpm or vendor/arbinexus missing; running Rust-only paper mode"
    echo "trader: (install pnpm with: sudo npm install -g pnpm)"
fi
