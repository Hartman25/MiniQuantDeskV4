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

---

## 10. Normal Desktop Launcher (LAUNCHER-MD-01)

### 10.0 Official launcher (OFFICIAL-DUAL-MODE-LAUNCHER-01)

`Start-MiniQuantDesk.ps1` is the **official, top-level entrypoint** for starting
MiniQuantDesk manually and (in a future patch) via Windows Task Scheduler. It
selects between two top-level trading modes and delegates the actual startup
work to the existing accepted scripts below rather than reimplementing them.

```powershell
# Interactive Paper/Live menu
.\scripts\windows\Start-MiniQuantDesk.ps1

# Paper — full startup (delegates to Launch-VeritasLedger.ps1 + market-data +
# reconcile + halt-recovery; never calls start-system)
.\scripts\windows\Start-MiniQuantDesk.ps1 -Mode Paper

# Paper — read-only diagnostic, no daemon/GUI/build/mutation
.\scripts\windows\Start-MiniQuantDesk.ps1 -Mode Paper -CheckOnly

# Live — read-only readiness report against the real ledger (blocked today;
# never starts a live process, never calls a broker, never mutates a DB)
.\scripts\windows\Start-MiniQuantDesk.ps1 -Mode Live
.\scripts\windows\Start-MiniQuantDesk.ps1 -Mode Live -CheckOnly

# Future scheduled Paper start (Task Scheduler registration is a separate,
# not-yet-built patch: PAPER-AUTOMATIC-PREOPEN-SCHEDULER-01)
.\scripts\windows\Start-MiniQuantDesk.ps1 -Mode Paper -Scheduled
```

`-Scheduled` with no `-Mode` fails closed (`STARTUP_REFUSED`,
`reason=scheduled_mode_requires_explicit_trading_mode`, exit 2) — an
unattended invocation can never silently default to either mode. Selecting
`-Mode Live` in the launcher does **not** mean LiveCapital is authorized:
the launcher's live-readiness chain reads real, current blockers from
`MiniQuantDesk_Master_Patch_Ledger_v2_updated.md` and
`research-py/src/mqk_research/deployment/parity.py`; existing live
trust/reconcile/risk gates in `mqk-daemon` remain the sole authority and are
never weakened or bypassed by this launcher.

**Known follow-up (not done in this patch):** `Install-VeritasLedgerDesktopShortcut.ps1`
still targets `Launch-VeritasLedger.ps1` directly rather than
`Start-MiniQuantDesk.ps1`. Retargeting the desktop shortcut is the next
launcher UX patch, deferred here to keep this patch's scope to orchestration
only.

`Launch-VeritasLedger.ps1` remains the canonical **normal operator startup**
path underneath Paper mode. It starts the daemon and native desktop GUI but
does **not** auto-arm, auto-start the runtime, or submit orders. It is
separate from the smoke harness.

### Script role separation

| Script | Purpose |
|--------|---------|
| `Start-MiniQuantDesk.ps1` | **Official entrypoint** — Paper/Live mode selection, orchestrates the scripts below |
| `Launch-VeritasLedger.ps1` | Normal desktop startup — daemon + GUI (`-SkipGui` for headless attach), optional arm, no runtime auto-start |
| `Start-PaperTradingSmoke.ps1` | Proof / smoke harness — full lifecycle including arm and autonomous runtime start |
| `Prep-PremarketMarketData.ps1` | Standalone market-data prep or check — no daemon, no orders |
| `Refresh-IntradayMarketData.ps1` | Standalone recurring intraday bar refresh loop |
| `Capture-PaperSmokeEvidence.ps1` | Read-only evidence bundle capture — API snapshots, DB snapshots, operator notes |

### Canonical desktop launcher commands

**1. Data-check only — verify bar count and freshness, do not start launcher**

```powershell
.\scripts\windows\Launch-VeritasLedger.ps1 -Mode Observe -CheckMarketData
```

Calls `Prep-PremarketMarketData.ps1 -CheckOnly`.  Read-only — no DB writes.
Fails launcher with a clear message if bar count or freshness gates are not met.
Does not start the daemon or GUI.  Safe to run anytime, including off-hours.

