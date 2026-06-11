# Operator Control Surface — MiniQuantDesk V4 (OPERATOR-CONTROL-SURFACE-BUNDLE-01)

## Purpose

This document is the single reference for every operator action available on
the paper + autonomous paper trading surface.  It covers:

- Repo/code-change validation before market open (script guards, GUI tests/build, targeted Rust tests)
- Operator action matrix (every action, endpoint, safety gates, evidence)
- Monday AAPL smoke quick checklist
- Safe flatten instructions
- Emergency abort rules
- Evidence capture and review rules
- GUI and Discord observation checklist
- Status endpoint first-line triage
- Remaining market proof blockers

**Scope:** Paper + Alpaca autonomous path.  Non-autonomous manual paths are
documented in `operator_workflows.md`.  LiveShadow / LiveCapital are in
`live_shadow_operational_proof.md`.

**Normal operation vs. smoke / proof harness — read this first:** This
document is oriented around the **smoke / proof-harness workflow** (§3 Monday
AAPL Smoke Quick Checklist, §6 Evidence Capture and Review, §10 Remaining
Market Proof Blockers) — supervised, evidence-gathering runs used to *prove*
whether autonomous paper trading can be trusted to operate unattended. Smoke
runs are **not** the way this system is meant to run forever; they are
time-boxed proof exercises with extra capture/review overhead layered on top
of the same underlying operator surface.

The **canonical normal day-to-day autonomous paper operation runbook** is
`autonomous_paper_ops.md` (`AUTON-OPS-01`) — start there for ordinary session
startup, monitoring, and end-of-day shutdown. The pieces of *this* document
that are shared infrastructure — the status check (§1), operator action
matrix (§2), safe-flatten instructions (§4), emergency abort rules (§5), and
shutdown checklist (§9) — apply equally to normal sessions and smoke sessions.
The underlying authorized actions and evidence surfaces are identical between
the two contexts; only the evidence `-Label` and the surrounding review rigor
differ (smoke runs require full strict-lifecycle evidence review per §6;
normal sessions do not).

**Claim boundary:** Autonomous paper session hygiene is code-proven (AUTONOMOUS-PAPER-SESSION-HYGIENE-BUNDLE-01, commit `1d77a72`).  Full market lifecycle proof is still pending.  Do not claim CLOSED on any smoke item until evidence is reviewed and classified `NATURAL-TRADE-LIFECYCLE-CLOSED`.

---

## 0. Repo / Code-Change Validation (Before Market Open)

**Run this first whenever the repo has been edited since the last market
session** (new commits, patches, doc edits) — *before* §1's status check,
before `Launch-VeritasLedger.ps1 -CheckOnly`, and before any market-smoke
retest.

```powershell
powershell -ExecutionPolicy Bypass -File scripts\windows\Invoke-PaperPremarketValidation.ps1
```

This is **not** `full_repo_proof.ps1` and is much cheaper. It answers six
questions in order, each reported as `PASS` / `FAIL` / `SKIPPED` with elapsed
time:

1. **Repo status** — confirms repo root, and that the tracked working tree is
   clean (untracked `evidence/`, `exports/`, logs, and generated artifacts are
   ignored). Refuses to continue on a dirty tracked tree unless `-AllowDirty`
   is passed.
2. **Script guards** — runs `tests\script_guards\run_all_script_guards.ps1`
   (all guards, `-NonInteractive`).
3. **GUI tests** — `npm run test` in `core-rs\mqk-gui`.
4. **GUI build** — `npm run build` in `core-rs\mqk-gui` (typecheck + Vite
   build). Skip with `-SkipGuiBuild`.
5. **Rust baseline/reconcile snapshot tests** — targeted `cargo test` runs for
   `mqk-daemon::state::snapshot`, `scenario_reconcile_baseline_seed_01`, and
   `scenario_runtime_start_reconcile_baseline_01`. Skip with `-SkipRust`.
6. **Rust runtime orchestrator tests** — targeted `cargo test -p mqk-runtime
   --lib orchestrator`. Skip with `-SkipRust`.

**Flags:**
- `-AllowDirty` — allow a dirty tracked working tree (still reports it).
- `-SkipGuiBuild` — skip step 4.
- `-SkipRust` — skip steps 5 and 6.
- `-Fast` — skip steps 4-6 (repo status + script guards + GUI tests only).

**What this script does NOT do** (by design — it is scripts/tests/build only):
it does not start, build-and-run, or arm the daemon; does not clear a halted
run; does not flatten positions; does not submit orders or signals; does not
call any Alpaca/broker endpoint; does not call a Discord webhook; does not
read or print `.env.local` contents; does not run `full_repo_proof.ps1`,
`Run-AAPL5mMarketSmoke.ps1`, or `Start-PaperTradingSmoke.ps1` in normal mode.

**Output:** ends with `FINAL: PASS - safe to proceed to
Launch-VeritasLedger.ps1 -CheckOnly and market-smoke CheckOnly.` (exit 0) or
`FINAL: FAIL - <N> of 6 check(s) failed...` naming the first failing command
(exit 1).

**After `FINAL: PASS`**, proceed to:

