#!/usr/bin/env bash
# =============================================================================
# paper_soak_day.sh — AUTON-SOAK-01
# Canonical one-day autonomous paper soak harness for Paper + Alpaca.
#
# Usage:
#   MQK_DAEMON_URL=http://127.0.0.1:8899 \
#   MQK_OPERATOR_TOKEN=<token> \
#   bash scripts/paper_soak_day.sh [--intraday-interval-secs 1800]
#
# What this script does:
#   1. Validates required env / config for the canonical Paper + Alpaca path.
#   2. Checks daemon truth surfaces before open (preflight).
#   3. Snapshots key truth surfaces at pre-open, intraday (every N seconds),
#      and at the end-of-day boundary.
#   4. Captures daemon console output (if MQK_LOG_FILE is set).
#   5. Packages a review bundle (.tar.gz) in the output directory.
#
# Truthful surfaces polled:
#   GET /api/v1/system/status
#   GET /api/v1/system/preflight
#   GET /api/v1/autonomous/readiness
#   GET /api/v1/alerts/active
#   GET /api/v1/events/feed
#
# This script does NOT:
#   - Start or stop the daemon (operator must do that).
#   - Send signals or place orders.
#   - Run more than one day (re-run for each soak day).
#
# Output directory: ./soak_output/<YYYY-MM-DD_HH-MM-SS>/
# =============================================================================
set -euo pipefail

# ---------------------------------------------------------------------------
# Defaults
# ---------------------------------------------------------------------------
INTRADAY_INTERVAL_SECS="${MQK_SOAK_INTERVAL:-1800}"   # snapshot every 30 min
MAX_SOAK_SECS="${MQK_SOAK_MAX_SECS:-36000}"             # stop after 10 h
DAEMON_URL="${MQK_DAEMON_URL:-http://127.0.0.1:8899}"
OPERATOR_TOKEN="${MQK_OPERATOR_TOKEN:-}"
LOG_FILE="${MQK_LOG_FILE:-}"

# ---------------------------------------------------------------------------
# Parse flags
# ---------------------------------------------------------------------------
while [[ $# -gt 0 ]]; do
    case "$1" in
        --intraday-interval-secs)
            INTRADAY_INTERVAL_SECS="$2"; shift 2 ;;
        --max-soak-secs)
            MAX_SOAK_SECS="$2"; shift 2 ;;
        --daemon-url)
            DAEMON_URL="$2"; shift 2 ;;
        *)
            echo "Unknown flag: $1" >&2; exit 1 ;;
    esac
done

# ---------------------------------------------------------------------------
# Output directory
# ---------------------------------------------------------------------------
TS="$(date -u +%Y-%m-%d_%H-%M-%S)"
OUT_DIR="./soak_output/${TS}"
mkdir -p "${OUT_DIR}"
MANIFEST="${OUT_DIR}/soak_manifest.json"

echo "Soak output directory: ${OUT_DIR}"

# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------

fail() {
    echo "FATAL: $*" >&2
    exit 1
}

warn() {
    echo "WARN: $*" >&2
}

curl_get() {
    local url="$1"
    curl --silent --max-time 10 --fail "${url}"
}

curl_get_auth() {
    local url="$1"
    if [[ -n "${OPERATOR_TOKEN}" ]]; then
        curl --silent --max-time 10 --fail \
             -H "Authorization: Bearer ${OPERATOR_TOKEN}" \
             "${url}"
    else
        curl_get "${url}"
    fi
}

snapshot_surface() {
    local label="$1"
    local url="$2"
    local outfile="$3"
    local auth="${4:-no}"

    local ts_iso
    ts_iso="$(date -u +%Y-%m-%dT%H:%M:%SZ)"

    local body
    if [[ "${auth}" == "yes" ]]; then
        body="$(curl_get_auth "${url}" 2>/dev/null || echo '{"error":"request_failed"}')"
    else
        body="$(curl_get "${url}" 2>/dev/null || echo '{"error":"request_failed"}')"
    fi

    # Wrap in envelope with timestamp.
    printf '{"snapshot_ts":"%s","label":"%s","url":"%s","body":%s}\n' \
        "${ts_iso}" "${label}" "${url}" "${body}" > "${outfile}"

    echo "  [${ts_iso}] ${label} → ${outfile}"
}