**2. Normal observe startup — prep data, start daemon + GUI, capture evidence**

```powershell
.\scripts\windows\Launch-VeritasLedger.ps1 -Mode Observe -PrepMarketData -CaptureStartupEvidence
```

Calls `Prep-PremarketMarketData.ps1`, then starts daemon + GUI, then captures a
`launcher_startup` evidence bundle under `evidence/`.  No arm.  No runtime start.

**3. Trade-ready startup — prep data, start daemon + GUI in trade-ready mode, capture evidence**

```powershell
.\scripts\windows\Launch-VeritasLedger.ps1 -Mode TradeReady -PrepMarketData -CaptureStartupEvidence
```

Requires the backend to report `overall_ready=true` before attaching the GUI.
Still does **not** arm or start the runtime.  Operator arms and starts explicitly.

**4. Explicit paper arm — trade-ready startup with operator arm**

```powershell
.\scripts\windows\Launch-VeritasLedger.ps1 -Mode TradeReady -PrepMarketData -CaptureStartupEvidence -ArmPaper
```

After verifying trade-ready backend, calls `POST /api/v1/ops/action arm-execution`
with Bearer auth and confirms arm via `GET /api/v1/autonomous/readiness`.
- Does **not** start the execution runtime.
- Does **not** submit orders.
- Fails closed on any pre-check failure (live routing, mode mismatch, not trade-ready).
- Requires `MQK_OPERATOR_TOKEN` to be configured.

To start the runtime after arming, use the GUI or the smoke harness explicitly.

**5. Smoke harness (separate — not the normal launcher)**

```powershell
.\scripts\windows\Start-PaperTradingSmoke.ps1 -WatchSeconds 900
```

Full proof lifecycle.  Starts daemon, arms, waits for autonomous runtime start,
runs watcher.  Use for smoke sessions, not for normal daily desktop startup.

**GUI observation during the smoke harness (STEP 8B):**

STEP 1 stops any stale `mqk-gui` process before the daemon restarts.  STEP 8B
relaunches the desktop GUI in observe mode (plain launch, no arm/trade args)
once daemon identity is verified, so the operator can watch the rest of the
run.  This is non-fatal: a missing GUI binary or a launch failure is reported
in STEP 8B and in the final summary's "GUI observation" block, but never
stops the smoke.  Pass `-SkipGui` to skip the relaunch entirely (also reported
in the final summary).

### Market-data parameters (optional overrides)

| Parameter | Default | Notes |
|-----------|---------|-------|
| `-Symbols` | `MQK_STRATEGY_SYMBOL` or `AAPL` | Comma-separated ticker list |
| `-Timeframe` | `MQK_STRATEGY_MD_TIMEFRAME` or `1D` | Bar timeframe |
| `-MinCompletedBars` | `30` | Minimum completed bars required |
| `-MaxStalenessDays` | `1` for `5m`; `4` for `1D` and others | Auto-derived from timeframe unless specified |

Example with explicit overrides:
```powershell
.\scripts\windows\Launch-VeritasLedger.ps1 -Mode Observe -CheckMarketData `
    -Symbols AAPL -Timeframe 5m -MinCompletedBars 30 -MaxStalenessDays 1