```powershell
powershell -ExecutionPolicy Bypass -File scripts\windows\Launch-VeritasLedger.ps1 -CheckOnly
powershell -ExecutionPolicy Bypass -File scripts\windows\Run-AAPL5mMarketSmoke.ps1 -CheckOnly
```

Guard coverage: `tests\script_guards\test_paper_premarket_validation.ps1`
(`CI-GUARD-CONSOLIDATION-01`, 15 static assertions PPV01-PPV15) proves the
script's safety boundaries by source inspection.

---

## 1. First-Line Status Check

Before any operator action, run the read-only status script:

```powershell
powershell -ExecutionPolicy Bypass -File scripts\windows\Get-PaperOperatorStatus.ps1
```

This calls five read-only endpoints and prints a compact summary:
- `readiness_classification` — is the system ready, blocked, or proof-pending?
- `next_operator_action` — what the system recommends next
- `live_routing_enabled` — must always be `false` on paper path
- `runtime_status`, `arm_state`, `ws_continuity`, `reconcile_status`
- Open positions and open orders

No daemon call is required before running this script; it fails soft if the
daemon is offline.

**Before the daemon is started at all** (e.g. first thing in the morning,
before double-clicking the desktop icon), run the desktop launcher's own
read-only preflight instead:

```powershell
powershell -ExecutionPolicy Bypass -File scripts\windows\Launch-VeritasLedger.ps1 -CheckOnly
```

This reports repo root, `.env.local` presence (never its contents), Docker
and paper-DB-container status, daemon binary / GUI binary presence, AAPL/5m
`md_bars` count and freshness, the persisted `sys_arm_state`, and -- if a
daemon happens to already be reachable -- `live_routing_enabled`,
`runtime_status`, `kill_switch_active`, and reconcile status, plus a "Next
action" recommendation. It does not start, build, arm, or call the daemon,
GUI, broker, or Discord, and does not require `.env.local` or
`MQK_OPERATOR_TOKEN` to be present. The desktop shortcut is unaffected --
double-clicking it (no `-CheckOnly`) still performs the normal startup.

For raw endpoint triage:

```
GET /v1/health                             → daemon reachable?
GET /api/v1/system/status                  → mode, arm, runtime, DB, WS, deadman
GET /api/v1/autonomous/readiness           → overall_ready, gates, blockers
GET /api/v1/autonomous/paper-status        → compact paper readiness summary
GET /api/v1/reconcile/status               → dirty or ok?
GET /api/v1/alerts/active                  → active fault signals
GET /api/v1/ops/catalog                    → which actions are enabled/disabled
```

---

## 2. Operator Action Matrix

Every operator action available on the paper control surface.

### Read-Only Status Endpoints (no auth required)

| Action | Endpoint | When to use | Expected response | Market must be open? | Submits order? |
|--------|----------|-------------|-------------------|----------------------|----------------|
| Check status | `GET /api/v1/autonomous/paper-status` | Pre-session, any time | `readiness_classification`, `next_operator_action` | No | No |
| System status | `GET /api/v1/system/status` | Ongoing monitoring | `runtime_status`, `arm_state`, `ws_continuity` | No | No |
| Check readiness | `GET /api/v1/autonomous/readiness` | Before arm / start | `overall_ready`, `blockers` list | No | No |
| Check reconcile | `GET /api/v1/reconcile/status` | Before arm, after fill | `status` (ok / dirty) | No | No |
| Reconcile mismatches | `GET /api/v1/reconcile/mismatches` | When status=dirty | Mismatch rows | No | No |
| Active alerts | `GET /api/v1/alerts/active` | Before arm / any alert | `fault_signals`, alert rows | No | No |
| Action catalog | `GET /api/v1/ops/catalog` | When action is refused | Enabled/disabled action list | No | No |
| Preflight check | `GET /api/v1/system/preflight` | Before start | `deployment_start_allowed` | No | No |
| Portfolio positions | `GET /api/v1/portfolio/positions` | After fill, before flatten | Position rows | No | No |
| Open orders | `GET /api/v1/portfolio/orders/open` | To check pending orders | Open order rows | No | No |
| Execution orders | `GET /api/v1/execution/orders` | OMS state check | Order list with status | No | No |
| OMS overview | `GET /api/v1/oms/overview` | Full OMS snapshot | Orders, fills, positions | No | No |
| Events feed | `GET /api/v1/events/feed` | Session timeline | Runtime transitions, signals | No | No |
| Watchlist status | `GET /api/v1/watchlist/status` | Scanner intake check | Watchlist truth_state | No | No |
| Mode-change guidance | `GET /api/v1/ops/mode-change-guidance` | Before mode switch | 7-step operator workflow | No | No |
| Health check | `GET /v1/health` | Daemon reachable? | `ok=true`, `service=mqk-daemon` | No | No |

### Operator Mutating Routes (Bearer token required)

**Auth header:** `Authorization: Bearer <MQK_OPERATOR_TOKEN>`

