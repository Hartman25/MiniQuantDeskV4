# Operator Workflows — MiniQuantDesk V4

This runbook covers concrete, step-by-step operator workflows for the real control surfaces
of the daemon.  Each workflow names the exact route, the required precondition, the expected
response shape, and the post-check that confirms the action took effect.

**Manual vs automated boundary (this runbook — non-autonomous / manual operation):**
- The daemon enforces preconditions automatically (gates, auth, DB requirement).
- The operator must initiate each action explicitly.  No auto-arm, auto-start, or
  auto-mode-change occurs without operator input.
- Reconcile checks require the operator to verify the response; the daemon does not
  automatically block arming on dirty reconcile at every call site (reconcile gate
  is enforced at the reconcile logic layer).
- **Exception — autonomous Paper + Alpaca path:** Auto-arm and auto-start both occur
  when the autonomous session controller is active (proven in AUTON-01/AC-01).
  See `docs/runbooks/autonomous_paper_ops.md` for the authoritative autonomous runbook.
  The statements above apply to non-autonomous (manual) operation modes only.

**Auth requirement:**
All operator (mutating) routes require:
```
Authorization: Bearer <MQK_OPERATOR_TOKEN>
```
Read-only telemetry routes (health, status, system/status) do not require auth.
If `MQK_OPERATOR_TOKEN` is not configured, operator routes return 503 with gate=operator_auth_config.

---

## 1. Startup / Readiness Checks

Run these before arming or starting a run.

### 1a. Verify daemon is reachable

```
GET /v1/health
```

Expected: `{"ok": true, "service": "mqk-daemon", "version": "..."}` — 200 OK.

If this fails, the daemon process is not reachable.  Stop here.

### 1b. Check system status

```
GET /api/v1/system/status
```

Key fields to check:
- `daemon_mode` — confirms which mode (paper, live) the daemon loaded.
- `integrity_status` — should be "disarmed" at a clean boot.
- `runtime_status` — should be "idle" at a clean boot.
- `db_status` — must not be "unavailable" if you intend to start a run.
- `alpaca_ws_continuity` — note the WS continuity state before arming.
- `kill_switch_active` — if true, a halt occurred.  Do not arm without investigating.
- `has_warning` — if true, inspect `fault_signals` before arming.

### 1c. Check reconcile status

```
GET /api/v1/reconcile/status
```

Field `truth_state` must not be "dirty" before arming.  If the reconcile status
is unknown (e.g. first boot), review positions manually before arming.

### 1d. Check available actions

```
GET /api/v1/ops/catalog
```

Shows which action keys are currently enabled and why others are disabled.
A disabled arm entry indicates a precondition is not met.

---

## 2. Normal Start Workflow

**Preconditions:**
- Daemon is reachable (`/v1/health` returns ok=true).
- `db_status` is not "unavailable" (DB connection pool is configured).
- `integrity_status` is "disarmed" or will be armed in step 1.
- `kill_switch_active` is false.
- Reconcile is clean.

**Steps:**

### Step 1 — Arm the integrity gate

```
POST /v1/integrity/arm
Authorization: Bearer <MQK_OPERATOR_TOKEN>
```

Expected response:
```json
{"armed": true, "active_run_id": null, "state": "idle"}
```

If the response is 401 or 503, check that `MQK_OPERATOR_TOKEN` is set correctly.

### Step 2 — Verify armed

```
GET /v1/status
```

Confirm `integrity_armed == true`.  If not, the arm action did not persist.
Check DB connectivity via `/api/v1/system/status` → `db_status`.

### Step 3 — Start the execution runtime

```
POST /v1/run/start
Authorization: Bearer <MQK_OPERATOR_TOKEN>
```

Expected response: StatusSnapshot with `active_run_id` set (non-null UUID)
and `state == "running"`.

Gate failures and their meaning:
- 403 (gate=integrity_armed): arm was not completed — go back to Step 1.
- 503 (fault_class=runtime.start_refused.service_unavailable): DB pool is not
  configured.  The daemon cannot start a run without DB backing.
- 409: a run is already active.  Check active_run_id.

### Step 4 — Verify running

```
GET /v1/status
```

Confirm `state == "running"` and `active_run_id` is non-null.

```
GET /api/v1/system/status
```

Confirm `runtime_status == "running"` and `deadman_status == "healthy"`.

---

## 3. Normal Stop Workflow

**Preconditions:**
- A run is currently active (`state == "running"`).

**Steps:**

### Step 1 — Stop the execution runtime

```
POST /v1/run/stop
Authorization: Bearer <MQK_OPERATOR_TOKEN>
```

Expected response: StatusSnapshot with `state == "idle"` and
`active_run_id == null`.  Stop is idempotent: if already idle, returns idle.

### Step 2 — Verify idle

```
GET /v1/status
```

Confirm `state == "idle"`, `active_run_id == null`.

### Step 3 — Disarm (recommended after stop)

```
POST /v1/integrity/disarm
Authorization: Bearer <MQK_OPERATOR_TOKEN>
```

