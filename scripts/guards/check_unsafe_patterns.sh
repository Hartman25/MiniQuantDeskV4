#!/usr/bin/env bash
# =============================================================================
# P0-1: MiniQuantDesk V4 — Deterministic Unsafe-Pattern Guard
# =============================================================================
#
# Purpose: Fail CI if forbidden patterns appear in production source code.
#          This script contains NO randomness, NO network calls, NO wall-clock
#          time. It is a pure grep-based static check.
#
# Patterns ENFORCED (all under core-rs/crates/*/src/):
#
#   [U] Uuid::new_v4()       — RNG run/event identity (breaks determinism)
#   [T] Utc::now()           — wall-clock in mqk-db/src/ (enforcement scope)
#   [S] SystemTime::now      — system clock anywhere in production src/
#   [M] timestamp_millis()   — usually paired with Utc::now; flags temporal coupling
#   [R] rand::               — any rand crate usage in production src/
#   [N] DEFAULT now()        — semantics-bearing DB columns in migrations >= 0012
#
# Exemption mechanism:
#   Lines containing "// allow:" are excluded from all [U/T/S/M/R] checks.
#   SQL comment lines (starting with --) are excluded from [N] checks.
#   Pure Rust comment lines (leading //) are excluded from [U/T/S/M/R] checks.
#   This lets maintainers explicitly acknowledge a use is intentional.
#
#   Current allow-listed items (Rust // allow:):
#     mqk-daemon/src/state.rs spawn_heartbeat ts  — "// allow: ops-metadata"
#     (WallClock::now_utc() was moved to mqk-runtime/src/orchestrator.rs in D1-3;
#      it is outside the [T] guard scope of mqk-db/src/)
#
#   Current allow-listed items (SQL -- allow:, used in [Q] guard):
#     mqk-db/src/lib.rs arm_run armed_at_utc      — "-- allow: ops-metadata"
#     mqk-db/src/lib.rs begin_run running_at_utc  — "-- allow: ops-metadata"
#     mqk-db/src/lib.rs stop_run stopped_at_utc   — "-- allow: ops-metadata"
#     mqk-db/src/lib.rs persist_arm_state upd_at  — "-- allow: ops-metadata"
#
# Exit codes: 0 = clean, 1 = violations found.
#
# Usage:
#   bash scripts/guards/check_unsafe_patterns.sh
# =============================================================================

set -euo pipefail

# Resolve repo root (two levels up from this script's directory).
REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"

VIOLATIONS=0

red()   { printf '\033[0;31m%s\033[0m\n' "$*"; }
green() { printf '\033[0;32m%s\033[0m\n' "$*"; }
info()  { printf '\033[0;36m%s\033[0m\n' "$*"; }

echo "============================================================"
echo " MQK P0-1 Safety Guard"
echo " Repo root: ${REPO_ROOT}"
echo "============================================================"

# =============================================================================
# Helper: grep for PATTERN in FILE, filtering out:
#   1. Pure comment lines  — content (after stripping line-num prefix) starts
#                            with optional whitespace then "//"
#   2. Allow-listed lines  — content contains "// allow:"
#
# grep -n produces "LINENUM:CONTENT". We strip the "LINENUM:" prefix when
# testing whether the content itself is a comment.
#
# Usage: check_rs_pattern PATTERN FILE
# Returns: matching lines (may be empty); exits 0 regardless.
# =============================================================================
check_rs_pattern() {
    local pattern="$1" file="$2"
    grep -n "$pattern" "$file" 2>/dev/null \
        | grep -v "^[0-9][0-9]*:[[:space:]]*//" \
        | grep -v "// allow:" \
        || true
}

# =============================================================================
# [U] Uuid::new_v4 in production src/ (all crates)
# =============================================================================

echo ""
info "--- [U] Uuid::new_v4 in production src/ ---"

UUID_VIOLATIONS=0
UUID_MATCH_LINES=""