| Action | Endpoint / body | When to use | Safety gates | Success response | Failure response | Evidence it worked | Market must be open? | Submits order? |
|--------|----------------|-------------|-------------|-----------------|------------------|--------------------|----------------------|----------------|
| Arm paper | `POST /api/v1/ops/action` `{action_key: "arm-execution"}` | Pre-session, after halt recovery | DB available, not halted, not already armed | `accepted=true, disposition=applied` | 403 gate=integrity_armed | `autonomous/readiness arm_state=armed` | No | No |
| Disarm | `POST /api/v1/ops/action` `{action_key: "disarm-execution"}` | Before mode change, before clear-halted | Not required | `accepted=true` | 503 (DB unavailable) | `arm_state=disarmed` | No | No |
| Clear halted run | `POST /api/v1/ops/action` `{action_key: "clear-halted-run"}` | After halt investigation (step 3 of recovery) | Active run must be in HALTED state | `accepted=true` | 409 (no halted run) | Run record HALTED→STOPPED; `kill_switch_active=false` | No | No |
| Stop runtime | `POST /api/v1/ops/action` `{action_key: "stop-system"}` | Clean session end, manual override | Run must be active | `accepted=true` | 409 (no active run) | `runtime_status=idle` | No | No |
| Halt (kill-switch) | `POST /api/v1/ops/action` `{action_key: "kill-switch"}` | Emergency, invariant violation | DB available | `accepted=true` | 503 (DB unavailable) | `kill_switch_active=true`, `runtime_status=halted` | No | No |
| Flatten paper positions | `POST /api/v1/ops/action` `{action_key: "flatten-paper-positions"}` | After smoke, EOD cleanup, risk control | `flatten_available=true`, `live_routing_enabled=false`, arm+run active | `accepted=true`, symbols list | `flatten_available=false` or blockers | Position qty→0, reconcile clean, Discord `flatten.requested` | Yes (market hours for fill) | Yes (paper flatten order) |
| Adopt broker baseline | `POST /api/v1/ops/repair/adopt-broker-position-baseline` `{confirmation: "ADOPT_BROKER_POSITION_BASELINE"}` | On fresh daemon start to sync positions | Not halted | `accepted=true`, position_count | 409 (already adopted) | `reconcile_status=ok` | No | No |
| Submit signal | `POST /api/v1/strategy/signal` | Operator signal injection (paper smoke) | 7 gates: ingestion, DB, arm, active run, running state, not suppressed, outbox enqueue | `intent_placed=true` | 409/503 with gate name | Outbox row written, order submitted via WS path | Yes (for fill) | Yes (paper order via outbox) |

### Halt Recovery Sequence (4-step — mandatory in order)

```
1. POST /api/v1/ops/action {"action_key": "disarm-execution"}
2. Investigate halt reason: GET /api/v1/audit/operator-actions
                            GET /control/status  (deadman_reason)
3. POST /api/v1/ops/action {"action_key": "clear-halted-run"}
4. POST /api/v1/ops/action {"action_key": "arm-execution"}
```

Do not skip step 3.  Without clearing the halted run record the daemon will
find the prior run in HALTED state and refuse a new start.

---

## 3. Monday AAPL Smoke Quick Checklist

Run these steps in order before and during the Monday market smoke.

### Retest after a halted prior session (read this first)

If the previous paper-smoke session ended with `kill_switch_active=true`,
`integrity_halt_active=true`, or `arm_state=halted` (for example,
`next_operator_action` from `/api/v1/autonomous/paper-status` says "Kill
switch is active. Clear the halted run..."), this is normal after any halted
session and does **not** require manual recovery before the next retest:

- Baseline broker positions inherited from a prior session are seeded into
  the ledger as equivalent Fill entries (`seed_portfolio_from_baseline` in
  `mqk-daemon/src/state/snapshot.rs`), so `check_capital_invariants` is
  satisfied immediately on startup. A prior-session position no longer
  produces a false IntegrityViolation/ReconcileDrift halt by itself.
- `Run-AAPL5mMarketSmoke.ps1 -CheckOnly` (which calls
  `Start-PaperTradingSmoke.ps1 -CheckOnly`) includes a STEP 5C dry-check that
  reads the persisted `sys_arm_state` row read-only and reports
  `ARMED`/`DISARMED` plus the disarm reason. This tells you what state the
  prior session left behind without starting the daemon or touching
  arm/halt state.
- STEP 10 of the full smoke run reads `kill_switch_active` / `arm_state` /
  `runtime_status` from the **freshly started** daemon and automatically
  runs `disarm-execution` -> `clear-halted-run` -> `arm-execution` if a
  halted state is detected. **Do not manually call `clear-halted-run` or
  `arm-execution` before running the full smoke** -- STEP 10 handles it, and
  a manual call beforehand races against the daemon (re)start.
- If STEP 10's automatic recovery fails (`kill_switch_active` still `true`
  afterward), the script exits non-zero and captures evidence via
  `Write-EvidenceCapture`. Treat that as a real blocker, not something to
  retry manually -- investigate before the next attempt.

### Pre-smoke (any time, including weekend)

