# Stressed Recovery Proof Matrix — MiniQuantDesk V4 (LO-02)

This document defines the explicit stressed recovery matrix for the MiniQuantDesk V4
daemon.  Each entry names the failure/recovery case, the expected behavior, and the
proof lane(s) that cover it.

**What this matrix is not:**
- This matrix does not prove that all recovery risks are solved.
- It does not cover every possible failure scenario.
- Proving recoverability here is not the same as proving production trust.

**How to use this matrix:**
- Use it to verify that important recovery behaviors are explicitly covered.
- Reference the named proof lanes when auditing.
- Note which cases are covered by in-process proof (always runnable in CI) vs
  DB-backed proof (requires `MQK_DATABASE_URL`).

---

## Matrix

### SR-01 — Fresh boot is clean and safe baseline

**Failure case:** Daemon starts for the first time or after all state is cleared.

**Expected behavior:**
- `GET /v1/status` returns `state=idle`, `integrity_armed=false`.
- No active run.  No claimed ownership.
- Operator must explicitly arm before any run can start.

**Proof lane:**
- In-process, always runnable:
  `crates/mqk-daemon/tests/scenario_stressed_recovery_lo02.rs::lo02_sr01_fresh_boot_is_disarmed_and_idle`
- Supplementary:
  `crates/mqk-daemon/tests/scenario_daemon_boot_is_fail_closed.rs::boot_status_reports_integrity_disarmed`

---

### SR-02 — Poisoned in-memory cache cannot survive a cold start

**Failure case:** An in-memory status struct claims "running" but no DB-backed run
authority exists.  This could happen if a prior process exited uncleanly and an
in-memory state was incorrectly initialized.

**Expected behavior:**
- `GET /v1/status` returns `state=idle`, NOT `state=running`.
- The daemon ignores placeholder running state and fails closed.

**Proof lane:**
- In-process, always runnable:
  `crates/mqk-daemon/tests/scenario_stressed_recovery_lo02.rs::lo02_sr02_placeholder_running_cannot_survive_cold_start`
- Supplementary:
  `crates/mqk-daemon/tests/scenario_daemon_runtime_lifecycle.rs::cannot_report_running_from_placeholder_state_alone`

---

### SR-03 — Missing operator token fails closed on operator routes

**Failure case:** Daemon is started without `MQK_OPERATOR_TOKEN` configured.

**Expected behavior:**
- Operator routes (run/start, run/halt, integrity/arm, etc.) return 503 with
  `gate=operator_auth_config`, not a permissive 200.
- Read-only routes (health, status, system/status) remain available.

**Proof lane:**
- In-process, always runnable:
  `crates/mqk-daemon/tests/scenario_stressed_recovery_lo02.rs::lo02_sr03_missing_operator_token_fails_closed`
- Supplementary:
  `crates/mqk-daemon/tests/scenario_daemon_boot_is_fail_closed.rs::production_mode_without_token_refuses_startup_or_operator_access`

---

### SR-04 — Mode-change request returns guided workflow, not a dead end

**Failure case:** Operator (or GUI) attempts a mode-change action during a live session.

**Expected behavior:**
- `POST /api/v1/ops/action {"action_key": "change-system-mode"}` returns 409
  with a `ModeChangeGuidanceResponse` body: `transition_permitted=false`,
  `preconditions` and `operator_next_steps` arrays are non-empty.
- The guidance response matches `GET /api/v1/ops/mode-change-guidance`.
- Neither 400 (silent refusal) nor 500 (crash) is acceptable.

**Proof lane:**
- In-process, always runnable:
  `crates/mqk-daemon/tests/scenario_stressed_recovery_lo02.rs::lo02_sr04_mode_change_guidance_is_non_empty_and_actionable`
- Supplementary:
  `crates/mqk-daemon/tests/scenario_daemon_routes.rs::cc03_change_system_mode_returns_guidance_response`
  `crates/mqk-daemon/tests/scenario_daemon_routes.rs::cc03_mode_change_guidance_get_returns_200`

---

### SR-05 — Run/start without DB returns explicit error, not crash

**Failure case:** Operator attempts to start a run when `MQK_DATABASE_URL` is not
configured (DB pool is absent).

**Expected behavior:**
- `POST /v1/run/start` (after arm) returns 503 with `fault_class=runtime.start_refused.service_unavailable`.
- The error message explicitly states "runtime DB is not configured".
- No crash, no phantom run row, no silent success.

**Proof lane:**
- In-process, always runnable:
  `crates/mqk-daemon/tests/scenario_stressed_recovery_lo02.rs::lo02_sr05_start_without_db_returns_explicit_error`
- Supplementary:
  `crates/mqk-daemon/tests/scenario_daemon_routes.rs::run_start_requires_db_backed_runtime_after_arm`

---

### SR-06 — Halt without DB returns explicit error, not crash

**Failure case:** Operator attempts to halt when DB is not configured.

**Expected behavior:**
- `POST /v1/run/halt` returns 503 with `fault_class=runtime.start_refused.service_unavailable`.
- The halt cannot be persisted durably without DB.  Failing closed is correct.

**Proof lane:**
- In-process, always runnable:
  `crates/mqk-daemon/tests/scenario_stressed_recovery_lo02.rs::lo02_sr06_halt_without_db_returns_explicit_error`
- Supplementary:
  `crates/mqk-daemon/tests/scenario_daemon_routes.rs::run_halt_requires_db_backed_runtime_authority`