```

### Safety guarantees

- `-CheckMarketData` never writes to the DB.
- `-PrepMarketData` touches `md_bars` only; never touches `oms_outbox`,
  `oms_inbox`, `runs`, `arm_state`, or any execution table.
- Neither flag enables live routing, submits orders, or arms the system.
- Both flags fail closed: launcher exits with a clear error if gates are not met.
- `-CheckMarketData` and `-PrepMarketData` are mutually exclusive.
- `-ArmPaper` is fail-closed: refuses if live_routing_enabled=true, mode≠paper,
  adapter≠alpaca, or backend is not trade-ready (reconcile, WS, arm checks).
  It calls only `arm-execution` — no runtime start, no order submission.
- `-CaptureStartupEvidence` is non-fatal: a capture failure warns but does not
  abort the launcher.  It never mutates DB or prints secrets.

---

## Discord Observability — Paper Trade Lifecycle

Configure `DISCORD_WEBHOOK_URL` in the environment to enable best-effort Discord
alerts.  Discord is an outbound signal rail only — it is NOT the source of truth.
Delivery failure never blocks trading.

### Expected Discord messages during AAPL sell/flatten smoke

| Stage | Discord stage key | When fired |
|-------|-------------------|------------|
| Run start | `autonomous.run.start` (operator action) | Autonomous controller starts execution runtime |
| Signal admitted | `signal.admitted` | Signal passes all 7 gates and is queued to outbox |
| Signal blocked | `signal.blocked` (with gate name) | Any gate refusal (budget, sizing, risk, session, suppression, WS gap) |
| Order submitted | `order.submitted` | Outbox row marked SENT and broker map written (Phase 1) |
| Broker ACK | `order.acked` | ACK event applied from broker inbox (Phase 3) |
| Fill terminal | `fill.terminal` | Terminal fill applied from broker inbox (Phase 3) |
| Fill partial | `fill.partial` | Partial fill applied from broker inbox (Phase 3) |
| Reconcile drift halt | `halt.reconcile_drift` | Phase 0c detects irrecoverable drift |
| Reconcile clean | `reconcile.clean` | Background reconcile tick transitions from a non-ok state to clean (fires once per dirty→clean transition) |
| Operator flatten | `flatten.requested` | `flatten-paper-positions` action accepted; outbox enqueued; includes `live_routing_enabled=false` and enqueued symbols |
| WS gap | `paper.ws_continuity.gap_detected` (critical alert) | WS transport drops and gap is detected |
| Deadman halt | critical alert | Deadman expired, supervisor failure, or heartbeat persist failure |
| Recovery quarantine | `halt.recovery_quarantine` | Ambiguous outbox rows detected on restart |

### Secret safety

- Webhook URL is never logged, printed, or included in any alert payload.
- Alert payloads include: stage, symbol, side, qty, price (if fill), run_id (short), environment, detail.
- Secrets (`DISCORD_WEBHOOK_URL`, API keys, account numbers) are never serialised into payloads.
- `live_routing_enabled=false` is explicitly included in `flatten.requested` alerts to confirm paper mode.

### If Discord is not configured

- `DISCORD_WEBHOOK_URL` absent or empty → all `notify_*` calls are silent no-ops.
- No error is raised; no trading behavior is affected.
- Visibility classification: **PARTIAL** (lifecycle events are still in daemon logs and DB audit trail).
- Discord absence is NOT a trading failure; it is an observability gap only.

### Proving lifecycle closure via Discord

To claim AAPL sell/flatten smoke is Discord-closed:
1. Observe `signal.admitted` → `order.submitted` → `order.acked` → `fill.terminal` in Discord.
2. Observe `flatten.requested` with `live_routing_enabled=false` and symbol=AAPL.
3. Observe `reconcile.clean` after the fill/flatten settle window.
4. Confirm no `halt.*` or `paper.ws_continuity.gap_detected` alerts during the window.

Until market observation confirms these messages, Discord lifecycle visibility remains **PARTIAL**.

### Sending a one-off Discord test alert (safe, offline)

`scripts\windows\Test-DiscordAlert.ps1` lets an operator verify Discord alert
delivery configuration in isolation -- no daemon trading runtime start, no
paper arm, no order submission, no broker/Alpaca calls, no direct DB writes.

```powershell
# Configuration check only -- no alert is sent, no POST issued
powershell -ExecutionPolicy Bypass -File scripts\windows\Test-DiscordAlert.ps1 -CheckOnly

