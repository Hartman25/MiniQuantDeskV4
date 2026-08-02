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

This runbook also covers the durable **daily-operation lifecycle** truth
surfaces built by `AUTONOMOUS-DAILY-PAPER-OPERATIONS-01` (Phases D–F1): the
per-market-day operation record, its finalization/outcome/evidence
vocabulary, the five read-only routes that project it, and the operator's
recovery and evidence procedures around it (§15 onward).

**What this runbook does NOT cover:**
- LiveShadow / LiveCapital modes (see `live_shadow_operational_proof.md`)
- Artifact promotion and deployability gating (see `operator_workflows.md` §9)
- CI pipeline and guard scripts (`scripts/guards/`)

---

## 0. Safety boundary

This runbook governs exactly one supported operating lane:

- **Paper + Alpaca only.** No live broker adapter, no live credentials, no
  live capital, on this lane, ever.
- **Single-symbol, long-only, US equity/ETF.** Multi-symbol autonomous
  rollout is not enabled on this lane.
- **Active operator supervision is required.** The unattended 10–20-session
  paper soak has **not started**.
- **Live capital is not ready.** No production/live trading authority exists
  on this lane, and nothing in this runbook grants it.

If any check below reports a different deployment mode, adapter, symbol
scope, or an unattended/unsupervised posture, stop and investigate before
proceeding — do not continue the sequence.

### 0a. Prerequisites

Verify before starting a session:

| Requirement | How to verify |
|---|---|
| Supported host | Windows, PowerShell available (this runbook's commands are PowerShell-first) |
| Repository / commit | `git status` clean or intentionally dirty as expected; `git rev-parse HEAD` matches the commit you intend to run |
| Docker / Postgres (operating DB) | The **operating paper database** runs on host port `5432` (container name and image per `README_TECHNICAL.md` §"Postgres via Docker"), reachable via `MQK_DATABASE_URL=postgres://postgres:postgres@localhost:5432/mqk_dev` (or your configured equivalent). This is **not** the isolated port-`5434` test database and **not** the port-`5440` reality-test lane — see §0b. |
| Paper credentials | `ALPACA_API_KEY_PAPER` / `ALPACA_API_SECRET_PAPER` are set in `.env.local` (never printed; never committed). Never use live Alpaca credentials on this lane. |
| Configuration files | `.env.local` present at repo root (copy from `.env.local.example`); its contents are never displayed by any command in this runbook |
| Daemon | Buildable/runnable via `cargo run --manifest-path .\core-rs\Cargo.toml -p mqk-daemon`; binds `127.0.0.1:8899` by default |
| GUI | Buildable/runnable via `npm run dev` in `core-rs\mqk-gui` (or the packaged desktop shortcut, `Launch-VeritasLedger.ps1`) |
| System clock | Accurate — the session window, market-date resolution, and daily-operation slot lookups are all UTC-clock-derived |
| Exchange calendar / session awareness | NYSE regular session is the default session-window truth (§3); confirm no unexpected calendar override env vars are set |

Never display `.env.local` contents in any command output, log, or evidence
capture — see §15.8's read-only capture tooling for the enforced equivalent.

### 0b. Operating database vs. test/reality-test databases (do not confuse these)

| Lane | Host port | Purpose | Use for operator sessions? |
|---|---|---|---|
| Operating paper DB | `5432` | The durable database an actual daemon session in this runbook reads/writes | **Yes — this is the one** |
| Isolated test DB | `5434` | Used only by `cargo test` scenario binaries and CI guards | No — never point a running operator daemon at this |
| Manual proof DB | `55432` | Manual one-off proof/bootstrap work (`scripts/db_proof_bootstrap.sh`) | No |
| Reality-test DB | `5440` | `autonomous_reality_test_paper.ps1.ps1`'s own isolated snapshot/crash-recovery harness | No — never use as the operating paper database |

Before trusting any of the ports above, run `docker ps` and confirm what is
actually listening — see `README_TECHNICAL.md` §"Verify ports before
trusting any default above" for the full caution (a stale host-side port
forward can otherwise make a correct password look like an authentication
failure).

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

# Risk gate — REQUIRED for any orders to be submitted (AUTON-PAPER-BLOCKER-01)
# Without these the risk gate fails closed and NO orders will be placed even
# when the run is active. Set both; use values appropriate to your paper account.
MQK_RISK_INITIAL_EQUITY_USD=100000        # paper account equity in USD
MQK_RISK_DAILY_LOSS_LIMIT=0.02            # fraction of equity (0 < r < 1)

# Native strategy fleet — REQUIRED for autonomous bar-driven signal generation
MQK_STRATEGY_IDS=intraday_scalper         # built-in engine name
MQK_STRATEGY_SYMBOL=SPY                   # ticker symbol

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
3. Clears the halted run record: `POST /api/v1/ops/action {"action_key": "clear-halted-run"}`.
   This transitions the durable run record from HALTED → STOPPED so a fresh start is not blocked.
   The action is available in the ops catalog (`GET /api/v1/ops/catalog`) only when a HALTED run exists.
4. Re-arms: `POST /api/v1/ops/action {"action_key": "arm-execution"}` (or `POST /v1/integrity/arm`).

Do not skip step 3.  Without it the daemon will find the prior run in HALTED state and refuse a new start.

---

## 5. Pre-session readiness checks

Run these before the session window opens on any new paper day.

**If the repo has been edited since the last session** (new commits, patches,
doc edits), validate the repo first:

```powershell
powershell -ExecutionPolicy Bypass -File scripts\windows\Invoke-PaperPremarketValidation.ps1
```

