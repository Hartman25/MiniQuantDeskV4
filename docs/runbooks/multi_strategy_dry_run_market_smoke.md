# Multi-Strategy Dry-Run Market Smoke — Operator Runbook (MARKET-SMOKE-RUNBOOK-01)

## What this runbook covers

This is the repeatable, off-market-prepared runbook for the
**MULTI-STRATEGY-DRY-RUN-MARKET-SMOKE-01** smoke: proving that the secondary
dry-run strategy `intraday_short_scalper` evaluates alongside the primary
`intraday_scalper` strategy during a real Paper + Alpaca session, surfaces
honest diagnostics on the daemon and GUI, and **never** submits an order.

This runbook does NOT:
- enable real short-entry trading (the short-entry policy gate stays
  fail-closed; see `docs/specs/...` and `capital_policy.rs` — out of scope here)
- let any secondary/dry-run strategy submit an order (structurally impossible —
  see `core-rs/crates/mqk-daemon/src/state/dry_run_strategy.rs` module docs)
- change daemon dispatch, broker submit code, or risk gates
- replace `docs/runbooks/autonomous_paper_ops.md` or
  `docs/runbooks/operator_control_surface.md` — this runbook composes the
  existing proven scripts those documents describe; it does not re-implement
  daemon startup, arming, or shutdown logic.

The companion helper script is
[`scripts/windows/Smoke-MultiStrategyDryRun.ps1`](../../scripts/windows/Smoke-MultiStrategyDryRun.ps1).
It automates every **read-only** check below and emits the exact commands for
every step that mutates daemon/runtime/market-data state. It never starts the
daemon, never starts the feed scheduler, never arms or starts the runtime, and
never calls stop/disarm — see §16.

---

## 1. Pre-flight repo cleanliness

```powershell
cd C:\Users\Zacha\Desktop\MiniQuantDeskV4
git branch --show-current
git log --oneline -5
git status --short --untracked-files=no
```

The tracked working tree must be clean before a market smoke. Untracked files
(e.g. `smoke_logs/`, ledger drafts) do not block — only modified/staged
**tracked** files do. If tracked files are dirty, stop and resolve before
proceeding; do not smoke against an uncommitted diff.

---

## 2. Market-session check (fail-closed)

The only authoritative session truth in this repo is
`mqk_integrity::CalendarSpec::NyseWeekdays` (`core-rs/crates/mqk-integrity/src/calendar.rs`),
consumed by the daemon's `NyseWeekdaysProvider`
(`core-rs/crates/mqk-daemon/src/state/market_calendar.rs`) and surfaced live at:

```
GET /api/v1/autonomous/readiness    -> session_in_window, session_window_state
GET /api/v1/system/preflight        -> deployment_start_allowed, blockers
```

**Before the daemon is running**, there is no live route to query. The helper
script's preflight check parses the actual `HOLIDAYS` / `EARLY_CLOSE_DATES`
tables out of `calendar.rs` at run time (not a hand-copied duplicate) and
classifies the current Eastern-Time moment the same way `nyse_classify_session`
does: weekday Mon–Fri, 09:30–16:00 ET (or 09:30–13:00 ET on an early-close
day), excluding the holiday table. This is a **heuristic mirror**, labeled as
such — once the daemon is reachable, prefer its live `session_window_state`.

### Blocked-market-closed path

```text
If repo calendar/session truth says market is closed:
  report BLOCKED_MARKET_CLOSED
  do not start daemon/provider/scheduler
```

This applies whether the closure is a weekend, a full holiday, or simply
outside 09:30–16:00 ET on a trading day. `BLOCKED_MARKET_CLOSED` is not a
failure of the patch or the repo — it is the correct, honest outcome. Record
it (date, reason, retry date) and stop. Do not inject synthetic bars, do not
override the session window via `MQK_SESSION_START_HH_MM`/`MQK_SESSION_STOP_HH_MM`
to force an off-hours "smoke," and do not start the feed scheduler or runtime.

---

## 3. Paper DB readiness

Canonical paper container (matches `scripts/windows/Start-PaperTradingSmoke.ps1`):

```powershell
docker inspect mqk-paper-postgres
docker exec mqk-paper-postgres pg_isready -U postgres -d miniquantdesk_paper
```

If the container is missing, create it exactly as documented in
`Start-PaperTradingSmoke.ps1`'s header (port `5440->5432`, db
`miniquantdesk_paper`). Do not point this smoke at any other database.

---