```powershell
# 1. Verify prerequisites (read-only, no mutations)
powershell -ExecutionPolicy Bypass -File scripts\windows\Run-AAPL5mMarketSmoke.ps1 -CheckOnly

# 2. Verify bar count and freshness
powershell -ExecutionPolicy Bypass -File scripts\windows\Prep-PremarketMarketData.ps1 -CheckOnly

# 3. Quick operator status check
powershell -ExecutionPolicy Bypass -File scripts\windows\Get-PaperOperatorStatus.ps1
```

Expected in CheckOnly:
- `bar_check_exit: 0` (≥30 completed AAPL/5m bars)
- `smoke_check_exit: 0` (.env.local present, docker available)
- `readiness_classification` is not `blocked`
- STEP 5C reports the persisted `sys_arm_state` (`ARMED`/`DISARMED` + reason).
  `DISARMED` is expected after a halted prior session and is auto-recovered by
  STEP 10 of the full run -- see "Retest after a halted prior session" above.

### Day-of smoke (market hours — 09:30–16:00 ET)

```powershell
# Full market smoke — 30-minute watch window
powershell -ExecutionPolicy Bypass -File scripts\windows\Run-AAPL5mMarketSmoke.ps1 -WatchSeconds 1800
```

### During smoke — monitor

```powershell
# Every 5–10 minutes during the watch window
powershell -ExecutionPolicy Bypass -File scripts\windows\Get-PaperOperatorStatus.ps1
```

Confirm:
- `runtime_status=running`
- `ws_continuity=live`
- `kill_switch_active=false`
- `live_routing_enabled=false`

### After smoke — capture and review evidence

```powershell
# Capture evidence bundle
powershell -ExecutionPolicy Bypass -File scripts\windows\Capture-PaperSmokeEvidence.ps1 -Label aapl5m_post_smoke

# Review and classify
powershell -ExecutionPolicy Bypass -File scripts\windows\Review-PaperSmokeEvidence.ps1 -Latest -WriteSummary
```

Target classification: `NATURAL-TRADE-LIFECYCLE-CLOSED`

---

## 4. Safe Flatten Instructions

Flatten is the only operator action that submits a paper order.  Use only when:
- A position exists after market hours (risk management)
- EOD cleanup after a smoke run
- Reconcile shows an unexpected position

### Before flattening

1. Confirm flatten is available:
   ```powershell
   powershell -ExecutionPolicy Bypass -File scripts\windows\Get-PaperOperatorStatus.ps1
   ```
   Field `flatten_available` must be `True`.
   Field `flatten_blockers` must be empty.

2. Confirm market is open (flatten order requires a live market to fill on paper).

3. Confirm `live_routing_enabled=false` (printed in status output).

### Flatten command

```powershell
# Using curl / Invoke-RestMethod (requires Bearer token):
$body = @{ action_key = 'flatten-paper-positions' } | ConvertTo-Json
Invoke-RestMethod -Uri 'http://127.0.0.1:8899/api/v1/ops/action' `
    -Method POST -ContentType 'application/json' -Body $body `
    -Headers @{ Authorization = "Bearer $env:MQK_OPERATOR_TOKEN" }
```

### After flattening

1. Check `GET /api/v1/portfolio/positions` — expect empty or zero qty.
2. Check `GET /api/v1/reconcile/status` — expect `status=ok` after fill settles.
3. Confirm Discord fired `flatten.requested` alert with `live_routing_enabled=false`.
4. Capture evidence:
   ```powershell
   powershell -ExecutionPolicy Bypass -File scripts\windows\Capture-PaperSmokeEvidence.ps1 -Label post_flatten
   ```

### Flatten blockers

| Blocker | Meaning | Resolution |
|---------|---------|------------|
| `live_routing_enabled=true` | Safety gate — flatten refused | **Stop immediately.** Paper-only invariant violated. |
| `daemon_mode != paper` | Wrong mode | Verify `MQK_DAEMON_DEPLOYMENT_MODE=paper` |
| `arm_state != armed` | Not armed | Arm first, then flatten |
| `runtime_status != running` | Runtime not active | Start runtime, then flatten |
| `ws_continuity != live` | WS gap | Wait for WS re-establishment; GapDetected blocks flatten |

---

## 5. Emergency Abort Rules

Use halt when there is unexpected behavior, invariant violation, or loss of control.

### Halt command

```powershell
$body = @{ action_key = 'kill-switch' } | ConvertTo-Json
Invoke-RestMethod -Uri 'http://127.0.0.1:8899/api/v1/ops/action' `
    -Method POST -ContentType 'application/json' -Body $body `
    -Headers @{ Authorization = "Bearer $env:MQK_OPERATOR_TOKEN" }
```

Or kill the daemon process directly:
```powershell
Stop-Process -Name 'mqk-daemon' -Force
```

### When to halt (not just stop)

- `live_routing_enabled=true` appears (should never happen on paper path)
- `kill_switch_active=true` observed unexpectedly mid-session
- Unexpected fills from Alpaca that don't match outbox
- Reconcile drift persists after multiple ticks
- Discord alerts show `halt.reconcile_drift` or WS gap with open orders
- Any security anomaly

### After an emergency halt

