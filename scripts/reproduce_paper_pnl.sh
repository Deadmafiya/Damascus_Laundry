#!/usr/bin/env bash
# reproduce_paper_pnl.sh — single-command reproduction
# (Phase 7 / plan 02, AC-6).
#
# Given a capture file, produces:
#   - out/ledger.dld             (v3 paper ledger)
#   - out/report.json            (ReconReport, JSON)
#   - out/PAPER_PNL_REPORT.md    (human-readable summary)
#
# Usage:
#   scripts/reproduce_paper_pnl.sh --capture <path> [--out <dir>]
#
# Refuses to run without --capture (CI-safe: no surprise
# live network calls). For live-capture mode, see the
# README; that flow is out of scope for v1.0 reproducibility.
#
# Exit codes:
#   0   success
#   1   usage error
#   2   capture file not found or unreadable
#   3   engine returned a non-zero exit code

set -euo pipefail

CAPTURE=""
OUT_DIR="out"

usage() {
    cat <<EOF
USAGE:
    $0 --capture <path> [--out <dir>]

EXAMPLE:
    $0 --capture crates/dl-feed/tests/fixtures/sample_capture.bincode
EOF
}

while [[ $# -gt 0 ]]; do
    case "$1" in
        --capture)
            CAPTURE="$2"
            shift 2
            ;;
        --out)
            OUT_DIR="$2"
            shift 2
            ;;
        -h|--help)
            usage
            exit 0
            ;;
        *)
            echo "unknown argument: $1" >&2
            usage >&2
            exit 1
            ;;
    esac
done

if [[ -z "$CAPTURE" ]]; then
    echo "error: --capture is required (no surprise network calls)" >&2
    usage >&2
    exit 1
fi

if [[ ! -f "$CAPTURE" ]]; then
    echo "error: capture file not found: $CAPTURE" >&2
    exit 2
fi

if ! command -v cargo >/dev/null 2>&1; then
    echo "error: cargo not on PATH" >&2
    exit 2
fi

# Resolve paths.
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
ABS_CAPTURE="$(cd "$(dirname "$CAPTURE")" && pwd)/$(basename "$CAPTURE")"

# Normalize OUT_DIR: allow absolute or relative; strip
# leading slashes that would re-anchor against PROJECT_ROOT.
case "$OUT_DIR" in
    /*) ABS_OUT="$OUT_DIR" ;;
    *)  ABS_OUT="$PROJECT_ROOT/$OUT_DIR" ;;
esac

mkdir -p "$ABS_OUT"

LEDGER_PATH="$ABS_OUT/ledger.dld"
REPORT_PATH="$ABS_OUT/report.json"
SUMMARY_PATH="$ABS_OUT/PAPER_PNL_REPORT.md"

echo "reproduce_paper_pnl: capture = $ABS_CAPTURE"
echo "reproduce_paper_pnl: out dir = $ABS_OUT"

# Step 1: clean the v3 ledger.
echo "step 1/3: build dl-app (release) ..."
(
    cd "$PROJECT_ROOT"
    cargo build --release -p dl-app 2>&1 | tail -3
) || { echo "error: cargo build failed" >&2; exit 3; }

BINARY="$PROJECT_ROOT/target/release/dl-app"
if [[ ! -x "$BINARY" ]]; then
    BINARY="$PROJECT_ROOT/target/debug/dl-app"
fi

# Step 2: dry-run with the supplied capture. The dry-run
# path decodes the capture and writes the v3 ledger
# when DL_LEDGER_PATH is set.
echo "step 2/3: dry-run replay (writes ledger.dld) ..."
DL_LEDGER_PATH="$LEDGER_PATH" DL_DRY_RUN=1 "$BINARY" 2>&1 | tail -10 \
    || { echo "error: dry-run failed" >&2; exit 3; }

# Step 3: run the recon harness against the same capture.
# The recon CLI compares engine aggregates against an
# anchor dataset (default $OUT_DIR/anchors.v0.jsonl is
# optional; if absent, the recon is run without the
# compare step and the divergences list is empty).
echo "step 3/3: recon report (writes report.json) ..."
"$BINARY" recon \
    --capture "$ABS_CAPTURE" \
    --report-json "$REPORT_PATH" 2>&1 | tail -20 \
    || { echo "error: recon failed" >&2; exit 3; }

# Markdown summary. Built by hand here (no Rust template)
# to keep the script self-contained.
TS="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
LEDGER_BYTES=$(stat -c%s "$LEDGER_PATH" 2>/dev/null || stat -f%z "$LEDGER_PATH")
REPORT_BYTES=$(stat -c%s "$REPORT_PATH" 2>/dev/null || stat -f%z "$REPORT_PATH")

cat > "$SUMMARY_PATH" <<EOF
# Paper PnL Reproduction Report

- Generated: \`$TS\`
- Source capture: \`$ABS_CAPTURE\`
- Engine: dl-app (reproducible build)
- Phase: 7 / plan 02 (AC-6)

## Outputs

| File | Size | Description |
| --- | --- | --- |
| \`ledger.dld\` | $LEDGER_BYTES B | v3 paper ledger (DLD-LDG1 schema v3) |
| \`report.json\` | $REPORT_BYTES B | ReconReport, JSON-serialized |
| \`PAPER_PNL_REPORT.md\` | (this file) | Human-readable summary |

## Reproducibility

\`\`\`
cargo build --release -p dl-app
DL_LEDGER_PATH=$LEDGER_PATH DL_DRY_RUN=1 ./target/release/dl-app
./target/release/dl-app recon --capture $ABS_CAPTURE --report-json $REPORT_PATH
\`\`\`

EOF

# Append basic stats from the JSON report (best-effort).
if command -v jq >/dev/null 2>&1; then
    {
        echo "## Engine aggregates"
        echo
        echo "| Metric | Value |"
        echo "| --- | --- |"
        for f in cycle_records cycles_evaluated; do
            v=$(jq -r "if .report.${f} != null then .report.${f} else \"\" end" "$REPORT_PATH" 2>/dev/null || true)
            if [[ -n "$v" ]]; then
                echo "| \`$f\` | \`$v\` |"
            fi
        done
        if [[ -f "$REPORT_PATH" ]]; then
            v=$(jq -r '.report.summary.would_trade // 0' "$REPORT_PATH" 2>/dev/null || true)
            echo "| \`would_trade\` | \`$v\` |"
            v=$(jq -r '.report.summary.total_tip_lamports // 0' "$REPORT_PATH" 2>/dev/null || true)
            echo "| \`total_tip_lamports\` | \`$v\` |"
        fi
        echo
    } >> "$SUMMARY_PATH"
fi

echo
echo "reproduce_paper_pnl: done."
echo "  ledger:    $LEDGER_PATH"
echo "  report:    $REPORT_PATH"
echo "  summary:   $SUMMARY_PATH"