while IFS= read -r -d '' rs_file; do
    matches=$(check_rs_pattern "Uuid::new_v4" "$rs_file")
    if [ -n "$matches" ]; then
        UUID_VIOLATIONS=$((UUID_VIOLATIONS + 1))
        rel="${rs_file#"${REPO_ROOT}/"}"
        while IFS= read -r line; do
            UUID_MATCH_LINES="${UUID_MATCH_LINES}  ${rel}:${line}"$'\n'
        done <<< "$matches"
    fi
done < <(find "${REPO_ROOT}/core-rs/crates" \
    -type f -name "*.rs" -path "*/src/*" ! -path "*/target/*" -print0)

if [ "$UUID_VIOLATIONS" -eq 0 ]; then
    green "  OK — no Uuid::new_v4() in production src/"
else
    VIOLATIONS=$((VIOLATIONS + UUID_VIOLATIONS))
    red "  FAIL — Uuid::new_v4() found in ${UUID_VIOLATIONS} file(s):"
    printf '%s' "$UUID_MATCH_LINES"
    red "  Remediation: D1-1 (run IDs), D1-2 (audit event IDs)."
fi

# =============================================================================
# [T] Utc::now() in mqk-db/src/ (enforcement scope)
#
# mqk-db/src/ contains deadman_expired() and enforce_deadman_or_halt() which
# gate capital execution. Wall-clock time here breaks determinism.
# The sole permitted call is WallClock::now_utc() marked "// allow: wall-clock-canonical".
# =============================================================================

echo ""
info "--- [T] Utc::now() in mqk-db/src/ (enforcement scope) ---"

UTC_VIOLATIONS=0
UTC_MATCH_LINES=""
MQK_DB_SRC="${REPO_ROOT}/core-rs/crates/mqk-db/src"

if [ -d "$MQK_DB_SRC" ]; then
    while IFS= read -r -d '' rs_file; do
        matches=$(check_rs_pattern "Utc::now()" "$rs_file")
        if [ -n "$matches" ]; then
            UTC_VIOLATIONS=$((UTC_VIOLATIONS + 1))
            rel="${rs_file#"${REPO_ROOT}/"}"
            while IFS= read -r line; do
                UTC_MATCH_LINES="${UTC_MATCH_LINES}  ${rel}:${line}"$'\n'
            done <<< "$matches"
        fi
    done < <(find "$MQK_DB_SRC" \
        -type f -name "*.rs" ! -path "*/target/*" -print0)
fi

if [ "$UTC_VIOLATIONS" -eq 0 ]; then
    green "  OK — no ungated Utc::now() in mqk-db/src/"
else
    VIOLATIONS=$((VIOLATIONS + UTC_VIOLATIONS))
    red "  FAIL — Utc::now() found in ${UTC_VIOLATIONS} file(s) in mqk-db/src/:"
    printf '%s' "$UTC_MATCH_LINES"
    red "  Remediation: D1-3 (inject TimeSource into enforcement path)."
fi

# =============================================================================
# [S] SystemTime::now in production src/ (all crates)
#
# std::time::SystemTime::now() is a wall-clock read with platform-specific
# behavior (monotonicity not guaranteed, affected by NTP). Use injected
# TimeSource instead for any path that affects gating or determinism.
# =============================================================================

echo ""
info "--- [S] SystemTime::now in production src/ ---"

SYS_VIOLATIONS=0
SYS_MATCH_LINES=""

while IFS= read -r -d '' rs_file; do
    matches=$(check_rs_pattern "SystemTime::now" "$rs_file")
    if [ -n "$matches" ]; then
        SYS_VIOLATIONS=$((SYS_VIOLATIONS + 1))
        rel="${rs_file#"${REPO_ROOT}/"}"
        while IFS= read -r line; do
            SYS_MATCH_LINES="${SYS_MATCH_LINES}  ${rel}:${line}"$'\n'
        done <<< "$matches"
    fi
done < <(find "${REPO_ROOT}/core-rs/crates" \
    -type f -name "*.rs" -path "*/src/*" ! -path "*/target/*" -print0)

if [ "$SYS_VIOLATIONS" -eq 0 ]; then
    green "  OK — no SystemTime::now in production src/"
