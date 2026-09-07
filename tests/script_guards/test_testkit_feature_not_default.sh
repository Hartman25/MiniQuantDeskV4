#!/usr/bin/env bash
# =============================================================================
# Mutation-negative tests for scripts/guards/check_testkit_feature_not_default.{sh,py}
#
# CI-TESTKIT-FEATURE-GUARD-VERIFY-01-REPAIR-01: proves the testkit-feature
# guard is Cargo/TOML-structural, not a grep/awk approximation -- i.e. it
# actually catches every required release-safety bypass route, including
# ones a line-oriented grep/awk implementation silently misses (multiline
# arrays, dotted/target-specific dependency tables, workspace-inherited
# dependency features, feature-alias chains).
#
# Each mutation test builds an isolated fake repo skeleton (mirroring the
# real repo's core-rs/Cargo.toml + core-rs/crates/<name>/Cargo.toml layout)
# with a *copy* of the real guard entrypoint (.sh) and its real
# implementation (.py), then invokes that copy exactly as CI does
# (`bash .../check_testkit_feature_not_default.sh`). No test reimplements
# or bypasses the guard's own logic -- every proof below invokes the real
# production guard. The real repo tree is never mutated.
#
# Required cases (per CI-TESTKIT-FEATURE-GUARD-VERIFY-01-REPAIR-01):
#   TKG-A  direct default testkit                          -> FAIL
#   TKG-B  multiline default testkit                        -> FAIL
#   TKG-C  ordinary dependency testkit                       -> FAIL
#   TKG-D  target-specific production dependency testkit    -> FAIL
#   TKG-E  workspace-inherited dependency route              -> FAIL
#   TKG-F  feature alias/default chain                       -> FAIL
#   TKG-G  dev-dependency-only testkit                        -> PASS
#   TKG-H  current real repo                                  -> PASS
#
# Required cases (per CI-TESTKIT-FEATURE-GUARD-VERIFY-01-REPAIR-02): a real
# TWO-crate cross-crate feature graph -- a source crate ("fake-crate")
# depending on a target crate ("fake-dep") whose OWN [features] table is what
# actually reaches testkit, which a guard comparing only literal requested
# feature strings against "testkit" cannot see.
#   TKG-R2-01  dep features=["full"], target full=["testkit"]              -> FAIL
#   TKG-R2-02  dep features=["full"], target full=["testing"]->testkit      -> FAIL
#   TKG-R2-03  source default activates "dep/full" -> target testkit        -> FAIL
#   TKG-R2-04  target-specific production dependency alias -> testkit       -> FAIL
#   TKG-R2-05  workspace-inherited dependency feature alias -> testkit      -> FAIL
#   TKG-R2-06  renamed local dependency alias -> target alias -> testkit    -> FAIL
#   TKG-R2-07  dev-dependency features=["full"] -> target testkit           -> PASS
#   TKG-R2-08  current real repo                                            -> PASS
#
# Usage: bash tests/script_guards/test_testkit_feature_not_default.sh
# Exit codes: 0 = all pass, 1 = one or more failures.
# =============================================================================
set -u

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"

GUARD_SH="$REPO_ROOT/scripts/guards/check_testkit_feature_not_default.sh"
GUARD_PY="$REPO_ROOT/scripts/guards/check_testkit_feature_not_default.py"

FAILURES=0

pass() { echo "  PASS  [$1] $2"; }
fail() { echo "  FAIL  [$1] $2"; FAILURES=$((FAILURES + 1)); }

echo ""
echo "=== check_testkit_feature_not_default.{sh,py} mutation-negative tests ==="
echo "    Guard: $GUARD_SH"
echo ""

if [ ! -f "$GUARD_SH" ] || [ ! -f "$GUARD_PY" ]; then
    fail "TKG00" "Guard entrypoint/implementation not found ($GUARD_SH / $GUARD_PY)"
    echo ""
    echo "=== $FAILURES INVARIANT(S) FAILED ==="
    exit 1
fi
pass "TKG00" "Guard entrypoint and implementation exist"

TMP_ROOT="$(mktemp -d)"
trap 'rm -rf "$TMP_ROOT"' EXIT

