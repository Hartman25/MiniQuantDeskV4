#!/usr/bin/env bash
# =============================================================================
# Testkit-feature release-safety guard.
#
# CI-TESTKIT-FEATURE-GUARD-VERIFY-01: proves a release-profile build cannot
# accidentally enable the `testkit` Cargo feature (mqk-cli's own optional
# `testkit` feature, and the same-named feature it propagates to
# mqk-execution/mqk-runtime/mqk-db). `testkit` gates test-only escape hatches
# (BrokerGateway::for_test, OutboxClaimToken::for_test, the disposable-DB
# test_support module) that MUST NOT compile into a production binary.
#
# Two independent, workspace-wide checks:
#   1. Default-feature gate: no crate's [features] `default = [...]` list may
#      include "testkit". A plain `cargo build`/`cargo build --release` with
#      no explicit --features flag enables only default features, so this is
#      the single check that rules out "accidentally on by default".
#   2. Production-dependency gate: no crate's *production* [dependencies]
#      section (as opposed to [dev-dependencies]) may enable "testkit" on any
#      dependency. Only [dev-dependencies] may -- that scopes the feature to
#      `cargo test`, never to `cargo build`/`cargo build --release` of a
#      binary target. (Generalizes
#      check_disposable_db_not_in_production.sh's existing mqk-db-only check
#      to every crate testkit can reach: mqk-db, mqk-execution, mqk-runtime,
#      mqk-cli.)
#
# Usage: bash scripts/guards/check_testkit_feature_not_default.sh
# Exit codes: 0 = clean, 1 = violation found.
# =============================================================================
set -u

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
CRATES_DIR="$REPO_ROOT/core-rs/crates"

violations=0
fail() {
    echo "  FAIL: $1"
    violations=$((violations + 1))
}

echo "============================================================"
echo " Testkit-feature release-safety guard"
echo "============================================================"

if [ ! -d "$CRATES_DIR" ]; then
    fail "$CRATES_DIR not found"
    echo ""
    echo " FAIL -- $violations violation(s) found above."
    exit 1
fi

JOBS_DIR="$(mktemp -d)"
trap 'rm -rf "$JOBS_DIR"' EXIT

# ---------------------------------------------------------------------------
# 1. Default-feature gate: no crate's [features] `default = [...]` may
#    include "testkit".
# ---------------------------------------------------------------------------
default_violations=0
while IFS= read -r -d '' cargo_toml; do
    rel="${cargo_toml#"${REPO_ROOT}/"}"

    features_section="$JOBS_DIR/$(echo "$rel" | tr '/\\' '__').features.txt"
    awk '
        /^\[features\]/ { in_section = 1; next }
        /^\[/ { in_section = 0 }
        in_section { print }
    ' "$cargo_toml" > "$features_section"

    if [ -s "$features_section" ]; then
        default_line=$(grep -E '^default[[:space:]]*=' "$features_section")
        if [ -n "$default_line" ] && echo "$default_line" | grep -q '"testkit"'; then
            default_violations=$((default_violations + 1))
            fail "$rel: [features] default list includes \"testkit\" -- a plain cargo build/--release would enable it: $default_line"
        fi
    fi
done < <(find "$CRATES_DIR" -maxdepth 2 -name "Cargo.toml" -print0)

if [ "$default_violations" -eq 0 ]; then
    echo " OK -- no crate's [features] default list includes \"testkit\"."
fi

# ---------------------------------------------------------------------------
# 2. Production-dependency gate: no crate's [dependencies] section may
#    enable "testkit" on any dependency.
# ---------------------------------------------------------------------------
dep_violations=0
while IFS= read -r -d '' cargo_toml; do
    rel="${cargo_toml#"${REPO_ROOT}/"}"

    deps_section="$JOBS_DIR/$(echo "$rel" | tr '/\\' '__').deps.txt"
    awk '
        /^\[dependencies\]/ { in_section = 1; next }
        /^\[/ { in_section = 0 }
        in_section { print }
    ' "$cargo_toml" > "$deps_section"

    if [ -s "$deps_section" ]; then
        testkit_line=$(grep -E 'features[[:space:]]*=.*"testkit"' "$deps_section")
        if [ -n "$testkit_line" ]; then
            dep_violations=$((dep_violations + 1))
            fail "$rel: production [dependencies] section enables a dependency's \"testkit\" feature -- move this to [dev-dependencies] only: $testkit_line"
        fi
    fi
done < <(find "$CRATES_DIR" -maxdepth 2 -name "Cargo.toml" -print0)

if [ "$dep_violations" -eq 0 ]; then
    echo " OK -- no crate's production [dependencies] section enables any dependency's testkit feature."
fi

echo ""
if [ "$violations" -eq 0 ]; then
    echo " OK -- the testkit feature cannot reach a release-profile build."
    exit 0
else
    echo " FAIL -- $violations violation(s) found above."
    exit 1
fi