---

### SR-07 — Stop on idle is idempotent

**Failure case:** Operator calls stop when no run is active (e.g. after an earlier stop
or on a freshly started daemon).

**Expected behavior:**
- `POST /v1/run/stop` returns 200 with `state=idle`.
- No error, no invented state, no crash.

**Proof lane:**
- In-process, always runnable:
  `crates/mqk-daemon/tests/scenario_stressed_recovery_lo02.rs::lo02_sr07_stop_on_idle_is_idempotent`
- Supplementary:
  `crates/mqk-daemon/tests/scenario_daemon_routes.rs::run_stop_on_idle_remains_idle`

---

### SR-08 — Arm/disarm cycle is stable after stress

**Failure case:** Operator arms and disarms multiple times, or disarms on an already-disarmed daemon.

**Expected behavior:**
- arm → disarm → arm produces consistent state transitions.
- Disarm on boot state (already disarmed) returns armed=false cleanly.

**Proof lane:**
- In-process, always runnable:
  `crates/mqk-daemon/tests/scenario_stressed_recovery_lo02.rs::lo02_sr08_arm_disarm_cycle_is_stable`

---

### SR-09 — Restart after controlled stop shows safe unknown state (DB-backed)

**Failure case:** Daemon process exits after a durable run was started.  New process starts.

**Expected behavior:**
- New process creates fresh AppState with the same DB pool.
- `GET /v1/status` returns `state=unknown` (durable run record found but not locally owned).
- NOT `state=running`.  The daemon refuses to claim running without local ownership.

**Proof lane:**
- DB-backed (requires `MQK_DATABASE_URL`):
  `crates/mqk-daemon/tests/scenario_daemon_runtime_lifecycle.rs::restart_reconstructs_safe_runtime_status`

---

### SR-10 — Hostile restart with poisoned local cache reports durable halt truth (DB-backed)

**Failure case:** Prior process was halted durably.  New process starts with local
in-memory state incorrectly indicating a running (not halted) state.

**Expected behavior:**
- DB halt truth overrides poisoned in-memory cache.
- `GET /v1/status` returns `state=halted`, `active_run_id` = the durable run.
- `GET /api/v1/system/status` shows `runtime_status=halted`, `kill_switch_active=true`.

**Proof lane:**
- DB-backed (requires `MQK_DATABASE_URL`):
  `crates/mqk-daemon/tests/scenario_daemon_runtime_lifecycle.rs::hostile_restart_with_poisoned_local_cache_still_reports_durable_halt_truth`

---

### SR-11 — Deadman expiry halts and disarms the runtime (DB-backed)

**Failure case:** Heartbeat stops being written (e.g. process is frozen or deadlocked).
Deadman TTL expires.

**Expected behavior:**
- Execution loop detects stale heartbeat and halts the run.
- DB run status transitions to `Halted`.
- Arm state transitions to `DISARMED` with reason `DeadmanExpired`.
- Subsequent attempt to restart is refused (403).

**Proof lane:**
- DB-backed (requires `MQK_DATABASE_URL`):
  `crates/mqk-daemon/tests/scenario_daemon_runtime_lifecycle.rs::deadman_expiry_halts_and_disarms_runtime`
  `crates/mqk-daemon/tests/scenario_daemon_runtime_lifecycle.rs::runtime_refuses_to_continue_after_deadman_expiry`

---

### SR-12 — Heartbeat persistence failure fails closed (DB-backed)

**Failure case:** DB becomes unavailable while a run is active; heartbeat cannot be written.

**Expected behavior:**
- Runtime loop cannot refresh heartbeat.
- State transitions away from "running" (fail closed).

**Proof lane:**
- DB-backed (requires `MQK_DATABASE_URL`):
  `crates/mqk-daemon/tests/scenario_daemon_runtime_lifecycle.rs::heartbeat_persistence_failure_fails_closed`

---

## Coverage Summary

| Case | In-process (CI) | DB-backed (integration) |
|---|---|---|
| SR-01 Fresh boot baseline | scenario_stressed_recovery_lo02.rs | — |
| SR-02 Poisoned cache ignored | scenario_stressed_recovery_lo02.rs | scenario_daemon_runtime_lifecycle.rs |
| SR-03 Missing token | scenario_stressed_recovery_lo02.rs | — |
| SR-04 Mode-change guided | scenario_stressed_recovery_lo02.rs | scenario_daemon_routes.rs |
| SR-05 Start without DB | scenario_stressed_recovery_lo02.rs | — |
| SR-06 Halt without DB | scenario_stressed_recovery_lo02.rs | — |
| SR-07 Stop idempotent | scenario_stressed_recovery_lo02.rs | — |
| SR-08 Arm/disarm cycle | scenario_stressed_recovery_lo02.rs | — |
| SR-09 Restart after stop | — | scenario_daemon_runtime_lifecycle.rs |
| SR-10 Hostile restart | — | scenario_daemon_runtime_lifecycle.rs |
| SR-11 Deadman expiry | — | scenario_daemon_runtime_lifecycle.rs |
| SR-12 Heartbeat failure | — | scenario_daemon_runtime_lifecycle.rs |

In-process cases (SR-01 through SR-08) are always runnable in CI.
DB-backed cases (SR-09 through SR-12) require `MQK_DATABASE_URL` and are
marked `#[ignore]` in their respective test files.
