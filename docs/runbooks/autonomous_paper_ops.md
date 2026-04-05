# Autonomous Paper Trading Operations — MiniQuantDesk V4 (AUTON-OPS-01)

## What this runbook covers

This document is the canonical operator guide for the **Paper + Alpaca autonomous path** — the only mode where the session controller starts and stops execution runs automatically without per-run operator intervention.

This runbook covers:
- Required env and configuration
- First-time arm / autonomous arm behavior
- Pre-session readiness checks
- Session boundary behavior (auto-start / auto-stop)
- WS gap and recovery handling
- Supervisor history and the `autonomous_history_degraded` truth flag
- What the one-day soak harness produces and how to interpret it

**What this runbook does NOT cover:**
- LiveShadow / LiveCapital modes (see `live_shadow_operational_proof.md`)
- Artifact promotion and deployability gating (see `operator_workflows.md` §9)
- CI pipeline and guard scripts (`scripts/guards/`)

---

## 1. Canonical path: Paper + Alpaca

The autonomous path requires exactly this combination:

| Field | Required value |
|---|---|
| `MQK_DAEMON_DEPLOYMENT_MODE` | `paper` (or absent — paper is the default) |
| `MQK_DAEMON_ADAPTER_ID` | `alpaca` |
| Alpaca paper credentials | `ALPACA_API_KEY_PAPER`, `ALPACA_API_SECRET_PAPER` |
| `ALPACA_PAPER_BASE_URL` | `https://paper-api.alpaca.markets` |
| `MQK_DATABASE_URL` | Postgres URL (required for durable state) |
| `MQK_OPERATOR_TOKEN` | Any non-empty string (required for mutating routes) |

Any other deployment-mode / broker-kind combination returns `truth_state = "not_applicable"` from `/api/v1/autonomous/readiness` and the session controller is **disabled**.

Verifying active configuration:
```
GET /api/v1/system/status
```
Confirm `daemon_mode == "paper"` and `alpaca_ws_continuity` is present (not `"not_applicable"`).

---

## 2. Required env vars

```bash
# Broker — Alpaca paper (ENV-TRUTH-01)
ALPACA_API_KEY_PAPER=<your paper key>
ALPACA_API_SECRET_PAPER=<your paper secret>
ALPACA_PAPER_BASE_URL=https://paper-api.alpaca.markets

# Adapter selection
MQK_DAEMON_ADAPTER_ID=alpaca

# Database — required for durable arm state, run records, supervisor history
MQK_DATABASE_URL=postgres://postgres:postgres@localhost:5432/mqk_dev

# Operator auth — required for all mutating routes
MQK_OPERATOR_TOKEN=<any strong token>

# Optional: override autonomous session window (default: NYSE regular session)
# MQK_SESSION_START_HH_MM=14:30   # UTC HH:MM
# MQK_SESSION_STOP_HH_MM=21:00    # UTC HH:MM

# Optional: Discord notifications
# DISCORD_WEBHOOK_PAPER=...
# DISCORD_WEBHOOK_ALERTS=...
```

Copy `.env.local.example` → `.env.local` and fill in real values.  `.env.local` is gitignored.

---

## 3. Session window behavior

By default the autonomous session controller uses **NYSE regular-session truth**:
- In-window: 14:30 UTC – 21:00 UTC, Monday–Friday, non-holiday NYSE trading days.
- Outside that window the controller will not attempt a start even if all gates pass.

To override with a fixed UTC window (useful for testing or non-US sessions):
```bash
MQK_SESSION_START_HH_MM=14:30
MQK_SESSION_STOP_HH_MM=21:00
```

Both vars must be set and valid (`HH:MM` format, start ≠ stop) for the override to take effect.  If either is absent or invalid, the NYSE seam is used.

Current session-window truth:
```
GET /api/v1/autonomous/readiness
```
Fields: `session_in_window` (bool), `session_window_state` (`"in_window"` | `"outside_window"`).

---

## 4. Arm behavior: first-time vs autonomous re-arm

### First-time arm (after a fresh boot or halt recovery)

The in-memory integrity starts `disarmed`.  On a fresh deployment, the operator must arm once explicitly:

```
POST /v1/integrity/arm
Authorization: Bearer <MQK_OPERATOR_TOKEN>
```

This writes `ArmState::Armed` to the DB and advances in-memory integrity to armed.

Verify:
```
GET /v1/status
```
Confirm `integrity_armed == true`.