## 4. Daemon port readiness / stale daemon cleanup

```powershell
netstat -ano | findstr :8899
```

If a stale `mqk-daemon` or `mqk-gui` process is still bound to the port, stop
it the same way `Start-PaperTradingSmoke.ps1` STEP 1 does:

```powershell
Get-Process -Name mqk-daemon -ErrorAction SilentlyContinue | Stop-Process -Force
Get-Process -Name mqk-gui    -ErrorAction SilentlyContinue | Stop-Process -Force
```

If a daemon is already reachable on the port, check its identity before
reusing it — do not assume it is the build you expect:

```powershell
Invoke-RestMethod http://127.0.0.1:8899/api/v1/system/status |
  Select-Object daemon_mode, adapter_id, live_routing_enabled, runtime_status
```

---

## 5. Required environment

| Variable | Required value | Why |
|---|---|---|
| `MQK_DAEMON_DEPLOYMENT_MODE` | `paper` | Canonical paper path. |
| `MQK_DAEMON_ADAPTER_ID` | `alpaca` | Paper+Alpaca is the only credible autonomous path (`README.md`). |
| `MQK_STRATEGY_IDS` | `intraday_scalper` | Primary, long-only, live-order-eligible fleet. |
| `MQK_STRATEGY_SYMBOL` | `AAPL` | Smoke symbol. |
| `MQK_STRATEGY_MD_TIMEFRAME` | `5m` | Intraday bars. |
| `MQK_DRY_RUN_STRATEGY_IDS` | `intraday_short_scalper` | Secondary dry-run evaluation only — see `DRY_RUN_STRATEGY_IDS_ENV` in `state/dry_run_strategy.rs`. |

```powershell
$env:MQK_DAEMON_DEPLOYMENT_MODE = "paper"
$env:MQK_DAEMON_ADAPTER_ID      = "alpaca"
$env:MQK_STRATEGY_IDS           = "intraday_scalper"
$env:MQK_STRATEGY_SYMBOL        = "AAPL"
$env:MQK_STRATEGY_MD_TIMEFRAME  = "5m"
$env:MQK_DRY_RUN_STRATEGY_IDS   = "intraday_short_scalper"
```

These are **process environment variables in the operator's shell**, set
immediately before starting the daemon from that same shell. Do not write
them into `.env.local`. Live routing is never set here — there is no
`MQK_LIVE_ROUTING_ENABLED=true` anywhere in this runbook, and
`Start-PaperTradingSmoke.ps1` independently refuses to proceed if it finds one
set truthy in the environment.

---

## 6. Starting the daemon

Use the existing, proven startup runbook — do not hand-start the daemon for
this smoke:

```powershell
powershell -ExecutionPolicy Bypass -File scripts\windows\Start-PaperTradingSmoke.ps1 -CheckOnly
powershell -ExecutionPolicy Bypass -File scripts\windows\Start-PaperTradingSmoke.ps1 -WatchSeconds 1800
```

`Start-PaperTradingSmoke.ps1` already: refuses if `daemon_mode != paper`,
refuses if `live_routing_enabled=true`, reasserts the paper DB URL, runs
migrations, builds the daemon if stale, waits for `/v1/health`, verifies
identity, and watches WS continuity. Run it from a shell where the §5 env vars
(especially `MQK_DRY_RUN_STRATEGY_IDS`) are already exported, so the daemon
process inherits them.

---

## 7. Starting the AAPL/5m feed scheduler

```powershell
$body = @{
  provider_id              = "alpaca"
  symbols                  = @("AAPL")
  timeframe                = "5m"
  dry_run                  = $false
  allow_provider_api_calls = $true
} | ConvertTo-Json -Compress

Invoke-RestMethod -Uri http://127.0.0.1:8899/api/v1/market-data/feed/scheduler/start `
  -Method Post -ContentType 'application/json' -Body $body

Invoke-RestMethod http://127.0.0.1:8899/api/v1/market-data/feed/scheduler/status
```

This is the route backing the GUI's "Market data feed scheduler" panel
(`core-rs/crates/mqk-daemon/src/routes/ingest.rs`, `IngestScreen.tsx`).
`allow_provider_api_calls=true` is required whenever `dry_run=false` — the
route refuses otherwise (`market_data_feed_scheduler_start`). This only
ingests bars into `md_bars`; it has no path to the outbox, OMS, or broker.

---

## 8. Starting/arming the primary paper runtime

`Start-PaperTradingSmoke.ps1` (§6) already adopts the broker baseline, gates on
a clean reconcile, and arms execution as part of its STEP 10–15 sequence — no
separate action is needed. If you started the daemon some other way, the
manual sequence is:

```powershell
Invoke-RestMethod -Uri http://127.0.0.1:8899/api/v1/ops/action -Method Post `
  -Headers @{Authorization="Bearer $env:MQK_OPERATOR_TOKEN"} -ContentType 'application/json' `
  -Body '{"action_key":"arm-execution"}'
```