else
    VIOLATIONS=$((VIOLATIONS + SYS_VIOLATIONS))
    red "  FAIL — SystemTime::now found in ${SYS_VIOLATIONS} file(s):"
    printf '%s' "$SYS_MATCH_LINES"
    red "  Remediation: replace with injected TimeSource."
fi

# =============================================================================
# [M] timestamp_millis() in production src/ (all crates)
#
# .timestamp_millis() is typically called on Utc::now() or similar, creating
# a wall-clock dependency. Legitimate ops-metadata uses should be annotated
# "// allow: ops-metadata" to make the intent explicit and suppress this check.
# =============================================================================

echo ""
info "--- [M] timestamp_millis() in production src/ ---"

MS_VIOLATIONS=0
MS_MATCH_LINES=""

while IFS= read -r -d '' rs_file; do
    matches=$(check_rs_pattern "timestamp_millis" "$rs_file")
    if [ -n "$matches" ]; then
        MS_VIOLATIONS=$((MS_VIOLATIONS + 1))
        rel="${rs_file#"${REPO_ROOT}/"}"
        while IFS= read -r line; do
            MS_MATCH_LINES="${MS_MATCH_LINES}  ${rel}:${line}"$'\n'
        done <<< "$matches"
    fi
done < <(find "${REPO_ROOT}/core-rs/crates" \
    -type f -name "*.rs" -path "*/src/*" ! -path "*/target/*" -print0)

if [ "$MS_VIOLATIONS" -eq 0 ]; then
    green "  OK — no ungated timestamp_millis() in production src/"
else
    VIOLATIONS=$((VIOLATIONS + MS_VIOLATIONS))
    red "  FAIL — timestamp_millis() found in ${MS_VIOLATIONS} file(s):"
    printf '%s' "$MS_MATCH_LINES"
    red "  Remediation: remove wall-clock coupling or annotate '// allow: ops-metadata'."
fi

# =============================================================================
# [R] rand:: in production src/ (all crates)
#
# The rand crate must not be used in production execution paths. All IDs and
# ordering must be deterministic and derived from inputs.
# =============================================================================

echo ""
info "--- [R] rand:: in production src/ ---"

RAND_VIOLATIONS=0
RAND_MATCH_LINES=""

while IFS= read -r -d '' rs_file; do
    matches=$(check_rs_pattern "rand::" "$rs_file")
    if [ -n "$matches" ]; then
        RAND_VIOLATIONS=$((RAND_VIOLATIONS + 1))
        rel="${rs_file#"${REPO_ROOT}/"}"
        while IFS= read -r line; do
            RAND_MATCH_LINES="${RAND_MATCH_LINES}  ${rel}:${line}"$'\n'
        done <<< "$matches"
    fi
done < <(find "${REPO_ROOT}/core-rs/crates" \
    -type f -name "*.rs" -path "*/src/*" ! -path "*/target/*" -print0)

if [ "$RAND_VIOLATIONS" -eq 0 ]; then
    green "  OK — no rand:: in production src/"
else
    VIOLATIONS=$((VIOLATIONS + RAND_VIOLATIONS))
    red "  FAIL — rand:: found in ${RAND_VIOLATIONS} file(s):"
    printf '%s' "$RAND_MATCH_LINES"
    red "  Remediation: replace with deterministic derivation."
fi

# =============================================================================
# [N] DEFAULT now() in SQL migrations >= 0012
#
# Migrations 0001–0011 use DEFAULT now() for bookkeeping columns (created_at,
# received_at, etc.) that are NOT in any enforcement or capital-decision path.
# These are the D1-4 legacy whitelist — SQLx checksum immutability forbids
# retroactive changes.
#
# All migrations numbered >= 0012 must NOT use DEFAULT now() on any column.
# Semantics-bearing timestamps must be injected by the caller.
#
# Note: SQL comment lines (starting with --) are excluded from this check.
# =============================================================================

echo ""
info "--- [N] DEFAULT now() in new migration files (>= 0012) ---"

SQL_VIOLATIONS=0