# Builds an isolated fake repo skeleton at $1: copies the REAL guard
# entrypoint + implementation (never reimplemented), then writes the given
# workspace-root and crate Cargo.toml contents (read from stdin-fed files
# $2 and $3).
build_repo() {
    local root="$1" ws_toml_file="$2" crate_toml_file="$3"
    mkdir -p "$root/scripts/guards" "$root/core-rs/crates/fake-crate"
    cp "$GUARD_SH" "$root/scripts/guards/check_testkit_feature_not_default.sh"
    cp "$GUARD_PY" "$root/scripts/guards/check_testkit_feature_not_default.py"
    cp "$ws_toml_file" "$root/core-rs/Cargo.toml"
    cp "$crate_toml_file" "$root/core-rs/crates/fake-crate/Cargo.toml"
}

run_guard() {
    local root="$1"
    bash "$root/scripts/guards/check_testkit_feature_not_default.sh" >"$root/guard_output.txt" 2>&1
    echo $?
}

WS_BASE="$TMP_ROOT/ws_base.toml"
cat > "$WS_BASE" <<'EOF'
[workspace]
resolver = "2"
members = ["crates/fake-crate"]

[workspace.dependencies]
serde = { version = "1" }
mqk-db = { path = "../mqk-db" }
mqk-execution = { path = "../mqk-execution" }
EOF

# ---------------------------------------------------------------------------
# TKG-BASELINE: positive baseline -- the guard passes on an unmutated
# fixture matching the real repo's accepted pattern (testkit only reachable
# via [dev-dependencies], never default, never in production [dependencies]
# or via workspace inheritance).
# ---------------------------------------------------------------------------
CRATE_BASELINE="$TMP_ROOT/crate_baseline.toml"
cat > "$CRATE_BASELINE" <<'EOF'
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
BASELINE="$TMP_ROOT/baseline"
build_repo "$BASELINE" "$WS_BASE" "$CRATE_BASELINE"
exit_code="$(run_guard "$BASELINE")"
if [ "$exit_code" -eq 0 ]; then
    pass "TKG-BASELINE" "Guard exits 0 on an unmutated, correctly-scoped fixture"
else
    fail "TKG-BASELINE" "Guard exits $exit_code on an unmutated fixture (expected 0); see $BASELINE/guard_output.txt"
fi

# ---------------------------------------------------------------------------
# TKG-A: direct default testkit -> FAIL
# ---------------------------------------------------------------------------
CRATE_A="$TMP_ROOT/crate_a.toml"
cat > "$CRATE_A" <<'EOF'
[package]
name = "fake-crate"
version = "0.0.1"
edition = "2021"

[features]
default = ["testkit"]
testkit = ["dep:mqk-testkit"]

[dependencies]
serde = { workspace = true }
mqk-db = { path = "../mqk-db" }

[dev-dependencies]
mqk-db = { path = "../mqk-db", features = ["testkit"] }
EOF
ROOT_A="$TMP_ROOT/tkg_a"
build_repo "$ROOT_A" "$WS_BASE" "$CRATE_A"
exit_code="$(run_guard "$ROOT_A")"
if [ "$exit_code" -ne 0 ] && grep -q 'default transitively activates "testkit"' "$ROOT_A/guard_output.txt"; then
    pass "TKG-A" "Guard catches testkit listed directly in [features] default"
else
    fail "TKG-A" "Guard did NOT catch direct default testkit (exit=$exit_code); see $ROOT_A/guard_output.txt"
fi

# ---------------------------------------------------------------------------
# TKG-B: multiline default testkit -> FAIL
# (A grep-only guard matching only the "default = [" opening line misses
# this; tomllib parses the array regardless of formatting.)
# ---------------------------------------------------------------------------
CRATE_B="$TMP_ROOT/crate_b.toml"
cat > "$CRATE_B" <<'EOF'
[package]
name = "fake-crate"
version = "0.0.1"
edition = "2021"

[features]
testkit = ["dep:mqk-testkit"]
default = [
    "testkit",
]

[dependencies]
serde = { workspace = true }
mqk-db = { path = "../mqk-db" }

