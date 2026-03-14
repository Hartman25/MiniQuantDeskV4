#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$ROOT_DIR"

python - <<'PY'
from pathlib import Path
import re
import sys

protected = {
    "core-rs/crates/mqk-db/tests/scenario_run_lifecycle_enforced.rs": [
        "run_lifecycle_enforced_and_live_exclusive",
    ],
    "core-rs/crates/mqk-db/tests/scenario_stale_claim_recovery.rs": [
        "stale_claim_older_than_threshold_reset_to_pending",
        "fresh_claim_newer_than_threshold_untouched",
        "sent_rows_never_reset_by_stale_reaper",
    ],
    "core-rs/crates/mqk-db/tests/scenario_outbox_idempotency_prevents_double_submit.rs": [
        "outbox_idempotency_key_dedupes_inserts",
    ],
    "core-rs/crates/mqk-db/tests/scenario_inbox_dedupe_prevents_double_fill.rs": [
        "inbox_broker_message_id_dedupes_inserts",
    ],
    "core-rs/crates/mqk-db/tests/scenario_arm_preflight_requires_reconcile.rs": [
        "forged_audit_event_cannot_satisfy_arming",
    ],
    "core-rs/crates/mqk-db/tests/scenario_db_check_constraints.rs": [
        "check_constraints_reject_invalid_enum_values",
    ],
    "core-rs/crates/mqk-daemon/tests/scenario_daemon_runtime_lifecycle.rs": [
        "start_spawns_real_execution_loop",
        "stop_terminates_active_loop",
        "halt_disarms_or_halts_active_loop",
        "hostile_restart_with_poisoned_local_cache_still_reports_durable_halt_truth",
        "durable_halted_run_is_reported_as_halted_by_operator_surfaces",
        "runtime_loop_heartbeats_deadman_while_running",
        "deadman_expiry_halts_and_disarms_runtime",
        "runtime_refuses_to_continue_after_deadman_expiry",
        "status_surface_reports_deadman_truth",
    ],
}

errors = []
for rel_path, tests in protected.items():
    path = Path(rel_path)
    if not path.exists():
        errors.append(f"missing protected proof file: {rel_path}")
        continue

    lines = path.read_text(encoding="utf-8").splitlines()
    for name in tests:
        fn_line = None
        for idx, line in enumerate(lines):
            if re.search(rf"\bfn\s+{re.escape(name)}\s*\(", line):
                fn_line = idx
                break
        if fn_line is None:
            errors.append(f"{rel_path}: missing protected test function {name}")
            continue

        window_start = max(0, fn_line - 4)
        header = lines[window_start:fn_line]
        if any(re.match(r"\s*#\[ignore(?:\s*=.*)?\]", h) for h in header):
            errors.append(f"{rel_path}:{fn_line+1} protected test {name} is marked #[ignore]")

if errors:
    print("FAIL: load-bearing proof tests cannot be ignored.", file=sys.stderr)
    for err in errors:
        print(f"  - {err}", file=sys.stderr)
    sys.exit(1)

print("PASS: load-bearing proof tests are not ignored.")
PY
