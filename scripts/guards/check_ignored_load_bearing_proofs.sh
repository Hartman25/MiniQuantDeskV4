#!/usr/bin/env bash
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"

# Critical DB-backed proofs that must never be hidden behind #[ignore].
PROOF_FILES=(
  "core-rs/crates/mqk-daemon/tests/scenario_daemon_runtime_lifecycle.rs"
  "core-rs/crates/mqk-db/src/runtime_lease.rs"
  "core-rs/crates/mqk-db/tests/scenario_run_lifecycle_enforced.rs"
  "core-rs/crates/mqk-db/tests/scenario_deadman_enforces_halt.rs"
  "core-rs/crates/mqk-db/tests/scenario_stale_claim_recovery.rs"
  "core-rs/crates/mqk-db/tests/scenario_outbox_ack_transition_guard.rs"
  "core-rs/crates/mqk-db/tests/scenario_outbox_idempotency_prevents_double_submit.rs"
  "core-rs/crates/mqk-db/tests/scenario_inbox_dedupe_prevents_double_fill.rs"
  "core-rs/crates/mqk-db/tests/scenario_inbox_insert_then_apply_is_atomic.rs"
  "core-rs/crates/mqk-db/tests/scenario_inbox_apply_atomic_recovery.rs"
  "core-rs/crates/mqk-db/tests/scenario_outbox_first_enforced.rs"
  "core-rs/crates/mqk-db/tests/scenario_outbox_claim_lock_prevents_double_dispatch.rs"
  "core-rs/crates/mqk-db/tests/scenario_arm_preflight_requires_reconcile.rs"
  "core-rs/crates/mqk-db/tests/scenario_arm_preflight_blocks_zero_risk_limits.rs"
  "core-rs/crates/mqk-db/tests/scenario_arm_preflight_forged_audit_rejected.rs"
  "core-rs/crates/mqk-db/tests/scenario_db_check_constraints.rs"
)

violations=0

echo "Checking for forbidden #[ignore] on load-bearing proof tests..."

for rel in "${PROOF_FILES[@]}"; do
  file="$REPO_ROOT/$rel"
  if [[ ! -f "$file" ]]; then
    echo "FAIL: expected proof file missing: $rel" >&2
    violations=$((violations + 1))
    continue
  fi

  if rg -n '^\s*#\[ignore' "$file" >/tmp/mqk-proof-ignore.txt; then
    echo "FAIL: load-bearing proof contains #[ignore]: $rel" >&2
    cat /tmp/mqk-proof-ignore.txt >&2
    violations=$((violations + 1))
  fi
done

rm -f /tmp/mqk-proof-ignore.txt

if [[ "$violations" -ne 0 ]]; then
  echo "Ignored load-bearing proof guard failed with $violations violation(s)." >&2
  exit 1
fi

echo "Ignored load-bearing proof guard passed."
