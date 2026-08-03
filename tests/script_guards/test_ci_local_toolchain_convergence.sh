#!/usr/bin/env bash
# =============================================================================
# Mutation-negative tests for scripts/guards/check_ci_local_toolchain_convergence.sh
#
# FULL-AUDIT-SAFE-IGNORED-AND-SHARED-DB-FINAL-CLOSURE-01 Part 4: proves the
# convergence guard actually catches drift, not merely that it exits 0 on a
# clean repo. Each mutation test builds an isolated fake repo skeleton
# (mirroring the real repo's relative path layout: scripts/guards/,
# .github/workflows/, core-rs/, scripts/windows/) containing a *copy* of the
# guard script plus a deliberately broken copy of one real source file, then
# asserts the guard exits non-zero against that broken copy. The real repo
# tree is never mutated.
#
# Usage: bash tests/script_guards/test_ci_local_toolchain_convergence.sh
# Exit codes: 0 = all pass, 1 = one or more failures.
# =============================================================================
set -u

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"

GUARD_SRC="$REPO_ROOT/scripts/guards/check_ci_local_toolchain_convergence.sh"
WORKFLOW_SRC="$REPO_ROOT/.github/workflows/ci.yml"
TOOLCHAIN_SRC="$REPO_ROOT/core-rs/rust-toolchain.toml"
FMT_SCRIPT_SRC="$REPO_ROOT/scripts/windows/Invoke-CanonicalFmtCheck.ps1"
LOCAL_PROOF_SRC="$REPO_ROOT/full_repo_proof.ps1"

FAILURES=0

pass() { echo "  PASS  [$1] $2"; }
fail() { echo "  FAIL  [$1] $2"; FAILURES=$((FAILURES + 1)); }

echo ""
echo "=== check_ci_local_toolchain_convergence.sh mutation-negative tests ==="
echo "    Guard: $GUARD_SRC"
echo ""

if [ ! -f "$GUARD_SRC" ]; then
    fail "TCG00" "Guard script not found at $GUARD_SRC"
    echo ""
    echo "=== $FAILURES INVARIANT(S) FAILED ==="
    exit 1
fi
pass "TCG00" "Guard script exists"

# Builds an isolated fake repo skeleton at $1 with unmodified copies of every
# file the guard reads, so a caller can then mutate exactly one of them.
build_fake_repo() {
    local root="$1"
    mkdir -p "$root/scripts/guards" "$root/scripts/windows" "$root/.github/workflows" "$root/core-rs"
    cp "$GUARD_SRC" "$root/scripts/guards/check_ci_local_toolchain_convergence.sh"
    cp "$WORKFLOW_SRC" "$root/.github/workflows/ci.yml"
    cp "$TOOLCHAIN_SRC" "$root/core-rs/rust-toolchain.toml"
    cp "$FMT_SCRIPT_SRC" "$root/scripts/windows/Invoke-CanonicalFmtCheck.ps1"
    cp "$LOCAL_PROOF_SRC" "$root/full_repo_proof.ps1"
}

run_guard() {
    local root="$1"
    bash "$root/scripts/guards/check_ci_local_toolchain_convergence.sh" >"$root/guard_output.txt" 2>&1
    echo $?
}

TMP_ROOT="$(mktemp -d)"
trap 'rm -rf "$TMP_ROOT"' EXIT

# ---------------------------------------------------------------------------
# TCG01: positive baseline -- the guard passes on an unmutated copy.
# ---------------------------------------------------------------------------
BASELINE="$TMP_ROOT/baseline"
build_fake_repo "$BASELINE"
exit_code="$(run_guard "$BASELINE")"
if [ "$exit_code" -eq 0 ]; then
    pass "TCG01" "Guard exits 0 on an unmutated copy of the real repo files"
else
    fail "TCG01" "Guard exits $exit_code on an unmutated copy (expected 0); see $BASELINE/guard_output.txt"
fi

# ---------------------------------------------------------------------------
# TCG02: one job using a different channel (hardcoded literal instead of
# the dynamic step output) must be caught.
# ---------------------------------------------------------------------------
CHANNEL_MUT="$TMP_ROOT/channel_mut"
build_fake_repo "$CHANNEL_MUT"
perl -0pi -e 's/rustup toolchain install "\$\{\{ steps\.toolchain\.outputs\.channel \}\}"/rustup toolchain install "1.70.0"/' \
    "$CHANNEL_MUT/.github/workflows/ci.yml" 2>/dev/null || \
sed -i 's/rustup toolchain install "\${{ steps.toolchain.outputs.channel }}"/rustup toolchain install "1.70.0"/' \
    "$CHANNEL_MUT/.github/workflows/ci.yml"
exit_code="$(run_guard "$CHANNEL_MUT")"
if [ "$exit_code" -ne 0 ] && grep -q 'steps.toolchain.outputs.channel' "$CHANNEL_MUT/guard_output.txt"; then
    pass "TCG02" "Guard catches a job hardcoding a different channel instead of steps.toolchain.outputs.channel"
else
    fail "TCG02" "Guard did NOT catch a hardcoded-channel mutation (exit=$exit_code); see $CHANNEL_MUT/guard_output.txt"