The autonomous session controller starts the run automatically inside the
session window once armed (see `docs/runbooks/autonomous_paper_ops.md` §4 and
§6) — do not call `start-system` directly; it races the session controller
(documented pitfall in `Start-PaperTradingSmoke.ps1` STEP 15).

---

## 9. Polling dry-run status and readiness

```powershell
Invoke-RestMethod http://127.0.0.1:8899/api/v1/strategy/dry-run/status | ConvertTo-Json -Depth 6
Invoke-RestMethod http://127.0.0.1:8899/api/v1/autonomous/readiness    | ConvertTo-Json -Depth 6
```

`GET /api/v1/strategy/dry-run/status` is a pure read of in-memory diagnostics
computed by `evaluate_dry_run_strategies` — no DB query, no broker call, no
outbox access (`routes/strategy/dry_run_status.rs` module docs; structurally
enforced and proven by `scenario_multi_strategy_dry_run_status_01.rs` test
`s09_route_handler_source_contains_no_broker_or_outbox_calls`).

Expected shape once `intraday_short_scalper` has evaluated at least one bar:

| Field | Expected |
|---|---|
| `truth_state` | `"active"` |
| `configured_dry_run_strategy_ids` | `["intraday_short_scalper"]` |
| `dry_run_strategy_diagnostics[0].strategy_id` | `"intraday_short_scalper"` |
| `dry_run_strategy_diagnostics[0].submitted` | `false` (always) |
| `dry_run_strategy_diagnostics[0].would_classify_as` | `"ShortOpen"` on a bearish window, `"NoOp"`/`"already_at_target"` otherwise |
| `dry_run_strategy_diagnostics[0].would_b5_block` | `true` whenever `would_classify_as == "ShortOpen"` |
| `dry_run_strategy_diagnostics[0].would_policy_block` | `true` whenever `would_classify_as == "ShortOpen"` (default fail-closed policy) |
| `dry_run_strategy_diagnostics[0].policy_reason_code` | `"short_entries_disabled"` when `would_policy_block=true`, else `null` |

If `truth_state == "not_configured"` or `dry_run_strategy_diagnostics` is
empty after the runtime has ticked at least once, re-check §5 — the env var
was not present in the daemon process's environment (it must be set in the
shell **before** the daemon starts; the daemon does not hot-reload it).

---

## 10. GUI check (manual)