# Send one [TEST] Discord alert via the daemon's test-discord-alert action
powershell -ExecutionPolicy Bypass -File scripts\windows\Test-DiscordAlert.ps1
```

- `-CheckOnly` reports `daemon_reachable`, `discord_webhook_configured`, and
  `operator_token_configured` (presence checks only -- secret values are never
  printed) and exits without issuing any request to `/api/v1/ops/action`.
- Normal mode fails closed (exits 1) if the daemon is unreachable at
  `/v1/health` or `MQK_OPERATOR_TOKEN` is not configured.
- Normal mode calls only `POST /api/v1/ops/action {"action_key":"test-discord-alert"}`.
  The disposition (`delivery_attempted` / `noop_unconfigured` / `delivery_failed`)
  is reported without ever printing the webhook URL or operator token.
- `DISCORD_WEBHOOK_URL` must be set only in `.env.local` (gitignored) -- never
  commit or paste a real webhook URL into any tracked file.

### Sending a paper-readiness / strategy-fit artifact alert (safe, offline)

`scripts\windows\Send-PaperReadinessDiscordAlert.ps1` lets an operator send an
optional Discord summary of an offline paper-readiness or strategy-fit
artifact JSON file produced by the research pipeline (e.g.
`exports/scanner/proofs/.../paper_readiness/paper_readiness_first_report.json`
or `.../strategy_fit/strategy_fit_*.json`). It is observability only and is
never invoked automatically.

```powershell
# Classification + configuration check only -- no alert is sent, no POST issued
powershell -ExecutionPolicy Bypass -File scripts\windows\Send-PaperReadinessDiscordAlert.ps1 -ArtifactPath <path-to-artifact.json> -CheckOnly

# Send one sanitized summary alert for the artifact
powershell -ExecutionPolicy Bypass -File scripts\windows\Send-PaperReadinessDiscordAlert.ps1 -ArtifactPath <path-to-artifact.json>
```

- `-CheckOnly` reports `artifact_type` (`paper-readiness` / `strategy-fit` /
  `unknown`), `category`, `discord_webhook_configured`, `refusal_reason` (if
  any), and `sendable` (presence checks only -- secret values are never
  printed) and exits without issuing any webhook POST.
- Normal mode POSTs directly to `$env:DISCORD_WEBHOOK_URL` (the daemon is
  never contacted) and fails closed (sends nothing) if:
  - `DISCORD_WEBHOOK_URL` is not configured,
  - the artifact file is missing, not valid JSON, or has an unsupported
    `schema_version`,
  - the artifact contains `recommended_for_live=true`,
    `approved_for_live=true`, or `eligible_for_live=true`,
  - the artifact contains a Discord webhook URL or an embedded secret/token.
- The Discord summary includes only the artifact's FILE NAME (never the local
  path), its classification, status, and up to the first 8 reasons /
  failure_reasons. The raw artifact JSON is never sent.
- This workflow is operator-triggered only -- the paper-readiness pipeline
  never calls it automatically.
- `DISCORD_WEBHOOK_URL` must be set only in `.env.local` (gitignored) -- never
  commit or paste a real webhook URL into any tracked file.

### Sending a paper smoke evidence review alert (safe, offline)

`scripts\windows\Send-PaperSmokeReviewDiscordAlert.ps1` lets an operator send
an optional Discord summary of a `review-v2` evidence review produced by
`Review-PaperSmokeEvidence.ps1` (`review_summary.json` / `review_summary.md`
in an `evidence/paper_smoke_*` folder). It is observability only, never
changes evidence classification logic, and is never invoked automatically.

```powershell
# Classification + configuration check only -- no alert is sent, no POST issued
powershell -ExecutionPolicy Bypass -File scripts\windows\Send-PaperSmokeReviewDiscordAlert.ps1 -ReviewPath <path-to-evidence-folder-or-review_summary.json> -CheckOnly

