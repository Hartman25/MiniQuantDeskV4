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
- `/api/v1/autonomous/paper-status` — **first-line smoke triage** (PAPER-SMOKE-AUTOMATION-BUNDLE-01)
- `/api/v1/alerts/active`
- `/api/v1/events/feed`
- `/api/v1/oms/overview`
- `/api/v1/strategy/multi-symbol-dispatch-summary` — see Section 5c (WATCHLIST-PROMO-V2-MULTI-SYMBOL-SMOKE-EVIDENCE-01)
- `/api/v1/risk/summary`
- `/api/v1/reconcile/status`

And read-only DB snapshots:
- `runs_recent.txt`
- `oms_outbox_recent.txt`
- `oms_inbox_recent.txt`
- `broker_order_map_recent.txt`
- `fill_quality_recent.txt`

---

## 5b. Autonomous paper status — first-line smoke triage (PAPER-SMOKE-AUTOMATION-BUNDLE-01)

`GET /api/v1/autonomous/paper-status` is the primary smoke triage endpoint.  It is
automatically captured as `api/autonomous_paper_status.json` in every evidence pack.

**Interpreting `readiness_classification`:**

| Value | Meaning | Action |
|---|---|---|
| `ready_for_market_smoke` | All gates pass, WS live, reconcile clean, positions flat | Proceed |
| `market_proof_pending` | Session healthy but no full buy→sell cycle proven yet | Proceed (expected during first smokes) |
| `blocked` | Hard blocker present (gap detected, halt, dirty reconcile, etc.) | Stop — resolve `flatten_blockers` first |

**Key fields in the response:**
- `readiness_classification` — top-level triage verdict
- `next_operator_action` — explicit next step for the operator
- `flatten_available` / `flatten_blockers` — whether a safe flatten can proceed
- `current_position_qty` / `target_qty` / `computed_delta_qty` — position state
- `no_order_reason` — why no order was generated (if any)
- `ws_continuity` — Alpaca WS state (must be `live` before run starts)
- `reconcile_status` — must be `clean` for healthy run

**Pre-run gate in Start-PaperTradingSmoke.ps1 (STEP 14b):**
After the daemon is up, armed, and WS is live, STEP 14b queries the paper-status
endpoint and hard-blocks if `readiness_classification == blocked`.  The operator must
resolve the stated blockers before the smoke can proceed.

**Important:** `readiness_classification` alone cannot mark a trade lifecycle closed.
Trade lifecycle closure still requires fill evidence, inbox apply, and reconcile clean
from the evidence pack.

---

## 5c. Multi-symbol dispatch summary evidence (`WATCHLIST-PROMO-V2-MULTI-SYMBOL-SMOKE-EVIDENCE-01` -- CLOSED)

`GET /api/v1/strategy/multi-symbol-dispatch-summary` is captured as
`api/multi_symbol_dispatch_summary.json` (raw GET response, fail-soft, not mandatory for
evidence-pack completeness).

`Review-PaperSmokeEvidence.ps1` renders a "Multi-symbol dispatch summary" section and writes
the following fields to `review_summary.json`:
- `multi_symbol_dispatch_captured`, `multi_symbol_dispatch_truth_state` (`no_snapshot` or
  `active`), `multi_symbol_dispatch_canonical_route`, `multi_symbol_dispatch_backend`,
  `multi_symbol_dispatch_runtime_execution_mode`, `multi_symbol_dispatch_configured_symbol_count`
- `multi_symbol_dispatch_row_count`, `multi_symbol_dispatch_symbols_seen`
- `multi_symbol_dispatch_blocked_or_skipped_symbols` — symbols whose latest
  `no_order_reason` is `b5_short_sale_guard`, `max_new_orders_per_tick_reached`, or
  `symbol_mismatch_skipped`
- `multi_symbol_dispatch_per_symbol` — per-symbol `current_qty`, `target_qty`, `delta`,
  `no_order_reason`, `last_decision_id`, `last_decision_disposition`, `day_order_count`,
  `day_order_limit`, `bar_staleness_secs` (preserved verbatim; `bar_staleness_secs` shows
  `n/a` when null)
- `multi_symbol_dispatch_warnings` — e.g. "not captured" when the snapshot file is missing
  or the daemon was unavailable at capture time

**Interpreting this section:** a missing snapshot or `truth_state = "no_snapshot"` (empty
`per_symbol`) is reported honestly as zero rows — it is **not** evidence of a healthy or
passing trade, and this section does not by itself contribute to a `NATURAL-TRADE-LIFECYCLE-
CLOSED` classification (Section 8). It is observability only.

Covered by MS-EV-01..14 (`test_capture_paper_smoke_evidence.ps1` /
`test_paper_smoke_evidence_review.ps1`) and `test_multi_symbol_smoke_evidence.ps1` (G01-G16).
Parent patch `WATCHLIST-PROMO-V2-MULTI-SYMBOL-AND-SMOKE-01` (Patch 11) remains OPEN — this
sub-patch covers evidence capture/review only, not a real multi-symbol paper smoke run.

---

## 5d. Multi-symbol smoke preflight gate (`MULTI-SYMBOL-SMOKE-RUNNER-PREFLIGHT-GATE-01` — CLOSED)