This runs script guards, GUI tests/build, and targeted Rust baseline/reconcile
tests. It is **not** `full_repo_proof.ps1` and is much cheaper -- and it does
not start, build-and-run, or arm the daemon, clear a halted run, flatten,
trade, run a market smoke, or call Discord/broker. See
`operator_control_surface.md` §0 for the full check list and flags. Proceed
to the checks below only on `FINAL: PASS`.

**Before the daemon is started at all** (e.g. before double-clicking the
desktop icon), an optional read-only preflight is available:

```powershell
powershell -ExecutionPolicy Bypass -File scripts\windows\Launch-VeritasLedger.ps1 -CheckOnly
```

This reports `.env.local` presence (never its contents), Docker / paper-DB
status, daemon and GUI binary presence, AAPL/5m `md_bars` freshness, the
persisted `sys_arm_state`, and -- if a daemon is already reachable --
`live_routing_enabled` / `runtime_status` / `kill_switch_active` / reconcile
status, plus a "Next action" recommendation. It does not start, build, arm,
or call the daemon, GUI, broker, or Discord. The desktop shortcut itself is
unchanged -- double-clicking it (no `-CheckOnly`) still performs the normal
startup. See `operator_control_surface.md` §1 for the full field list.

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

### 5e. Multi-symbol smoke preflight gate (manual smoke runner only)

If running a multi-symbol smoke via `scripts\windows\Start-PaperTradingSmoke.ps1`, STEP 9B
gates entry to STEP 10+ on `GET /api/v1/watchlist/status`. The gate fails closed unless all
of the following hold: `schema_version == "watchlist-v2"`, `symbols` count `> 1`,
`approved_for_autonomous_paper == true`, and `approved_for_live == false`. See
`docs/design/native_multi_symbol_dispatch.md` §9.1 and
`docs/runbooks/paper_smoke_evidence_pack.md` §5d for the stable
`MULTI_SYMBOL_SMOKE_BLOCKED_*` codes and evidence-on-refusal behavior. This gate does not
apply to the autonomous session controller itself.

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

At daemon boot, `seed_ws_continuity_from_db` loads the last persisted broker
cursor and derives the boot-time continuity state from it:
- **Prior cursor = Live** → demoted to `ColdStartUnproven`. `Live` is not
  earned until the WS subscription is reconfirmed after restart.
- **Prior cursor = GapDetected** → preserved as `GapDetected`. The
  BRK-00R-04 gate immediately blocks a new run start.
- **No cursor in DB, or a cursor parse failure** → `ColdStartUnproven`
  (a parse failure is treated fail-closed).

**A persisted `GapDetected` condition remains fail-closed across a daemon
restart, and restarting the daemon is not itself a repair.** Restarting does
not clear a persisted gap by itself — the gap clears only through the WS
transport task's own reconnection. That task starts automatically at daemon
boot, independent of whether any run is active, and continuously attempts to
(re)establish the Alpaca `trade_updates` stream with backoff between
attempts. When it successfully authenticates and the server confirms
subscription, it repairs the cursor to `Live` (BRK-08R) — whether it started
from `GapDetected` or `ColdStartUnproven` — and this is the supported
recovery path. It requires no operator command beyond keeping the daemon
running and waiting for, or independently verifying, that it completes.

**Before resuming supervision after any gap, the operator must:**
1. Wait for, or independently verify, the WS transport task's successful
   reconnection — confirm `ws_continuity == "live"` via
   `GET /api/v1/autonomous/readiness` — rather than assume recovery from the
   passage of time alone.
2. Inspect broker positions, reconcile status, and risk posture
   (`GET /api/v1/portfolio/positions`, `GET /api/v1/reconcile/status`,
   `GET /api/v1/reconcile/mismatches`, `GET /api/v1/risk/denials`) before
   resuming or trusting the session. A repaired cursor that came from a
   `GapDetected` state only recovers fill events via REST catch-up —
   non-fill lifecycle events (Ack/CancelAck/ReplaceAck/Reject) from the gap
   window are permanently unrecoverable from the Alpaca REST API, so this
   inspection step is required, not optional.
3. If recovery cannot be proven — WS will not re-establish, or reconcile /
   risk posture cannot be confirmed clean — keep the lane stopped or halted
   rather than forcing a start. Do not bypass the BRK-00R-04 gate.

**There is no operator-invocable "repair" command for WS continuity.** The
only function with a similar name,
`repair_ws_continuity_from_persisted_cursor_for_test`, exists solely to seed
test fixtures in the Rust test suite — it is not reachable from any daemon
route or operator surface. Never treat it as, and never attempt to invoke it
as, a production operator procedure. Similarly, never restart the daemon
*for the purpose of* resetting `GapDetected` to `ColdStartUnproven` — the
cursor state that drives the BRK-00R-04 gate is derived from the
DB-persisted cursor, which a restart does not clear (see the table above: a
persisted `GapDetected` cursor survives restart unchanged).

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

## 11. The paper soak harness (AUTON-SOAK-01) — legacy/reference tooling

> **Boundary.** `scripts/paper_soak_day.sh` is legacy/reference tooling
> only. It is **not** the currently authorized unattended or canonical
> Bundle 3 evidence process, running it is **not** authorization to begin
> the unattended soak, and a run of it is **not** evidence that a
> supervised session or the unattended soak has completed. Current Bundle 3
> supervised-evidence preparation uses `scripts/soak/` (§21–§22 below);
> future supervised captures remain operator-controlled. This section is
> preserved as historical documentation of the older harness, bounded by
> this warning — it is not superseded content to delete.

