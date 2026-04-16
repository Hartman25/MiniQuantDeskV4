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

Proof harness for MiniQuantDesk V4.

Runs two proof lanes in order:

  1. AP series: external broker proof lane — pure in-memory Alpaca adapter tests.
     Always runs. No MQK_DATABASE_URL required.

  2. DB-backed proof lane — full CI-10 mandatory matrix plus the DB-backed
     external broker runtime inbound scenario (BRK-08R RT path).
     Requires MQK_DATABASE_URL.

Options:
  --start-postgres   Start (or reuse) a local Docker Postgres 16 container
                     named mqk-postgres-proof and export MQK_DATABASE_URL
                     to postgres://mqk:mqk@127.0.0.1:5432/mqk_test

Behavior:
  - Pure in-memory Alpaca proof lane runs before any DB-URL check.
  - DB-backed lane validates that MQK_DATABASE_URL exists (or sets it when
    --start-postgres is used).
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

cd "$CORE_RS_DIR"

# ---------------------------------------------------------------------------
# AP series: external broker proof lane — pure in-memory, no DB required.
#
# Runs BEFORE the MQK_DATABASE_URL gate so it executes unconditionally.
# Covers: Alpaca event normalization (all 11 event strings, 8 BrokerEvent
# variants), InboundBatch cursor contract, WS parse path, snapshot
# normalization (AP-03), lifecycle variants, live adapter failure-mode
# isolation.
# ---------------------------------------------------------------------------
echo "== AP series: external broker proof lane (Alpaca adapter, pure in-memory) =="
cargo test -p mqk-broker-alpaca

# ---------------------------------------------------------------------------
# DB gate — everything below requires a live Postgres instance.
# ---------------------------------------------------------------------------

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

echo "== DB proof: migration manifest + bootstrap / replay =="
cargo test -p mqk-db --test scenario_migration_manifest_matches_files -- --test-threads=1
cargo test -p mqk-db --test scenario_migrate_idempotent_on_clean_db -- --ignored --test-threads=1
cargo test -p mqk-db --test scenario_migration_bootstrap_replay_proof -- --ignored --test-threads=1

# CI-08: inbox dedupe + apply-fence proofs
echo "== CI-08: inbox dedupe + apply-fence =="
cargo test -p mqk-db --test scenario_inbox_insert_then_apply_is_atomic -- --test-threads=1
cargo test -p mqk-db --test scenario_inbox_apply_atomic_recovery -- --test-threads=1
cargo test -p mqk-db --test scenario_inbox_dedupe_prevents_double_fill -- --ignored --test-threads=1

# CI-07: outbox claim, dispatch, sent, idempotency proofs
echo "== CI-07: outbox claim + dispatch + sent + idempotency =="
cargo test -p mqk-db --features testkit --test scenario_outbox_first_enforced -- --test-threads=1
cargo test -p mqk-db --features testkit --test scenario_outbox_claim_lock_prevents_double_dispatch -- --test-threads=1
cargo test -p mqk-db --test scenario_outbox_idempotency_prevents_double_submit -- --ignored --test-threads=1
cargo test -p mqk-db --test scenario_outbox_ack_transition_guard -- --ignored --test-threads=1
cargo test -p mqk-db --features testkit --test scenario_stale_claim_recovery -- --ignored --test-threads=1
cargo test -p mqk-db --features testkit --test scenario_recovery_query_returns_pending_outbox -- --ignored --test-threads=1

# CI-06: ambiguous outbox restart quarantine proofs
echo "== CI-06: broker cursor + restart quarantine =="
cargo test -p mqk-testkit --test scenario_broker_cursor_restart -- --test-threads=1
cargo test -p mqk-testkit --test scenario_restart_quarantines_dispatching_outbox -- --test-threads=1

# CI-05: runtime lease acquire, refresh, release, stale-owner proofs
echo "== CI-05: runtime lease =="
cargo test -p mqk-db runtime_lease -- --ignored --test-threads=1