snapshot_all() {
    local prefix="$1"
    local seq="$2"
    local seq_dir="${OUT_DIR}/snapshots/${seq}"
    mkdir -p "${seq_dir}"

    snapshot_surface "${prefix}_system_status" \
        "${DAEMON_URL}/api/v1/system/status" \
        "${seq_dir}/system_status.json"

    snapshot_surface "${prefix}_preflight" \
        "${DAEMON_URL}/api/v1/system/preflight" \
        "${seq_dir}/preflight.json"

    snapshot_surface "${prefix}_autonomous_readiness" \
        "${DAEMON_URL}/api/v1/autonomous/readiness" \
        "${seq_dir}/autonomous_readiness.json"

    snapshot_surface "${prefix}_alerts" \
        "${DAEMON_URL}/api/v1/alerts/active" \
        "${seq_dir}/alerts_active.json"

    snapshot_surface "${prefix}_events_feed" \
        "${DAEMON_URL}/api/v1/events/feed" \
        "${seq_dir}/events_feed.json"
}

# ---------------------------------------------------------------------------
# Step 1 — Validate required env
# ---------------------------------------------------------------------------
echo ""
echo "=== STEP 1: Env validation ==="

MISSING=""

check_env() {
    local var="$1"
    local desc="$2"
    if [[ -z "${!var:-}" ]]; then
        warn "Missing required env var: ${var} (${desc})"
        MISSING="${MISSING} ${var}"
    else
        echo "  OK: ${var} is set"
    fi
}

# Canonical Paper + Alpaca credentials (ENV-TRUTH-01)
check_env "ALPACA_API_KEY_PAPER"    "Alpaca paper API key"
check_env "ALPACA_API_SECRET_PAPER" "Alpaca paper API secret"
check_env "ALPACA_PAPER_BASE_URL"   "Alpaca paper base URL"
check_env "MQK_DATABASE_URL"        "PostgreSQL database URL"

# Daemon address
echo "  INFO: Using daemon at ${DAEMON_URL}"

# Operator token is strongly recommended but not required for read-only surfaces.
if [[ -z "${OPERATOR_TOKEN}" ]]; then
    warn "MQK_OPERATOR_TOKEN is not set; operator routes will be skipped"
fi

if [[ -n "${MISSING}" ]]; then
    fail "Required env vars are missing:${MISSING}. Set them before running the soak."
fi

# ---------------------------------------------------------------------------
# Step 2 — Verify daemon is reachable
# ---------------------------------------------------------------------------
echo ""
echo "=== STEP 2: Daemon reachability ==="

HEALTH_BODY="$(curl --silent --max-time 5 "${DAEMON_URL}/v1/health" 2>/dev/null || echo '{}')"
HEALTH_OK="$(echo "${HEALTH_BODY}" | python3 -c 'import sys,json; d=json.load(sys.stdin); print(d.get("ok",""))' 2>/dev/null || echo "")"

if [[ "${HEALTH_OK}" != "True" && "${HEALTH_OK}" != "true" ]]; then
    fail "Daemon is not reachable at ${DAEMON_URL}/v1/health. Start the daemon first."
fi
echo "  Daemon is reachable: /v1/health → ok=true"

# ---------------------------------------------------------------------------
# Step 3 — Pre-open snapshot
# ---------------------------------------------------------------------------
echo ""
echo "=== STEP 3: Pre-open snapshot ==="
snapshot_all "pre_open" "00_pre_open"

# Inspect autonomous readiness for paper+alpaca truth.
READINESS_BODY="$(curl_get "${DAEMON_URL}/api/v1/autonomous/readiness" 2>/dev/null || echo '{}')"
TRUTH_STATE="$(echo "${READINESS_BODY}" | python3 -c 'import sys,json; d=json.load(sys.stdin); print(d.get("truth_state",""))' 2>/dev/null || echo "unknown")"
OVERALL_READY="$(echo "${READINESS_BODY}" | python3 -c 'import sys,json; d=json.load(sys.stdin); print(d.get("overall_ready",""))' 2>/dev/null || echo "unknown")"
HIST_DEGRADED="$(echo "${READINESS_BODY}" | python3 -c 'import sys,json; d=json.load(sys.stdin); print(d.get("autonomous_history_degraded",""))' 2>/dev/null || echo "unknown")"

echo "  autonomous/readiness: truth_state=${TRUTH_STATE} overall_ready=${OVERALL_READY}"