[dev-dependencies]
mqk-db = { path = "../mqk-db", features = ["testkit"] }
EOF
ROOT_B="$TMP_ROOT/tkg_b"
build_repo "$ROOT_B" "$WS_BASE" "$CRATE_B"
exit_code="$(run_guard "$ROOT_B")"
if [ "$exit_code" -ne 0 ] && grep -q 'default transitively activates "testkit"' "$ROOT_B/guard_output.txt"; then
    pass "TKG-B" "Guard catches testkit inside a multiline [features] default array"
else
    fail "TKG-B" "Guard did NOT catch multiline default testkit (exit=$exit_code); see $ROOT_B/guard_output.txt"
fi

# ---------------------------------------------------------------------------
# TKG-C: ordinary dependency testkit -> FAIL
# (testkit moved from [dev-dependencies] into production [dependencies])
# ---------------------------------------------------------------------------
CRATE_C="$TMP_ROOT/crate_c.toml"
cat > "$CRATE_C" <<'EOF'
[package]
name = "fake-crate"
version = "0.0.1"
edition = "2021"

[features]
testkit = ["dep:mqk-testkit"]

[dependencies]
serde = { workspace = true }
mqk-db = { path = "../mqk-db", features = ["testkit"] }

[dev-dependencies]
mqk-execution = { path = "../mqk-execution" }
EOF
ROOT_C="$TMP_ROOT/tkg_c"
build_repo "$ROOT_C" "$WS_BASE" "$CRATE_C"
exit_code="$(run_guard "$ROOT_C")"
if [ "$exit_code" -ne 0 ] && grep -q 'testkit feature in a production dependency table' "$ROOT_C/guard_output.txt"; then
    pass "TKG-C" "Guard catches testkit enabled in an ordinary production [dependencies] entry"
else
    fail "TKG-C" "Guard did NOT catch ordinary production dependency testkit (exit=$exit_code); see $ROOT_C/guard_output.txt"
fi

# ---------------------------------------------------------------------------
# TKG-D: target-specific production dependency testkit -> FAIL
# ([target.'cfg(windows)'.dependencies] is invisible to a guard that only
# recognizes a literal "[dependencies]" header.)
# ---------------------------------------------------------------------------
CRATE_D="$TMP_ROOT/crate_d.toml"
cat > "$CRATE_D" <<'EOF'
[package]
name = "fake-crate"
version = "0.0.1"
edition = "2021"

[features]
testkit = ["dep:mqk-testkit"]

[dependencies]
serde = { workspace = true }

[target.'cfg(windows)'.dependencies]
mqk-db = { path = "../mqk-db", features = ["testkit"] }

[dev-dependencies]
mqk-execution = { path = "../mqk-execution" }
EOF
ROOT_D="$TMP_ROOT/tkg_d"
build_repo "$ROOT_D" "$WS_BASE" "$CRATE_D"
exit_code="$(run_guard "$ROOT_D")"
if [ "$exit_code" -ne 0 ] && grep -q 'target\.cfg(windows)\.dependencies.*testkit feature' "$ROOT_D/guard_output.txt"; then
    pass "TKG-D" "Guard catches testkit enabled in a target-specific production dependency table"
else
    fail "TKG-D" "Guard did NOT catch target-specific production dependency testkit (exit=$exit_code); see $ROOT_D/guard_output.txt"
fi

# ---------------------------------------------------------------------------
# TKG-E: workspace-inherited dependency route -> FAIL
# (crate does `mqk-db.workspace = true` with no local override; the
# workspace root's own [workspace.dependencies] entry carries
# features = ["testkit"], so the crate inherits it silently.)
# ---------------------------------------------------------------------------
WS_E="$TMP_ROOT/ws_e.toml"
cat > "$WS_E" <<'EOF'
[workspace]
resolver = "2"
members = ["crates/fake-crate"]

[workspace.dependencies]
serde = { version = "1" }
mqk-db = { path = "../mqk-db", features = ["testkit"] }
mqk-execution = { path = "../mqk-execution" }
EOF
CRATE_E="$TMP_ROOT/crate_e.toml"
cat > "$CRATE_E" <<'EOF'
[package]
name = "fake-crate"
version = "0.0.1"
edition = "2021"