1. `cd core-rs\mqk-gui && npm run dev` (or use the packaged desktop build).
2. Open `http://127.0.0.1:1420`.
3. Navigate to the **Strategy** screen.
4. Locate the **"Multi-strategy dry-run diagnostics"** panel
   (subtitle: *"Read-only secondary-strategy evaluation. DRY RUN ONLY —
   submitted=false, no order submission."*).
5. Confirm:
   - "Configured dry-run strategy ids" shows `intraday_short_scalper`.
   - The diagnostics table has a row for `intraday_short_scalper` / `AAPL`.
   - The **Submitted** column reads `false` for every row.
   - If the row's Decision is `b5_short_sale_guard`, **B5 Block** and
     **Policy Block** both read `true`.

This panel has no order submit/cancel/replace control of any kind
(`StrategyScreen.tsx` comment above `DryRunStrategyDiagnosticsPanel`) — it
cannot be used to accidentally place an order.

---

## 11. No-order proof: `intraday_short_scalper`

There is no DB column named `strategy_id` on any order/outbox table — the
durable outbox (`oms_outbox`, migration `0001_init.sql`) stores the full order
intent (including `strategy_id`) inside the `order_json` jsonb column (see
`mqk-daemon/src/decision.rs`: `"strategy_id": d.strategy_id.trim()`). The
authoritative read-only proof query is:

```powershell
docker exec mqk-paper-postgres psql -U postgres -d miniquantdesk_paper -t -A -q -c `
  "SELECT count(*) FROM oms_outbox WHERE order_json->>'strategy_id' = 'intraday_short_scalper';"
```

Expected result: `0`.

There is also no persisted `broker_order_map` or any other execution table
keyed by `strategy_id` (checked against `0007_broker_order_map.sql` and every
other migration) — the outbox query above is the complete, authoritative
no-order proof. `GET /api/v1/execution/orders` always returns
`strategy_id: null` (not wired — `routes/execution.rs`) and must **not** be
used as evidence either way.

---

## 12. Live safety proof

```powershell
Invoke-RestMethod http://127.0.0.1:8899/api/v1/system/status |
  Select-Object daemon_mode, adapter_id, live_routing_enabled
```

Expected: `daemon_mode = "paper"`, `adapter_id = "alpaca"`,
`live_routing_enabled = false`. `live_routing_enabled` is derived from the
active run's mode (`true` only if the run mode is `LIVE`/`LIVE-CAPITAL` while
`running`; `false` whenever idle/halted/unknown — `routes/helpers.rs`). A
paper-mode run can never report `true`.

---

## 13. Shutdown

Use the existing proven clean-shutdown script — do not hand-roll a shutdown
sequence:

```powershell
powershell -ExecutionPolicy Bypass -File scripts\windows\Stop-PaperTradingClean.ps1 -Label dry_run_market_smoke
```

This captures pre-stop evidence, calls `stop-system`, verifies stopped,
calls `disarm-execution`, captures post-stop evidence, and prints the manual
process/DB-container shutdown reminder (it never kills the daemon process or
stops the Postgres container itself). Then, if desired:

```powershell
Stop-Process -Name mqk-daemon -ErrorAction SilentlyContinue
Stop-Process -Name mqk-gui    -ErrorAction SilentlyContinue
netstat -ano | findstr :8899
```

Confirm the last command returns nothing — the port is free.

---

## 14. Final proof checklist

Fill this in from live values observed during the smoke. Do not mark
`Verdict` `CLOSED`/`PASS` from memory — every field must come from a command
run in this session (per `.claude/rules/audit_repo_truth_rules.md`).

```text
Verdict:
Branch/commit:
Market session:
Daemon mode:
MQK_DRY_RUN_STRATEGY_IDS:
Dry-run status truth_state:
Dry-run configured ids:
Dry-run diagnostics count:
Dry-run strategy_id:
Dry-run target_qty:
Dry-run would_classify_as:
Dry-run would_b5_block:
Dry-run would_policy_block:
Dry-run policy_reason_code:
Dry-run submitted:
GUI observed:
intraday_short_scalper outbox row count:
intraday_short_scalper order/execution row count:
live_routing:
Shutdown proof:
```

---

## 15. If the market is closed

```text
BLOCKED_MARKET_CLOSED
Date: <today, ET>
Reason: <weekend | holiday name | outside 09:30-16:00 ET>
Code changes: none
Pre-smoke validation: <green|red> (see §16)
Retry date: <next NYSE trading day>
```

Do not start the daemon, the feed scheduler, or the runtime when this path is
hit. Re-run the helper script's preflight on the retry date.

---

## 16. Using the helper script

[`scripts/windows/Smoke-MultiStrategyDryRun.ps1`](../../scripts/windows/Smoke-MultiStrategyDryRun.ps1)
automates §1–§5, §11, and §12's read-only checks, and prints the exact
commands for §6–§10 and §13. It never starts the daemon, never starts the
feed scheduler, never arms or starts the runtime, and never calls
stop/disarm.

```powershell
# Safe default — preflight checks + checklist only:
powershell -ExecutionPolicy Bypass -File scripts\windows\Smoke-MultiStrategyDryRun.ps1 -PreflightOnly

# Same checks, plus live polling of an already-running daemon (still read-only):
powershell -ExecutionPolicy Bypass -File scripts\windows\Smoke-MultiStrategyDryRun.ps1 -RunSmoke
```

See the script header for the full safety contract and parameter list.

---

## 17. What this runbook does not change

- The short-entry policy gate (`capital_policy.rs`,
  `evaluate_short_entry_policy`) remains fail-closed by default. Nothing here
  configures `ShortEntryConfig` to allow short entries.
- B5 (`would_b5_block`) is not touched, weakened, or bypassed.
- Secondary/dry-run strategies remain structurally incapable of submitting an
  order (`evaluate_dry_run_strategy`/`evaluate_dry_run_strategies` take no
  `AppState`, no `PgPool`, no broker handle).
- No broker submit code, daemon dispatch code, or DB migration is part of
  this patch.