1. Do NOT re-arm immediately.
2. Capture evidence: `Capture-PaperSmokeEvidence.ps1 -Label emergency_halt`
3. Inspect halt reason: `GET /api/v1/audit/operator-actions`
4. Follow 4-step halt recovery sequence (Section 2) only after investigation.
5. Restart daemon cleanly before re-arming.

---

## 6. Evidence Capture and Review Rules

### Capture evidence

```powershell
# Standard capture (label describes the moment)
powershell -ExecutionPolicy Bypass -File scripts\windows\Capture-PaperSmokeEvidence.ps1 -Label <label>
```

Common labels: `pre_smoke`, `post_smoke`, `post_flatten`, `emergency_halt`, `eod_snapshot`

Evidence folder: `evidence\paper_smoke_<YYYYMMDD_HHMMSS>_<label>\`

Contents:
- `api/` — JSON snapshots from all daemon endpoints
- `db/` — SELECT-only DB snapshots (runs, outbox, inbox, fill_quality)
- `notes/` — Operator-fillable lifecycle checklist, Discord/GUI observation, final verdict

### Review and classify

```powershell
# Review latest evidence bundle
powershell -ExecutionPolicy Bypass -File scripts\windows\Review-PaperSmokeEvidence.ps1 -Latest -WriteSummary

# Review specific folder
powershell -ExecutionPolicy Bypass -File scripts\windows\Review-PaperSmokeEvidence.ps1 `
    -EvidencePath evidence\paper_smoke_<timestamp>_<label> -WriteSummary
```

### Viewing evidence in the GUI

`review_summary.json` (and `promotion_chain.json` / `premarket_revalidation.json`
when present) can also be viewed in the desktop GUI on the **Backtest Results**
screen (diagnostics/oversight monitor) by entering the evidence folder path.
The screen renders the same classification, ledger-specific verdicts, and
Discord workflow guidance (see Section 8) as the Markdown summary -- useful
for a quick visual review without opening the JSON/Markdown files directly.

### Classification meanings

| Verdict | Meaning | What it proves |
|---------|---------|---------------|
| `NATURAL-TRADE-LIFECYCLE-CLOSED` | Full lifecycle: running → signal → outbox → broker submit → ACK → fill → portfolio → reconcile clean | Order submitted, filled, reconcile clean |
| `READINESS-CLOSED-NO-TRADE` | Runtime ran, bars loaded, no trade signal/order, reconcile clean | Strategy evaluated, no signal, system healthy |
| `PARTIAL` | Some lifecycle steps completed, not all | Further smoke needed |
| `OPEN` | Active blocker: halt, kill switch, dirty reconcile | Resolve blocker first |
| `FALSE-CLOSED` | Live routing enabled, secrets in evidence, no proof files | Do NOT record as passed |

### Smoke is not closed until

- Classification is `NATURAL-TRADE-LIFECYCLE-CLOSED` (for trade lifecycle)
- OR `READINESS-CLOSED-NO-TRADE` (for no-trade sessions)
- AND evidence bundle is present and durable
- AND `live_routing_enabled=false` is confirmed in the evidence

### Source of truth

- `review_summary.json` / `review_summary.md` (written by
  `Review-PaperSmokeEvidence.ps1 -WriteSummary`) are the generated
  evidence-review source of truth. The `VERDICT` / `classification` field
  there is authoritative.
- `notes/final_verdict.txt` is an operator-completed note/template only. It
  is generated as **TEMPLATE ONLY / Status: PENDING OPERATOR REVIEW** with
  every verdict option as an unchecked `[ ]` checkbox -- it is NOT
  authoritative and does not override `review_summary`.
- Do not call a smoke "PASSED" because `notes/final_verdict.txt` has text
  mentioning `SMOKE PASSED`. Only `review_summary` classification
  `NATURAL-TRADE-LIFECYCLE-CLOSED` (or `READINESS-CLOSED-NO-TRADE` for a
  no-trade session) with the lifecycle/reconcile criteria above satisfied
  means PASSED.
- If an operator checks `[x] SMOKE PASSED` in `notes/final_verdict.txt` while
  `review_summary` classification is `OPEN`, `PARTIAL`, or `FALSE-CLOSED`,
  `review_summary` reports `manual_verdict_conflict: true` and prints a
  `*** MANUAL VERDICT CONFLICT ***` warning -- treat the evidence as not
  passed regardless of the operator note.

---

## 7. GUI Observation Checklist

Before recording GUI observation as complete, confirm each item:

- [ ] GUI launched (`Launch-VeritasLedger.ps1` or Tauri app)
- [ ] Runtime status shows `running` in the GUI status bar
- [ ] WS continuity shows `live` (not `cold_start_unproven`)
- [ ] Order appears in OMS order list after signal injection
- [ ] Fill appears in OMS / portfolio after broker fill
- [ ] Reconcile panel shows `ok` (not `dirty`)
- [ ] No red alert banners in the GUI during the smoke window
- [ ] GUI data matches backend (`GET /api/v1/oms/overview` matches GUI display)
- [ ] Screenshot captured and saved in evidence `notes/gui_observation.txt`

---

## 8. Discord Observation Checklist

Configure `DISCORD_WEBHOOK_URL` in `.env.local` to enable paper trade alerts.
Discord is observability only — delivery failure never blocks trading.