`scripts/paper_soak_day.sh` is an existing one-day paper soak harness.

### What it does

1. Validates required env vars for Paper + Alpaca.
2. Confirms daemon reachability.
3. Takes a pre-open snapshot of all truth surfaces.
4. Polls truth surfaces every `--intraday-interval-secs` seconds (default 1800 = 30 min).
5. Takes an end-of-day snapshot.
6. Packages all snapshots into a `.tar.gz` review bundle.

### Running it

Do not place credentials directly on the command line or in shell history.
Populate `.env.local` (§2) — the existing secured environment/configuration
source — and load it into your shell session through your normal
environment-loading mechanism before running:

```bash
bash scripts/paper_soak_day.sh --intraday-interval-secs 1800
```

`MQK_DAEMON_URL` is optional and, if overridden from its default, is not a
credential and may be set inline.

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

- [ ] If repo edited since last session: `Invoke-PaperPremarketValidation.ps1` → `FINAL: PASS`
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
│   ├── A persisted GapDetected condition remains fail-closed across
│   │   restart (§8) — this holds whether the daemon just restarted or the
│   │   gap occurred mid-session.  Restarting the daemon is not itself a
│   │   repair, and there is no operator-invocable repair command.
│   │   1. Wait for, or independently verify, the WS transport task's own
│   │      automatic reconnection (`ws_continuity == "live"` in
│   │      `GET /api/v1/autonomous/readiness`).  This happens on its own —
│   │      do not attempt to force it.
│   │   2. Inspect broker positions, reconcile status, and risk posture
│   │      before resuming or trusting the session (§8 step 2).
│   │   3. If WS does not recover, or positions/reconcile/risk cannot be
│   │      confirmed clean: inspect DISCORD_WEBHOOK_ALERTS if configured;
│   │      keep the lane stopped or halted; manual stop if positions are at
│   │      risk.  Do not bypass the BRK-00R-04 gate and do not start a new
│   │      run until recovery is proven.
└── NO — WS = Live → autonomous path proceeds normally.

autonomous_history_degraded = true?
├── YES — DB absent or write failure.  History incomplete.
│         Restart daemon with working DB for next session.
└── NO  — History is durable.  Events visible in /api/v1/events/feed.

arm_state = "halted"?
├── YES — Halt requires operator investigation.
│         1. GET /api/v1/audit/operator-actions
│         2. GET /control/status
│         3. POST /api/v1/ops/action {"action_key": "disarm-execution"}
│         4. POST /api/v1/ops/action {"action_key": "clear-halted-run"}
│            (transitions run record HALTED → STOPPED; required before re-arm)
│         5. POST /api/v1/ops/action {"action_key": "arm-execution"}
└── NO  — Arm state is healthy.
```

---

## 14. Stale assumptions corrected

The following assumptions from the older operator_workflows.md §1 are **not correct** for the autonomous path:

| Old assumption | Correct behavior |
|---|---|
| "No auto-arm, auto-start, or auto-mode-change occurs without operator input" | **Auto-arm and auto-start both occur on the autonomous paper path.** The session controller calls `try_autonomous_arm` and `start_execution_runtime` automatically within the session window when all gates pass — these are scheduled runtime actions the controller performs on its own, without a per-run operator click. |
| "The operator must initiate each action explicitly" | The controller performs scheduled per-session start/stop actions automatically. **This does not mean the lane is authorized to run unattended.** Active operator supervision is still required for the supported Paper + Alpaca lane (§0) — the controller autonomously executing routine start/stop actions is not the same thing as an unsupervised/unattended authorization. The unattended 10–20-session paper soak has **not started** and is not authorized by this runbook. |
| Arm is always manual | After a clean stop, the DB arm state is `Armed` and the controller will self-arm on the next session tick without operator intervention. Manual arm is only required after a halt or a first-time deployment. This automatic re-arm is a scheduled runtime action, not a grant of unsupervised operation. |

**No routine intervention expected during a healthy session is not the same
claim as supervision being unnecessary.** That a healthy session requires no
routine operator clicks (§6) does not mean operator supervision is
unnecessary — the operator is still expected to be actively monitoring the
truth surfaces in §7 and Part 2 §18 throughout the session, per the §0
safety boundary.

The §1 statement applies to **non-autonomous (manual)** operation modes. For
the Paper + Alpaca autonomous path, this runbook is the authoritative
reference — the supported lane is supervised automatic start/stop, never
unattended or unsupervised operation.

---

# Part 2 — Daily-Operation Lifecycle Truth (AUTONOMOUS-DAILY-PAPER-OPERATIONS-01)

Part 1 above (§§0–14) covers the **session controller**: per-day arm/start/
stop of the execution runtime itself. This part covers the **durable
daily-operation record** — the per-market-day, per-(deployment_mode,
adapter_id) lifecycle row built by Phases D–F1 of
`AUTONOMOUS-DAILY-PAPER-OPERATIONS-01` that tracks coverage, evaluation
activity, finalization, and outcome classification for that day, independent
of any single process's in-memory state. Durable database state — not
process memory — is the lifecycle authority for everything in this part.

## 15. Start-of-day sequence

Run these in order at the start of every supervised paper day.

1. **Start or verify the operating database** (host port `5432`, §0b):
   ```powershell
   docker ps --filter "name=mqk-postgres-dev"
   ```
   If not running, start it per `README_TECHNICAL.md` §"Postgres via
   Docker". Do not start or point at the port-`5434` test container or the
   port-`5440` reality-test container.

2. **Check schema/migration readiness (read-only)**:
   ```powershell
   $env:MQK_DATABASE_URL = "postgres://postgres:postgres@localhost:5432/mqk_dev"
   cargo run --manifest-path .\core-rs\Cargo.toml -p mqk-cli -- db status
   ```
   `db status` reports pending migrations without applying them. Only run
   `db migrate` if you have independently confirmed it is expected and safe
   for this deployment — this runbook does not authorize routine migration
   application as part of daily startup.

3. **Start the daemon**:
   ```powershell
   $env:MQK_DATABASE_URL = "postgres://postgres:postgres@localhost:5432/mqk_dev"
   cargo run --manifest-path .\core-rs\Cargo.toml -p mqk-daemon --bin mqk-daemon
   ```
   Binds `127.0.0.1:8899` by default.

4. **Start the GUI**:
   ```powershell
   cd core-rs\mqk-gui
   npm run dev
   ```
   Or launch the packaged desktop shortcut. The GUI defaults to daemon URL
   `http://127.0.0.1:8899` (override with `VITE_MQK_DAEMON_URL` or the GUI's
   saved daemon URL setting).

