# Paper Smoke Evidence Pack (PAPER-SMOKE-EVIDENCE-01)

## Purpose

This runbook describes the durable evidence capture workflow for MiniQuantDesk V4
Paper + Alpaca smoke runs.  After each smoke the operator captures a proof bundle
that demonstrates real paper lifecycle end-to-end: signal in, order out, ACK, fill,
OMS terminal, portfolio update, reconcile clean, Discord alert.

This runbook covers the pre-smoke and post-smoke evidence steps only.
For the smoke script itself see `Start-PaperTradingSmoke.ps1` and
`docs/runbooks/autonomous_paper_ops.md`.

---

## 1. Evidence script

```
scripts\windows\Capture-PaperSmokeEvidence.ps1
```

**Safety properties (enforced by the script):**
- Never prints secrets (API keys, operator token, DB password, Discord webhook).
- Never mutates the DB (SELECT-only via psql).
- Never starts or stops a trading run.
- Never calls external broker endpoints.
- All daemon calls are localhost read-only GETs.
- Handles offline daemon and offline DB gracefully (marks each surface UNAVAILABLE).

---

## 2. Pre-smoke evidence capture

Run this **before** starting the smoke, while the daemon is not yet started
(or is running but not yet armed):

```powershell
powershell -ExecutionPolicy Bypass -File scripts\windows\Capture-PaperSmokeEvidence.ps1 -Label pre_smoke
```

This creates:
```
evidence\paper_smoke_<YYYYMMDD_HHMMSS>_pre_smoke\
  summary.md
  git_state.txt
  proof_tail.txt
  api\                   (UNAVAILABLE if daemon not yet running)
  db\                    (snapshots if MQK_DATABASE_URL is set)
  notes\
    discord_observation.txt
    gui_observation.txt
    smoke_lifecycle_checklist.txt
    final_verdict.txt
```

---

## 3. CheckOnly gate (mandatory before smoke)

```powershell
powershell -ExecutionPolicy Bypass -File scripts\windows\Start-PaperTradingSmoke.ps1 -CheckOnly
```

This must exit 0.  If it fails, resolve the prereqs before continuing.

---

## 4. Run the smoke

```powershell
powershell -ExecutionPolicy Bypass -File scripts\windows\Start-PaperTradingSmoke.ps1 -WatchSeconds 420
```

Capture the exact command, WatchSeconds, start/end time, and strategy env values
in `notes\smoke_lifecycle_checklist.txt`.

---

## 5. Post-smoke evidence capture

Run this **after** the smoke completes (daemon still running):

```powershell
powershell -ExecutionPolicy Bypass -File scripts\windows\Capture-PaperSmokeEvidence.ps1 -Label post_smoke
```

This captures live daemon API snapshots while state is fresh:
- `/api/v1/system/status`
- `/api/v1/system/preflight`
- `/api/v1/autonomous/readiness`
- `/api/v1/alerts/active`
- `/api/v1/events/feed`
- `/api/v1/oms/overview`
- `/api/v1/risk/summary`
- `/api/v1/reconcile/status`

And read-only DB snapshots:
- `runs_recent.txt`
- `oms_outbox_recent.txt`
- `oms_inbox_recent.txt`
- `broker_order_map_recent.txt`
- `fill_quality_recent.txt`

---

## 6. Operator notes (fill in manually)

After both captures complete, fill in the four note files in the post-smoke
evidence folder:

| File | What to capture |
|---|---|
| `notes/smoke_lifecycle_checklist.txt` | Full lifecycle YES/NO checklist + exact smoke command |
| `notes/discord_observation.txt` | Discord trade alert observation + screenshot path |
| `notes/gui_observation.txt` | GUI observation + screenshot path |
| `notes/final_verdict.txt` | SMOKE PASSED / PARTIAL / FAILED + any blockers |

---

## 7. Required lifecycle evidence fields

| Field | Source |
|---|---|
| Signal produced | lifecycle checklist |
| Order submitted | `oms_outbox_recent.txt` |
| Alpaca paper ACK | `oms_inbox_recent.txt` |
| Partial fill | `oms_inbox_recent.txt` |
| Final fill | `oms_inbox_recent.txt` + `fill_quality_recent.txt` |
| OMS terminal state | `oms_overview.json` |
| Portfolio updated | `oms_overview.json` |
| Reconcile clean | `reconcile_status.json` |
| Discord alert | `discord_observation.txt` |
| GUI matched backend | `gui_observation.txt` |

---

## 8. Verdict criteria

**SMOKE PASSED** requires all of:
- Full lifecycle from signal to OMS terminal state.
- `reconcile_status.json` shows clean (no dirty positions).
- `autonomous_readiness.json` shows `overall_ready = true` (or `outside_window` after stop).
- `alerts_active.json` shows no `gap_detected` or unresolved fault signals.
- Discord trade alert fired.
- GUI showed filled order matching backend.
- No halt triggered.
- `final_verdict.txt` marked SMOKE PASSED by operator.

**SMOKE PARTIAL** — lifecycle partial; document what was and was not proven.

**SMOKE FAILED** — lifecycle did not complete; document the failure mode.

---

## 9. Evidence folder convention

Evidence folders are committed to the `evidence/` directory (gitignored by default
except `.gitkeep`).  Operator discretion on whether to commit a specific evidence
pack as a durable artifact vs keeping it local only.

Evidence folders are never committed with API keys, DB passwords, or webhook URLs.
The capture script enforces this by never printing these values.

---

## 10. Validation (guard test)

The capture script is covered by a static guard:

```powershell
powershell -ExecutionPolicy Bypass -File tests\script_guards\test_capture_paper_smoke_evidence.ps1
```

This guard (CPE01-CPE10) proves the script is read-only, prints no secrets,
does not mutate the DB, does not call broker endpoints, and handles offline
daemon gracefully.

---

## 11. Quick reference

| Step | Command |
|---|---|
| Pre-smoke capture | `powershell -ExecutionPolicy Bypass -File scripts\windows\Capture-PaperSmokeEvidence.ps1 -Label pre_smoke` |
| CheckOnly gate | `powershell -ExecutionPolicy Bypass -File scripts\windows\Start-PaperTradingSmoke.ps1 -CheckOnly` |
| Run smoke | `powershell -ExecutionPolicy Bypass -File scripts\windows\Start-PaperTradingSmoke.ps1 -WatchSeconds 420` |
| Post-smoke capture | `powershell -ExecutionPolicy Bypass -File scripts\windows\Capture-PaperSmokeEvidence.ps1 -Label post_smoke` |
| Guard check | `powershell -ExecutionPolicy Bypass -NonInteractive -File tests\script_guards\run_all_script_guards.ps1` |