Expected response: `{"armed": false, "active_run_id": null, "state": "idle"}`.

Disarming after stop prevents accidental re-start without a fresh explicit arm.

### Step 4 — Verify disarmed

```
GET /v1/status
```

Confirm `integrity_armed == false`.

---

## 4. Halt (Kill-Switch) Workflow

Use halt when you need immediate shutdown with a durable record.
Halt is stronger than stop: it sets `kill_switch_active=true` and requires
a fresh reconcile and disarm/arm cycle before the next start.

**When to use halt vs stop:**
- Use stop for controlled, planned shutdowns where the run finished cleanly.
- Use halt for emergency stops, unexpected state, or when a control invariant is violated.

**Steps:**

### Step 1 — Halt the execution runtime

**Option A — direct halt route:**

```
POST /v1/run/halt
Authorization: Bearer <MQK_OPERATOR_TOKEN>
```

Response type: `StatusSnapshot`.
Expected: `state == "halted"`, `integrity_armed == false`.

**Option B — action dispatcher:**

```
POST /api/v1/ops/action
Authorization: Bearer <MQK_OPERATOR_TOKEN>
{"action_key": "kill-switch"}
```

Response type: `OperatorActionResponse` (not StatusSnapshot).
Expected: `accepted == true`, `disposition == "applied"`.
The `audit.durable_targets` field will list `"audit_events"` when DB is
present, but the audit_events row is only written if a run was active at
halt time (see Durable audit note below).

503 means DB is not configured — halt requires DB authority to persist the
halt record durably.

**Durable audit note:**
The primary durable halt record is written to `sys_arm_state`
(reason: OperatorHalt).  A `run.halt` audit event in `audit_events`
(visible via `GET /api/v1/audit/operator-actions`) is only written if an
active run was present when halt was triggered.  After halt, the HALTED
runtime transition is always visible in `GET /api/v1/ops/operator-timeline`
as a `kind="runtime_transition"` row with `detail="HALTED"` (sourced from
the `runs` table).

### Step 2 — Verify halted

```
GET /v1/status
```

Confirm `state == "halted"`, `integrity_armed == false`.

```
GET /api/v1/system/status
```

Confirm `runtime_status == "halted"`, `kill_switch_active == true`.

### Step 3 — Inspect the halt reason

```
GET /control/status
Authorization: Bearer <MQK_OPERATOR_TOKEN>
```

Check `deadman_armed_state` and `deadman_reason` to understand why the halt
was triggered or persisted.

Do not re-arm until the halt reason has been investigated and resolved.

### Step 4 — Clear the halted run record (required before re-arm)

```
POST /api/v1/ops/action
Authorization: Bearer <MQK_OPERATOR_TOKEN>
{"action_key": "clear-halted-run"}
```

This transitions the durable run record from HALTED → STOPPED in the `runs`
table so a fresh start is not blocked.  The action is only accepted when the
most recent run is in HALTED state (`enabled: true` in `GET /api/v1/ops/catalog`).

After this action the operator must disarm and re-arm before a new start:

```
POST /api/v1/ops/action {"action_key": "disarm-execution"}
POST /api/v1/ops/action {"action_key": "arm-execution"}
POST /v1/run/start
```

---

## 5. Controlled Mode Transition / Restart Workflow

Hot switching of daemon mode is not supported.  A mode change requires a
controlled process restart with updated configuration.

The authoritative 7-step workflow is available at:

```
GET /api/v1/ops/mode-change-guidance
```

This endpoint always returns 200 with the current operator guidance.
The same response (with status 409) is returned if you POST
`{"action_key": "change-system-mode"}` to `/api/v1/ops/action`.