[features]
testkit = ["dep:mqk-testkit"]

[dependencies]
mqk-db = { workspace = true }

[dev-dependencies]
mqk-execution = { path = "../mqk-execution" }
EOF
ROOT_E="$TMP_ROOT/tkg_e"
build_repo "$ROOT_E" "$WS_E" "$CRATE_E"
exit_code="$(run_guard "$ROOT_E")"
if [ "$exit_code" -ne 0 ] && grep -q 'testkit feature in a production dependency table' "$ROOT_E/guard_output.txt"; then
    pass "TKG-E" "Guard catches testkit inherited from [workspace.dependencies] via workspace=true"
else
    fail "TKG-E" "Guard did NOT catch workspace-inherited testkit (exit=$exit_code); see $ROOT_E/guard_output.txt"
fi

# ---------------------------------------------------------------------------
# TKG-F: feature alias/default chain -> FAIL
# (default = ["full"], full = ["testkit"] -- testkit is never named in
# default itself, only reachable by expanding the alias chain.)
# ---------------------------------------------------------------------------
CRATE_F="$TMP_ROOT/crate_f.toml"
cat > "$CRATE_F" <<'EOF'
[package]
name = "fake-crate"
version = "0.0.1"
edition = "2021"

[features]
testkit = ["dep:mqk-testkit"]
full = ["testkit"]
default = ["full"]

[dependencies]
serde = { workspace = true }

[dev-dependencies]
mqk-execution = { path = "../mqk-execution" }
EOF
ROOT_F="$TMP_ROOT/tkg_f"
build_repo "$ROOT_F" "$WS_BASE" "$CRATE_F"
exit_code="$(run_guard "$ROOT_F")"
if [ "$exit_code" -ne 0 ] && grep -q 'default transitively activates "testkit"' "$ROOT_F/guard_output.txt"; then
    pass "TKG-F" "Guard catches testkit reachable only through a feature-alias default chain"
else
    fail "TKG-F" "Guard did NOT catch the feature-alias default chain (exit=$exit_code); see $ROOT_F/guard_output.txt"
fi

# ---------------------------------------------------------------------------
# TKG-G: dev-dependency-only testkit -> PASS
# (the correct, accepted pattern must not be flagged -- proves the guard
# discriminates [dependencies] from [dev-dependencies] rather than matching
# "testkit" anywhere in the file.)
# ---------------------------------------------------------------------------
ROOT_G="$TMP_ROOT/tkg_g"
build_repo "$ROOT_G" "$WS_BASE" "$CRATE_BASELINE"
exit_code="$(run_guard "$ROOT_G")"
if [ "$exit_code" -eq 0 ] && ! grep -q 'FAIL' "$ROOT_G/guard_output.txt"; then
    pass "TKG-G" "Guard does not false-positive on the correct dev-dependency-only testkit pattern"
else
    fail "TKG-G" "Guard false-positived on a correctly-scoped dev-dependency testkit activation (exit=$exit_code); see $ROOT_G/guard_output.txt"
fi

# ---------------------------------------------------------------------------
# TKG-H: current real repo -> PASS
# (runs the real guard directly against REPO_ROOT, no fixture -- proves no
# false positive against the actual production Cargo.toml graph.)
# ---------------------------------------------------------------------------
real_output="$(bash "$GUARD_SH" 2>&1)"
real_exit=$?
if [ "$real_exit" -eq 0 ]; then
    pass "TKG-H" "Guard exits 0 against the current real repo"
else
    fail "TKG-H" "Guard exits $real_exit against the current real repo (expected 0): $real_output"
fi

# ---------------------------------------------------------------------------
# TKG-COMBINED: multiple simultaneous violations across different routes
# must ALL be reported, not just the first one found (proves the guard does
# not short-circuit).
# ---------------------------------------------------------------------------
CRATE_COMBINED="$TMP_ROOT/crate_combined.toml"
cat > "$CRATE_COMBINED" <<'EOF'
[package]
name = "fake-crate"
version = "0.0.1"
edition = "2021"

