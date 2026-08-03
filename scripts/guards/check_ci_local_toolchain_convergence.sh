#!/usr/bin/env bash
# =============================================================================
# CI/local toolchain-and-format convergence guard.
#
# FULL-AUDIT-SAFE-IGNORED-AND-SHARED-DB-FINAL-CLOSURE-01 Part 4: proves
# .github/workflows/ci.yml cannot silently drift from core-rs/rust-toolchain.toml
# (the single source of truth for the pinned Rust version), that no job
# depends on a floating third-party toolchain-install action, and that the
# Windows job's format check and script-guard invocation stay wired to the
# same canonical authorities local verification uses.
#
# Per-job validation (not aggregate string counts): every job block that
# declares a "Read canonical Rust toolchain channel" step is checked
# independently for (a) no floating toolchain-install action reference,
# (b) a rustup install line that reads steps.toolchain.outputs.channel,
# and (c) both rustfmt and clippy components requested. A single compliant
# job can no longer hide a violation in a sibling job the way a whole-file
# grep -c count could.
#
# Usage: bash scripts/guards/check_ci_local_toolchain_convergence.sh
# Exit codes: 0 = clean, 1 = violation found.
# =============================================================================
set -u

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"

WORKFLOW="$REPO_ROOT/.github/workflows/ci.yml"
TOOLCHAIN_FILE="$REPO_ROOT/core-rs/rust-toolchain.toml"
FMT_SCRIPT="$REPO_ROOT/scripts/windows/Invoke-CanonicalFmtCheck.ps1"
JOBS_DIR="$(mktemp -d)"
trap 'rm -rf "$JOBS_DIR"' EXIT

violations=0

echo "============================================================"
echo " CI/local toolchain-and-format convergence guard"
echo " Repo root: $REPO_ROOT"
echo "============================================================"

fail() {
    echo "  FAIL: $1"
    violations=$((violations + 1))
}

# Any known floating/unpinned reference to a third-party toolchain-install
# action. A SHA-pinned use (40 hex chars after @) is explicitly NOT flagged
# here -- this guard governs floating refs, not the presence of the action.
FLOATING_ACTION_PATTERN='dtolnay/rust-toolchain@(master|stable|beta|nightly|main)|actions-rs/toolchain@'

if [ ! -f "$WORKFLOW" ]; then
    fail "$WORKFLOW not found"
elif [ ! -f "$TOOLCHAIN_FILE" ]; then
    fail "$TOOLCHAIN_FILE not found"