### Autonomous arm (subsequent sessions, same deployment)

After a clean stop (not a halt), the DB arm state remains `Armed`.  At the next start of a session window, the session controller calls `try_autonomous_arm`, which:
1. Checks that the run is not halted.
2. Verifies `ArmState::Armed` is persisted in DB.
3. Advances in-memory integrity to armed without operator action.

This is the standard autonomous path — **no manual arm is needed between consecutive sessions** as long as no halt occurred.

`/api/v1/autonomous/readiness` reflects this:
- `arm_state == "arm_pending"` — in-memory disarmed but DB=Armed; controller will self-arm on next tick.
- `arm_state == "armed"` — armed; start may proceed if all other gates pass.
- `arm_state == "halted"` — requires explicit operator arm after halt investigation.

### After a halt

A halt sets `kill_switch_active = true` and `ArmState::Halted` in DB.  Autonomous arm is **refused** until the operator manually:
1. Investigates the halt reason (`GET /control/status`, `GET /api/v1/audit/operator-actions`).
2. Disarms: `POST /api/v1/ops/action {"action_key": "disarm-execution"}`.
3. Re-arms: `POST /v1/integrity/arm`.

---

## 5. Pre-session readiness checks

Run these before the session window opens on any new paper day.

### 5a. Health

```
GET /v1/health
```
Must return `ok = true`.

### 5b. Autonomous readiness (primary check)

```
GET /api/v1/autonomous/readiness
```

| Field | Expected value | What to do if wrong |
|---|---|---|
| `truth_state` | `"active"` | Verify `MQK_DAEMON_ADAPTER_ID=alpaca` and daemon_mode=paper |
| `ws_continuity` | `"live"` | WS must establish — wait for the WS task to connect, or investigate network |
| `ws_continuity_ready` | `true` | Same as above |
| `reconcile_ready` | `true` | Clean positions with broker; run reconcile |
| `arm_state` | `"armed"` or `"arm_pending"` | If `"halted"`: follow halt recovery steps |
| `signal_ingestion_configured` | `true` | If false: check MQK_DAEMON_ADAPTER_ID is set to `alpaca` |
| `session_in_window` | `true` (at session open) | Wait for session window, or check session env vars |
| `runtime_start_allowed` | `true` | If false: a run is already active (check status) |
| `overall_ready` | `true` | Fix each false gate — `blockers` list explains what is blocking |
| `autonomous_history_degraded` | `false` | If true: DB is absent or had a write failure; restart with working DB |

### 5c. Preflight gate surface

```
GET /api/v1/system/preflight
```

Key autonomous fields:
- `ws_continuity_ready` — must be true before start.
- `session_in_window` — non-null; reflects current window state.
- `autonomous_readiness_applicable` — must be true for paper+alpaca.

### 5d. Active alerts

```
GET /api/v1/alerts/active
```

Inspect `fault_signals` before arming.  Relevant autonomous signals:
- `alpaca_ws_gap_detected` — WS continuity lost; start is blocked.
- `alpaca_ws_cold_start` — WS not yet proven; start is blocked.
- `autonomous_recovery_succeeded` / `autonomous_recovery_failed` — last recovery truth.
- `day_limit_reached` — per-run signal cap (100 signals) has been hit.

---

## 6. What happens during a session

Once `overall_ready = true` and the session window opens:

1. The session controller calls `start_execution_runtime` automatically.
2. The execution run acquires a run ID and transitions to `"running"`.
3. Signals arrive via `POST /api/v1/strategy/signal` (operator or external system).
4. The orchestrator ticks, processing the outbox and routing broker events.
5. At the session window close, the controller calls `stop` — the run transitions to `"idle"`.

**Signal cap:** `MAX_AUTONOMOUS_SIGNALS_PER_RUN = 100` signals per run. Once reached, Gate 1d refuses further signals with `fault_class = signal.daily_limit_reached`. Alert `day_limit_reached` is visible in `/api/v1/alerts/active`.

**No manual action is required during a normal session.** The operator should monitor truth surfaces (see §7) and intervene only if alerts appear.

---

## 7. Intraday monitoring

Poll these surfaces during the session. The soak harness (`scripts/paper_soak_day.sh`) automates this.