This guidance is also available read-only in the desktop GUI on the
**Backtest Results** screen, in the "Discord observability workflows" panel.

For a complete AAPL sell/flatten smoke, confirm these Discord messages appeared:

| Stage | Discord stage key | Expected content |
|-------|-------------------|-----------------|
| Run start | `autonomous.run.start` | run_id, session window |
| Signal admitted | `signal.admitted` | symbol=AAPL, side, qty |
| Order submitted | `order.submitted` | order_id, symbol, side |
| Broker ACK | `order.acked` | broker confirmation |
| Fill | `fill.terminal` or `fill.partial` | fill_qty, fill_price |
| Operator flatten | `flatten.requested` | `live_routing_enabled=false`, symbols |
| Reconcile clean | `reconcile.clean` | transition to clean state |

**Discord lifecycle is PARTIAL until market observation confirms all messages.**

**Discord is NOT the source of truth** — it is observability only.  An order is real when the DB outbox row, inbox row, and broker_order_map row all exist.

### Sending a Test Alert (Safe, Offline)

Use `scripts\windows\Test-DiscordAlert.ps1` to verify Discord alert delivery
configuration without starting the daemon trading runtime, arming paper
trading, submitting orders, or touching broker/Alpaca endpoints.

```powershell
# Check configuration only -- does NOT send a Discord alert
powershell -ExecutionPolicy Bypass -File scripts\windows\Test-DiscordAlert.ps1 -CheckOnly

# Send one [TEST] alert via the daemon (requires daemon running + MQK_OPERATOR_TOKEN)
powershell -ExecutionPolicy Bypass -File scripts\windows\Test-DiscordAlert.ps1
```

- `-CheckOnly` reports daemon reachability and whether `DISCORD_WEBHOOK_URL` /
  `MQK_OPERATOR_TOKEN` are configured (presence only -- values are never
  printed). It never calls `/api/v1/ops/action`.
- Normal mode calls only `POST /api/v1/ops/action {"action_key":"test-discord-alert"}`,
  which the daemon documents as not mutating trading/arm/integrity state.
  It fails closed if the daemon is unreachable or `MQK_OPERATOR_TOKEN` is not set.
- `DISCORD_WEBHOOK_URL` must live only in `.env.local` (gitignored) or the
  process environment -- never paste it into tracked files, scripts, or docs.

### Sending a Paper-Readiness / Strategy-Fit Artifact Alert (Safe, Offline)

Use `scripts\windows\Send-PaperReadinessDiscordAlert.ps1` to send an optional,
operator-triggered Discord summary of an offline `paper-readiness-v1` or
`strategy-fit-v1` artifact JSON file. This is observability only -- it never
starts the daemon trading runtime, arms paper trading, submits orders, calls
broker/Alpaca endpoints, or writes to the database. The daemon is never
contacted; the script POSTs directly to `$env:DISCORD_WEBHOOK_URL`.

```powershell
# Check configuration + classification only -- does NOT send a Discord alert
powershell -ExecutionPolicy Bypass -File scripts\windows\Send-PaperReadinessDiscordAlert.ps1 -ArtifactPath <path-to-artifact.json> -CheckOnly

# Send one sanitized summary alert for the artifact
powershell -ExecutionPolicy Bypass -File scripts\windows\Send-PaperReadinessDiscordAlert.ps1 -ArtifactPath <path-to-artifact.json>

# Optional: prefix the Discord message with a short title
powershell -ExecutionPolicy Bypass -File scripts\windows\Send-PaperReadinessDiscordAlert.ps1 -ArtifactPath <path-to-artifact.json> -Title "Overnight scan"
```

- Supports `paper-readiness-v1` (from `paper_readiness_runner.py`) and
  `strategy-fit-v1` (from `backtest_gates.py` / `backtest_runner.py`)
  artifacts. Any other or missing `schema_version` is refused (fail-closed).
- `-CheckOnly` parses and classifies the artifact, prints a sanitized summary,
  reports whether `DISCORD_WEBHOOK_URL` appears configured (presence only --
  values are never printed), and reports whether the artifact is `sendable`.
  It never issues a webhook POST.
- Normal mode sends exactly ONE sanitized summary message directly to
  `$env:DISCORD_WEBHOOK_URL` -- the daemon is never contacted. It fails
  closed (sends nothing) if `DISCORD_WEBHOOK_URL` is not configured, the
  artifact is missing/invalid/unsupported, or the artifact contains a forged
  `recommended_for_live` / `approved_for_live` / `eligible_for_live` flag, a
  Discord webhook URL, or an embedded secret/token.
- Only the artifact's FILE NAME is sent -- never the local file path, and
  never the raw artifact JSON.
- This workflow is operator-triggered only. It is never invoked automatically
  by the paper-readiness pipeline.
- `DISCORD_WEBHOOK_URL` must live only in `.env.local` (gitignored) or the
  process environment -- never paste it into tracked files, scripts, or docs.

### Sending a Paper Smoke Evidence Review Alert (Safe, Offline)