fi

# ---------------------------------------------------------------------------
# TCG03: one job omitting the clippy component must be caught.
# ---------------------------------------------------------------------------
CLIPPY_MUT="$TMP_ROOT/clippy_mut"
build_fake_repo "$CLIPPY_MUT"
# Remove the clippy component from exactly the first occurrence (the `rust`
# job) while leaving db-proof/windows untouched, proving per-job isolation.
awk '
    BEGIN { done = 0 }
    !done && /--component clippy/ {
        sub(/ --component clippy/, "")
        done = 1
    }
    { print }
' "$CLIPPY_MUT/.github/workflows/ci.yml" > "$CLIPPY_MUT/.github/workflows/ci.yml.tmp"
mv "$CLIPPY_MUT/.github/workflows/ci.yml.tmp" "$CLIPPY_MUT/.github/workflows/ci.yml"
exit_code="$(run_guard "$CLIPPY_MUT")"
if [ "$exit_code" -ne 0 ] && grep -q 'clippy component' "$CLIPPY_MUT/guard_output.txt"; then
    pass "TCG03" "Guard catches a job omitting the clippy component"
else
    fail "TCG03" "Guard did NOT catch a missing-clippy-component mutation (exit=$exit_code); see $CLIPPY_MUT/guard_output.txt"
fi

# ---------------------------------------------------------------------------
# TCG04: one job reverting to a floating third-party toolchain action must
# be caught.
# ---------------------------------------------------------------------------
FLOATING_MUT="$TMP_ROOT/floating_mut"
build_fake_repo "$FLOATING_MUT"
awk '
    BEGIN { done = 0 }
    !done && /rustup toolchain install/ {
        print "      - name: Install pinned Rust toolchain (REGRESSED to floating action)"
        print "        uses: dtolnay/rust-toolchain@master"
        print "        with:"
        print "          toolchain: ${{ steps.toolchain.outputs.channel }}"
        print "          components: rustfmt, clippy"
        done = 1
        next
    }
    !done { print; next }
    done && /rustup default/ { done_default = 1; next }
    done && done_default { done_default = 0; next }
    { print }
' "$FLOATING_MUT/.github/workflows/ci.yml" > "$FLOATING_MUT/.github/workflows/ci.yml.tmp"
mv "$FLOATING_MUT/.github/workflows/ci.yml.tmp" "$FLOATING_MUT/.github/workflows/ci.yml"
exit_code="$(run_guard "$FLOATING_MUT")"
if [ "$exit_code" -ne 0 ] && grep -qi 'floating' "$FLOATING_MUT/guard_output.txt"; then
    pass "TCG04" "Guard catches a job reverting to a floating dtolnay/rust-toolchain@master action"
else
    fail "TCG04" "Guard did NOT catch a floating-action regression (exit=$exit_code); see $FLOATING_MUT/guard_output.txt"
fi

# ---------------------------------------------------------------------------
# TCG05: Windows job reverting to a hard-coded powershell.exe must be caught.
# ---------------------------------------------------------------------------
PWSH_MUT="$TMP_ROOT/pwsh_mut"
build_fake_repo "$PWSH_MUT"
sed -i "s#\$selfHost = Join-Path \$PSHOME 'pwsh.exe'#\$selfHost = 'powershell.exe'#" \
    "$PWSH_MUT/.github/workflows/ci.yml"
exit_code="$(run_guard "$PWSH_MUT")"
if [ "$exit_code" -ne 0 ] && grep -qi 'powershell.exe' "$PWSH_MUT/guard_output.txt"; then
    pass "TCG05" "Guard catches the Windows job reverting to a hard-coded powershell.exe"
else
    fail "TCG05" "Guard did NOT catch a hard-coded powershell.exe regression (exit=$exit_code); see $PWSH_MUT/guard_output.txt"
fi

# ---------------------------------------------------------------------------
# TCG06: Windows/local format commands diverging (workflow stops delegating
# to the canonical fmt-check script) must be caught.
# ---------------------------------------------------------------------------
FMT_MUT="$TMP_ROOT/fmt_mut"
build_fake_repo "$FMT_MUT"
sed -i 's#& scripts\\windows\\Invoke-CanonicalFmtCheck\.ps1 -RepoRoot (Get-Location) -CargoManifest core-rs\\Cargo\.toml#cargo fmt --check#' \
    "$FMT_MUT/.github/workflows/ci.yml"
exit_code="$(run_guard "$FMT_MUT")"
if [ "$exit_code" -ne 0 ] && grep -qi 'Invoke-CanonicalFmtCheck' "$FMT_MUT/guard_output.txt"; then
    pass "TCG06" "Guard catches the Windows job diverging from the canonical fmt-check script"
else
    fail "TCG06" "Guard did NOT catch a diverging format-check mutation (exit=$exit_code); see $FMT_MUT/guard_output.txt"
fi

echo ""
if [ "$FAILURES" -eq 0 ]; then
    echo "=== ALL TCG INVARIANTS PASSED ==="
    exit 0
else
    echo "=== $FAILURES INVARIANT(S) FAILED ==="
    exit 1
fi
