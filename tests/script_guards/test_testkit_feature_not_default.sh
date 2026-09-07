#!/usr/bin/env bash
# =============================================================================
# Mutation-negative tests for scripts/guards/check_testkit_feature_not_default.sh
#
# CI-TESTKIT-FEATURE-GUARD-VERIFY-01: proves the testkit-feature guard actually
# catches a forbidden release-feature configuration, not merely that it exits
# 0 on a clean repo. Each mutation test builds an isolated fake repo skeleton
# (mirroring the real repo's core-rs/crates/<name>/Cargo.toml layout)
# containing a *copy* of the guard script plus deliberately mutated Cargo.toml
# fixtures, then asserts the guard exits non-zero against that mutation. The
# real repo tree is never mutated.
#
# Usage: bash tests/script_guards/test_testkit_feature_not_default.sh
# Exit codes: 0 = all pass, 1 = one or more failures.
# =============================================================================
set -u

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"

GUARD_SRC="$REPO_ROOT/scripts/guards/check_testkit_feature_not_default.sh"

FAILURES=0

pass() { echo "  PASS  [$1] $2"; }
fail() { echo "  FAIL  [$1] $2"; FAILURES=$((FAILURES + 1)); }

echo ""
echo "=== check_testkit_feature_not_default.sh mutation-negative tests ==="
echo "    Guard: $GUARD_SRC"
echo ""

if [ ! -f "$GUARD_SRC" ]; then
    fail "TKG00" "Guard script not found at $GUARD_SRC"
    echo ""
    echo "=== $FAILURES INVARIANT(S) FAILED ==="
    exit 1
fi
pass "TKG00" "Guard script exists"

# Builds an isolated fake repo skeleton at $1 with a copy of the guard script
# and one well-formed crate Cargo.toml a caller can then mutate.
build_fake_repo() {
    local root="$1"
    mkdir -p "$root/scripts/guards" "$root/core-rs/crates/fake-crate"
    cp "$GUARD_SRC" "$root/scripts/guards/check_testkit_feature_not_default.sh"
    cat > "$root/core-rs/crates/fake-crate/Cargo.toml" <<'EOF'
[package]
name = "fake-crate"
version = "0.0.1"
edition = "2021"

[features]
testkit = ["dep:mqk-testkit"]

[dependencies]
serde = { workspace = true }
mqk-db = { path = "../mqk-db" }

[dev-dependencies]
mqk-db = { path = "../mqk-db", features = ["testkit"] }
EOF
}

run_guard() {
    local root="$1"
    bash "$root/scripts/guards/check_testkit_feature_not_default.sh" >"$root/guard_output.txt" 2>&1
    echo $?
}

TMP_ROOT="$(mktemp -d)"
trap 'rm -rf "$TMP_ROOT"' EXIT

# ---------------------------------------------------------------------------
# TKG01: positive baseline -- the guard passes on an unmutated fixture
# (testkit only reachable via [dev-dependencies], never default, never in
# [dependencies]).
# ---------------------------------------------------------------------------
BASELINE="$TMP_ROOT/baseline"
build_fake_repo "$BASELINE"
exit_code="$(run_guard "$BASELINE")"
if [ "$exit_code" -eq 0 ]; then
    pass "TKG01" "Guard exits 0 on an unmutated, correctly-scoped fixture"
else
    fail "TKG01" "Guard exits $exit_code on an unmutated fixture (expected 0); see $BASELINE/guard_output.txt"
fi

# ---------------------------------------------------------------------------
# TKG02: testkit added to a crate's default feature list must be caught.
# ---------------------------------------------------------------------------
DEFAULT_MUT="$TMP_ROOT/default_mut"
build_fake_repo "$DEFAULT_MUT"
sed -i 's/^testkit = \["dep:mqk-testkit"\]$/testkit = ["dep:mqk-testkit"]\ndefault = ["testkit"]/' \
    "$DEFAULT_MUT/core-rs/crates/fake-crate/Cargo.toml"