5. **Confirm daemon connectivity**:
   ```
   GET /v1/health
   ```
   Must return `ok = true`.

6. **Confirm paper deployment mode and Alpaca paper adapter**:
   ```
   GET /api/v1/system/status
   ```
   Confirm `daemon_mode == "paper"` and the adapter identity reflects
   Alpaca (`alpaca_ws_continuity` present, not `"not_applicable"`).

7. **Confirm live routing is disabled**:
   ```
   GET /api/v1/system/preflight
   ```
   Confirm the live-routing gate reports disabled/blocked for this
   deployment mode (paper mode never permits live routing; see
   `operator_control_surface.md` for the full live-routing gate matrix if
   this ever reads otherwise — do not proceed if it does).

## 16. Required read-only checks — the five authoritative routes

These five routes are the required read-only truth surfaces for a
supervised session. All are `GET`, all are safe to poll repeatedly, none
mutate state:

```
GET /api/v1/autonomous/readiness
GET /api/v1/autonomous/paper-status
GET /api/v1/system/preflight
GET /api/v1/autonomous/daily-operation
GET /api/v1/autonomous/daily-operations?limit=20
```

The first three carry an additive `daily_operation` summary block
(`AutonomousDailyOperationSummary`) built from the same projection as the
last two. Full route contract:
`docs/specs/autonomous_daily_paper_operations_01e4_read_only_daily_operation_api.md`.

### 16a. Top-level route truth_state vocabulary

```
active               -- operation row (and its required read-model fields) queried successfully
not_found             -- DB reachable, no operation row exists for the requested/current slot
backend_unavailable   -- no DB pool configured
query_failed          -- DB pool present but a required read failed
```

A route reporting `active` means the operation row (and its required
read-model fields) was queried successfully — this is the only truth_state
under which the projected fields (finalization status, outcome, evidence)
should be treated as authoritative for the requested slot.

**`not_found` is not a backend failure.** It means the DB answered and no
row exists yet for today's slot (e.g. before the session has started) — this
is expected and healthy at the start of a day. Do not treat it as degraded.

**Null counts are unavailable, not zero.** `strategy_evaluation_count`,
`order_activity_count`, and `fill_count` are `null` (rendered "Unavailable"
in the GUI) whenever the underlying full-run-lineage read could not be
completed — never fabricate `0` from a `null` count, and never read a `null`
count as "confirmed no activity."

### 16b. Finalization-status vocabulary

```
not_yet_eligible               -- operation is not yet stopped/eligible for finalization
awaiting_finalization           -- durably stopped, awaiting the classifier/finalizer to run
blocked_insufficient_evidence   -- evidence_degraded and durably stopped; finalization blocked
finalized                       -- a terminal outcome has been durably committed
```

### 16c. Outcome/evidence posture

A `finalized` operation carries `outcome_class` (`no_trade` |
`with_activity` | `completed`) and `evidence_state` (`complete` | `pending`
| `degraded` | `unavailable`). **Generic `completed` is not automatic
no-trade/activity proof** — it is the out-of-scope manual/administrative
terminal path (E1 contract §2), never treated by this runbook or by the GUI
as equivalent evidence-completeness to the two automatic classifier terminal
states (`no_trade`, `with_activity`), even when its own `evidence_state`
happens to read `"pending"` rather than `"complete"`. Do not report a
generic `completed` day as an automatically-verified no-trade day.

### 16d. Durable portfolio and P&L truth (DURABLE-PAPER-PORTFOLIO-AND-PNL-01)

Three additional read-only routes surface restart-surviving portfolio/P&L
truth, distinct from the older in-memory-only broker-snapshot routes
(`GET /api/v1/portfolio/summary`, `/positions` — reset on every daemon
restart):

```
GET /api/v1/portfolio/durable-summary[?run_id=]
GET /api/v1/portfolio/durable-positions[?run_id=]
GET /api/v1/portfolio/durable-snapshots?limit=20
```

**How to tell the two apart.** The in-memory routes answer "what does the
currently-running process believe right now" and go blank/unknown the
instant the daemon restarts. The durable routes answer "what was the last
authoritative Paper+Alpaca broker truth this system captured, and what
does the durably-replayed fill history prove about it" — both survive a
restart, both are backed by Postgres, not process memory. Never read one as
a substitute for the other; the GUI's Portfolio screen renders them as two
separate panels for exactly this reason.

**Truth-state vocabulary added:**

