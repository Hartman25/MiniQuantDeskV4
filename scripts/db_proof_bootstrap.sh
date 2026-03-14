#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
CORE_RS_DIR="$ROOT_DIR/core-rs"
CONTAINER_NAME="mqk-postgres-proof"
DEFAULT_DB_URL="postgres://mqk:mqk@127.0.0.1:5432/mqk_test"
START_POSTGRES=0

usage() {
  cat <<'USAGE'
Usage: bash scripts/db_proof_bootstrap.sh [--start-postgres]

Default-safe DB proof harness for MiniQuantDesk V4.

Options:
  --start-postgres   Start (or reuse) a local Docker Postgres 16 container
                     named mqk-postgres-proof and export MQK_DATABASE_URL
                     to postgres://mqk:mqk@127.0.0.1:5432/mqk_test

Behavior:
  - Validates that MQK_DATABASE_URL exists (or sets it when --start-postgres is used).
  - Executes the same DB-backed proof lane used by CI.
  - Fails closed on missing DB config or DB connection issues.
USAGE
}

for arg in "$@"; do
  case "$arg" in
    --start-postgres) START_POSTGRES=1 ;;
    -h|--help) usage; exit 0 ;;
    *)
      echo "Unknown argument: $arg" >&2
      usage >&2
      exit 2
      ;;
  esac
done

if [[ ! -d "$CORE_RS_DIR" ]]; then
  echo "Expected Rust workspace at $CORE_RS_DIR" >&2
  exit 1
fi

if [[ "$START_POSTGRES" -eq 1 ]]; then
  if ! command -v docker >/dev/null 2>&1; then
    echo "--start-postgres requested, but docker is not installed." >&2
    exit 1
  fi

  if ! docker ps --format '{{.Names}}' | grep -Fxq "$CONTAINER_NAME"; then
    if docker ps -a --format '{{.Names}}' | grep -Fxq "$CONTAINER_NAME"; then
      echo "Starting existing container: $CONTAINER_NAME"
      docker start "$CONTAINER_NAME" >/dev/null
    else
      echo "Creating Postgres container: $CONTAINER_NAME"
      docker run --name "$CONTAINER_NAME" \
        -e POSTGRES_USER=mqk \
        -e POSTGRES_PASSWORD=mqk \
        -e POSTGRES_DB=mqk_test \
        -p 5432:5432 \
        -d postgres:16 >/dev/null
    fi
  fi

  export MQK_DATABASE_URL="$DEFAULT_DB_URL"
fi

if [[ -z "${MQK_DATABASE_URL:-}" ]]; then
  cat >&2 <<'MSG'
MQK_DATABASE_URL is not set.

Set it explicitly, or run:
  bash scripts/db_proof_bootstrap.sh --start-postgres
MSG
  exit 1
fi

echo "Using MQK_DATABASE_URL=$MQK_DATABASE_URL"

cd "$CORE_RS_DIR"

run_test() {
  echo "→ $*"
  "$@"
}

echo "== CI-10: bootstrap migration proof =="
run_test cargo test -p mqk-db --test scenario_migrate_idempotent_on_clean_db -- --ignored --test-threads=1

echo "== CI-02: daemon start/stop DB lifecycle proofs =="
run_test cargo test -p mqk-daemon --test scenario_daemon_runtime_lifecycle start_spawns_real_execution_loop -- --include-ignored --exact --test-threads=1
run_test cargo test -p mqk-daemon --test scenario_daemon_runtime_lifecycle stop_terminates_active_loop -- --include-ignored --exact --test-threads=1

echo "== CI-03: durable halt/disarm/status truth proofs =="
run_test cargo test -p mqk-daemon --test scenario_daemon_runtime_lifecycle halt_disarms_or_halts_active_loop -- --include-ignored --exact --test-threads=1
run_test cargo test -p mqk-daemon --test scenario_daemon_runtime_lifecycle hostile_restart_with_poisoned_local_cache_still_reports_durable_halt_truth -- --include-ignored --exact --test-threads=1
run_test cargo test -p mqk-daemon --test scenario_daemon_runtime_lifecycle durable_halted_run_is_reported_as_halted_by_operator_surfaces -- --include-ignored --exact --test-threads=1

echo "== CI-04: daemon deadman proofs =="
run_test cargo test -p mqk-daemon --test scenario_daemon_runtime_lifecycle runtime_loop_heartbeats_deadman_while_running -- --include-ignored --exact --test-threads=1
run_test cargo test -p mqk-daemon --test scenario_daemon_runtime_lifecycle deadman_expiry_halts_and_disarms_runtime -- --include-ignored --exact --test-threads=1
run_test cargo test -p mqk-daemon --test scenario_daemon_runtime_lifecycle runtime_refuses_to_continue_after_deadman_expiry -- --include-ignored --exact --test-threads=1
run_test cargo test -p mqk-daemon --test scenario_daemon_runtime_lifecycle status_surface_reports_deadman_truth -- --include-ignored --exact --test-threads=1

echo "== CI-05: runtime lease acquire/refresh/release/stale-owner proofs =="
run_test cargo test -p mqk-db --test scenario_run_lifecycle_enforced -- --include-ignored --test-threads=1
run_test cargo test -p mqk-db --test scenario_stale_claim_recovery -- --include-ignored --test-threads=1

echo "== CI-06: ambiguous outbox restart quarantine proofs =="
run_test cargo test -p mqk-testkit --test scenario_ambiguous_submit_quarantine_a4 -- --test-threads=1
run_test cargo test -p mqk-testkit --test scenario_restart_quarantines_dispatching_outbox -- --test-threads=1

echo "== CI-07: outbox claim/dispatch/sent/idempotency proofs =="
run_test cargo test -p mqk-db --test scenario_outbox_first_enforced -- --test-threads=1
run_test cargo test -p mqk-db --test scenario_outbox_claim_lock_prevents_double_dispatch -- --test-threads=1
run_test cargo test -p mqk-db --test scenario_outbox_idempotency_prevents_double_submit -- --include-ignored --test-threads=1

echo "== CI-08: inbox dedupe/apply-fence proofs =="
run_test cargo test -p mqk-db --test scenario_inbox_dedupe_prevents_double_fill -- --include-ignored --test-threads=1
run_test cargo test -p mqk-db --test scenario_inbox_insert_then_apply_is_atomic -- --test-threads=1
run_test cargo test -p mqk-db --test scenario_inbox_apply_atomic_recovery -- --test-threads=1

echo "== CI-09: arm-preflight + DB constraint proofs =="
run_test cargo test -p mqk-db --test scenario_arm_preflight_requires_reconcile -- --include-ignored --test-threads=1
run_test cargo test -p mqk-db --test scenario_arm_preflight_blocks_zero_risk_limits -- --include-ignored --test-threads=1
run_test cargo test -p mqk-db --test scenario_arm_preflight_forged_audit_rejected -- --include-ignored --test-threads=1
run_test cargo test -p mqk-db --test scenario_db_check_constraints -- --include-ignored --test-threads=1

echo "DB proof lane passed."
