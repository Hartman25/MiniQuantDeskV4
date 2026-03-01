#!/usr/bin/env bash
# =============================================================================
# P0-1: MiniQuantDesk V4 — Deterministic Unsafe-Pattern Guard
# =============================================================================
#
# Purpose: Fail CI if forbidden patterns appear in production source code.
#          This script contains NO randomness, NO network calls, NO wall-clock
#          time. It is a pure grep-based static check.
#
# Patterns ENFORCED:
#
#   [U] Uuid::new_v4() in production src/ files
#       Rationale: Run IDs and event IDs used in enforcement/audit paths must
#       be deterministic (derived from inputs), not random. Random IDs break
#       replay guarantees and audit correlation.
#       Remediation: Patches D1-1 (run IDs) and D1-2 (audit event IDs).
#
#   [T] Utc::now() in mqk-db/src/ (enforcement scope only)
#       Rationale: mqk-db/src/lib.rs contains deadman_expired() and
#       enforce_deadman_or_halt(), which directly gate capital execution.
#       Wall-clock time in this path makes halt decisions non-deterministic
#       and non-replayable. Other Utc::now() calls (audit timestamps,
#       artifact ingestion, heartbeat ticks, CLI metadata) are ops-metadata
#       and do NOT affect execution gating — widening that enforcement is
#       the scope of D1-3, not P0-1.
#       Remediation: Patch D1-3 (inject TimeSource into deadman).
#
# Pattern NOT enforced (rationale documented):
#
#   [N] DEFAULT now() in SQL migrations
#       Rationale: Every existing migration already uses DEFAULT now().
#       Enforcing a blanket ban here would immediately break CI with no
#       repair path until D1-4 runs. The correct sequencing is: D1-4
#       removes semantics-bearing instances first, then this guard can be
#       re-enabled to prevent regression. See TODO(D1-4) block below.
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
# [U] Uuid::new_v4 in production src/ files
# =============================================================================
# We scan files under crates/*/src/ only — not crates/*/tests/.
# Pattern catches BOTH call forms:
#   Uuid::new_v4()              — direct call
#   unwrap_or_else(Uuid::new_v4) — function pointer (also calls the RNG)
#
# Note on #[cfg(test)] blocks inside src/ files: grep cannot distinguish
# lines inside cfg(test) modules from production code. This is intentional:
# we are conservative. If a Uuid::new_v4 call must live in a src/ file
# under a cfg(test) block, the correct fix is to move it to a tests/ file.
# =============================================================================

echo ""
info "--- [U] Uuid::new_v4 in production src/ ---"

UUID_FILE_COUNT=0
UUID_MATCH_LINES=""

while IFS= read -r -d '' rs_file; do
    matches=$(grep -n "Uuid::new_v4" "$rs_file" 2>/dev/null || true)
    if [ -n "$matches" ]; then
        UUID_FILE_COUNT=$((UUID_FILE_COUNT + 1))
        rel="${rs_file#"${REPO_ROOT}/"}"
        while IFS= read -r line; do
            UUID_MATCH_LINES="${UUID_MATCH_LINES}  ${rel}:${line}"$'\n'
        done <<< "$matches"
    fi
done < <(find "${REPO_ROOT}/core-rs/crates" \
    -type f \
    -name "*.rs" \
    -path "*/src/*" \
    ! -path "*/target/*" \
    -print0)

if [ "$UUID_FILE_COUNT" -eq 0 ]; then
    green "  OK — no Uuid::new_v4() in production src/"
else
    VIOLATIONS=$((VIOLATIONS + UUID_FILE_COUNT))
    red "  FAIL — Uuid::new_v4 found in ${UUID_FILE_COUNT} production file(s):"
    printf '%s' "$UUID_MATCH_LINES"
    red "  Remediation: D1-1 (run IDs: daemon routes + cli), D1-2 (audit event IDs)."
    red "  Note: mqk-db/src/md.rs ingest_id fallback also flagged — address in D1-1 or separately."
fi

# =============================================================================
# [T] Utc::now() in mqk-db/src/ (enforcement scope)
# =============================================================================

echo ""
info "--- [T] Utc::now() in mqk-db/src/ (enforcement scope) ---"

UTC_FILE_COUNT=0
UTC_MATCH_LINES=""

MQK_DB_SRC="${REPO_ROOT}/core-rs/crates/mqk-db/src"

if [ -d "$MQK_DB_SRC" ]; then
    while IFS= read -r -d '' rs_file; do
        matches=$(grep -n "Utc::now()" "$rs_file" 2>/dev/null || true)
        if [ -n "$matches" ]; then
            UTC_FILE_COUNT=$((UTC_FILE_COUNT + 1))
            rel="${rs_file#"${REPO_ROOT}/"}"
            while IFS= read -r line; do
                UTC_MATCH_LINES="${UTC_MATCH_LINES}  ${rel}:${line}"$'\n'
            done <<< "$matches"
        fi
    done < <(find "$MQK_DB_SRC" \
        -type f \
        -name "*.rs" \
        ! -path "*/target/*" \
        -print0)
fi

if [ "$UTC_FILE_COUNT" -eq 0 ]; then
    green "  OK — no Utc::now() in mqk-db/src/"
else
    VIOLATIONS=$((VIOLATIONS + UTC_FILE_COUNT))
    red "  FAIL — Utc::now() found in ${UTC_FILE_COUNT} file(s) in mqk-db/src/:"
    printf '%s' "$UTC_MATCH_LINES"
    red "  Remediation: D1-3 (inject TimeSource abstraction into deadman)."
fi

# =============================================================================
# TODO(D1-4): DEFAULT now() in SQL migrations.
#
# Enable this block ONLY after D1-4 cleans existing migration files.
# Until then, this guard is intentionally disabled to avoid blocking CI
# before a repair path exists.
#
# echo ""
# info "--- [N] DEFAULT now() in SQL migrations ---"
# SQL_VIOLATIONS=0
# while IFS= read -r -d '' sql_file; do
#     matches=$(grep -in "default now()\|DEFAULT CURRENT_TIMESTAMP" "$sql_file" 2>/dev/null || true)
#     if [ -n "$matches" ]; then
#         SQL_VIOLATIONS=$((SQL_VIOLATIONS + 1))
#         VIOLATIONS=$((VIOLATIONS + 1))
#         rel="${sql_file#"${REPO_ROOT}/"}"
#         red "  FAIL: ${rel}"
#         printf '%s\n' "$matches"
#     fi
# done < <(find "${REPO_ROOT}/core-rs/crates/mqk-db/migrations" \
#     -type f -name "*.sql" -print0)
# if [ "$SQL_VIOLATIONS" -eq 0 ]; then
#     green "  OK — no DEFAULT now() in migrations"
# else
#     red "  Remediation: D1-4 (remove DEFAULT now() from semantics-bearing columns)."
# fi
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
    red " These are known tracked violations. Remediation patches:"
    red "   D1-1: Uuid::new_v4() in daemon routes + cli run command"
    red "   D1-2: Uuid::new_v4() in audit event IDs"
    red "   D1-3: Utc::now() in mqk-db deadman enforcement path"
    red "   D1-4: DEFAULT now() in SQL migrations (guard disabled until then)"
    exit 1
fi