```
snapshot_truth_state:    active | snapshot_unavailable | snapshot_stale | db_unavailable | query_failed
accounting_truth_state:  active | fill_history_incomplete | not_found | db_unavailable | query_failed
```

`fill_history_incomplete` means the durable fill history known to this
system does not fully explain a nonzero broker-reported position — most
commonly a position adopted from before this system's fill history began
(see §22's restart-adoption note below). **This is never silently fixed by
inventing an opening fill.** The position itself remains visible (from the
durable snapshot) even while `realized_pnl` stays `null` with an explicit
reason. If you see `fill_history_incomplete` and did not expect an adopted
position, investigate before trusting any displayed realized P&L for that
symbol — there is none to trust yet.

`GET /api/v1/execution/paper-lifecycle`'s `portfolio_truth_state`/
`pnl_truth_state` fields now read this same durable truth (previously a
hardcoded placeholder). Its `overall_lifecycle_state` gains two new values
once a fill has been seen: `order_filled_portfolio_durable_pnl_available`
(durable accounting is complete) and
`order_filled_portfolio_durable_pnl_incomplete` (durable accounting exists
but its epoch is incomplete) — both refine the older
`order_filled_pnl_pending` (no durable accounting row yet).

**Do not trust or report P&L when:** `snapshot_truth_state` is anything
other than `active`, `accounting_truth_state` is anything other than
`active`, or `realized_pnl`/`unrealized_pnl`/`daily_pnl` is `null` — a
`null` financial value is unavailable truth, never zero.

## 17. Before-session checklist

- [ ] `daemon_mode == "paper"` (§15.6)
- [ ] Alpaca paper adapter confirmed active, not a live adapter
- [ ] Live routing confirmed disabled (§15.7)
- [ ] Broker/account connectivity: `GET /api/v1/autonomous/readiness` →
      `ws_continuity == "live"` (or proven at session open)
- [ ] Database connectivity: `db_status != "unavailable"` in
      `GET /api/v1/system/status`
- [ ] Calendar/session truth: `session_in_window` / `session_window_state`
      reflect the expected NYSE session (§3)
- [ ] Daily-data readiness: upstream market-data readiness gate is green
      (per `docs/runbooks/intraday_market_data_refresh.md` if applicable)
- [ ] Watchlist/promotion readiness (if running a promoted strategy):
      `GET /api/v1/watchlist/status`
- [ ] Completed-bar task health: no persistent
      `CoverageAuthorityUnavailable` / adapter fault visible for the
      completed-bar dispatch task
- [ ] Risk and reconcile posture: `reconcile_ready == true`,
      `kill_switch_active == false`
- [ ] No unexpected orders or positions:
      `GET /api/v1/portfolio/positions`, `GET /api/v1/oms/overview`
- [ ] `GET /api/v1/autonomous/daily-operation` for today: either
      `not_found` (expected pre-session) or an existing non-finalized row —
      never a `finalized` row for today before the session has run

## 18. During-session supervision

**GUI screens to monitor:**
- **Daily Operations** — durable daily-operation state, finalization
  status, outcome/evidence posture, and recent history (§16). Read-only;
  no controls.
- **Session** — session-controller arm/window state (Part 1).
- **Ops / Dashboard** — runtime status, deadman health, alerts.

**What to watch:**
- Daily-operation state progression: expect a non-finalized row to appear
  once the session starts, then remain `not_yet_eligible` through the
  session, then `awaiting_finalization` shortly after session close, then
  `finalized` once the classifier/finalizer runs.
- Completed-bar dispatch/session and order/fill visibility via the existing
  Part 1 §7 surfaces (`system/status`, `alerts/active`, `events/feed`,
  `oms/overview`).
- Risk and reconcile visibility: no unexpected `kill_switch_active`
  transitions; `reconcile_ready` stays true.
- Blocker interpretation: an `evidence_degraded` state with
  `finalization_status == "blocked_insufficient_evidence"` means
  finalization is durably blocked pending investigation — see §19.

**When not to intervene:** a non-finalized state during the session window
(`not_yet_eligible`), a `not_found` result before session start, or a
`query_failed`/`backend_unavailable` reading that self-resolves on the next
poll are all expected transient states. Do not take a recovery action (§19)
on the basis of a single poll — confirm the condition persists across at
least two polling cycles first.

## 19. Recovery procedures

Bounded operator responses only. **Never**: manually rewrite a
`sys_autonomous_daily_operations` row, force a terminal outcome, bypass a
blocker, or manually create coverage or evaluation evidence. There is no manual finalization command, and none should ever be invented or run against the database directly.

| Condition | Bounded response |
|---|---|
| Preflight blocker (`GET /api/v1/system/preflight` reports a gate false) | Identify the specific failing gate from the response; resolve the underlying cause (e.g. WS not yet live, DB unreachable); do not start a run until the gate clears |
| `evidence_state == "degraded"` | Investigate the specific `evidence_blockers` codes (bounded closed vocabulary); confirm whether the underlying read-model gap (lineage/DB) is transient; do not attempt to manually supply evidence |
| `finalization_status == "awaiting_finalization"` persisting unusually long | Confirm the coordinator is ticking (daemon alive, not crashed); this is expected to resolve on the next coordinator tick — do not force finalization |
| Completed-bar task failure | Check `GET /api/v1/system/status` / alerts for the specific adapter fault; restart the daemon if the task appears stuck; do not manually insert bar-dispatch or evaluation rows |
| Runtime interruption (daemon crash mid-session) | Restart the daemon; durable state resumes from DB (§21); do not assume the prior in-memory run state |
| Reconcile mismatch | Follow `operator_control_surface.md`'s reconcile investigation steps; do not flatten or clear state until the mismatch is understood |
| Risk halt (`kill_switch_active == true`) | Follow Part 1 §4 "After a halt" recovery sequence (investigate → disarm → clear-halted-run → re-arm); this does not touch the daily-operation record, which remains durable and unaffected |
| Database/API unavailable (`backend_unavailable` / `query_failed`) | Confirm DB container is healthy (`docker ps`, §0b); restart the daemon once DB is confirmed reachable; do not treat this as a finalized/degraded operation outcome — it is a transport/backend truth distinct from operation truth |
| Daemon restart needed | Stop the daemon process; restart per §15.3; verify §15.5–15.7 again before resuming supervision |
| GUI restart needed | Restart per §15.4; the GUI has no independent state — a restart only re-establishes polling, it never changes daemon/DB truth |

## 20. Stop and emergency posture

- **Normal end-of-session observation**: at session close, the session
  controller stops the runtime automatically (Part 1 §10). Confirm
  `GET /v1/status` → `state == "idle"`, then confirm the daily-operation
  record transitions toward `awaiting_finalization` and eventually
  `finalized` (§16b) — this may take one or more coordinator ticks after
  the runtime stop.
- **Supervised runtime stop** (mid-session, operator-initiated):
  ```
  POST /v1/run/stop
  Authorization: Bearer <MQK_OPERATOR_TOKEN>
  ```
  See Part 1 §10 "Manual override stop". This durably records a stop; the
  daily-operation record's finalization eligibility is governed by
  `stopped_at_utc` on the durable row, not by process memory.
- **Flatten availability and blockers**: see `operator_control_surface.md`
  §4 for the full flatten procedure and blocker table
  (`flatten_available`, `flatten_blockers`, `live_routing_enabled`).
  Flattening is a session-controller/execution action; it does not itself
  finalize or alter the daily-operation record.
- **Kill switch / risk halt**: `POST /api/v1/ops/action
  {"action_key": "kill-switch"}` — see Part 1 §4 and
  `operator_control_surface.md` §5 for the full emergency-abort procedure.
- **Evidence preservation before restart**: capture the end-of-day evidence
  set (§21) before restarting the daemon whenever a restart follows an
  unusual condition (halt, evidence-degraded, reconcile mismatch) — durable
  state survives the restart, but point-in-time surface snapshots do not.

## 21. End-of-day evidence

Capture (read-only) at end of day, or before any restart following an
unusual condition:

- Repository commit (`git rev-parse HEAD`)
- `GET /api/v1/autonomous/daily-operation` (today's operation)
- `GET /api/v1/autonomous/daily-operations?limit=20` (recent operations)
- `GET /api/v1/autonomous/readiness`
- `GET /api/v1/autonomous/paper-status`
- `GET /api/v1/system/preflight`
- `GET /api/v1/system/status`
- Orders and fills (`GET /api/v1/oms/overview`, `GET /api/v1/portfolio/positions`)
- Durable portfolio/P&L truth (`GET /api/v1/portfolio/durable-summary`,
  `/durable-positions`, `/durable-snapshots?limit=20` — §16d)
- Risk posture (`GET /api/v1/risk/denials`)
- Reconcile posture (`GET /api/v1/reconcile/status`, `GET /api/v1/reconcile/mismatches`)
- The day's outcome class and evidence-blocker reasons (from the
  daily-operation row itself)
- Relevant daemon logs for the session window

See §22 (F3) for the read-only capture tooling that automates this list
into a single evidence manifest. Never include `.env.local` contents or any
credential in captured evidence.

## 22. Restart distinctions

Durable database state — never process memory — is lifecycle authority for
all three of the following:

| Restart scenario | What is durably true | What the operator should expect |
|---|---|---|
| Restart after a durable stop but **before finalization** | `stopped_at_utc` is set; `finalization_status` is `awaiting_finalization` or `not_yet_eligible` | On restart, the coordinator resumes ticking; finalization proceeds from durable state on its own schedule — no operator action re-triggers it |
| Restart after a **terminal commit** (already `finalized`) | `outcome_class` / `finalized_at_utc` are durably set and immutable for that day | The daily-operation row for that market date is read-only history from this point; a restart does not reopen or re-finalize it |
| Restart after an **evidence blocker** (`blocked_insufficient_evidence`) | The blocker and its reason codes are durably recorded | On restart, investigate per §19's `evidence_state == "degraded"` row before assuming the blocker will self-clear; it will only clear if the underlying evidence read-model condition resolves |
| Restart with an **already-adopted broker position** and no fill history in this system for it | The durable snapshot shows the position (from real broker truth); the durable accounting row (if any) reports `accounting_epoch: "incomplete"` for that symbol | Position quantity/cost basis remain trustworthy (broker-sourced); do not trust `realized_pnl` for that symbol until enough fill history accumulates to fully explain it — never manually patch the accounting row to force `"complete"` |

## 22a. How the durable portfolio/P&L surface fits the daily-P&L baseline flow

The existing daily-P&L baseline (`sys_account_equity_baseline`, its capture
action, and `daily_pnl` on `GET /api/v1/portfolio/summary`) is **unchanged**
by Bundle 4 and remains the sole source of `daily_pnl`. The new durable
routes (§16d) read that same existing baseline via the same existing
`resolve_daily_pnl` logic — they do not recompute daily P&L from the fill
ledger. Realized P&L (fill-derived, FIFO) and daily P&L (baseline-derived,
equity-delta) are two independent figures that will not generally agree —
this is expected, not a discrepancy to reconcile.

## 23. Explicit prohibitions

- Do not use live Alpaca credentials on this lane, under any circumstance.
- Do not enable live mode / live routing.
- Do not use the port-`5434` test database, or the port-`5440`
  reality-test database, as the operating paper database (§0b).
- Do not bypass a preflight, evidence, or finalization blocker.
- Do not manually rewrite `sys_autonomous_daily_operations` rows or invent
  a manual finalization command.
- Do not manually edit `sys_paper_portfolio_snapshots`,
  `sys_paper_portfolio_snapshot_positions`, or
  `sys_paper_portfolio_accounting_state` rows, and do not invent a
  synthetic opening fill to force `accounting_epoch` from `"incomplete"` to
  `"complete"` — an incomplete epoch reflects genuinely incomplete fill
  history and must be resolved by accumulating real fills, not by editing
  the row.
- Do not interpret empty/`null` data without checking its `truth_state` —
  an empty or `null` field is meaningless without the surface's own
  truth-state qualifier (§16a).
- Do not begin unattended (unsupervised) operation. The unattended
  10–20-session soak has not started and is not authorized by this runbook.

# Part 3 — Bundle 7 Dynamic Selection Formal Soak Contract (DYNAMIC-STRATEGY-SYMBOL-SELECTION-01-PHASE-7C)

This Part governs the formal, actively-supervised soak required before
Bundle 7's `paper_enforced` dynamic strategy/symbol selection may be
considered for any expanded operating posture. It is a **prerequisite to,
and strictly narrower than,** the unattended 10–20-session soak referenced
throughout Part 1 and §23 — completing this Part's sessions does not begin,
authorize, or shorten that later unattended milestone. Live capital remains
unauthorized on every lane this runbook governs, in this Part and every
other.

### 24. Accepted baseline SHA

- A formal soak session may only be counted against a specific, named
  accepted commit SHA (the SHA the operator/reviewer explicitly accepted
  Bundle 7 Phase 7C closure at).
- Before starting any session, verify `git rev-parse HEAD` on the deployed
  worktree equals that accepted SHA exactly. A session run against any other
  SHA — including a SHA that is a superset of accepted commits, or one
  commit ahead for an unrelated fix — is not a countable session under this
  Part.
- **No pre-final-SHA session counts.** Any session run before the accepted
  SHA existed (including sessions run during development, against a draft
  branch, or against an earlier partial Bundle 7 patch) counts zero toward
  the required session count in this Part. The count starts fresh at zero
  the first time a session is run against the accepted SHA.
- If the accepted SHA changes (a follow-up repair patch lands and is
  separately accepted), the session count resets to zero under the new SHA
  unless the operator explicitly records a reasoned exception before
  resuming — never a silent carry-forward.

### 25. Required clean session count

- **Five (5) consecutive clean sessions** are required before dynamic
  selection may be considered for any posture beyond active, supervised
  Paper + Alpaca operation under this runbook. This preserves the existing
  five-session initial requirement already established for this lane; it is
  not loosened or tightened by this Part.
- "Consecutive" means no immediate invalidator (§27) fired between the
  start of session *N* and the start of session *N+1* — a gap for a
  legitimate non-trading day (market holiday, weekend) does not break
  consecutiveness by itself, provided no invalidator fired during the gap.
- An invalidated session (§27) resets the count to zero. It does not merely
  pause it.

### 26. Definition of one session

One session is exactly one supervised Paper + Alpaca autonomous daily
operation (§15–§17) that follows the **formal two-stage gate sequence**
(FORMAL-SOAK-GATE-TRUTH-REPAIR-01):

1. Run `Invoke-Bundle7Phase7cPremarketValidation.ps1 -Stage PreStart` to a
   genuine `FINAL: PASS` before the runtime starts a run, using the
   operator-supplied paper database (`-AllowNonTestDbPort -Environment
   Paper`, never port 5434 or 5440 — §0b, §23). A PreStart PASS proves it is
   safe to start; it writes only a `bundle7_prestart_readiness_manifest.json`
   artifact, which explicitly states `run_id`/`plan_id` are not yet
   committed and cannot authorize or count a session on its own.
2. Start the paper runtime through the existing procedure (§15–§17), with
   `MQK_DYNAMIC_STRATEGY_SYMBOL_SELECTION_MODE=paper_enforced` and the live
   lock resolving to `paper_enforced` (never demoted to `off`) — verified
   via `GET /api/v1/dynamic-selection/status`'s `committed_effective_mode`
   after start.
3. Run `Invoke-Bundle7Phase7cPremarketValidation.ps1 -Stage ActiveCommit`
   to a genuine `FINAL: PASS` after the run has started — this stage proves
   the durable committed truth of the run itself (exactly one committed
   active run/lease, committed disposition/mode/plan/evidence, API-vs-DB
   agreement, every selected binding's exact fresh bar window) by real
   value, never endpoint reachability alone.
4. **Only an ActiveCommit `FINAL: PASS` creates the countable session
   manifest** (`bundle7_soak_session_manifest.json`). A PreStart PASS alone
   never counts a session, regardless of what happens afterward.
5. Only after that manifest is written may the session begin counting
   toward §25's five-session requirement.
6. A restart or a new run during the session requires a fresh ActiveCommit
   gate run and a fresh manifest — a manifest from a prior run/plan_id is
   never reused or treated as still covering a new run (§31).
7. An `ActiveCommit FINAL: FAIL` at any point invalidates the session (§27)
   — a session is never counted on PreStart evidence alone, no matter how
   long the run remained active afterward.

Is actively supervised end-to-end (§0) — never unattended. Reaches a clean
end-of-day stop (§10) or an explicit, logged operator halt for a reason
unrelated to Bundle 7 evidence/dispatch correctness (e.g. a scheduled
infrastructure maintenance halt) — a halt caused by any Bundle 7
evidence/validation/dispatch defect is an invalidator (§27), not a
countable clean session.

### 27. Immediate invalidators

Any of the following during a session immediately invalidates it (resets
the count to zero, §25) and requires operator investigation before the next
session may start:
- `evidence_validation_state` is ever anything other than `valid` while a
  committed plan exists (per `GET /api/v1/dynamic-selection/status`).
- `approved_for_live` is ever observed `true` anywhere (API, GUI, or DB) —
  this is a hard defect, not a configuration mistake, and must be treated
  as a stop-everything incident, not merely a session failure.
- A durable evidence write failure, payload collision, or read-side
  validation failure blocks a `paper_enforced` start (working as designed —
  but it means the session never started under valid dynamic-selection
  evidence, so it does not count).
- The final Bundle 7 guard (`check_bundle7_phase7c_final_closure.ps1`) or
  either stage of the premarket validator (Part 7) fails when re-run
  against the session's own commit.
- The `ActiveCommit` gate ever reports `FINAL: FAIL` for the session's
  active run — a `PreStart FINAL: PASS` alone never excuses this; the
  session was never formally committed and does not count (§26).
- Any selected-host dispatch discrepancy: a fill, order, or signal
  evaluation attributable to a symbol/strategy/timeframe binding not present
  in the committed plan's selected bindings.
- An unattended gap (loss of active operator supervision) of any duration.

### 28. Per-session evidence

For each session, capture and retain (mirroring the existing session
evidence capture convention, §11, `scripts/soak/`):
- The `-Stage PreStart` validator's full output and the
  `bundle7_prestart_readiness_manifest.json` artifact it wrote, under
  `smoke_logs/` (never staged).
- The `-Stage ActiveCommit` validator's full output and the countable
  `bundle7_soak_session_manifest.json` it wrote, under `smoke_logs/` (never
  staged) — this is the file that actually proves the session counts;
  retain it even if the PreStart artifact above is also retained.
- `GET /api/v1/dynamic-selection/status` and `GET /api/v1/dynamic-
  selection/plans/:plan_id` (for the committed plan) captured at least once
  pre-session and once post-session.
- The standard end-of-day evidence already required by §21.
- A one-line operator note recording session number, accepted SHA, and
  clean/invalidated status.

### 29. Honest no-trade-session handling

A session in which dynamic selection resolves a committed `paper_enforced`
plan with zero selected pairs (e.g. no symbol passed every evidence gate
that day), or in which the selected host(s) generated no signal, is still a
countable clean session provided every check in §26/§27 otherwise held. A
quiet day is not a failure — but it must be recorded as `selected_count: 0`
truthfully (per the durable plan evidence itself), never conflated with an
untested or skipped session.

### 30. Selected-plan changes between sessions

The selected symbol/strategy/timeframe bindings are not required to be
identical across the five sessions — each session's plan is independently
resolved from that day's real evidence (promotion state, market data,
watchlist). A change in selected bindings between sessions is not itself an
invalidator. What must hold every session is that the *evidence backing
whatever was selected* passes read-side validation (§27) — the content of
the selection is allowed to vary; its durable proof is not allowed to be
missing or invalid.

### 31. Overnight / restart / reset procedure

- A daemon restart between sessions is expected and does not by itself
  invalidate the prior session or the running count, provided the prior
  session already reached a clean stop (§26) before the restart.
- A restart *during* a session follows the existing restart/recovery
  procedure (§8, §19, §22) unchanged — Bundle 7 adds no new restart
  semantics. On restart, a new run gets new durable dynamic-selection plan
  evidence (a fresh `plan_id`); the prior run's evidence rows are never
  rewritten (Part 1 requirement 8) — verify this by confirming the old
  `plan_id` still resolves via `GET /api/v1/dynamic-selection/plans/:plan_id`
  with its original content after the restart.
- A restart or new run always requires a fresh `-Stage ActiveCommit` gate
  run and a fresh `bundle7_soak_session_manifest.json` bound to the new
  `run_id`/`plan_id` (§26) — a manifest minted for the prior run is never
  reused, extended, or treated as still covering the post-restart run.
- If a restart happens mid-session for a reason unrelated to Bundle 7 (e.g.
  an OS-level maintenance restart) and the session resumes cleanly with a
  fresh valid plan, the operator may judge the session countable — record
  the restart in the per-session note (§28) either way.

### 32. Stop / halt / reconciliation requirements

- End-of-day stop, halt, and reconciliation follow the existing procedures
  (§10, §19, §20) unchanged. Bundle 7 adds no new stop/halt authority and
  removes none.
- Before the next session starts, reconciliation must not be dirty/stale/
  unavailable/unknown (mirrors the premarket validator's own
  `reconciliation_truth_acceptable` check, run in both stages).
- A halt triggered by any Bundle 7 evidence/validation defect (§27) must be
  fully investigated and the root cause documented before the count resumes
  from zero.

### 33. Live capital remains unauthorized

Nothing in this Part, the premarket validator (Part 7), the soak manifest
(Part 6), or a completed five-session count grants live capital authority.
`approved_for_live` is hard-`false` throughout Bundle 7 by construction
(DB constraint, writer, and API/GUI projection) and remains so regardless
of how many clean sessions accumulate. Any future live-capital authorization
is a separate, explicit decision outside this runbook's and this patch's
scope.
