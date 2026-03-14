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

echo "== DB proof: migration bootstrap / idempotence =="
cargo test -p mqk-db --test scenario_migrate_idempotent_on_clean_db -- --ignored --test-threads=1

echo "== DB proof: daemon DB-backed lifecycle truth (start/stop/halt/disarm/status/deadman) =="
cargo test -p mqk-daemon --test scenario_daemon_runtime_lifecycle -- --test-threads=1

echo "== DB proof: run lifecycle + deadman enforcement =="
cargo test -p mqk-db --test scenario_run_lifecycle_enforced -- --test-threads=1
cargo test -p mqk-db --test scenario_deadman_enforces_halt -- --test-threads=1

echo "== DB proof: runtime leader lease acquire/refresh/release/stale-owner =="
cargo test -p mqk-db --lib runtime_lease::tests::acquire_when_no_lease_exists -- --test-threads=1
cargo test -p mqk-db --lib runtime_lease::tests::second_contender_cannot_acquire_active_lease -- --test-threads=1
cargo test -p mqk-db --lib runtime_lease::tests::expired_lease_can_be_reacquired -- --test-threads=1
cargo test -p mqk-db --lib runtime_lease::tests::stale_epoch_cannot_renew -- --test-threads=1
cargo test -p mqk-db --lib runtime_lease::tests::release_allows_new_acquire -- --test-threads=1

echo "== DB proof: inbox dedupe + apply atomicity =="
cargo test -p mqk-db --test scenario_inbox_insert_then_apply_is_atomic -- --test-threads=1
cargo test -p mqk-db --test scenario_inbox_apply_atomic_recovery -- --test-threads=1

echo "== DB proof: outbox claim + dispatch =="
cargo test -p mqk-db --test scenario_outbox_first_enforced -- --test-threads=1
cargo test -p mqk-db --test scenario_outbox_claim_lock_prevents_double_dispatch -- --test-threads=1
cargo test -p mqk-db --test scenario_outbox_ack_transition_guard -- --test-threads=1
cargo test -p mqk-db --test scenario_outbox_idempotency_prevents_double_submit -- --test-threads=1

echo "== DB proof: stale-claim recovery quarantine =="
cargo test -p mqk-db --test scenario_stale_claim_recovery -- --test-threads=1

echo "== DB proof: inbox dedupe + apply-fence =="
cargo test -p mqk-db --test scenario_inbox_dedupe_prevents_double_fill -- --test-threads=1

echo "== DB proof: broker cursor + restart quarantine =="
cargo test -p mqk-testkit --test scenario_broker_cursor_restart -- --test-threads=1
cargo test -p mqk-testkit --test scenario_restart_quarantines_dispatching_outbox -- --test-threads=1

echo "== DB proof: arm preflight + DB constraints =="
cargo test -p mqk-db --test scenario_arm_preflight_requires_reconcile -- --test-threads=1
cargo test -p mqk-db --test scenario_arm_preflight_blocks_zero_risk_limits -- --test-threads=1
cargo test -p mqk-db --test scenario_arm_preflight_forged_audit_rejected -- --test-threads=1
cargo test -p mqk-db --test scenario_db_check_constraints -- --test-threads=1
cargo test -p mqk-db --test scenario_broker_map_fk_enforced -- --test-threads=1
cargo test -p mqk-db --test scenario_idempotency_constraints -- --test-threads=1

echo "DB proof lane passed."