| Surface | What to check |
|---|---|
| `GET /api/v1/system/status` | `runtime_status == "running"`, `deadman_status == "healthy"` |
| `GET /api/v1/autonomous/readiness` | `overall_ready`, `ws_continuity`, `autonomous_history_degraded` |
| `GET /api/v1/alerts/active` | Any new fault signals |
| `GET /api/v1/events/feed` | Autonomous session events, signal admissions, runtime transitions |
| `GET /api/v1/oms/overview` | Execution truth, open orders, outbox health |

---

## 8. WS gap and recovery behavior

When the Alpaca WebSocket disconnects mid-session:
1. `alpaca_ws_continuity` transitions to `GapDetected`.
2. `autonomous_session_truth` is set to the current recovery state (e.g. `RecoveryRetrying`).
3. The WS transport task calls `mark_gap_detected` and attempts reconnection.
4. If reconnection succeeds, continuity returns to `Live` and `RecoverySucceeded` is recorded.
5. If the reconnection fails and a new run start is attempted, `GapDetected` blocks start (BRK-00R-04 gate).

### What the operator sees

During gap:
- `GET /api/v1/autonomous/readiness` → `ws_continuity = "gap_detected"`, `ws_continuity_ready = false`, `overall_ready = false`.
- `GET /api/v1/alerts/active` → `alpaca_ws_gap_detected` signal present.
- `GET /api/v1/events/feed` → `autonomous_session` kind rows with `event_type` showing recovery state.

### Gap recovery after daemon restart (BRK-07R)

At daemon boot, the last persisted broker cursor is loaded:
- **Prior cursor = Live** → demoted to `ColdStartUnproven`. WS must re-establish.
- **Prior cursor = GapDetected** → preserved. The BRK-00R-04 gate immediately blocks start. The operator must resolve the gap (confirm broker positions are clean, cursor is safe) before the autonomous path can resume.
- **No cursor in DB** → `ColdStartUnproven`. Normal cold-start path.

To inspect cursor state after restart:
```
GET /api/v1/autonomous/readiness
```
→ `ws_continuity` field shows the current cursor-derived state.

---

## 9. Supervisor history and `autonomous_history_degraded`

Autonomous session events (start refused, recovery retrying, recovery succeeded, recovery failed, etc.) are persisted to `sys_autonomous_session_events` and surfaced in:

```
GET /api/v1/events/feed
```

Events appear as `kind = "autonomous_session"` rows.

### `autonomous_history_degraded` flag (AUTON-HIST-01)

If the DB is absent or a write fails, the event is dropped silently to execution — **but the flag `autonomous_history_degraded` is set in `/api/v1/autonomous/readiness`**.

| `autonomous_history_degraded` | Meaning | Action |
|---|---|---|
| `false` | All events are persisting normally | None required |
| `true` | At least one event could not be persisted (no DB or write failure) | The events/feed history is incomplete; restart daemon with a working DB to restore durability |

The flag is **sticky** — it is not cleared within the same daemon process lifetime. A clean restart with a working DB resets it.

---

## 10. End-of-day / clean stop

At the configured session window close, the session controller issues a stop automatically.

Verify:
```
GET /v1/status
```
`state == "idle"`, `active_run_id == null`.

```
GET /api/v1/events/feed
```
A `kind = "autonomous_session"` row with `StoppedAtBoundary` should appear (if DB is present).

The daemon remains running and will start again automatically at the next session window open. No operator action is required between sessions.

### Manual override stop

If you need to stop mid-session:
```
POST /v1/run/stop
Authorization: Bearer <MQK_OPERATOR_TOKEN>
```
This stops the run cleanly. The session controller will attempt a new start at the next window open unless you also disarm.

---

## 11. The paper soak harness (AUTON-SOAK-01)

`scripts/paper_soak_day.sh` is the canonical one-day paper soak harness.

### What it does

1. Validates required env vars for Paper + Alpaca.
2. Confirms daemon reachability.
3. Takes a pre-open snapshot of all truth surfaces.
4. Polls truth surfaces every `--intraday-interval-secs` seconds (default 1800 = 30 min).
5. Takes an end-of-day snapshot.
6. Packages all snapshots into a `.tar.gz` review bundle.

### Running it

```bash
MQK_DAEMON_URL=http://127.0.0.1:8899 \
MQK_OPERATOR_TOKEN=<token> \
ALPACA_API_KEY_PAPER=<key> \
ALPACA_API_SECRET_PAPER=<secret> \
ALPACA_PAPER_BASE_URL=https://paper-api.alpaca.markets \
MQK_DATABASE_URL=postgres://... \
bash scripts/paper_soak_day.sh --intraday-interval-secs 1800
```

