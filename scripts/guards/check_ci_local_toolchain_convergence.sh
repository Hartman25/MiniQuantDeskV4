#!/usr/bin/env bash
# =============================================================================
# CI/local toolchain-and-format convergence guard.
#
# FULL-AUDIT-FINAL-HERMETIC-CLOSURE-01 Part 4: proves .github/workflows/ci.yml
# cannot silently drift from core-rs/rust-toolchain.toml (the single source
# of truth for the pinned Rust version), and that the Windows job's format
# check and script-guard invocation stay wired to the same canonical
# authorities local verification uses.
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

violations=0

echo "============================================================"
echo " CI/local toolchain-and-format convergence guard"
echo " Repo root: $REPO_ROOT"
echo "============================================================"

fail() {
    echo "  FAIL: $1"
    violations=$((violations + 1))
}

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

    # No job may use the bare @stable ref (a floating alias, not a pin).
    if grep -qE 'dtolnay/rust-toolchain@stable' "$WORKFLOW"; then
        fail "$WORKFLOW still uses dtolnay/rust-toolchain@stable somewhere (floating, not pinned); every Rust job must use @master with an explicit toolchain: input read from core-rs/rust-toolchain.toml"
    fi

    # Every dtolnay/rust-toolchain@master use must be paired with a
    # steps.toolchain.outputs.channel input, not a hardcoded literal version
    # (which would itself be a second source of truth).
    MASTER_USES=$(grep -c 'dtolnay/rust-toolchain@master' "$WORKFLOW" || true)
    CHANNEL_REFS=$(grep -c 'steps.toolchain.outputs.channel' "$WORKFLOW" || true)
    if [ "$MASTER_USES" -eq 0 ]; then
        fail "$WORKFLOW has no dtolnay/rust-toolchain@master job -- toolchain pinning step may have been removed"
    elif [ "$CHANNEL_REFS" -eq 0 ]; then
        fail "$WORKFLOW uses dtolnay/rust-toolchain@master but never references steps.toolchain.outputs.channel -- toolchain may be hardcoded instead of read from core-rs/rust-toolchain.toml"
    fi

    # Every "Read canonical Rust toolchain channel" step must parse the real
    # file, not a hardcoded value.
    if grep -q 'Read canonical Rust toolchain channel' "$WORKFLOW"; then
        if ! grep -q "core-rs/rust-toolchain.toml" "$WORKFLOW" && ! grep -q 'core-rs\\rust-toolchain.toml' "$WORKFLOW"; then
            fail "$WORKFLOW has a toolchain-read step but it does not reference core-rs/rust-toolchain.toml"
        fi
    else
        fail "$WORKFLOW has no 'Read canonical Rust toolchain channel' step"
    fi

    # No job may hard-code the channel string as a literal toolchain: value
    # (that would be a second, independently-driftable source of truth).
    if grep -qE "toolchain:\s*[\"']?${CANONICAL_CHANNEL}[\"']?\s*$" "$WORKFLOW" 2>/dev/null; then
        fail "$WORKFLOW appears to hard-code toolchain: $CANONICAL_CHANNEL literally instead of \${{ steps.toolchain.outputs.channel }}"
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
    echo "       and scripts/windows/Invoke-CanonicalFmtCheck.ps1; no drift found."
    exit 0
else
    echo " FAIL -- $violations convergence violation(s) found above."
    exit 1
fi
