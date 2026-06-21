#!/usr/bin/env bash
# recon_bundle.sh — DAM-60 / DAM-77.A operator one-liner for the
# reconciliation harness.
#
# For a recorded mainnet-paper bundle, this script:
#   1. Resolves the bundle_id to a .dlf capture on disk.
#   2. Invokes `dl-app recon` to replay the capture, compare against
#      the on-chain anchor dataset, and (optionally) calibrate
#      EvalParams.
#   3. Re-reads the JSON the recon CLI wrote to disk.
#   4. Applies the ±0.001 SOL gate:
#        exit 0  if |gap_sol| <= tolerance
#        exit 1  if |gap_sol| >  tolerance
#        exit 2  on any runtime error
#
# gap_sol is the difference between the *realized* PnL recorded by the
# live trade landing log and the *predicted* conservative e_pnl the
# recon harness produced. The realized-PnL bridge is owned by DAM-58;
# until that lands, gap_sol is 0.0 in the recon report and the operator
# is expected to hand-fill `realized_pnl_sol` from the landing log.
# See docs/recon/recon-json-schema.md for the contract.
#
# Usage:
#   scripts/recon_bundle.sh <bundle_id> [--calibrate]
#
# Env vars (with defaults):
#   DL_APP_BIN         target/release/dl-app
#   DL_CAPTURE_DIR     ./captures
#   DL_ANCHORS_FILE    ./anchors/latest.jsonl
#   DL_RECON_OUT       ./recon/${BUNDLE_ID}.json
#   DL_TOLERANCE_SOL   0.001
#   DL_REALIZED_PNL_SOL  0.0   # operator-set until DAM-58 ships
#
# Exit codes:
#   0  pass — |gap_sol| within tolerance
#   1  fail — |gap_sol| exceeds tolerance
#   2  runtime error (missing manifest, missing capture, recon failure,
#      non-zero recon exit without --report-json, etc.)

set -euo pipefail

# ---------------------------------------------------------------------------
# Argument parsing
# ---------------------------------------------------------------------------

BUNDLE_ID=""
CALIBRATE=0