[features]
default = ["testkit"]
testkit = ["dep:mqk-testkit"]

[dependencies]
serde = { workspace = true }
mqk-db = { path = "../mqk-db", features = ["testkit"] }

[target.'cfg(windows)'.dependencies]
mqk-execution = { path = "../mqk-execution", features = ["testkit"] }
EOF
ROOT_COMBINED="$TMP_ROOT/tkg_combined"
build_repo "$ROOT_COMBINED" "$WS_BASE" "$CRATE_COMBINED"
exit_code="$(run_guard "$ROOT_COMBINED")"
if [ "$exit_code" -ne 0 ] \
    && grep -q 'default transitively activates "testkit"' "$ROOT_COMBINED/guard_output.txt" \
    && grep -c 'testkit feature in a production dependency table' "$ROOT_COMBINED/guard_output.txt" | grep -q '^2$'; then
    pass "TKG-COMBINED" "Guard reports all violations when default + [dependencies] + target-specific routes are all mutated at once"
else
    fail "TKG-COMBINED" "Guard did NOT report all simultaneous violations (exit=$exit_code); see $ROOT_COMBINED/guard_output.txt"
fi

# ---------------------------------------------------------------------------
# TKG-R2: real TWO-crate cross-crate feature graph cases (REPAIR-02).
# build_repo2 lays down a source crate ("fake-crate") AND a real target
# crate ("fake-dep", or a caller-chosen dir name for the rename case) with
# its own [features] table, so testkit is reachable only by resolving the
# TARGET crate's feature graph -- never by literal string matching on the
# source's requested feature name.
# ---------------------------------------------------------------------------
build_repo2() {
    local root="$1" ws_toml_file="$2" source_toml_file="$3" dep_toml_file="$4" dep_dir="${5:-fake-dep}"
    mkdir -p "$root/scripts/guards" "$root/core-rs/crates/fake-crate" "$root/core-rs/crates/$dep_dir"
    cp "$GUARD_SH" "$root/scripts/guards/check_testkit_feature_not_default.sh"
    cp "$GUARD_PY" "$root/scripts/guards/check_testkit_feature_not_default.py"
    cp "$ws_toml_file" "$root/core-rs/Cargo.toml"
    cp "$source_toml_file" "$root/core-rs/crates/fake-crate/Cargo.toml"
    cp "$dep_toml_file" "$root/core-rs/crates/$dep_dir/Cargo.toml"
}

WS_R2_BASE="$TMP_ROOT/ws_r2_base.toml"
cat > "$WS_R2_BASE" <<'EOF'
[workspace]
resolver = "2"
members = ["crates/fake-crate", "crates/fake-dep"]

[workspace.dependencies]
serde = { version = "1" }
fake-dep = { path = "../fake-dep" }
EOF

DEP_FULL_DIRECT="$TMP_ROOT/dep_full_direct.toml"
cat > "$DEP_FULL_DIRECT" <<'EOF'
[package]
name = "fake-dep"
version = "0.0.1"
edition = "2021"

[features]
testkit = []
full = ["testkit"]
EOF

DEP_FULL_MULTILEVEL="$TMP_ROOT/dep_full_multilevel.toml"
cat > "$DEP_FULL_MULTILEVEL" <<'EOF'
[package]
name = "fake-dep"
version = "0.0.1"
edition = "2021"

[features]
testkit = []
testing = ["testkit"]
full = ["testing"]
EOF

# TKG-R2-01: dependency features=["full"], target full=["testkit"] -> FAIL
SRC_R2_01="$TMP_ROOT/src_r2_01.toml"
cat > "$SRC_R2_01" <<'EOF'
[package]
name = "fake-crate"
version = "0.0.1"
edition = "2021"

[dependencies]
serde = { workspace = true }
fake-dep = { path = "../fake-dep", features = ["full"] }
EOF
ROOT_R2_01="$TMP_ROOT/tkg_r2_01"
build_repo2 "$ROOT_R2_01" "$WS_R2_BASE" "$SRC_R2_01" "$DEP_FULL_DIRECT"
exit_code="$(run_guard "$ROOT_R2_01")"
if [ "$exit_code" -ne 0 ] && grep -q 'testkit feature in a production dependency table' "$ROOT_R2_01/guard_output.txt"; then
    pass "TKG-R2-01" "Guard resolves a cross-crate feature alias (full -> testkit) in the target crate's own feature table"