else
    CANONICAL_CHANNEL=$(grep -m1 '^channel' "$TOOLCHAIN_FILE" | sed -E 's/channel *= *"([^"]+)"/\1/')
    if [ -z "$CANONICAL_CHANNEL" ]; then
        fail "could not parse 'channel' out of $TOOLCHAIN_FILE"
    else
        echo " Canonical channel (core-rs/rust-toolchain.toml): $CANONICAL_CHANNEL"
    fi

    # -------------------------------------------------------------------
    # Global: no floating third-party toolchain-install action anywhere,
    # under any ref alias. This repo installs Rust via rustup directly.
    # -------------------------------------------------------------------
    if grep -qE "$FLOATING_ACTION_PATTERN" "$WORKFLOW"; then
        fail "$WORKFLOW references a floating third-party toolchain-install action (dtolnay/rust-toolchain@master|stable|beta|nightly|main or actions-rs/toolchain@*). Install Rust via rustup directly (rustup toolchain install <channel> --component rustfmt --component clippy) so no external action ref can drift the pinned version."
    fi

    # Any *use* of dtolnay/rust-toolchain that is not SHA-pinned is a
    # violation regardless of alias -- catches refs this pattern list has
    # not enumerated yet (e.g. a future @v2 tag).
    while IFS= read -r use_line; do
        [ -z "$use_line" ] && continue
        if ! echo "$use_line" | grep -qE '@[0-9a-f]{40}$'; then
            fail "Unpinned dtolnay/rust-toolchain reference found: '$use_line' (must be a full 40-char commit SHA, or removed in favor of direct rustup install)"
        fi
    done < <(grep -oE 'uses:\s*dtolnay/rust-toolchain@[^[:space:]]+' "$WORKFLOW" | sed -E 's/^uses:\s*//')

    # -------------------------------------------------------------------
    # Split the workflow into per-job blocks (top-level jobs are declared
    # at exactly 2-space indent under `jobs:`) so each job that reads the
    # canonical toolchain channel is validated independently.
    # -------------------------------------------------------------------
    awk -v jobsdir="$JOBS_DIR" '
        /^jobs:/ { in_jobs = 1; next }
        in_jobs && /^  [A-Za-z0-9_-]+:[[:space:]]*$/ {
            if (current != "") { close(out) }
            current = substr($1, 1, length($1) - 1)
            gsub(/^  /, "", current)
            out = jobsdir "/" current ".yml"
            print > out
            next
        }
        in_jobs && current != "" { print > out }
    ' "$WORKFLOW"

    rust_job_count=0
    for job_file in "$JOBS_DIR"/*.yml; do
        [ -e "$job_file" ] || continue
        job_name="$(basename "$job_file" .yml)"

        if ! grep -q 'Read canonical Rust toolchain channel' "$job_file"; then
            continue
        fi
        rust_job_count=$((rust_job_count + 1))

        if grep -qE "$FLOATING_ACTION_PATTERN" "$job_file"; then
            fail "job '$job_name': still references a floating toolchain-install action"
        fi

        if ! grep -qE "core-rs[/\\\\]rust-toolchain\.toml" "$job_file"; then
            fail "job '$job_name': toolchain-read step does not reference core-rs/rust-toolchain.toml"
        fi

        if ! grep -q 'rustup toolchain install' "$job_file"; then
            fail "job '$job_name': no 'rustup toolchain install' step found -- toolchain may no longer be installed explicitly"
        elif ! grep -E 'rustup toolchain install' "$job_file" | grep -q 'steps\.toolchain\.outputs\.channel'; then
            fail "job '$job_name': 'rustup toolchain install' does not reference steps.toolchain.outputs.channel on the same line -- channel may be hardcoded (a second source of truth)"
        fi

        if ! grep -q 'rustup default' "$job_file"; then
            fail "job '$job_name': no 'rustup default' step found -- installed toolchain may not be selected as active"
        elif ! grep -E 'rustup default' "$job_file" | grep -q 'steps\.toolchain\.outputs\.channel'; then
            fail "job '$job_name': 'rustup default' does not reference steps.toolchain.outputs.channel on the same line -- channel may be hardcoded (a second source of truth)"
        fi

        if ! grep -q -- '--component rustfmt' "$job_file"; then
            fail "job '$job_name': rustup install does not request the rustfmt component"
        fi
        if ! grep -q -- '--component clippy' "$job_file"; then
            fail "job '$job_name': rustup install does not request the clippy component"
        fi

        # No job may hard-code the channel string as a literal instead of
        # the dynamic step output (a second, independently-driftable source
        # of truth for the pinned version).
        if [ -n "${CANONICAL_CHANNEL:-}" ] && grep -qE "rustup (toolchain install|default)[^\n]*[\"']${CANONICAL_CHANNEL}[\"']" "$job_file"; then
            fail "job '$job_name': appears to hard-code the channel literal '$CANONICAL_CHANNEL' instead of \${{ steps.toolchain.outputs.channel }}"
        fi
    done

    if [ "$rust_job_count" -eq 0 ]; then
        fail "no job in $WORKFLOW declares a 'Read canonical Rust toolchain channel' step -- toolchain pinning may have been removed entirely"
    else
        echo " Validated $rust_job_count Rust toolchain-installing job(s) individually."
    fi
fi

# The Windows job must not hard-code powershell.exe.
if grep -q 'powershell\.exe' "$WORKFLOW"; then
    fail "$WORKFLOW hard-codes powershell.exe; must use the job's own declared pwsh host (see PowerShell script guards step) or another bounded shell resolver"
fi

# The Windows job's fmt step must delegate to the canonical script, not run
# a bare `cargo fmt --check`.
if [ ! -f "$FMT_SCRIPT" ]; then
    fail "$FMT_SCRIPT not found -- canonical fmt-check authority is missing"
else
    if ! grep -q 'Invoke-CanonicalFmtCheck.ps1' "$WORKFLOW"; then
        fail "$WORKFLOW's windows job does not invoke scripts/windows/Invoke-CanonicalFmtCheck.ps1"
    fi
    # full_repo_proof.ps1 (local verification) must call the same script.
    LOCAL_PROOF_SCRIPT="$REPO_ROOT/full_repo_proof.ps1"
    if [ -f "$LOCAL_PROOF_SCRIPT" ] && ! grep -q 'Invoke-CanonicalFmtCheck.ps1' "$LOCAL_PROOF_SCRIPT"; then
        fail "$LOCAL_PROOF_SCRIPT does not delegate its fmt-check lane to scripts/windows/Invoke-CanonicalFmtCheck.ps1 -- CI and local verification would be running two independent format-check implementations again"
    fi
fi

echo ""
if [ "$violations" -eq 0 ]; then
    echo " OK -- CI and local toolchain/format authority converge on core-rs/rust-toolchain.toml"
    echo "       via direct rustup install (no floating third-party action); no drift found."
    exit 0
else
    echo " FAIL -- $violations convergence violation(s) found above."
    exit 1
fi