Use `scripts\windows\Send-PaperSmokeReviewDiscordAlert.ps1` to send an
optional, operator-triggered Discord summary of a `review-v2` evidence review
produced by `Review-PaperSmokeEvidence.ps1` (`review_summary.json` /
`review_summary.md`). This is observability only -- it never starts the
daemon trading runtime, arms paper trading, submits orders, calls
broker/Alpaca endpoints, writes to the database, or changes evidence
classification logic. The daemon is never contacted; the script POSTs
directly to `$env:DISCORD_WEBHOOK_URL`.

```powershell
# Check configuration + classification only -- does NOT send a Discord alert
powershell -ExecutionPolicy Bypass -File scripts\windows\Send-PaperSmokeReviewDiscordAlert.ps1 -ReviewPath <path-to-evidence-folder-or-review_summary.json> -CheckOnly

# Send one sanitized summary alert for the review
powershell -ExecutionPolicy Bypass -File scripts\windows\Send-PaperSmokeReviewDiscordAlert.ps1 -ReviewPath <path-to-evidence-folder-or-review_summary.json>

# Optional: prefix the Discord message with a short title
powershell -ExecutionPolicy Bypass -File scripts\windows\Send-PaperSmokeReviewDiscordAlert.ps1 -ReviewPath <path-to-evidence-folder-or-review_summary.json> -Title "Overnight smoke"
```

- `-ReviewPath` accepts a direct path to `review_summary.json` or
  `review_summary.md`, or an evidence folder containing one of those files
  (`review_summary.json` is preferred when both are present).
- `-CheckOnly` parses and classifies the review, prints a sanitized summary,
  reports whether `DISCORD_WEBHOOK_URL` appears configured (presence only --
  values are never printed), and reports whether the review is `sendable`.
  It never issues a webhook POST.
- Normal mode sends exactly ONE sanitized summary message directly to
  `$env:DISCORD_WEBHOOK_URL` -- the daemon is never contacted. It fails
  closed (sends nothing) if `DISCORD_WEBHOOK_URL` is not configured, the
  review file is missing/invalid/unsupported, the classification cannot be
  determined or is not one of the known `Review-PaperSmokeEvidence.ps1`
  values (`NATURAL-TRADE-LIFECYCLE-CLOSED`, `READINESS-CLOSED-NO-TRADE`,
  `PARTIAL`, `OPEN`, `FALSE-CLOSED`), or the review file contains a forged
  `recommended_for_live` / `approved_for_live` / `eligible_for_live` flag, a
  Discord webhook URL, or an embedded secret/token.
- Only the evidence FOLDER NAME and review FILE NAME are sent -- never the
  local file path, and never the raw review JSON or Markdown contents.
- This workflow is operator-triggered only. It is never invoked automatically
  by `Review-PaperSmokeEvidence.ps1` or any evidence review pipeline.
- `DISCORD_WEBHOOK_URL` must live only in `.env.local` (gitignored) or the
  process environment -- never paste it into tracked files, scripts, or docs.

---

## 9. Shutdown Checklist (Clean Process Shutdown)

This checklist closes out **any** paper session — a normal autonomous
operating day (`autonomous_paper_ops.md` §10) or a smoke / proof-harness run
(§3 above). The sequence and the evidence surfaces captured are identical
either way; only the evidence `-Label` differs. The guiding rule
(`PAPER-SMOKE-CLEAN-SHUTDOWN-CAPTURE-01`) is: **capture useful evidence before
you stop anything, and capture it again after**, so the final state of the
session is provable rather than asserted.

### 9.1 Recommended: orchestrated clean-shutdown script

```powershell
powershell -ExecutionPolicy Bypass -File scripts\windows\Stop-PaperTradingClean.ps1 -Label eod_shutdown
```

`Stop-PaperTradingClean.ps1` runs the six phases below in strict order. It
uses **only existing authorized action keys** (`stop-system`,
`disarm-execution`) through the canonical `/api/v1/ops/action` route, and
**only existing read-only tooling** (`Capture-PaperSmokeEvidence.ps1`,
`Get-PaperOperatorStatus.ps1`) for evidence and verification. It never
introduces a new mutating route, never submits/cancels/replaces an order, and
never talks to the broker directly.

| Phase | Step | Mutates? | Purpose |
|-------|------|----------|---------|
| 1 | Pre-stop capture: `Capture-PaperSmokeEvidence.ps1 -Label <Label>_pre_stop` | No | Captures paper-status, reconcile, positions, orders, fills, alerts, watchlist status, and admission-check **while the runtime is still live** — the only point where in-flight state is observable |
| 2 | Stop: `ops/action` `stop-system` | Yes (pre-existing authorized action) | Stops the execution runtime |
| 3 | Verify stopped: `GET /api/v1/system/status` (read-only) | No | Confirms `runtime_status` reflects stopped before continuing — refuses to proceed blindly |
| 4 | Disarm: `ops/action` `disarm-execution` | Yes (pre-existing authorized action) | Prevents accidental restart |
| 5 | Post-stop capture: `Capture-PaperSmokeEvidence.ps1 -Label <Label>_post_stop` | No | Re-captures the same surfaces to record the final settled state (positions/orders/reconcile after stop+disarm) |
| 6 | Summary: prints manual daemon-process / DB-container steps | No | Operator decides whether to kill the daemon process or stop the DB container — intentionally **not** automated (see 9.3) |