while IFS= read -r -d '' sql_file; do
    basename=$(basename "$sql_file")
    # Only check files numbered >= 0012_
    [[ "$basename" < "0012_" ]] && continue
    # Exclude SQL comment lines (starting with optional whitespace + --)
    matches=$(grep -in "default now()\|DEFAULT CURRENT_TIMESTAMP" "$sql_file" 2>/dev/null \
        | grep -v "^[0-9][0-9]*:[[:space:]]*--" \
        || true)
    if [ -n "$matches" ]; then
        SQL_VIOLATIONS=$((SQL_VIOLATIONS + 1))
        VIOLATIONS=$((VIOLATIONS + 1))
        rel="${sql_file#"${REPO_ROOT}/"}"
        red "  FAIL: ${rel}"
        printf '%s\n' "$matches"
    fi
done < <(find "${REPO_ROOT}/core-rs/crates/mqk-db/migrations" \
    -type f -name "*.sql" -print0)

if [ "$SQL_VIOLATIONS" -eq 0 ]; then
    green "  OK — no DEFAULT now() in post-D1-4 migrations (>= 0012)"
else
    red "  Remediation: remove DEFAULT now(); inject timestamp via now: DateTime<Utc> caller parameter."
fi

# =============================================================================
# [Q] SQL now() in inline SQL strings within mqk-db/src/
#
# sqlx::query strings containing now() are equivalent to DEFAULT now() in a
# migration: the DB server supplies a non-deterministic wall-clock timestamp.
# Any column written by the enforcement or capital-decision path must receive
# an injected caller timestamp, not a DB-side now().
#
# Exemption:
#   Annotate the SQL line with a trailing SQL comment "-- allow: ops-metadata"
#   for columns that are pure bookkeeping (lifecycle timestamps, UI metadata)
#   and are NOT read by any enforcement or capital-decision path.
#   A trailing "// allow:" Rust annotation also suppresses the check.
# =============================================================================

echo ""
info "--- [Q] SQL now() in inline SQL strings within mqk-db/src/ ---"

SQL_NOW_VIOLATIONS=0
SQL_NOW_MATCH_LINES=""

if [ -d "$MQK_DB_SRC" ]; then
    while IFS= read -r -d '' rs_file; do
        matches=$(grep -n "now()" "$rs_file" 2>/dev/null \
            | grep -v "^[0-9][0-9]*:[[:space:]]*//" \
            | grep -v "// allow:" \
            | grep -v -e "-- allow:" \
            || true)
        if [ -n "$matches" ]; then
            SQL_NOW_VIOLATIONS=$((SQL_NOW_VIOLATIONS + 1))
            rel="${rs_file#"${REPO_ROOT}/"}"
            while IFS= read -r line; do
                SQL_NOW_MATCH_LINES="${SQL_NOW_MATCH_LINES}  ${rel}:${line}"$'\n'
            done <<< "$matches"
        fi
    done < <(find "$MQK_DB_SRC" \
        -type f -name "*.rs" ! -path "*/target/*" -print0)
fi

if [ "$SQL_NOW_VIOLATIONS" -eq 0 ]; then
    green "  OK — no unannotated SQL now() in mqk-db/src/"
else
    VIOLATIONS=$((VIOLATIONS + SQL_NOW_VIOLATIONS))
    red "  FAIL — SQL now() found in ${SQL_NOW_VIOLATIONS} file(s) in mqk-db/src/:"
    printf '%s' "$SQL_NOW_MATCH_LINES"
    red "  Remediation: inject timestamp as caller parameter, or annotate with"
    red "  '-- allow: ops-metadata' if the column is pure bookkeeping metadata."
fi

# =============================================================================
# Summary
# =============================================================================

echo ""
echo "============================================================"
echo " Summary"
echo "============================================================"

if [ "$VIOLATIONS" -eq 0 ]; then
    green " ALL GUARDS PASSED — no forbidden patterns detected."
    exit 0
else
    red " GUARD FAILED — ${VIOLATIONS} violation(s) found."
    echo ""
    red " Fix each flagged location or annotate with '// allow: <reason>'."
    red " Allowed exemptions:"
    red "   '// allow: wall-clock-canonical'  — WallClock::now_utc() in mqk-db"
    red "   '// allow: ops-metadata'          — non-enforcement UI/heartbeat paths"
    exit 1
fi
