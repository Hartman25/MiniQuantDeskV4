#!/usr/bin/env bash
# =============================================================================
# Testkit-feature release-safety guard -- thin CI entrypoint.
#
# CI-TESTKIT-FEATURE-GUARD-VERIFY-01: the actual check is Cargo/TOML-
# structural (tomllib), not grep/awk over raw text -- see
# check_testkit_feature_not_default.py for the full rationale and the two
# checks it performs (default-feature closure, production-dependency
# closure, including workspace-inherited and target-specific routes).
# Kept as a .sh entrypoint only so CI's existing
# `bash scripts/guards/check_testkit_feature_not_default.sh` invocation
# does not need to change.
#
# Usage: bash scripts/guards/check_testkit_feature_not_default.sh
# Exit codes: 0 = clean, 1 = violation found.
# =============================================================================
set -u

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

# Probe by actually running --version (not just `command -v`): on some
# Windows dev boxes `python3` resolves to a Microsoft Store app-execution
# alias stub that is present on PATH but fails at runtime.
PYTHON_BIN=""
for candidate in python3 python; do
    if command -v "$candidate" >/dev/null 2>&1 && "$candidate" --version >/dev/null 2>&1; then
        PYTHON_BIN="$candidate"
        break
    fi
done

if [ -z "$PYTHON_BIN" ]; then
    echo "FAIL: no working python3/python interpreter found on PATH" >&2
    exit 1
fi

exec "$PYTHON_BIN" "$SCRIPT_DIR/check_testkit_feature_not_default.py"
