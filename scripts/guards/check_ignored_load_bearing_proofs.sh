#!/usr/bin/env bash
# CI-11: Guard against silently ignored load-bearing proof tests.
#
# Load-bearing DB-backed proof tests must never have a bare `#[ignore]`
# without a descriptive reason string.  A bare `#[ignore]` makes the test
# invisible to anyone running `cargo test` without --include-ignored and
# provides no guidance on how to run the test.
#
# Allowed forms:
#   #[ignore = "requires MQK_DATABASE_URL; ..."]
#
# Disallowed (detected and rejected by this guard):
#   #[ignore]   (no reason string)
#
# Scope: promoted proof test files only.  Market-data and experimental tests
# outside the proof matrix are not checked here.
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
CORE_RS_DIR="$ROOT_DIR/core-rs"

# Promoted proof files that must not contain bare #[ignore].
PROMOTED_FILES=(
  "crates/mqk-db/tests/scenario_run_lifecycle_enforced.rs"
  "crates/mqk-db/tests/scenario_deadman_enforces_halt.rs"
  "crates/mqk-db/tests/scenario_inbox_dedupe_prevents_double_fill.rs"
  "crates/mqk-db/tests/scenario_inbox_insert_then_apply_is_atomic.rs"
  "crates/mqk-db/tests/scenario_inbox_apply_atomic_recovery.rs"
  "crates/mqk-db/tests/scenario_outbox_idempotency_prevents_double_submit.rs"
  "crates/mqk-db/tests/scenario_outbox_ack_transition_guard.rs"
  "crates/mqk-db/tests/scenario_stale_claim_recovery.rs"
  "crates/mqk-db/tests/scenario_recovery_query_returns_pending_outbox.rs"
  "crates/mqk-db/tests/scenario_outbox_first_enforced.rs"
  "crates/mqk-db/tests/scenario_outbox_claim_lock_prevents_double_dispatch.rs"
  "crates/mqk-db/tests/scenario_arm_preflight_blocks_zero_risk_limits.rs"
  "crates/mqk-db/tests/scenario_arm_preflight_forged_audit_rejected.rs"
  "crates/mqk-db/tests/scenario_arm_preflight_requires_reconcile.rs"
  "crates/mqk-db/tests/scenario_db_check_constraints.rs"
  "crates/mqk-db/tests/scenario_idempotency_constraints.rs"
  "crates/mqk-db/tests/scenario_migrate_idempotent_on_clean_db.rs"
  "crates/mqk-db/tests/scenario_migration_bootstrap_replay_proof.rs"
  "crates/mqk-daemon/tests/scenario_daemon_runtime_lifecycle.rs"
)

violations=0

for rel_path in "${PROMOTED_FILES[@]}"; do
  full_path="$CORE_RS_DIR/$rel_path"
  if [[ ! -f "$full_path" ]]; then
    echo "[CI-11] MISSING promoted proof file: $rel_path" >&2
    violations=$((violations + 1))
    continue
  fi

  # Match #[ignore] on its own line (with optional leading whitespace),
  # NOT followed by = (which would be #[ignore = "..."]).
  # grep -n to show line numbers, grep -P for perl-compatible look-ahead.
  if grep -n '#\[ignore\]' "$full_path" >/dev/null 2>&1; then
    echo "[CI-11] BARE #[ignore] found in $rel_path:" >&2
    grep -n '#\[ignore\]' "$full_path" >&2
    violations=$((violations + 1))
  fi
done

if [[ "$violations" -gt 0 ]]; then
  echo "" >&2
  echo "[CI-11] $violations violation(s) detected." >&2
  echo "Every load-bearing proof test must use:" >&2
  echo '  #[ignore = "requires MQK_DATABASE_URL; run: ... cargo test -- --include-ignored"]' >&2
  echo "not a bare #[ignore]." >&2
  exit 1
fi

echo "[CI-11] check_ignored_load_bearing_proofs: OK — no bare #[ignore] in promoted proof files."
