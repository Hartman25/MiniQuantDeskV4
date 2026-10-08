#!/usr/bin/env bash
# IR-M1-PROOF-01: positive-execution check for the daemon capital-fraction dispatch DB proofs.
#
# Usage: check_cf_dispatch_db_proof.sh <cargo-test-output-log>
#
# The proof tests print `CFD_DB_PATH_TAKEN:<label>` only after they connected to the disposable
# database, migrated it and verified the required schema. Every `db_or_skip("...")` call site in the
# test module must have taken that path; a skipped test prints nothing and fails this check.
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
SRC="$ROOT_DIR/core-rs/crates/mqk-daemon/src/state/capital_fraction_dispatch_tests.rs"
LOG="${1:?usage: check_cf_dispatch_db_proof.sh <log>}"

expected=$(grep -c 'db_or_skip("' "$SRC" || true)
taken=$(grep -c 'CFD_DB_PATH_TAKEN:' "$LOG" || true)

if [[ "$expected" -lt 15 ]]; then
  echo "[IR-M1-PROOF-01] source scan found only $expected db_or_skip call sites (expected >= 15)" >&2
  exit 1
fi
if [[ "$taken" -ne "$expected" ]]; then
  echo "[IR-M1-PROOF-01] $taken of $expected DB-backed dispatch tests took the database path" >&2
  exit 1
fi
if ! grep -Eq '^test result: ok\.' "$LOG" || grep -Eq '^test result: FAILED' "$LOG"; then
  echo "[IR-M1-PROOF-01] cargo test did not report an all-pass result" >&2
  exit 1
fi
echo "[IR-M1-PROOF-01] OK: $taken/$expected DB-backed dispatch tests executed against the configured database."