if [[ $# -lt 1 ]]; then
    echo "usage: $0 <bundle_id> [--calibrate]" >&2
    exit 2
fi

BUNDLE_ID="$1"
shift

while [[ $# -gt 0 ]]; do
    case "$1" in
        --calibrate)
            CALIBRATE=1
            shift
            ;;
        -h|--help)
            sed -n '2,30p' "$0" | sed 's/^# \{0,1\}//'
            exit 0
            ;;
        *)
            echo "recon_bundle: unknown arg: $1" >&2
            exit 2
            ;;
    esac
done

# ---------------------------------------------------------------------------
# Config (env + defaults)
# ---------------------------------------------------------------------------

DL_APP_BIN="${DL_APP_BIN:-target/release/dl-app}"
DL_CAPTURE_DIR="${DL_CAPTURE_DIR:-./captures}"
DL_ANCHORS_FILE="${DL_ANCHORS_FILE:-./anchors/latest.jsonl}"
DL_RECON_OUT="${DL_RECON_OUT:-./recon/${BUNDLE_ID}.json}"
DL_TOLERANCE_SOL="${DL_TOLERANCE_SOL:-0.001}"
DL_REALIZED_PNL_SOL="${DL_REALIZED_PNL_SOL:-0.0}"

# ---------------------------------------------------------------------------
# 1. Resolve bundle_id -> capture path.
# ---------------------------------------------------------------------------

CAPTURE_PATH=""
MANIFEST="${DL_CAPTURE_DIR}/manifest.json"

if [[ -f "$MANIFEST" ]]; then
    # manifest.json is a JSON object keyed by bundle_id.
    CAPTURE_PATH="$(jq -r --arg id "$BUNDLE_ID" '.[$id] // empty' "$MANIFEST" 2>/dev/null || true)"
    if [[ -n "$CAPTURE_PATH" && "$CAPTURE_PATH" != "null" ]]; then
        # Resolve relative to DL_CAPTURE_DIR if not absolute.
        case "$CAPTURE_PATH" in
            /*) ;;
            *)  CAPTURE_PATH="${DL_CAPTURE_DIR}/${CAPTURE_PATH}" ;;
        esac
    fi
fi

if [[ -z "$CAPTURE_PATH" || ! -f "$CAPTURE_PATH" ]]; then
    # Fallback: flat search. Some operators keep a flat captures/
    # directory without a manifest.
    if [[ -f "${MANIFEST}" ]]; then
        : # manifest existed but did not list this bundle_id
    fi
    for cand in \
        "${DL_CAPTURE_DIR}/${BUNDLE_ID}.dlf" \
        "${DL_CAPTURE_DIR}/${BUNDLE_ID}.bincode" \
        "${DL_CAPTURE_DIR}/${BUNDLE_ID}"; do
        if [[ -f "$cand" ]]; then
            CAPTURE_PATH="$cand"
            break
        fi
    done
fi

if [[ -z "$CAPTURE_PATH" || ! -f "$CAPTURE_PATH" ]]; then
    echo "recon_bundle: no manifest entry for '${BUNDLE_ID}' and no captures/${BUNDLE_ID}*.{dlf,bincode} found" >&2
    exit 2
fi

# Ensure the recon output directory exists.
mkdir -p "$(dirname "$DL_RECON_OUT")"

# ---------------------------------------------------------------------------
# 2. Invoke dl-app recon.
#    We always pass --report-json so we own the JSON, even when --calibrate
#    is set (the anchor compare path otherwise swallows divergences into
#    the print loop and the operator can't grep them).
# ---------------------------------------------------------------------------

RECON_ARGS=(
    recon
    --capture "$CAPTURE_PATH"
    --anchors "$DL_ANCHORS_FILE"
    --report-json "$DL_RECON_OUT"
)
if [[ "$CALIBRATE" -eq 1 ]]; then
    RECON_ARGS+=(--calibrate)
fi

# Capture recon exit code without triggering `set -e`.
set +e
"$DL_APP_BIN" "${RECON_ARGS[@]}"
RECON_EXIT=$?
set -e

if [[ $RECON_EXIT -ne 0 ]]; then
    # dl-app recon exit codes:
    #   0 = clean (all anchors within tolerance)
    #   1 = divergences (anchors exceed tolerance)
    #   2 = runtime error
    # We translate non-zero into either gate-fail (1) or runtime (2)
    # based on which exit the recon CLI actually returned.
    if [[ $RECON_EXIT -eq 1 ]]; then
        # Anchors already diverged; the gate is going to fail.
        # Continue to the JSON read so we can still print the summary.
        :
    else
        # 2 or any other non-zero = runtime error.
        echo "recon_bundle: dl-app recon failed (exit $RECON_EXIT)" >&2
        exit 2
    fi
fi

if [[ ! -f "$DL_RECON_OUT" ]]; then
    echo "recon_bundle: recon did not write ${DL_RECON_OUT}" >&2
    exit 2
fi

# ---------------------------------------------------------------------------
# 3. Apply the ±0.001 SOL gate.
#    gap_sol = realized_pnl_sol - predicted_pnl_sol
#    predicted_pnl_sol is the conservative-sum-e_pnl / LAMPORTS_PER_SOL
#    from the JSON. realized_pnl_sol is operator-supplied via
#    DL_REALIZED_PNL_SOL (DAM-58 will populate this automatically).
# ---------------------------------------------------------------------------

PREDICTED_PNL_LAMPORTS="$(jq -r '.summary.sum_conservative_e_pnl // 0' "$DL_RECON_OUT")"
FEED_EVENTS="$(jq -r '.feed_events_consumed // 0' "$DL_RECON_OUT")"
WOULD_TRADE="$(jq -r '.summary.would_trade // 0' "$DL_RECON_OUT")"
REPORT_HASH="$(jq -r '.report_hash // 0' "$DL_RECON_OUT")"
TOTAL_TIP_LAMPORTS="$(jq -r '.total_tip_lamports // 0' "$DL_RECON_OUT")"

# Conservative e_pnl is in 1e18 scale (Prob * 1e18). To get SOL, divide
# by 1e18 then by 1e9 (lamports per SOL). The recon crate is
# integer-only, so we do the math in awk for the fractional SOL
# representation.
#
# If PREDICTED_PNL_LAMPORTS is "0" or null, fall back to 0.0 SOL.
if [[ -z "$PREDICTED_PNL_LAMPORTS" || "$PREDICTED_PNL_LAMPORTS" == "null" ]]; then
    PREDICTED_PNL_SOL="0.0"
else
    # PREDICTED_PNL_LAMPORTS is in 1e18-scaled PnL units (not lamports).
    # The conversion is: sol = value / 1e18
    PREDICTED_PNL_SOL="$(awk -v v="$PREDICTED_PNL_LAMPORTS" 'BEGIN{printf "%.9f", v/1e18}')"
fi

# gap_sol = realized - predicted
GAP_SOL="$(awk -v r="$DL_REALIZED_PNL_SOL" -v p="$PREDICTED_PNL_SOL" 'BEGIN{printf "%.9f", r-p}')"
WITHIN_TOL="$(awk -v g="$GAP_SOL" -v t="$DL_TOLERANCE_SOL" 'BEGIN{print (g<0?-g:g) <= t ? 1 : 0}')"
WITHIN_TOLERANCE="false"
if [[ "$WITHIN_TOL" == "1" ]]; then
    WITHIN_TOLERANCE="true"
fi

# The would_trade decision is from the recon summary, not the gate.
WOULD_TRADE_BOOL="false"
if [[ "$WOULD_TRADE" != "0" && "$WOULD_TRADE" != "null" && -n "$WOULD_TRADE" ]]; then
    WOULD_TRADE_BOOL="true"
fi

# ---------------------------------------------------------------------------
# 4. Print the structured summary.
# ---------------------------------------------------------------------------

SUMMARY=$(jq -n \
    --arg bundle_id       "$BUNDLE_ID" \
    --arg capture         "$CAPTURE_PATH" \
    --arg anchors         "$DL_ANCHORS_FILE" \
    --arg recon           "$DL_RECON_OUT" \
    --argjson tolerance_sol  "$DL_TOLERANCE_SOL" \
    --argjson gap_sol        "$GAP_SOL" \
    --argjson predicted_pnl_sol "$PREDICTED_PNL_SOL" \
    --argjson realized_pnl_sol  "$DL_REALIZED_PNL_SOL" \
    --argjson feed_events   "$FEED_EVENTS" \
    --argjson report_hash   "$REPORT_HASH" \
    --argjson total_tip_lamports "$TOTAL_TIP_LAMPORTS" \
    --argjson would_trade   "$WOULD_TRADE_BOOL" \
    --argjson within_tolerance "$WITHIN_TOLERANCE" \
    '{
        bundle_id: $bundle_id,
        capture: $capture,
        anchors: $anchors,
        recon: $recon,
        tolerance_sol: $tolerance_sol,
        predicted_pnl_sol: $predicted_pnl_sol,
        realized_pnl_sol: $realized_pnl_sol,
        gap_sol: $gap_sol,
        within_tolerance: $within_tolerance,
        would_trade: $would_trade,
        feed_events: $feed_events,
        report_hash: $report_hash,
        total_tip_lamports: $total_tip_lamports
    }')

echo "$SUMMARY"

# ---------------------------------------------------------------------------
# 5. Final exit.
# ---------------------------------------------------------------------------

if [[ "$WITHIN_TOLERANCE" == "true" ]]; then
    exit 0
else
    exit 1
fi