### Output

```
soak_output/<YYYY-MM-DD_HH-MM-SS>/
  soak_manifest.json          # schema_version="soak-v1"; timestamps, interval, count
  snapshots/
    00_pre_open/              # pre-open truth surfaces
      system_status.json
      preflight.json
      autonomous_readiness.json
      alerts_active.json
      events_feed.json
    01_intraday/ ... NN_intraday/   # one per intraday snapshot
    NN_end_of_day/            # final snapshot
  daemon.log                  # copy of MQK_LOG_FILE (if set)
soak_<timestamp>.tar.gz       # packaged review bundle
```

### What to review after the soak

1. **`autonomous_readiness.json` in each snapshot** — confirm `overall_ready = true` during the session window; `false` outside it is expected. Check `autonomous_history_degraded` is consistently `false`.
2. **`alerts_active.json`** — any `alpaca_ws_gap_detected` signals indicate WS instability. Investigate before repeating the soak.
3. **`events_feed.json` at end-of-day** — confirm `autonomous_session` events appear and the history is complete (no gaps). If degraded, cross-check with daemon logs.
4. **`system_status.json` during session** — confirm `runtime_status == "running"` and `deadman_status == "healthy"` throughout the session window.
5. **Signal count** — check `autonomous_signal_count` field in the status surface to confirm signals were processed.

---

## 12. Checklist: paper day pre-flight

Run this before each autonomous paper day.

- [ ] Daemon is reachable: `GET /v1/health` → `ok = true`
- [ ] `GET /api/v1/autonomous/readiness` → `truth_state == "active"`
- [ ] `ws_continuity == "live"` (WS has connected and proven)
- [ ] `reconcile_ready == true` (no dirty/stale reconcile)
- [ ] `arm_state == "armed"` or `"arm_pending"` (not `"halted"`)
- [ ] `signal_ingestion_configured == true`
- [ ] `autonomous_history_degraded == false` (DB is healthy for event persistence)
- [ ] `GET /api/v1/alerts/active` — no `gap_detected` or `cold_start_unproven` signals
- [ ] DB connectivity: `db_status != "unavailable"` in `GET /api/v1/system/status`
- [ ] Strategy signals are queued and ready for the session window open

---

## 13. Gap / failure recovery decision tree

```
WS = GapDetected at session open?
├── YES
│   ├── Daemon just restarted?
│   │   ├── YES — Prior gap cursor was preserved (BRK-07R).
│   │   │        Check positions are clean.  If clean, manually repair:
│   │   │        use repair_ws_continuity seam (test/recovery path) or
│   │   │        simply confirm Alpaca positions, then restart daemon to
│   │   │        reset cursor to ColdStartUnproven → WS will re-establish.
│   │   └── NO  — WS disconnected mid-session.  Wait for WS reconnect.
│   │             If WS does not recover: inspect DISCORD_WEBHOOK_ALERTS.
│   │             Manual stop if positions are at risk.
└── NO — WS = Live → autonomous path proceeds normally.

autonomous_history_degraded = true?
├── YES — DB absent or write failure.  History incomplete.
│         Restart daemon with working DB for next session.
└── NO  — History is durable.  Events visible in /api/v1/events/feed.

arm_state = "halted"?
├── YES — Halt requires operator investigation.
│         1. GET /api/v1/audit/operator-actions
│         2. GET /control/status
│         3. Disarm + re-arm after investigation.
└── NO  — Arm state is healthy.
```

---

## 14. Stale assumptions corrected

The following assumptions from the older operator_workflows.md §1 are **not correct** for the autonomous path:

| Old assumption | Correct behavior |
|---|---|
| "No auto-arm, auto-start, or auto-mode-change occurs without operator input" | **Auto-arm and auto-start both occur on the autonomous paper path.** The session controller calls `try_autonomous_arm` and `start_execution_runtime` automatically within the session window when all gates pass. |
| "The operator must initiate each action explicitly" | The autonomous path is explicitly designed for unsupervised intraday operation. The operator arms once, then the controller handles per-session start/stop. |
| Arm is always manual | After a clean stop, the DB arm state is `Armed` and the controller will self-arm on the next session tick without operator intervention. Manual arm is only required after a halt or a first-time deployment. |

The §1 statement applies to **non-autonomous (manual)** operation modes.  For the Paper + Alpaca autonomous path, this runbook is the authoritative reference.