if [[ "${TRUTH_STATE}" != "active" ]]; then
    warn "Autonomous readiness truth_state='${TRUTH_STATE}' (expected 'active' for paper+alpaca)."
    warn "Confirm MQK_DAEMON_ADAPTER_ID=alpaca and the daemon is in Paper mode."
fi

if [[ "${HIST_DEGRADED}" == "True" || "${HIST_DEGRADED}" == "true" ]]; then
    warn "AUTON-HIST-01: autonomous_history_degraded=true at pre-open."
    warn "Supervisor history will be incomplete.  Restart daemon with a working DB."
fi

if [[ "${OVERALL_READY}" != "True" && "${OVERALL_READY}" != "true" ]]; then
    warn "overall_ready=false at pre-open.  Inspect blockers in pre-open snapshot."
    warn "The soak will continue but autonomous start may be refused."
fi

# ---------------------------------------------------------------------------
# Step 4 — Intraday polling loop
# ---------------------------------------------------------------------------
echo ""
echo "=== STEP 4: Intraday polling (interval=${INTRADAY_INTERVAL_SECS}s, max=${MAX_SOAK_SECS}s) ==="
echo "  Ctrl-C to stop early; the review bundle will still be packaged."

START_EPOCH="$(date +%s)"
SEQ=1

trap 'echo ""; echo "Interrupted — packaging review bundle..."; package_bundle; exit 0' INT TERM

package_bundle() {
    echo ""
    echo "=== Packaging review bundle ==="

    # Write manifest
    END_TS="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
    cat > "${MANIFEST}" <<MANIFEST_EOF
{
  "schema_version": "soak-v1",
  "soak_start_utc": "${TS}",
  "soak_end_utc": "${END_TS}",
  "daemon_url": "${DAEMON_URL}",
  "intraday_interval_secs": ${INTRADAY_INTERVAL_SECS},
  "snapshot_count": ${SEQ},
  "log_file": "${LOG_FILE}"
}
MANIFEST_EOF

    # Copy log file if available
    if [[ -n "${LOG_FILE}" && -f "${LOG_FILE}" ]]; then
        cp "${LOG_FILE}" "${OUT_DIR}/daemon.log"
        echo "  Copied daemon log: ${LOG_FILE} → ${OUT_DIR}/daemon.log"
    fi

    BUNDLE="${OUT_DIR}/../soak_${TS}.tar.gz"
    tar -czf "${BUNDLE}" -C "$(dirname "${OUT_DIR}")" "$(basename "${OUT_DIR}")" 2>/dev/null || true
    echo "  Review bundle: ${BUNDLE}"
    echo "  Snapshot directory: ${OUT_DIR}"
}

while true; do
    ELAPSED="$(( $(date +%s) - START_EPOCH ))"
    if [[ "${ELAPSED}" -ge "${MAX_SOAK_SECS}" ]]; then
        echo "  Max soak duration (${MAX_SOAK_SECS}s) reached."
        break
    fi

    SEQ_LABEL="$(printf '%02d_intraday' "${SEQ}")"
    echo ""
    echo "  Snapshot ${SEQ} (elapsed=${ELAPSED}s):"
    snapshot_all "intraday" "${SEQ_LABEL}"
    SEQ=$(( SEQ + 1 ))

    # Check for history degradation mid-session
    READINESS_NOW="$(curl_get "${DAEMON_URL}/api/v1/autonomous/readiness" 2>/dev/null || echo '{}')"
    HIST_NOW="$(echo "${READINESS_NOW}" | python3 -c 'import sys,json; d=json.load(sys.stdin); print(d.get("autonomous_history_degraded",""))' 2>/dev/null || echo "unknown")"
    if [[ "${HIST_NOW}" == "True" || "${HIST_NOW}" == "true" ]]; then
        warn "autonomous_history_degraded=true at snapshot ${SEQ}."
    fi

    sleep "${INTRADAY_INTERVAL_SECS}"
done

# ---------------------------------------------------------------------------
# Step 5 — End-of-day snapshot
# ---------------------------------------------------------------------------
echo ""
echo "=== STEP 5: End-of-day snapshot ==="
snapshot_all "end_of_day" "$(printf '%02d_end_of_day' "${SEQ}")"

package_bundle

echo ""
echo "=== Soak complete ==="