# Send one sanitized summary alert for the review
powershell -ExecutionPolicy Bypass -File scripts\windows\Send-PaperSmokeReviewDiscordAlert.ps1 -ReviewPath <path-to-evidence-folder-or-review_summary.json>
```

- `-ReviewPath` accepts a direct path to `review_summary.json` or
  `review_summary.md`, or an evidence folder containing one of those files
  (`review_summary.json` is preferred when both are present).
- `-CheckOnly` reports `classification`, `evidence_folder_name`,
  `discord_webhook_configured`, `refusal_reason` (if any), and `sendable`
  (presence checks only -- secret values are never printed) and exits without
  issuing any webhook POST.
- Normal mode POSTs directly to `$env:DISCORD_WEBHOOK_URL` (the daemon is
  never contacted) and fails closed (sends nothing) if:
  - `DISCORD_WEBHOOK_URL` is not configured,
  - the review file is missing, or (for `review_summary.json`) not valid JSON
    or has an unsupported `schema_version`,
  - a classification cannot be determined or is not one of
    `NATURAL-TRADE-LIFECYCLE-CLOSED`, `READINESS-CLOSED-NO-TRADE`, `PARTIAL`,
    `OPEN`, `FALSE-CLOSED`,
  - the review file contains `recommended_for_live=true`,
    `approved_for_live=true`, or `eligible_for_live=true`,
  - the review file contains a Discord webhook URL or an embedded secret/token.
- The Discord summary includes only the evidence FOLDER NAME and review FILE
  NAME (never the local path), the classification/VERDICT, up to the first 8
  classification reasons, and key runtime/lifecycle/reconcile fields. The raw
  review JSON or Markdown contents are never sent.
- This workflow is operator-triggered only -- `Review-PaperSmokeEvidence.ps1`
  never calls it automatically.
- `DISCORD_WEBHOOK_URL` must be set only in `.env.local` (gitignored) -- never
  commit or paste a real webhook URL into any tracked file.

---

## 11. Premarket market-data refresh (PREMARKET-DATA-SCHEDULER-01)

Required market data must be fresh before the readiness gate allows `start-system`.
This section covers the operator workflow for automated premarket data refresh.

### What the scheduler does

The scheduled task calls **only** `Prep-PremarketMarketData.ps1` on weekdays
before market open.  It does NOT start the daemon, the runtime, or trading.
It does NOT call `Start-PaperTradingSmoke.ps1`.

### Which symbols are required? (`GET /api/v1/market-data/ingest-plan`)

The daemon exposes one read-only, canonical answer to "which symbols/timeframe
does the bot require for trading readiness, and where did that list come
from":

```
GET /api/v1/market-data/ingest-plan
```

It reuses the exact same symbol-resolution logic as the premarket readiness
gate (Section 1's `market_data_readiness` field), so the two surfaces can
never disagree:

1. An approved `watchlist-v2` artifact (`MQK_PAPER_WATCHLIST_PATH`), if configured and valid.
2. Otherwise the legacy single `MQK_STRATEGY_SYMBOL` / `MQK_STRATEGY_MD_TIMEFRAME` pair.
3. Otherwise no symbols (`symbol_source: "none"`).

It never falls back to the full instrument registry (`config/instruments/equities.json`)
as a default trading watchlist — that registry remains a separate, larger
tracked/coverage universe (`GET /api/v1/ingest/tracked-equities`), not the
required-symbol source.

`truth_state` is `"active"` (symbols resolved), `"not_configured"` (nothing
usable is configured), or `"degraded"` (a watchlist path is configured but is
not the active source, e.g. missing/invalid/not-yet-approved — `warnings`
names why). This route makes no DB, provider, or broker calls and does not
touch live/paper execution state.

`Prep-PremarketMarketData.ps1 -SymbolsFromIngestPlan` (see below) calls this
route to resolve `-Symbols`/`-Timeframe` automatically instead of relying on
the script's `AAPL` default.

### Register the scheduled task (one-time setup)

```powershell
powershell -ExecutionPolicy Bypass -File scripts\windows\Register-PremarketDataRefreshTask.ps1
```

Default behavior:
- Task name: `MiniQuantDesk-PremarketDataRefresh`
- Runs Mon-Fri at 08:30 local machine time
- Calls `Prep-PremarketMarketData.ps1 -Symbols AAPL -Timeframe 1D -MinCompletedBars 30`
- Transcripts written to `exports\market_data\scheduled\refresh_YYYYMMDD_HHMMSS.log`
- Registration record written to `exports\market_data\scheduled\task_registration.json`

**TIMEZONE NOTE:** The trigger fires at 08:30 **local machine time**.
If your machine is not in ET, adjust `-TriggerTimeLocal` so the refresh
completes at least 30 minutes before your intended market-open window.

Custom parameters example:
```powershell
powershell -ExecutionPolicy Bypass -File scripts\windows\Register-PremarketDataRefreshTask.ps1 `
    -Symbols AAPL,AMD `
    -TriggerTimeLocal '07:45' `
    -MinCompletedBars 60
```