else
    fail "TKG-R2-01" "Guard did NOT catch cross-crate alias full->testkit (exit=$exit_code); see $ROOT_R2_01/guard_output.txt"
fi

# TKG-R2-02: dependency features=["full"], target full=["testing"], testing=["testkit"] -> FAIL
ROOT_R2_02="$TMP_ROOT/tkg_r2_02"
build_repo2 "$ROOT_R2_02" "$WS_R2_BASE" "$SRC_R2_01" "$DEP_FULL_MULTILEVEL"
exit_code="$(run_guard "$ROOT_R2_02")"
if [ "$exit_code" -ne 0 ] && grep -q 'testkit feature in a production dependency table' "$ROOT_R2_02/guard_output.txt"; then
    pass "TKG-R2-02" "Guard resolves a multi-level cross-crate alias chain (full -> testing -> testkit)"
else
    fail "TKG-R2-02" "Guard did NOT catch multi-level cross-crate alias chain (exit=$exit_code); see $ROOT_R2_02/guard_output.txt"
fi

# TKG-R2-03: source default activates "fake-dep/full" cross-crate edge -> FAIL
SRC_R2_03="$TMP_ROOT/src_r2_03.toml"
cat > "$SRC_R2_03" <<'EOF'
[package]
name = "fake-crate"
version = "0.0.1"
edition = "2021"

[features]
default = ["fake-dep/full"]

[dependencies]
serde = { workspace = true }
fake-dep = { path = "../fake-dep" }
EOF
ROOT_R2_03="$TMP_ROOT/tkg_r2_03"
build_repo2 "$ROOT_R2_03" "$WS_R2_BASE" "$SRC_R2_03" "$DEP_FULL_DIRECT"
exit_code="$(run_guard "$ROOT_R2_03")"
if [ "$exit_code" -ne 0 ] && grep -q 'default transitively activates "testkit"' "$ROOT_R2_03/guard_output.txt"; then
    pass "TKG-R2-03" "Guard resolves source default -> \"dep/feature\" cross-crate edge -> target testkit"
else
    fail "TKG-R2-03" "Guard did NOT catch source default cross-crate edge to testkit (exit=$exit_code); see $ROOT_R2_03/guard_output.txt"
fi

# TKG-R2-04: target-specific production dependency alias -> testkit -> FAIL
SRC_R2_04="$TMP_ROOT/src_r2_04.toml"
cat > "$SRC_R2_04" <<'EOF'
[package]
name = "fake-crate"
version = "0.0.1"
edition = "2021"

[dependencies]
serde = { workspace = true }

[target.'cfg(windows)'.dependencies]
fake-dep = { path = "../fake-dep", features = ["full"] }
EOF
ROOT_R2_04="$TMP_ROOT/tkg_r2_04"
build_repo2 "$ROOT_R2_04" "$WS_R2_BASE" "$SRC_R2_04" "$DEP_FULL_DIRECT"
exit_code="$(run_guard "$ROOT_R2_04")"
if [ "$exit_code" -ne 0 ] && grep -q 'target\.cfg(windows)\.dependencies.*testkit feature' "$ROOT_R2_04/guard_output.txt"; then
    pass "TKG-R2-04" "Guard resolves a cross-crate alias on a target-specific production dependency"
else
    fail "TKG-R2-04" "Guard did NOT catch target-specific cross-crate alias (exit=$exit_code); see $ROOT_R2_04/guard_output.txt"
fi

# TKG-R2-05: workspace-inherited dependency feature alias -> testkit -> FAIL
WS_R2_05="$TMP_ROOT/ws_r2_05.toml"
cat > "$WS_R2_05" <<'EOF'
[workspace]
resolver = "2"
members = ["crates/fake-crate", "crates/fake-dep"]