exit_code="$(run_guard "$DEFAULT_MUT")"
if [ "$exit_code" -ne 0 ] && grep -q 'default list includes "testkit"' "$DEFAULT_MUT/guard_output.txt"; then
    pass "TKG02" "Guard catches testkit added to a crate's [features] default list"
else
    fail "TKG02" "Guard did NOT catch testkit in a default feature list (exit=$exit_code); see $DEFAULT_MUT/guard_output.txt"
fi

# ---------------------------------------------------------------------------
# TKG03: testkit moved from [dev-dependencies] into production
# [dependencies] must be caught.
# ---------------------------------------------------------------------------
DEP_MUT="$TMP_ROOT/dep_mut"
build_fake_repo "$DEP_MUT"
sed -i 's/^mqk-db = { path = "..\/mqk-db" }$/mqk-db = { path = "..\/mqk-db", features = ["testkit"] }/' \
    "$DEP_MUT/core-rs/crates/fake-crate/Cargo.toml"
exit_code="$(run_guard "$DEP_MUT")"
if [ "$exit_code" -ne 0 ] && grep -q 'production \[dependencies\] section enables' "$DEP_MUT/guard_output.txt"; then
    pass "TKG03" "Guard catches testkit enabled in a crate's production [dependencies] section"
else
    fail "TKG03" "Guard did NOT catch testkit in production [dependencies] (exit=$exit_code); see $DEP_MUT/guard_output.txt"
fi

# ---------------------------------------------------------------------------
# TKG04: both mutations combined must report both violations, not just the
# first one found (proves the guard does not short-circuit).
# ---------------------------------------------------------------------------
BOTH_MUT="$TMP_ROOT/both_mut"
build_fake_repo "$BOTH_MUT"
sed -i 's/^testkit = \["dep:mqk-testkit"\]$/testkit = ["dep:mqk-testkit"]\ndefault = ["testkit"]/' \
    "$BOTH_MUT/core-rs/crates/fake-crate/Cargo.toml"
sed -i 's/^mqk-db = { path = "..\/mqk-db" }$/mqk-db = { path = "..\/mqk-db", features = ["testkit"] }/' \
    "$BOTH_MUT/core-rs/crates/fake-crate/Cargo.toml"
exit_code="$(run_guard "$BOTH_MUT")"
if [ "$exit_code" -ne 0 ] \
    && grep -q 'default list includes "testkit"' "$BOTH_MUT/guard_output.txt" \
    && grep -q 'production \[dependencies\] section enables' "$BOTH_MUT/guard_output.txt"; then
    pass "TKG04" "Guard reports both violations when both mutations are present simultaneously"
else
    fail "TKG04" "Guard did NOT report both violations (exit=$exit_code); see $BOTH_MUT/guard_output.txt"
fi

# ---------------------------------------------------------------------------
# TKG05: a dev-dependency-only testkit activation (the correct, accepted
# pattern) must NOT be flagged by the production-dependency gate -- proves
# the guard discriminates [dependencies] from [dev-dependencies] rather than
# matching "testkit" anywhere in the file.
# ---------------------------------------------------------------------------
DEV_ONLY="$TMP_ROOT/dev_only"
build_fake_repo "$DEV_ONLY"
exit_code="$(run_guard "$DEV_ONLY")"
if [ "$exit_code" -eq 0 ] && ! grep -q 'FAIL' "$DEV_ONLY/guard_output.txt"; then
    pass "TKG05" "Guard does not false-positive on the correct dev-dependency-only testkit pattern"
else
    fail "TKG05" "Guard false-positived on a correctly-scoped dev-dependency testkit activation (exit=$exit_code); see $DEV_ONLY/guard_output.txt"
fi

echo ""
if [ "$FAILURES" -eq 0 ]; then
    echo "=== ALL TKG INVARIANTS PASSED ==="
    exit 0
else
    echo "=== $FAILURES INVARIANT(S) FAILED ==="
    exit 1
fi
