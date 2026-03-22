# Runbooks — Common Failure Modes (V4)

Each recovery sequence is written to match real daemon route behavior.
For full step-by-step workflows see `operator_workflows.md` in this directory.

---

## Stale data (market data feed lag)

Symptom: signals or positions are based on old data; timestamps look stale.

Recovery:
1. Verify vendor connectivity and clock sync.
2. Inspect `GET /api/v1/system/status` for `alpaca_ws_continuity` state.
   If `GapDetected`, the WS inbound cursor has detected a gap — the broker
   adapter will fail closed on the gap lane and fall back to REST polling.
3. Restart feed / reconnect to data vendor (manual, outside daemon scope).
4. Verify reconcile is clean: `GET /api/v1/reconcile/status`.
5. If reconcile is dirty, resolve mismatches before re-arming.
6. Arm: `POST /v1/integrity/arm` with Bearer token.

---

## Reject storm (repeated order rejections from broker)

Symptom: risk denials or broker rejects accumulating rapidly.

Recovery:
1. Inspect reject reasons: `GET /api/v1/risk/denials` (truth_state must be
   "active" or "durable_history", not "no_snapshot").
2. Check account state and risk parameters.
3. Disarm to stop new order flow:
   `POST /api/v1/ops/action {"action_key": "disarm-execution"}` with Bearer token.
4. Fix risk/account issues manually (outside daemon scope).
5. Reconcile: verify `GET /api/v1/reconcile/status` is clean.
6. Re-arm: `POST /v1/integrity/arm` with Bearer token.

---

## Broker desync (local state diverges from broker state)

Symptom: reconcile shows mismatches between local and broker positions/orders.

Recovery:
1. Disarm immediately:
   `POST /api/v1/ops/action {"action_key": "disarm-execution"}` with Bearer token.
2. Inspect mismatches: `GET /api/v1/reconcile/mismatches`.
3. Inspect broker snapshot: `GET /v1/trading/snapshot` (if available).
4. Close/cancel positions safely via broker interface (manual — outside daemon scope).
5. Verify reconcile is clean: `GET /api/v1/reconcile/status` → truth_state == "active",
   no mismatches.
6. Arm: `POST /v1/integrity/arm` with Bearer token.

---

## Missing protective stop (position without stop order)

Symptom: a position exists in the portfolio with no stop order.

Recovery:
1. Attempt stop placement via broker interface (manual or via execute order route
   `POST /api/v1/execution/orders` with Bearer token).
2. If stop placement cannot be confirmed: disarm and flatten position manually.
3. Do not re-arm until positions are in a known safe state.

---

## Safe controlled restart

Use this sequence when you need to restart the daemon process for any reason
(mode change, config update, host maintenance).

Full preconditions and steps are in `operator_workflows.md` Section 5.  Short form:

```
DISARM  →  verify idle  →  stop process  →  update config if needed  →  restart process
→  health check  →  verify status (expect: idle or unknown)
→  reconcile clean check  →  ARM  →  START (if ready)
```

Key checks after restart:
- `GET /v1/health` must return ok=true.
- `GET /v1/status` will show "idle" or "unknown" (never "running" after clean restart).
- If "unknown": a durable run record exists — inspect before re-arming.
- If "halted": a kill-switch is active — investigate before clearing.

---

## Daemon reports "unknown" state at startup

Cause: a durable run record exists in DB from a previous session; this new process
instance does not own it locally.

What to do:
1. Inspect: `GET /control/status` → check `run_state`, `deadman_armed_state`, `deadman_reason`.
2. If the prior run was stopped cleanly and reconcile is clean: arm and start.
3. If the prior run was halted (deadman_reason == "DeadmanExpired" or "OperatorHalt"):
   investigate before clearing; do not arm until the halt cause is understood.
4. Note: the daemon will NOT automatically claim "running" from a durable "unknown" record.
   An operator must explicitly arm and start.

---

## Operator route returns 503 (operator_auth_config gate)

Cause: `MQK_OPERATOR_TOKEN` is not set and explicit dev mode is not configured.

Fix: Set `MQK_OPERATOR_TOKEN` in the environment and restart the daemon.

Operator routes (POST to /v1/run/*, /v1/integrity/*, /api/v1/ops/action, /control/*)
all require `Authorization: Bearer <MQK_OPERATOR_TOKEN>` or explicit dev mode.

---

## Run/start returns 503 (runtime DB not configured)

Cause: `MQK_DATABASE_URL` is not set; the daemon cannot start a run without DB backing.

Fix: Set `MQK_DATABASE_URL`, restart the daemon, and retry the start sequence.

Note: the daemon cannot report "running" from in-memory state alone.  DB backing
is required to start or halt any run.

---

## Halt required (kill-switch active)

Symptom: `/api/v1/system/status` shows `kill_switch_active: true`.

This means a halt was persisted durably.  The daemon will refuse to start
until the operator explicitly re-arms.

Before re-arming after a halt:
1. Inspect `GET /control/status` for `deadman_reason`.
2. Inspect `GET /api/v1/audit/operator-actions` for the halt audit record.
3. Verify positions are flat or in a known safe state.
4. Reconcile: `GET /api/v1/reconcile/status` must be clean.
5. Only then arm: `POST /v1/integrity/arm`.