### Preview what would be registered (CheckOnly -- no changes)

```powershell
powershell -ExecutionPolicy Bypass -File scripts\windows\Register-PremarketDataRefreshTask.ps1 -CheckOnly
```

Outputs: task name, trigger, symbols, timeframe, evidence path, task action argument.
Exits 0.  No task is created, updated, or removed.

### Run a one-time manual refresh (outside of scheduled task)

```powershell
powershell -ExecutionPolicy Bypass -File scripts\windows\Prep-PremarketMarketData.ps1
```

Or with custom symbols:
```powershell
powershell -ExecutionPolicy Bypass -File scripts\windows\Prep-PremarketMarketData.ps1 `
    -Symbols AAPL,AMD -Timeframe 1D -MinCompletedBars 30
```

Or resolving symbols/timeframe from the daemon's ingest plan instead of
`-Symbols`/`-Timeframe` (requires the daemon to already be running):
```powershell
powershell -ExecutionPolicy Bypass -File scripts\windows\Prep-PremarketMarketData.ps1 -SymbolsFromIngestPlan
```
This calls `GET /api/v1/market-data/ingest-plan` (see above) and fails
clearly (exit 1) if the daemon is unreachable or the plan has no required
symbols — it never silently falls back to the `AAPL` default.

### Check data readiness without mutations

```powershell
powershell -ExecutionPolicy Bypass -File scripts\windows\Prep-PremarketMarketData.ps1 -CheckOnly
```

Reports: completed bar count, date range, staleness in days, key presence.
No data is written to the database.

### Unregister the scheduled task

```powershell
powershell -ExecutionPolicy Bypass -File scripts\windows\Register-PremarketDataRefreshTask.ps1 -Unregister
```

### Verify the task is registered

```powershell
Get-ScheduledTask -TaskName 'MiniQuantDesk-PremarketDataRefresh'
```

### Monday premarket operator workflow

Before market open on Mondays (or after any long weekend):

1. Verify the scheduled task ran:
   - Open Task Scheduler -> `MiniQuantDesk-PremarketDataRefresh` -> History tab
   - Or check `exports\market_data\scheduled\` for a recent `.log` file
2. Confirm data is fresh:
   ```powershell
   powershell -ExecutionPolicy Bypass -File scripts\windows\Prep-PremarketMarketData.ps1 -CheckOnly
   ```
3. If data is stale or the task did not run, run a manual refresh:
   ```powershell
   powershell -ExecutionPolicy Bypass -File scripts\windows\Prep-PremarketMarketData.ps1
   ```
4. Confirm readiness via the daemon preflight:
   ```
   GET /api/v1/system/preflight
   ```
   Verify `market_data_freshness.gate == "PASS"` and `start_allowed == true`.
5. Proceed with normal startup workflow (Section 1).

### Evidence files

| File | Contents |
|------|----------|
| `exports\market_data\scheduled\task_registration.json` | Task config at registration time |
| `exports\market_data\scheduled\refresh_YYYYMMDD_HHMMSS.log` | Transcript per scheduled run |
| `exports\market_data\premarket_prep_YYYYMMDD_HHMMSS.json` | Per-run bar-count and freshness evidence |

### Safety constraints

- The scheduled task calls `Prep-PremarketMarketData.ps1` only.
- It targets the paper DB (port 5440) only.
- It does not start the daemon, runtime, or any trading path.
- It does not enqueue orders or touch `oms_outbox`, `oms_inbox`, or `runs`.
- Guard tests in `tests/script_guards/test_premarket_data_scheduler.ps1` enforce
  these invariants statically on every CI run.

---

## 12. Tomorrow market smoke — AAPL/5m (Paper+Alpaca)

Single operator command for the AAPL/5m Paper+Alpaca market-hours smoke.
Script: `scripts\windows\Run-AAPL5mMarketSmoke.ps1`

### CheckOnly (run any time — no mutations)

```powershell
powershell -ExecutionPolicy Bypass -File scripts\windows\Run-AAPL5mMarketSmoke.ps1 -CheckOnly
```

Checks: bar count/freshness, smoke prerequisites, launcher market-data gate.
No daemon start, no env mutation, no trading path.

### Full smoke (run during market hours)

```powershell
powershell -ExecutionPolicy Bypass -File scripts\windows\Run-AAPL5mMarketSmoke.ps1 -WatchSeconds 1800
```

What it does in order:
1. Refreshes AAPL/5m bars from Alpaca (`Refresh-IntradayMarketData.ps1 -Source alpaca -Once`).
2. Runs `Start-PaperTradingSmoke.ps1 -CheckOnly` — aborts if prerequisites fail.
3. Sets conservative strategy env vars (`MQK_STRATEGY_MD_TIMEFRAME=5m`, qty=3, notional=$1000).
4. Runs `Start-PaperTradingSmoke.ps1 -WatchSeconds 1800`.
5. Captures evidence via `Capture-PaperSmokeEvidence.ps1 -Label aapl5m_market_smoke`
   (always attempted even if smoke exits nonzero).
6. Writes `logs\aapl5m_market_smoke\latest_result.txt`.

### Log and evidence output locations

| Path | Contents |
|------|----------|
| `logs\aapl5m_market_smoke\<timestamp>\transcript.log` | Full session transcript |
| `logs\aapl5m_market_smoke\latest_result.txt` | Final verdict marker |
| `evidence\paper_smoke_<timestamp>_aapl5m_market_smoke\` | API/DB snapshots, lifecycle checklist |

### Safety notes

- Paper+Alpaca only. No live routing enabled or reachable through this script.
- No signal injection. Signals enter only via the normal smoke path inside `Start-PaperTradingSmoke.ps1`.
- No manual broker orders submitted outside the normal smoke path.
- No fake bars. Refresh calls `Refresh-IntradayMarketData.ps1` (Alpaca `md sync-provider`).
- No DB schema changes.
- `.env.local` is never committed by this script.
- No secrets printed (API keys, operator token, DB credentials).
- Guard: `tests\script_guards\test_aapl5m_market_smoke_runner.ps1` (14 assertions, runs in CI).

### After the run — review evidence

After `Run-AAPL5mMarketSmoke.ps1` completes, review the captured evidence bundle:

```powershell
powershell -ExecutionPolicy Bypass -File scripts\windows\Review-PaperSmokeEvidence.ps1 -Latest -WriteSummary
```

This produces `evidence\paper_smoke_<timestamp>_aapl5m_market_smoke\review_summary.md`.

**Classification meanings:**

| Verdict | Meaning |
|---------|---------|
| `TRADE-LIFECYCLE-CLOSED` | Full lifecycle proven: runtime ran, signal fired, order submitted, ACK received, fill applied, reconcile clean, no fault. |
| `READINESS-CLOSED-NO-TRADE` | Runtime ran, bars loaded, strategy evaluated — but no signal or order. Reconcile clean, no fault. Correct when market conditions did not trigger a signal. |
| `PARTIAL` | Some lifecycle steps completed but not all. Check `notes/smoke_lifecycle_checklist.txt` and `api/events_feed.json` for details. |
| `OPEN` | Active blocker present: halt, kill switch, missing bars, dirty reconcile, or DB unavailable. Resolve blocker and re-run. |
| `FALSE-CLOSED` | Live routing was enabled, secrets detected in evidence, or no proof files exist. Do **not** record as a passed smoke. |

**Review with explicit path:**
```powershell
powershell -ExecutionPolicy Bypass -File scripts\windows\Review-PaperSmokeEvidence.ps1 `
    -EvidencePath evidence\paper_smoke_<timestamp>_aapl5m_market_smoke -WriteSummary
```

**Send for ledger update:**
After reviewing, send `review_summary.md` to ChatGPT (or your ledger session) with the prompt:
> "Here is a paper smoke evidence review. Classify the run and update the session ledger."

The reviewer will use the `classification` field and `classification_reasons` to update patch and smoke tracking.
