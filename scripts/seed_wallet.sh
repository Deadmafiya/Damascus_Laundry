#!/usr/bin/env bash
# Reset wallet.json to a custom starting balance.
# Usage:  ./scripts/seed_wallet.sh <sol>
SOL=${1:-1.0}
LAM=$(awk -v s="$SOL" 'BEGIN { printf "%d", s * 1000000000 }')
cat > wallet.json <<JSON
{
  "starting_balance_lamports": $LAM,
  "balance_lamports": $LAM,
  "updated_at_unix_ms": $(date +%s%3N),
  "trades": []
}
JSON
echo "wallet: seeded with $SOL SOL ($LAM lamports)"