**The 7 steps (from the daemon's canonical guidance):**

1. Disarm the daemon:
   `POST /api/v1/ops/action {"action_key": "disarm-execution"}`
2. Verify no open positions or pending outbox orders remain.
3. Update the daemon configuration file with the target deployment mode.
4. Stop the daemon process (SIGTERM or service stop command).
5. Confirm the daemon exited cleanly (exit code 0; no active run remains in DB).
6. Restart the daemon with the updated configuration.
7. Verify `GET /v1/health` returns `ok=true` and confirm new mode via
   `GET /api/v1/ops/mode-change-guidance`.

**Precondition field check:**
The `preconditions` array in the guidance response lists the specific pre-flight
requirements that must hold before step 4.  The `restart_truth` field shows the
current local and durable run ownership state so you can confirm it is safe
to stop the process.

**What `transition_permitted: false` means:**
This is always false — it records that hot switching is not permitted by design,
not that something is wrong.  The guidance still gives you the exact steps to
complete the transition safely via restart.

---

## 6. Verifying Current State After Restart

After restarting the daemon process, run these checks in order.

### 6a. Health check

```
GET /v1/health
```

Must return `ok=true` before proceeding.

### 6b. Runtime status check

```
GET /v1/status
```

Expected states after a clean restart:
- `"idle"` — no durable run record remains for this engine.  Clean state.
- `"unknown"` — a durable run record exists in DB for this engine but the
  new process does not own it locally.  This is the safe, expected state
  when a run was active when the process was stopped or halted.
- `"halted"` — a durable halt record exists.  The kill-switch is active.
  Do not re-arm without investigating.

`"running"` should NOT appear after a clean restart.  If it does, verify DB.

**When "unknown" is expected:**
After a restart following a stop or halt, "unknown" is the correct safe
state.  The daemon refuses to claim "running" without local ownership.
An operator must explicitly reconcile, disarm (if needed), arm, and start.

**When "unknown" might indicate a problem:**
If "unknown" persists for longer than expected after a deliberate stop,
or if `active_run_id` remains non-null when the run was intentionally
stopped, check the DB run record directly.

### 6c. Leadership and recovery state

```
GET /api/v1/system/runtime-leadership
```

After restart:
- `post_restart_recovery_state` will show "in_progress" until DB is
  connected and the run state is resolved.
- `generation_id` will be null if no DB-backed authoritative identity
  has been established yet.

---

## 7. Handling No-Run / Unavailable Truth States

These states indicate the daemon does not have authoritative truth yet.
They are fail-closed: the daemon does not invent state.

### Unavailable DB

If `db_status` is "unavailable" in `/api/v1/system/status`:
- The daemon cannot start a run.
- The daemon cannot persist halt/arm state durably.
- Run/start and halt will return 503 with `fault_class=runtime.start_refused.service_unavailable`.
- Check `MQK_DATABASE_URL` and DB connectivity.

### No-run / idle truth

If `state == "idle"` with `active_run_id == null`:
- The daemon has no active run record.  Clean state.
- You may arm and start if reconcile is clean and you are ready to proceed.

### Unknown state after restart

If `state == "unknown"` after restart:
- The daemon found a durable run record in DB but does not own it locally.
- This is the expected safe state after restart from a running or halted condition.
- Inspect `GET /control/status` to see `run_state`, `deadman_armed_state`, and
  `deadman_reason` before proceeding.
- If the prior run was stopped cleanly, you may arm and start after reconcile.
- If the prior run was halted, investigate the halt reason first.

---

## 8. What to Inspect When a Control Request Is Refused

| HTTP Status | Gate field | Meaning | What to do |
|---|---|---|---|
| 401 UNAUTHORIZED | — | Missing or invalid Bearer token | Verify `MQK_OPERATOR_TOKEN` is set and the header is `Authorization: Bearer <token>` |
| 503 SERVICE_UNAVAILABLE | operator_auth_config | Token not configured and not in explicit dev mode | Set `MQK_OPERATOR_TOKEN` in environment |
| 503 SERVICE_UNAVAILABLE | — (fault_class: runtime.start_refused.service_unavailable) | DB pool not configured | Configure `MQK_DATABASE_URL` and restart |
| 403 FORBIDDEN | integrity_armed | Daemon is disarmed or halted | Arm first: `POST /v1/integrity/arm` |
| 403 FORBIDDEN | — | Reconcile not clean (if reconcile gate is active) | Reconcile positions and orders with broker first |
| 409 CONFLICT | — | Mode-change requested | Follow the 7-step guidance from `/api/v1/ops/mode-change-guidance` |
| 409 CONFLICT | — | Duplicate start | Check `/v1/status` — a run is already active |

For any 5xx response, check:
1. `GET /v1/health` — is the daemon reachable?
2. `GET /api/v1/system/status` → `db_status` — is DB available?
3. Daemon process logs for the specific error.

---

## 9. Artifacts and Evidence to Check Before Proceeding

These checks apply before starting a live or shadow run where a promoted signal_pack
is involved (TV-01/TV-02/TV-03 artifact chain).

### Promoted artifact manifest

Location: `promoted/signal_packs/<artifact_id>/promoted_manifest.json`

Verify:
- `schema_version == "promoted-v1"`
- `stage == "promoted"`
- `produced_by == "research-py"`
- All `required_files` exist in the artifact directory.

### Deployability gate result

Location: `promoted/signal_packs/<artifact_id>/deployability_gate.json`

Verify:
- `schema_version == "gate-v1"`
- `passed == true`
- Inspect the `checks` array for individual check results.

A failed gate (`passed == false`) means the artifact does not meet minimum
tradability or sample adequacy criteria.  Do not proceed to live/shadow.

### Parity evidence manifest

Location: `promoted/signal_packs/<artifact_id>/parity_evidence.json`

Verify:
- `schema_version == "parity-v1"`
- `gate_passed == true` (consistent with the gate result)
- `live_trust_complete == false` — this is always false at this stage.
  It becomes true only after LO-03 operator proof is completed.
- Review `live_trust_gaps` to understand what remains unproven.

**What these artifacts confirm:**
The artifact chain confirms minimum research viability and records available
shadow evidence.  It does NOT prove edge, profitability, or live execution trust.
The `live_trust_gaps` list in parity_evidence.json makes the remaining gaps explicit.