# CI-04: daemon deadman proofs
echo "== CI-04: daemon deadman =="
cargo test -p mqk-db --test scenario_deadman_enforces_halt -- --ignored --test-threads=1

# CI-03 + CI-04 + CI-02 + IR-01: all daemon runtime lifecycle proofs in one binary.
# (cargo test accepts only one TESTNAME filter before --; run the whole file once
#  with --ignored so every scenario_daemon_runtime_lifecycle proof is exercised)
#
# IR-01 coverage: control-plane operator-audit durable-truth closure.
#   ir01_control_arm_no_run_no_synthetic_run_created  — no synthetic run row on arm when no real run
#   ir01_control_disarm_no_run_no_synthetic_run_created — same for disarm
#   ir01_control_arm_with_real_run_writes_audit_event — real run anchor → audit event written correctly
# These are the direct DB-backed proof of the previously disputed operator-audit durability path.
echo "== CI-04/CI-03/CI-02 + IR-01: daemon runtime lifecycle (deadman + halt + start/stop + operator-audit truth) =="
cargo test -p mqk-daemon --test scenario_daemon_runtime_lifecycle -- --ignored --test-threads=1

# CI-11: market-data provider ingest + incremental sync proofs
# Run the provider-ingest and sync-provider DB scenarios explicitly because they
# are ignored by default but now part of the promoted DB proof lane.
echo "== CI-11: market-data ingest + sync-provider =="
cargo test -p mqk-db --test scenario_md_ingest_provider -- --ignored --test-threads=1
cargo test -p mqk-db --test scenario_md_sync_provider -- --ignored --test-threads=1

# CI-09: arm-preflight and DB constraint proofs
echo "== CI-09: arm-preflight + DB constraints =="
cargo test -p mqk-db --features testkit --test scenario_arm_preflight_blocks_zero_risk_limits -- --ignored --test-threads=1
cargo test -p mqk-db --features testkit --test scenario_arm_preflight_forged_audit_rejected -- --ignored --test-threads=1
cargo test -p mqk-db --features testkit --test scenario_arm_preflight_requires_reconcile -- --ignored --test-threads=1
cargo test -p mqk-db --features testkit --test scenario_db_check_constraints -- --ignored --test-threads=1
cargo test -p mqk-db --test scenario_run_lifecycle_enforced -- --ignored --test-threads=1
cargo test -p mqk-db --features testkit --test scenario_idempotency_constraints -- --ignored --test-threads=1

# AP series: DB-backed external broker runtime inbound proof (BRK-08R RT path).
# Proves the complete integrated inbound path:
#   raw WS bytes → parse_ws_message → build_inbound_batch_from_ws_update
#   → inbox_insert_deduped_with_identity → advance_broker_cursor
# Covers RT-I1 through RT-I6, RT-G1 through RT-G4, RT-O1 (11 scenarios).
echo "== AP series: external broker DB-backed runtime inbound (BRK-08R RT path) =="
cargo test -p mqk-runtime --test scenario_alpaca_inbound_rt_brk08r -- --ignored --test-threads=1

# RUNTIME-LONGRUN-01: DB-backed repeated-cycle runtime ingest / cursor / idempotency proofs.
# Proves invariants across repeated WS ingest cycles, gap/recover transitions, and
# resumed ingest after gap repair (LR-RT-01, LR-RT-02, LR-RT-03).
echo "== RUNTIME-LONGRUN-01: DB-backed repeated-cycle runtime long-run proofs =="
cargo test -p mqk-runtime --test scenario_runtime_longrun_01 -- --ignored --test-threads=1

echo ""
echo "All proof lanes passed:"
echo "  AP series (pure in-memory): Alpaca adapter normalization, event mapping, inbound, snapshot."
echo "  AP series (DB-backed):      runtime inbound ingest, cursor persistence (BRK-08R RT)."
echo "  RUNTIME-LONGRUN-01 (DB-backed): repeated-cycle ingest idempotency + cursor monotonicity."
echo "  DB proof (CI-10):           full mandatory proof matrix."
