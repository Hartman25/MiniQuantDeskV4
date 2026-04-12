#!/usr/bin/env bash
# =============================================================================
# DEP-GOV-01: Workspace Dependency Inheritance Guard
# =============================================================================
#
# Purpose: Fail CI if any crate Cargo.toml declares a workspace-pinned
#          dependency with an inline version rather than `workspace = true`.
#
# Background: The workspace root (core-rs/Cargo.toml) pins shared dependencies
# in [workspace.dependencies] so that all crates draw from a single version.
# When a crate declares its own inline version of a workspace-pinned dep, it:
#   - Creates a hidden divergence in feature sets (e.g. tls-native-tls gap)
#   - Prevents workspace-level upgrades from propagating automatically
#   - Causes the dep to be compiled at a different resolved version if the
#     version constraint differs (e.g. "0.7.4" vs "0.7")
#
# The highest-risk case is sqlx: the planned DEP-GOV-01-UPGRADE from 0.7→0.8
# will require updating the workspace pin. Any inline pin that survives that
# update silently holds the old version in the affected crate.
#
# Scope: core-rs/crates/*/Cargo.toml (all workspace member crates).
#        Checks [dependencies] and [dev-dependencies] sections.
#
# Guarded dependency:
#   sqlx — workspace pinned in [workspace.dependencies]; all inline version
#           declarations in crate Cargo.toml files are rejected.
#
# Allowed forms (accepted):
#   sqlx.workspace = true
#   sqlx = { workspace = true }
#   sqlx = { workspace = true, features = [...] }   (feature extension)
#
# Disallowed forms (rejected):
#   sqlx = "0.7"
#   sqlx = { version = "0.7", ... }
#   sqlx = { version = "0.7.4", ... }
#
# Exit codes: 0 = clean, 1 = violations found.
#
# Usage:
#   bash scripts/guards/check_workspace_dep_inheritance.sh
# =============================================================================

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
CRATES_DIR="$REPO_ROOT/core-rs/crates"

red()   { printf '\033[0;31m%s\033[0m\n' "$*"; }
green() { printf '\033[0;32m%s\033[0m\n' "$*"; }
info()  { printf '\033[0;36m%s\033[0m\n' "$*"; }

echo "============================================================"
echo " DEP-GOV-01: Workspace Dependency Inheritance Guard"
echo " Crates dir: ${CRATES_DIR}"
echo "============================================================"

# Workspace-pinned dependencies that MUST use workspace = true in all crate
# Cargo.toml files. Extend this list when new deps are added to [workspace.dependencies].
GUARDED_DEPS=(
  "sqlx"
)

VIOLATIONS=0

for dep in "${GUARDED_DEPS[@]}"; do
  echo ""
  info "--- [$dep] checking for inline version pins across crate Cargo.toml files ---"

  dep_violations=0

  while IFS= read -r -d '' cargo_toml; do
    rel="${cargo_toml#"${REPO_ROOT}/"}"

    # Detect inline version declarations.
    # Matches:
    #   dep = "..."           (bare string version)
    #   dep = { version = "..." ... }   (table with version key)
    # Does NOT match:
    #   dep.workspace = true
    #   dep = { workspace = true ... }
    #
    # We check both the bare-string form and the { version = ... } table form.
    # Using grep -E to handle both patterns in one pass.

    # Pattern 1: dep = "..."  (bare semver string, not workspace = true)
    bare_matches=$(grep -nE "^[[:space:]]*${dep}[[:space:]]*=[[:space:]]*\"" "$cargo_toml" 2>/dev/null || true)

    # Pattern 2: dep = { version = "..." } or dep = { ..., version = "..." ...}
    # Exclude lines that have "workspace = true" on the same line.
    table_matches=$(grep -nE "^[[:space:]]*${dep}[[:space:]]*=[[:space:]]*\{" "$cargo_toml" 2>/dev/null \
      | grep -v "workspace[[:space:]]*=[[:space:]]*true" \
      | grep "version[[:space:]]*=" \
      || true)

    combined="${bare_matches}${table_matches}"

    if [[ -n "$combined" ]]; then
      dep_violations=$((dep_violations + 1))
      VIOLATIONS=$((VIOLATIONS + 1))
      red "  FAIL: ${rel}"
      while IFS= read -r line; do
        [[ -z "$line" ]] && continue
        printf '    %s\n' "$line"
      done <<< "$combined"
    fi
  done < <(find "$CRATES_DIR" -maxdepth 2 -name "Cargo.toml" -print0)

  if [[ "$dep_violations" -eq 0 ]]; then
    green "  OK — all crate Cargo.toml files use ${dep}.workspace = true (or { workspace = true })"
  else
    red ""
    red "  Remediation for $dep:"
    red "    Replace:  ${dep} = { version = \"...\", features = [...] }"
    red "    With:     ${dep}.workspace = true"
    red "    Or:       ${dep} = { workspace = true }"
    red ""
    red "  If crate-specific feature additions are needed beyond the workspace set,"
    red "  extend the workspace features list instead, so all crates benefit uniformly."
    red ""
    red "  NOTE: The workspace pin in core-rs/Cargo.toml [workspace.dependencies]"
    red "  must include all features required by any consumer crate before migrating"
    red "  that crate to workspace = true."
  fi
done

echo ""
echo "============================================================"
echo " Summary"
echo "============================================================"

if [[ "$VIOLATIONS" -eq 0 ]]; then
  green " ALL WORKSPACE DEP INHERITANCE CHECKS PASSED."
  exit 0
else
  red " DEP-GOV-01 GUARD FAILED — ${VIOLATIONS} inline version pin(s) found."
  echo ""
  red " Each guarded workspace dependency must use 'workspace = true' in all"
  red " crate Cargo.toml files. Inline version pins bypass the workspace upgrade"
  red " path and create silent feature divergence."
  exit 1
fi