`Start-PaperTradingSmoke.ps1` STEP 9B is a read-only preflight gate inserted after STEP 9
(Alpaca WS continuity verified) and before STEP 10 (the first mutating operator action).
It calls `GET /api/v1/watchlist/status` and fails closed unless ALL of the following hold:
- `schema_version == "watchlist-v2"`
- `symbols` count `> 1`
- `approved_for_autonomous_paper == true`
- `approved_for_live == false`

If any condition is unmet, the smoke run is refused with one of five stable blocker codes
(an evidence capture is written before each exit):
- `MULTI_SYMBOL_SMOKE_BLOCKED_WATCHLIST_STATUS_UNAVAILABLE` — the status route could not be
  reached or returned no response
- `MULTI_SYMBOL_SMOKE_BLOCKED_SCHEMA_NOT_V2` — `schema_version != "watchlist-v2"`
- `MULTI_SYMBOL_SMOKE_BLOCKED_NOT_MULTI_SYMBOL` — fewer than 2 symbols in the watchlist
- `MULTI_SYMBOL_SMOKE_BLOCKED_NOT_APPROVED_FOR_AUTONOMOUS_PAPER` — promotion gates not yet
  satisfied for autonomous paper
- `MULTI_SYMBOL_SMOKE_BLOCKED_APPROVED_FOR_LIVE_TRUE` — `approved_for_live == true` (hard
  invariant violation; this should never be true in this codebase)

`-CheckOnly` performs a static self-check confirming the STEP 9B gate code is present in
the script (no daemon required); runtime validation against the live watchlist status
happens only during the full smoke run.

Covered by `test_multi_symbol_smoke_runner_gate.ps1` (MSG-01..MSG-14). Parent patch
`WATCHLIST-PROMO-V2-MULTI-SYMBOL-AND-SMOKE-01` (Patch 11) remains OPEN — this sub-patch
covers the smoke-runner preflight gate only, not a real multi-symbol paper smoke run.

---

## 6. Operator notes (fill in manually)

After both captures complete, fill in the four note files in the post-smoke
evidence folder:

| File | What to capture |
|---|---|
| `notes/smoke_lifecycle_checklist.txt` | Full lifecycle YES/NO checklist + exact smoke command |
| `notes/discord_observation.txt` | Discord trade alert observation + screenshot path |
| `notes/gui_observation.txt` | GUI observation + screenshot path |
| `notes/final_verdict.txt` | Operator note only -- check exactly one `[ ]` box (PASSED/PARTIAL/FAILED/NOT RUN) + any blockers. NOT authoritative; see Section 8. |

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

`review_summary.json` / `review_summary.md` (written by
`Review-PaperSmokeEvidence.ps1 -WriteSummary`) are the generated
evidence-review source of truth. `notes/final_verdict.txt` is an
operator-completed note/template only -- it is NOT authoritative and does
not override the `review_summary` classification below.

**SMOKE PASSED** requires `review_summary` classification
`NATURAL-TRADE-LIFECYCLE-CLOSED` (or `READINESS-CLOSED-NO-TRADE` for a
no-trade session), which in turn requires all of:
- Full lifecycle from signal to OMS terminal state.
- `reconcile_status.json` shows clean (no dirty positions).
- `autonomous_readiness.json` shows `overall_ready = true` (or `outside_window` after stop).
- `autonomous_paper_status.json` shows `readiness_classification` != `blocked`.
- `alerts_active.json` shows no `gap_detected` or unresolved fault signals.
- Discord trade alert fired.
- GUI showed filled order matching backend.
- No halt triggered.

If `notes/final_verdict.txt` has `[x] SMOKE PASSED` checked but
`review_summary` classification is `OPEN`, `PARTIAL`, or `FALSE-CLOSED`,
`review_summary` reports `manual_verdict_conflict: true` and prints a
`*** MANUAL VERDICT CONFLICT ***` warning -- treat the evidence as not
passed regardless of the operator note.

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

The smoke automation integration is covered by:

```powershell
powershell -ExecutionPolicy Bypass -File tests\script_guards\test_paper_smoke_automation_status.ps1
```

This guard (SA01-SA15) proves that:
- The capture script fetches and saves `autonomous_paper_status.json`.
- Both smoke runner scripts reference `readiness_classification` and `next_operator_action`.
- Both runners handle `blocked` and `market_proof_pending` classifications.
- The review script reads the status file and writes `computed_delta_qty` and `flatten_available`.
- No raw broker endpoints, order submission, live routing, DB mutation, or secret printing.

---

## 11. Quick reference

| Step | Command |
|---|---|
| Pre-smoke capture | `powershell -ExecutionPolicy Bypass -File scripts\windows\Capture-PaperSmokeEvidence.ps1 -Label pre_smoke` |
| CheckOnly gate | `powershell -ExecutionPolicy Bypass -File scripts\windows\Start-PaperTradingSmoke.ps1 -CheckOnly` |
| Run smoke | `powershell -ExecutionPolicy Bypass -File scripts\windows\Start-PaperTradingSmoke.ps1 -WatchSeconds 420` |
| Post-smoke capture | `powershell -ExecutionPolicy Bypass -File scripts\windows\Capture-PaperSmokeEvidence.ps1 -Label post_smoke` |
| Guard check | `powershell -ExecutionPolicy Bypass -NonInteractive -File tests\script_guards\run_all_script_guards.ps1` |
