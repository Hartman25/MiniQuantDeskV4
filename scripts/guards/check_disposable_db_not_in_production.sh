#!/usr/bin/env bash
# =============================================================================
# Disposable-test-database production-isolation guard.
#
# FULL-AUDIT-SAFE-IGNORED-AND-SHARED-DB-FINAL-CLOSURE-01 Part 3: proves the
# disposable-per-test-database helper (mqk_db::create_disposable_test_db /
# DisposableTestDb / run_isolated, living in mqk-db's `test_support` module)
# can never be compiled into a production binary.
#
# Two independent checks:
#   1. Source gate: mqk-db/src/lib.rs must declare `test_support` behind
#      `#[cfg(any(test, feature = "testkit"))]`, and must not expose the
#      module or its re-exports unconditionally.
#   2. Dependency gate: no crate's *production* [dependencies] section
#      (as opposed to [dev-dependencies]) may enable mqk-db's `testkit`
#      feature. Only [dev-dependencies] may -- that scopes the feature to
#      `cargo test`, never to `cargo build`/`cargo build --release` of a
#      binary target.
#
# Usage: bash scripts/guards/check_disposable_db_not_in_production.sh
# Exit codes: 0 = clean, 1 = violation found.
# =============================================================================
set -u

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
CRATES_DIR="$REPO_ROOT/core-rs/crates"
DB_LIB="$CRATES_DIR/mqk-db/src/lib.rs"
TEST_SUPPORT="$CRATES_DIR/mqk-db/src/test_support.rs"

violations=0
fail() {
    echo "  FAIL: $1"
    violations=$((violations + 1))
}

echo "============================================================"
echo " Disposable-test-database production-isolation guard"
echo "============================================================"

# ---------------------------------------------------------------------------
# 1. Source gate.
# ---------------------------------------------------------------------------
if [ ! -f "$TEST_SUPPORT" ]; then
    fail "$TEST_SUPPORT not found -- disposable-DB helper module is missing"
elif [ ! -f "$DB_LIB" ]; then
    fail "$DB_LIB not found"
else
    if ! grep -qE '#\[cfg\(any\(test, feature = "testkit"\)\)\]\s*$' "$DB_LIB"; then
        fail "$DB_LIB has no '#[cfg(any(test, feature = \"testkit\"))]' gate line at all"
    fi

    # The line immediately preceding `pub mod test_support;` must be the cfg gate.
    mod_decl_line=$(grep -n '^pub mod test_support;' "$DB_LIB" | head -1 | cut -d: -f1)
    if [ -z "$mod_decl_line" ]; then
        fail "$DB_LIB does not declare 'pub mod test_support;' -- module wiring missing or renamed"
    else
        prev_line=$((mod_decl_line - 1))
        prev_content=$(sed -n "${prev_line}p" "$DB_LIB")
        if ! echo "$prev_content" | grep -qE '#\[cfg\(any\(test, feature = "testkit"\)\)\]'; then
            fail "$DB_LIB: 'pub mod test_support;' is not immediately gated by #[cfg(any(test, feature = \"testkit\"))] on the preceding line -- module may be unconditionally public"
        fi
    fi

    # The re-export line must be similarly gated.
    reexport_line=$(grep -n '^pub use test_support::' "$DB_LIB" | head -1 | cut -d: -f1)
    if [ -z "$reexport_line" ]; then
        fail "$DB_LIB does not re-export test_support items -- run_isolated/create_disposable_test_db may no longer be reachable as mqk_db::run_isolated"
    else
        prev_line=$((reexport_line - 1))
        prev_content=$(sed -n "${prev_line}p" "$DB_LIB")
        if ! echo "$prev_content" | grep -qE '#\[cfg\(any\(test, feature = "testkit"\)\)\]'; then
            fail "$DB_LIB: the test_support re-export is not immediately gated by #[cfg(any(test, feature = \"testkit\"))] on the preceding line"
        fi
    fi
fi

# ---------------------------------------------------------------------------
# 2. Dependency gate: no crate's production [dependencies] section may
#    enable mqk-db's testkit feature.
# ---------------------------------------------------------------------------
JOBS_DIR="$(mktemp -d)"
trap 'rm -rf "$JOBS_DIR"' EXIT

dep_violations=0
while IFS= read -r -d '' cargo_toml; do
    rel="${cargo_toml#"${REPO_ROOT}/"}"

    # Extract only the [dependencies] section (stop at the next top-level
    # [section] header, e.g. [dev-dependencies], [features], [[bin]]).
    section_file="$JOBS_DIR/$(echo "$rel" | tr '/\\' '__').deps.txt"
    awk '
        /^\[dependencies\]/ { in_section = 1; next }
        /^\[/ { in_section = 0 }
        in_section { print }
    ' "$cargo_toml" > "$section_file"

    if [ -s "$section_file" ] && grep -q 'mqk-db' "$section_file"; then
        mqk_db_line=$(grep 'mqk-db' "$section_file")
        if echo "$mqk_db_line" | grep -q 'testkit'; then
            dep_violations=$((dep_violations + 1))
            fail "$rel: production [dependencies] section enables mqk-db's 'testkit' feature -- move this to [dev-dependencies] only: $mqk_db_line"
        fi
    fi
done < <(find "$CRATES_DIR" -maxdepth 2 -name "Cargo.toml" -print0)

if [ "$dep_violations" -eq 0 ]; then
    echo " OK -- no crate's production [dependencies] section enables mqk-db's testkit feature."
fi

echo ""
if [ "$violations" -eq 0 ]; then
    echo " OK -- the disposable-test-database helper is source-gated and cannot reach a production build."
    exit 0
else
    echo " FAIL -- $violations violation(s) found above."
    exit 1
fi