If the daemon is unreachable at any phase, the script records
`UNAVAILABLE: ...` in the captured evidence (matching
`Capture-PaperSmokeEvidence.ps1`'s existing fail-soft convention) and
continues to the next phase rather than aborting. The goal is to capture as
much honest evidence as possible — never to fabricate a clean result, and
never to skip capture because the daemon looked unhealthy.

### 9.2 Manual sequence (fallback / reference)

If the script is unavailable, run the same six phases by hand, in this order:

```powershell
# 1. Pre-stop evidence capture (while the runtime is still live)
powershell -ExecutionPolicy Bypass -File scripts\windows\Capture-PaperSmokeEvidence.ps1 -Label eod_shutdown_pre_stop

# 2. Stop the execution runtime (if running)
$body = @{ action_key = 'stop-system' } | ConvertTo-Json
Invoke-RestMethod -Uri 'http://127.0.0.1:8899/api/v1/ops/action' `
    -Method POST -ContentType 'application/json' -Body $body `
    -Headers @{ Authorization = "Bearer $env:MQK_OPERATOR_TOKEN" }

# 3. Verify stopped
Invoke-RestMethod -Uri 'http://127.0.0.1:8899/api/v1/system/status' -Method Get

# 4. Disarm (prevents accidental restart)
$body = @{ action_key = 'disarm-execution' } | ConvertTo-Json
Invoke-RestMethod -Uri 'http://127.0.0.1:8899/api/v1/ops/action' `
    -Method POST -ContentType 'application/json' -Body $body `
    -Headers @{ Authorization = "Bearer $env:MQK_OPERATOR_TOKEN" }

# 5. Post-stop evidence capture (final settled state, after stop+disarm)
powershell -ExecutionPolicy Bypass -File scripts\windows\Capture-PaperSmokeEvidence.ps1 -Label eod_shutdown_post_stop

# 6. Kill daemon process (manual operator decision)
Stop-Process -Name 'mqk-daemon' -ErrorAction SilentlyContinue
```

### 9.3 Postgres container

Killing the daemon process and stopping the DB container are deliberately left
as manual operator decisions (not scripted) — the container holds the durable
DB truth that the next session's restart-safety depends on, and the operator
should confirm capture succeeded before tearing anything down. The Postgres
container can remain running between sessions:

```powershell
# Stop paper DB container when done for the day
docker stop mqk-paper-postgres
```

---

## 10. Remaining Market Proof Blockers

These items are **open** and require market-hours observation to close.

| Item | Status | Required evidence |
|------|--------|------------------|
| AAPL natural sell lifecycle | OPEN | `NATURAL-TRADE-LIFECYCLE-CLOSED` evidence from live smoke |
| Full buy→sell lifecycle | OPEN | Signal→fill→sell signal→fill evidence |
| Safe flatten market proof | OPEN | `flatten.requested` Discord + positions→0 + reconcile clean |
| GUI market observation | OPEN | Screenshot with filled order visible in GUI |
| Discord market observation | OPEN | All 7 stage messages confirmed in Discord channel |
| Repeated autonomous cycle proof | OPEN | Two consecutive sessions auto-starting and stopping cleanly |

**Until all six items are proven:**
- `autonomous_paper_status.readiness_classification` will remain `market_proof_pending`
- Claim autonomous paper is PARTIAL, not CLOSED

---

## 11. Script Reference

| Script | Purpose | Mutates? |
|--------|---------|---------|
| `Get-PaperOperatorStatus.ps1` | Compact read-only status (this runbook's first step) | No |
| `Stop-PaperTradingClean.ps1` | Orchestrated clean shutdown: pre-stop capture → stop → verify → disarm → post-stop capture (§9) | Yes (stop-system, disarm-execution only) |
| `Start-PaperTradingSmoke.ps1` | Full smoke harness: daemon start, arm, runtime, watcher | Yes (arm, start) |
| `Run-AAPL5mMarketSmoke.ps1` | AAPL/5m smoke orchestrator | Yes (via smoke) |
| `Capture-PaperSmokeEvidence.ps1` | Evidence bundle capture (API + DB snapshots) | No |
| `Review-PaperSmokeEvidence.ps1` | Evidence classification and summary | No |
| `Launch-VeritasLedger.ps1` | Normal desktop startup (daemon + GUI). `-CheckOnly` prints a read-only startup status report and exits -- no start/build/arm | No (unless -ArmPaper); `-CheckOnly` is always read-only |
| `Prep-PremarketMarketData.ps1` | Market data bar prep (md_bars only) | Yes (md_bars only) |
| `Refresh-IntradayMarketData.ps1` | Intraday 5m bar refresh | Yes (md_bars only) |
| `Register-PremarketDataRefreshTask.ps1` | Windows scheduled task for premarket data | Yes (task only) |

Guard scripts:
- `tests\script_guards\test_paper_operator_status.ps1` (OPS01-OPS11)
- `tests\script_guards\test_launch_veritas_ledger.ps1` (LVL01-LVL17)