[workspace.dependencies]
serde = { version = "1" }
fake-dep = { path = "../fake-dep", features = ["full"] }
EOF
SRC_R2_05="$TMP_ROOT/src_r2_05.toml"
cat > "$SRC_R2_05" <<'EOF'
[package]
name = "fake-crate"
version = "0.0.1"
edition = "2021"

[dependencies]
fake-dep = { workspace = true }
EOF
ROOT_R2_05="$TMP_ROOT/tkg_r2_05"
build_repo2 "$ROOT_R2_05" "$WS_R2_05" "$SRC_R2_05" "$DEP_FULL_DIRECT"
exit_code="$(run_guard "$ROOT_R2_05")"
if [ "$exit_code" -ne 0 ] && grep -q 'testkit feature in a production dependency table' "$ROOT_R2_05/guard_output.txt"; then
    pass "TKG-R2-05" "Guard resolves a workspace-inherited dependency's feature alias to testkit"
else
    fail "TKG-R2-05" "Guard did NOT catch workspace-inherited cross-crate alias (exit=$exit_code); see $ROOT_R2_05/guard_output.txt"
fi

# TKG-R2-06: renamed local dependency (package = "...") alias -> target testkit -> FAIL
SRC_R2_06="$TMP_ROOT/src_r2_06.toml"
cat > "$SRC_R2_06" <<'EOF'
[package]
name = "fake-crate"
version = "0.0.1"
edition = "2021"

[dependencies]
serde = { workspace = true }
renamed_dep = { path = "../fake-dep", package = "fake-dep", features = ["full"] }
EOF
ROOT_R2_06="$TMP_ROOT/tkg_r2_06"
build_repo2 "$ROOT_R2_06" "$WS_R2_BASE" "$SRC_R2_06" "$DEP_FULL_DIRECT"
exit_code="$(run_guard "$ROOT_R2_06")"
if [ "$exit_code" -ne 0 ] && grep -q 'testkit feature in a production dependency table' "$ROOT_R2_06/guard_output.txt"; then
    pass "TKG-R2-06" "Guard resolves a renamed (package=\"...\") local dependency's feature alias to testkit"
else
    fail "TKG-R2-06" "Guard did NOT catch renamed-dependency cross-crate alias (exit=$exit_code); see $ROOT_R2_06/guard_output.txt"
fi

# TKG-R2-07: dev-dependency features=["full"] -> target testkit -> PASS (exempt lane)
SRC_R2_07="$TMP_ROOT/src_r2_07.toml"
cat > "$SRC_R2_07" <<'EOF'
[package]
name = "fake-crate"
version = "0.0.1"
edition = "2021"

[dependencies]
serde = { workspace = true }

[dev-dependencies]
fake-dep = { path = "../fake-dep", features = ["full"] }
EOF
ROOT_R2_07="$TMP_ROOT/tkg_r2_07"
build_repo2 "$ROOT_R2_07" "$WS_R2_BASE" "$SRC_R2_07" "$DEP_FULL_DIRECT"
exit_code="$(run_guard "$ROOT_R2_07")"
if [ "$exit_code" -eq 0 ] && ! grep -q 'FAIL' "$ROOT_R2_07/guard_output.txt"; then
    pass "TKG-R2-07" "Guard does not false-positive when the cross-crate alias to testkit is dev-dependency-only"
else
    fail "TKG-R2-07" "Guard false-positived on a dev-dependency-only cross-crate alias (exit=$exit_code); see $ROOT_R2_07/guard_output.txt"
fi

# TKG-R2-08: current real repo -> PASS (no false positive from the new graph resolution)
real_output_r2="$(bash "$GUARD_SH" 2>&1)"
real_exit_r2=$?
if [ "$real_exit_r2" -eq 0 ]; then
    pass "TKG-R2-08" "Guard exits 0 against the current real repo under the new cross-crate graph resolution"
else
    fail "TKG-R2-08" "Guard exits $real_exit_r2 against the current real repo (expected 0): $real_output_r2"
fi

echo ""
if [ "$FAILURES" -eq 0 ]; then
    echo "=== ALL TKG INVARIANTS PASSED ==="
    exit 0
else
    echo "=== $FAILURES INVARIANT(S) FAILED ==="
    exit 1
fi
