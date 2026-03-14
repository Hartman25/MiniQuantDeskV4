#!/usr/bin/env bash
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
CORE_RS_DIR="${REPO_ROOT}/core-rs"
AUTH_MIG_DIR="${CORE_RS_DIR}/crates/mqk-db/migrations"
MANIFEST_PATH="${AUTH_MIG_DIR}/manifest.json"

VIOLATIONS=0

red()   { printf '\033[0;31m%s\033[0m\n' "$*"; }
green() { printf '\033[0;32m%s\033[0m\n' "$*"; }
info()  { printf '\033[0;36m%s\033[0m\n' "$*"; }

echo "============================================================"
echo " MQK Migration Governance Guard"
echo " Repo root: ${REPO_ROOT}"
echo "============================================================"

if [[ ! -d "${AUTH_MIG_DIR}" ]]; then
    red "Authoritative migration directory missing: ${AUTH_MIG_DIR}"
    exit 1
fi

if [[ ! -f "${MANIFEST_PATH}" ]]; then
    red "Migration manifest missing: ${MANIFEST_PATH}"
    exit 1
fi

echo ""
info "--- [A] Unmanaged migration directories under core-rs/ ---"

mapfile -t extra_migration_dirs < <(
    while IFS= read -r -d '' dir; do
        if [[ "${dir}" != "${AUTH_MIG_DIR}" ]]; then
            printf '%s\n' "${dir#"${REPO_ROOT}/"}"
        fi
    done < <(find "${CORE_RS_DIR}" -type d -name migrations -print0)
)

if [[ ${#extra_migration_dirs[@]} -eq 0 ]]; then
    green "  OK — no unmanaged migration directories under core-rs/"
else
    VIOLATIONS=$((VIOLATIONS + ${#extra_migration_dirs[@]}))
    red "  FAIL — unmanaged migration directories detected:"
    printf '  %s\n' "${extra_migration_dirs[@]}"
fi

echo ""
info "--- [B] Authoritative manifest drift ---"

mapfile -t manifest_paths < <(
    grep -oE '"path"[[:space:]]*:[[:space:]]*"[^"]+"' "${MANIFEST_PATH}" \
        | sed -E 's/.*"([^"]+)"/\1/'
)

mapfile -t manifest_duplicate_paths < <(
    printf '%s\n' "${manifest_paths[@]}" \
        | sed '/^$/d' \
        | sort \
        | uniq -d
)

mapfile -t manifest_unique_paths < <(
    printf '%s\n' "${manifest_paths[@]}" \
        | sed '/^$/d' \
        | sort -u
)

mapfile -t actual_sql_paths < <(
    while IFS= read -r -d '' sql_file; do
        rel="${sql_file#"${AUTH_MIG_DIR}/"}"
        printf '%s\n' "${rel//\\//}"
    done < <(find "${AUTH_MIG_DIR}" -type f -name "*.sql" -print0) | sort
)

if [[ ${#manifest_duplicate_paths[@]} -gt 0 ]]; then
    VIOLATIONS=$((VIOLATIONS + ${#manifest_duplicate_paths[@]}))
    red "  FAIL — manifest.json contains duplicate migration path entries:"
    printf '  %s\n' "${manifest_duplicate_paths[@]}"
fi

if ! diff_output=$(
    diff -u \
        <(printf '%s\n' "${manifest_unique_paths[@]}") \
        <(printf '%s\n' "${actual_sql_paths[@]}")
); then
    VIOLATIONS=$((VIOLATIONS + 1))
    red "  FAIL — manifest.json does not match authoritative SQL files:"
    printf '%s\n' "${diff_output}"
else
    green "  OK — manifest.json matches the authoritative SQL chain"
fi

echo ""
echo "============================================================"
echo " Summary"
echo "============================================================"

if [[ "${VIOLATIONS}" -eq 0 ]]; then
    green " MIGRATION GOVERNANCE GUARD PASSED."
    exit 0
else
    red " MIGRATION GOVERNANCE GUARD FAILED — ${VIOLATIONS} violation(s) found."
    exit 1
fi
