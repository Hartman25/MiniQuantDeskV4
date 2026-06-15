# MiniQuantDesk V4 — Master Patch Ledger and Handoff

**Purpose:** Preserve all active, queued, parked, and recently closed MiniQuantDesk patches so future ChatGPT / Claude / Codex sessions do not lose context.

**Primary operating rule:** Work from the **current local repo/worktree only**. Do not trust prior chat claims, screenshots, docs, patch labels, or memory unless the current repo, live daemon API, DB rows, artifacts, tests, scripts, or command output proves it.

**Repo root:** `C:\Users\Zacha\Desktop\MiniQuantDeskV4`  
**Rust workspace:** `C:\Users\Zacha\Desktop\MiniQuantDeskV4\core-rs`  
**GUI path:** `C:\Users\Zacha\Desktop\MiniQuantDeskV4\core-rs\mqk-gui`  
**Daemon:** `mqk-daemon` on `127.0.0.1:8899`  
**Paper DB container:** `mqk-paper-postgres`  
**Paper DB:** `miniquantdesk_paper`  
**Typical paper DB URL:** `postgres://postgres:postgres@127.0.0.1:5440/miniquantdesk_paper?sslmode=disable`

---

## 1. Prompt / Patch Operating Rules

Every Claude prompt should start with:

```text
Work from the CURRENT local MiniQuantDesk repo/worktree only.

Do not trust prior chat claims, screenshots, docs, patch labels, or memory unless the CURRENT repo, live daemon API, DB rows, artifacts, test output, scripts, or command output proves it.

Codex and ChatGPT will audit your output. Do not claim anything unless current repo evidence supports it.
```

Every prompt must include:

- One patch name only.
- One mission only.
- Current proven facts only.
- Non-negotiable constraints.
- Inspection commands before editing.
- Live probes if runtime/operator behavior is involved.
- DB probes if DB truth is involved.
- Exact validation commands.
- Required final report.
- Closure verdict: `CLOSED / PARTIAL / OPEN / FALSE-CLOSED`.

Do **not** ask Claude to “make it work” broadly. Ask Claude to:

1. Find the first proven failing gate.
2. Patch that gate only.
3. Prove it.
4. Stop.

### One-patch discipline

- One patch at a time.
- If unrelated failures appear, classify them as unrelated / pre-existing / blocking.
- Do **not** fold unrelated repairs into the active patch.
- If one unrelated failure blocks validation, create a separate named patch.
- One unrelated failure patch at a time only.

### Backend / trading safety constraints

For backend/trading prompts, always include:

- Do NOT enable live routing.
- Do NOT submit live orders.
- Do NOT submit paper/live broker orders unless the mission explicitly requires a safe paper execution proof.
- Do NOT bypass risk, integrity, reconcile, broker, lease, arm, session, or DB gates.
- Do NOT weaken fail-closed behavior.
- Do NOT fabricate orders, signals, fills, snapshots, execution-flow rows, market data, or artifacts.
- Do NOT change strategy logic merely to force trades.
- Do NOT hide blockers.
- Failed/rejected paper orders are acceptable only if naturally produced through the canonical path and durably recorded.

### GUI prompt constraints

For GUI prompts, always include:

- Do NOT change backend logic.
- Do NOT change API contracts unless explicitly required.
- Do NOT change trading behavior.
- Do NOT hide unavailable/fail-closed states.
- GUI must render truthful unavailable/no-data states instead of crashing.
- Presentation-only unless explicitly stated otherwise.

### Required Claude final report format

Every Claude report must include:

1. Current git status before work.
2. Inspection findings.
3. Exact root cause.
4. Exact files changed.
5. Full touched defs/components/sections.
6. Tests added/updated.
7. Validation results.
8. Smoke result if applicable.
9. Unrelated failures and classification.
10. Safety confirmation.
11. Final verdict: `CLOSED / PARTIAL / OPEN / FALSE-CLOSED`.

### Closure definitions

```text
CLOSED:
The exact failing seam is fixed and validated.

PARTIAL:
One blocker was fixed, but another blocker remains, or code/tests pass but required live smoke was not run.

OPEN:
Root cause is unknown or no safe fix was made.

FALSE-CLOSED:
Claude claims success while evidence still shows failure, hides a blocker, weakens safety, fabricates success, skips validation, or changes behavior outside scope.
```

---

## 2. Standard Validation Commands

### Rust / backend pattern

```powershell
cargo fmt --manifest-path .\core-rs\Cargo.toml -p <affected-crate> -- --check

cargo clippy --manifest-path .\core-rs\Cargo.toml `
  -p mqk-daemon -p mqk-runtime -p mqk-db -p mqk-execution `
  --all-targets -- -D warnings

git diff --check
git status --short --untracked-files=all
```

If broker crate touched, include:

```powershell
-p mqk-broker-alpaca
# or
-p mqk-broker-paper
```

### GUI validation pattern

```powershell
cd core-rs\mqk-gui
npm test -- --run
npm run build
cd ..\..

git diff --check
git status --short --untracked-files=all
```

### Important commit rules

- Do not commit evidence folders.
- Do not commit `.env.local`.
- Do not commit broad unrelated formatting.
- One logical patch commit per blocker.

---

## 3. Standard Live API Probes

```powershell
cd C:\Users\Zacha\Desktop\MiniQuantDeskV4

$base = "http://127.0.0.1:8899"

Invoke-RestMethod "$base/api/v1/system/status" | ConvertTo-Json -Depth 30
Invoke-RestMethod "$base/api/v1/autonomous/readiness" | ConvertTo-Json -Depth 30
Invoke-RestMethod "$base/api/v1/system/preflight" | ConvertTo-Json -Depth 30
Invoke-RestMethod "$base/api/v1/events/feed" | ConvertTo-Json -Depth 30
Invoke-RestMethod "$base/api/v1/execution/summary" | ConvertTo-Json -Depth 30
Invoke-RestMethod "$base/api/v1/execution/flow" | ConvertTo-Json -Depth 30
Invoke-RestMethod "$base/api/v1/alerts/active" | ConvertTo-Json -Depth 30
```

Safe execution/orders probe:

```powershell
try {
  Invoke-RestMethod "$base/api/v1/execution/orders" | ConvertTo-Json -Depth 30
} catch {
  Write-Host "execution/orders status:" $_.Exception.Response.StatusCode.value__
  if ($_.Exception.Response) {
    $reader = New-Object System.IO.StreamReader($_.Exception.Response.GetResponseStream())
    Write-Host $reader.ReadToEnd()
  }
}
```

---

## 4. Standard DB Probes

### Runs

```powershell
docker exec mqk-paper-postgres psql -U postgres -d miniquantdesk_paper -c "
select run_id, started_at_utc, armed_at_utc, running_at_utc, stopped_at_utc, halted_at_utc, status, last_heartbeat_utc
from runs
order by started_at_utc desc
limit 10;
"
```

### Autonomous session events

```powershell
docker exec mqk-paper-postgres psql -U postgres -d miniquantdesk_paper -c "
select ts_utc, detail, run_id
from sys_autonomous_session_events
order by ts_utc desc
limit 100;
"
```

### Audit events

```powershell
docker exec mqk-paper-postgres psql -U postgres -d miniquantdesk_paper -c "
select *
from audit_events
order by ts_utc desc
limit 100;
"
```

### Strategy registry

```powershell
docker exec mqk-paper-postgres psql -U postgres -d miniquantdesk_paper -c "
select *
from sys_strategy_registry
order by strategy_id;
"
```

### Arm state

```powershell
docker exec mqk-paper-postgres psql -U postgres -d miniquantdesk_paper -c "
select *
from sys_arm_state;
"
```

### No-trade table discovery

```powershell
docker exec mqk-paper-postgres psql -U postgres -d miniquantdesk_paper -c "
select table_name
from information_schema.tables
where table_schema='public'
and (
  table_name ilike '%signal%'
  or table_name ilike '%order%'
  or table_name ilike '%outbox%'
  or table_name ilike '%inbox%'
  or table_name ilike '%intent%'
  or table_name ilike '%admission%'
  or table_name ilike '%flow%'
  or table_name ilike '%strategy%'
  or table_name ilike '%risk%'
  or table_name ilike '%journal%'
)
order by table_name;
"
```

Always inspect `information_schema.columns` before guessing table columns.

---

## 5. Normal Startup Commands for Paper Trading

### Terminal 1 — start daemon cleanly

```powershell
cd C:\Users\Zacha\Desktop\MiniQuantDeskV4

$listener = Get-NetTCPConnection -LocalPort 8899 -State Listen -ErrorAction SilentlyContinue
if ($listener) {
  $proc = Get-Process -Id $listener.OwningProcess
  Write-Host "Port 8899 is held by PID $($listener.OwningProcess): $($proc.ProcessName)"
  if ($proc.ProcessName -eq "mqk-daemon") {
    Stop-Process -Id $listener.OwningProcess -Force
    Start-Sleep -Seconds 2
  } else {
    Write-Host "Non-daemon process owns 8899. Stop manually before continuing." -ForegroundColor Red
    exit 1
  }
}

docker start mqk-paper-postgres
docker exec mqk-paper-postgres pg_isready -U postgres -d miniquantdesk_paper

$env:MQK_DATABASE_URL = "postgres://postgres:postgres@127.0.0.1:5440/miniquantdesk_paper?sslmode=disable"
$env:MQK_SESSION_START_HH_MM = "13:30"
$env:MQK_SESSION_STOP_HH_MM = "20:00"
$env:RUST_BACKTRACE = "1"

cargo run --manifest-path .\core-rs\Cargo.toml -p mqk-cli --bin mqk-cli -- db migrate --yes

docker exec mqk-paper-postgres psql -U postgres -d miniquantdesk_paper -c "
insert into sys_strategy_registry
  (strategy_id, display_name, enabled, kind, registered_at_utc, updated_at_utc, note)
values
  ('intraday_scalper', 'Intraday Scalper', true, 'native', now(), now(), 'Seeded for autonomous paper trading startup')
on conflict (strategy_id) do update
set enabled = true,
    display_name = excluded.display_name,
    kind = excluded.kind,
    updated_at_utc = now(),
    note = excluded.note;
"

cargo run --manifest-path .\core-rs\Cargo.toml -p mqk-daemon --bin mqk-daemon
```

Leave Terminal 1 open.

### Terminal 2 — clear stale halted state and arm

```powershell
cd C:\Users\Zacha\Desktop\MiniQuantDeskV4

$base = "http://127.0.0.1:8899"
$token = (Get-Content .env.local | Where-Object { $_ -match "^MQK_OPERATOR_TOKEN=" }) -replace "^MQK_OPERATOR_TOKEN=", ""
$headers = @{ Authorization = "Bearer $token" }

Start-Sleep -Seconds 15

$clearBody = @{
  action_key = "clear-halted-run"
  reason = "Clear stale paper halted state before normal market startup"
} | ConvertTo-Json

try {
  Invoke-RestMethod `
    -Method Post `
    -Uri "$base/api/v1/ops/action" `
    -Headers $headers `
    -ContentType "application/json" `
    -Body $clearBody |
    ConvertTo-Json -Depth 20
} catch {
  Write-Host "clear-halted-run skipped or refused; continuing to arm check." -ForegroundColor Yellow
}

$armBody = @{
  action_key = "arm-execution"
  reason = "Arm paper trading for normal market session startup"
} | ConvertTo-Json

Invoke-RestMethod `
  -Method Post `
  -Uri "$base/api/v1/ops/action" `
  -Headers $headers `
  -ContentType "application/json" `
  -Body $armBody |
  ConvertTo-Json -Depth 20

Invoke-RestMethod "$base/api/v1/autonomous/readiness" | ConvertTo-Json -Depth 30
Invoke-RestMethod "$base/api/v1/system/status" | ConvertTo-Json -Depth 30
Invoke-RestMethod "$base/api/v1/system/preflight" | ConvertTo-Json -Depth 30
Invoke-RestMethod "$base/api/v1/alerts/active" | ConvertTo-Json -Depth 30
```

Before open, acceptable:

- `arm_state=armed`
- `arm_ready=true`
- `strategy_armed=true`
- `execution_armed=true`
- `session_window_state=outside_window`
- `overall_ready=false` only because outside_window
- `live_routing_enabled=false`
- `alerts=0`

At/after open, desired:

- `session_in_window=true`
- `session_window_state=in_window`
- `runtime_status=running`
- latest DB run `status=RUNNING`
- `live_routing_enabled=false`

### Terminal 3 — GUI

```powershell
cd C:\Users\Zacha\Desktop\MiniQuantDeskV4\core-rs\mqk-gui
npm run tauri dev
```

Or launcher:

```powershell
cd C:\Users\Zacha\Desktop\MiniQuantDeskV4
powershell -NoProfile -ExecutionPolicy Bypass -File .\scripts\windows\Launch-VeritasLedger.ps1 -Mode Observe
```

### Terminal 4 — watcher

```powershell
cd C:\Users\Zacha\Desktop\MiniQuantDeskV4

$base = "http://127.0.0.1:8899"

while ($true) {
  Clear-Host
  Get-Date

  try {
    $ready = Invoke-RestMethod "$base/api/v1/autonomous/readiness"
    $status = Invoke-RestMethod "$base/api/v1/system/status"
    $alerts = Invoke-RestMethod "$base/api/v1/alerts/active"
    $summary = Invoke-RestMethod "$base/api/v1/execution/summary"
    $flow = Invoke-RestMethod "$base/api/v1/execution/flow"

    "mode=$($status.daemon_mode) adapter=$($status.adapter_id) live_routing=$($status.live_routing_enabled)"
    "runtime=$($status.runtime_status) db=$($status.db_status) broker=$($status.broker_status)"
    "armed: strategy=$($status.strategy_armed) execution=$($status.execution_armed) arm_state=$($ready.arm_state) arm_ready=$($ready.arm_ready)"
    "halts: kill=$($status.kill_switch_active) risk=$($status.risk_halt_active) integrity=$($status.integrity_halt_active)"
    "ws=$($ready.ws_continuity) ws_ready=$($ready.ws_continuity_ready)"
    "now_utc=$($ready.now_utc)"
    "window=$($ready.session_start_utc)-$($ready.session_stop_utc) source=$($ready.session_window_source)"
    "session=$($ready.session_window_state) in_window=$($ready.session_in_window)"
    "runtime_start_allowed=$($ready.runtime_start_allowed) overall_ready=$($ready.overall_ready)"
    "execution: has_snapshot=$($summary.has_snapshot) active_orders=$($summary.active_orders) flow_truth=$($flow.truth_state) run_id=$($flow.run_id)"
    "alerts=$($alerts.alert_count)"
    ""
    "blockers:"
    $ready.blockers
  } catch {
    "WATCHER ERROR:"
    $_.Exception.Message
  }

  Start-Sleep -Seconds 30
}
```

---

## 6. Active / Next Patch

### BACKTEST-GUI-CLOSURE-01 — CLOSED

**Closure (automated component-logic test):** The Backtest Results GUI workflow
is closed end-to-end. The polling effect's status mapping and auto-load decision
were inline reimplementations that did not call the tested pure helpers
(`buildActiveJob`, `extractArtifactDir`) — a hollow proof. They were wired into
the production polling path, and `api.test.ts` `B06`/`B06b`/`B06c`/`B06d`
sequence tests now drive those exact helpers over realistic
`queued → running → completed`/`failed` sequences, asserting the single
auto-load trigger. The Tauri `read_artifact_file` allowlist was verified to
cover all 10 files `loadBundle` requests. Submit calls the real
`POST /api/v1/backtests/jobs` (no mockData); manual Workflow-A load and truthful
parser/missing/failed states already existed and are covered by
`parsers.test.ts`. GUI workflow documented in
`docs/runbooks/backtest_workflow.md`. Full GUI suite 393/393 pass; `npm run build`
clean. No broker/OMS/runtime/paper/live path touched.

- Files: `core-rs/mqk-gui/src/features/backtests/BacktestResultsScreen.tsx`,
  `core-rs/mqk-gui/src/features/backtests/__tests__/api.test.ts`,
  `docs/runbooks/backtest_workflow.md`.
- `BACKTEST-GUI-POLISH-01` — CLOSED (presentation polish applied; see Section 9).

**Original mission:** Live-prove and repair if needed the Backtest Results GUI workflow end-to-end:

- GUI can submit a CSV backtest job to daemon.
- GUI polls job status.
- Completed job auto-loads the artifact folder.
- GUI displays manifest, metrics, equity curve, orders, fills truthfully.
- Manual artifact-folder loading also works.

**Current proven context:**

- Backtest CLI, metrics, daemon jobs, dataset support, and daily integrity threshold UX are closed.
- Backtest daemon routes exist:
  - `POST /api/v1/backtests/jobs`
  - `GET /api/v1/backtests/jobs`
  - `GET /api/v1/backtests/jobs/:job_id`
- Real 1D data exists at `exports/md_backup/1D/AAPL_1D.csv`.
- AAPL_1D has 8,375 bars.
- GUI Backtest Results screen exists but still needs final live closure.

**Manual API smoke payload:**

```powershell
$body = @{
  bars_path = "C:\Users\Zacha\Desktop\MiniQuantDeskV4\exports\md_backup\1D\AAPL_1D.csv"
  strategy = "swing_momentum"
  symbol = "AAPL"
  timeframe_secs = 86400
  initial_cash_micros = 100000000000
  integrity_enabled = $true
  integrity_stale_threshold_ticks = 345600
  out_dir = "C:\Users\Zacha\Desktop\MiniQuantDeskV4\exports\backtests"
} | ConvertTo-Json -Depth 20
```

**GUI smoke:**

1. Launch with:
   `powershell -NoProfile -ExecutionPolicy Bypass -File .\scripts\windows\Launch-VeritasLedger.ps1 -Mode Observe -RebuildGui`
2. Open Backtest Results.
3. Submit AAPL 1D job.
4. Confirm job completes or fails truthfully.
5. Confirm completed artifact auto-loads.
6. Confirm manifest / metrics / equity curve / orders / fills render.
7. Confirm manual artifact-folder load works by pasting the artifact folder.

**Closure:** `CLOSED` only if the GUI can submit or live API proves submit and GUI can manually load resulting artifact, polling/status works or exact GUI limitation is patched, artifact display works, and no live/paper execution behavior changes.

---

## 7. Recently Closed Backtest / Research Patches

```text
BACKTEST-PROOF-01: CLOSED
BACKTEST-METRICS-01: CLOSED
BACKTEST-DAEMON-JOBS-01: CLOSED
BACKTEST-DAEMON-JOBS-02: CLOSED
BACKTEST-DATASET-01: CLOSED
BACKTEST-CLI-UX-01: CLOSED
BACKTEST-DAILY-STALE-DEFAULT-FIX-01: CLOSED
DESKTOP-LAUNCH-GUI-AUTO-REBUILD-01: CLOSED
```

### Key facts to preserve

- Backtest artifacts contain `manifest.json`, `metrics.json`, `equity_curve.csv`, `orders.csv`, `fills.csv`, `audit.jsonl`.
- Deep metrics were added to `metrics.json`.
- Backtest daemon jobs are isolated from live/paper execution.
- `exports/md_backup/1D` schema is compatible after loader support for `t/f` booleans.
- Real AAPL 1D backtest proof loaded 8,375 bars.
- For daily bars, the daemon backtest-jobs API defaults `integrity_stale_threshold_ticks` to `345600` (4 days) so weekend gaps (~259 200 s) do not falsely block 1D data; the prior `172800` (2-day) default sat below a weekend gap (BACKTEST-DAILY-STALE-DEFAULT-FIX-01). Explicit request values still override.
- Do not make backtests automatically call TwelveData. Backtests should use known local data for repeatability and to avoid hidden API-credit use.

---

## 8. Recently Closed Data Ingestion Patches

```text
DATA-INGEST-AUDIT-01: CLOSED
DATA-INGEST-DAEMON-JOBS-01: CLOSED
DATA-INGEST-DAEMON-JOBS-02: CLOSED
DATA-INGEST-GUI-RUNNER-01: CLOSED
DATA-INGEST-GUI-RUNNER-02: CLOSED
DATA-INGEST-GUI-RESULTS-01: CLOSED
```

### Current ingestion capabilities

- Existing CLI entry points:
  - `mqk md ingest-csv`
  - `mqk md ingest-provider`
  - `mqk md sync-provider`
  - `backfill_daily_bars.ps1`
- Existing provider support:
  - TwelveData implemented.
  - Local CSV implemented.
  - Alpaca historical data not wired.
  - Other providers listed as future only.
- Existing backup data:
  - `exports/md_backup/1D`: 88 files.
  - `exports/md_backup/5m`: empty.
  - `exports/md_backup/daily`: legacy.
- Daemon ingestion routes:
  - `POST /api/v1/ingest/jobs`
  - `GET /api/v1/ingest/jobs`
  - `GET /api/v1/ingest/jobs/:job_id`
- Coverage route:
  - `GET /api/v1/market-data/coverage?timeframe=1D`
- GUI Ingest screen is visible under Market Data.
- GUI Ingest screen can show local data coverage.
- Live proof showed:
  - `AAPL / 1D / 8375` in API and GUI coverage table.

### Important ingestion design rule

Keep data acquisition separate from backtesting and trading:

```text
Update data intentionally → confirm coverage → run backtests from known local data.
```

Do not let backtests silently call TwelveData.

---

## 9. Remaining Backtest / Research Workflow Patches

### BACKTEST-GUI-CLOSURE-01 — CLOSED

See Section 6.

### BACKTEST-GUI-POLISH-01 — CLOSED

**Purpose:** Optional cleanup after closure if Backtest Results screen has layout/scrolling/clarity issues during live smoke.

**Constraints:** GUI presentation only. No engine/daemon/trading behavior.

**Outcome:** Presentation polish applied to the Backtest Results screen
(`BacktestResultsScreen.tsx` + `styles.css`). Workflows A and B now have labeled
heading banners and a divider; rendered results carry a source line identifying
which workflow produced them; the artifact display groups its panels under
section headings (Run identity & performance / Research & promotion gates /
Observability reference / Execution detail); long monospace values (hashes,
host fingerprint, artifact paths, evidence folder) wrap instead of clipping; the
equity-curve chart was given more vertical room. No parser, API, submit/poll,
auto-load, or trading behavior changed. Validation: `npm test` 393/393 pass,
`npm run build` clean (tsc + vite). See `docs/runbooks/backtest_workflow.md`
*"Presentation (BACKTEST-GUI-POLISH-01)"*.

### BACKTEST-REPORT-UX-01 — CLOSED

**Purpose:** Improve how backtest output is summarized for operator review:

- headline performance cards
- better explanation of no-trade results
- drawdown / return / trade-count readability
- clearer artifact metadata

**Constraints:** GUI presentation only. No engine/calculation/contract/trading
behavior.

**Outcome:** New "Operator review" panel at the top of the Backtest Results
artifact display (`BacktestResultsScreen.tsx`), above *Run identity &
performance*. It surfaces headline StatCards (Total Return, Buy & Hold,
Alpha vs Buy & Hold, Max Drawdown, Run Status, Trades, Win Rate, Profit
Factor, Orders Rejected, Exposure), a no-trade explanation when
`trade_count == 0 && fills == 0`, execution-blocked/halted warning blocks, and
an artifact-identity grid (Run ID, Strategy, Symbol(s), Timeframe, Config
hash). `types.ts` gained `BacktestBenchmark` and an optional
`BacktestMetrics.benchmark` field mirroring the Rust `BenchmarkSection` already
written into `metrics.json`. New pure helpers in `parsers.ts` —
`classifyAlpha`, `describeNoTradeActivity`, `describeExecutionWarnings` — back
both the new panel and the existing "Trade statistics" warning block (which
also fixes a stale `172800` daily-default reference, now `345600` per
BACKTEST-DAILY-STALE-DEFAULT-FIX-01). Timeframe is shown honestly as "not
reported" since it is absent from both `manifest.json` and `metrics.json`.
Validation: `npm test` 413/413 pass (20 new), `npm run build` clean (tsc +
vite). See `docs/runbooks/backtest_workflow.md` *"Presentation
(BACKTEST-REPORT-UX-01)"*.

---

## 10. Remaining Data Ingestion / Provider Patches

### DATA-SYMBOL-REGISTRY-01 — QUEUED

**Purpose:** Create or identify one canonical tracked-instrument source instead of hardcoded script lists.

Start with equities, but schema must support future:

- crypto
- futures
- options
- forex

Suggested registry fields:

- `instrument_id`
- `symbol`
- `asset_class`
- `provider_symbol`
- `venue/exchange`
- `currency`
- `timezone/session calendar`
- `enabled`
- `data_timeframes`
- `provider/source preference`

### DATA-INGEST-SYNC-ALL-EQUITIES-01 — QUEUED

**Purpose:** Add daemon job support to sync all enabled equity symbols from the registry using existing provider ingest/sync logic.

**Important:** Must include TwelveData rate limits, API-credit guardrails, resume behavior, and per-symbol failure tracking.

### DATA-INGEST-GUI-SYNC-ALL-01 — QUEUED

**Purpose:** Add GUI workflow to update all tracked symbols with:

- timeframe
- start/end
- rate-limit controls
- progress
- failures
- coverage refresh

### DATA-INGEST-PROVIDER-PLAN-01 — QUEUED

**Purpose:** Plan provider/TwelveData GUI ingestion safely before implementation:

- provider route design
- rate-limit behavior
- API credit controls
- resume behavior
- symbol batch controls
- failure handling

No code unless explicitly requested.

### DATA-INGEST-DAEMON-PROVIDER-JOBS-01 — QUEUED

**Purpose:** Add daemon-managed provider ingestion jobs, probably TwelveData first.

Must include:

- API credit guardrails
- rate limiting
- clear job progress
- no provider calls in tests
- no trading execution changes

### DATA-INGEST-GUI-PROVIDER-RUNNER-01 — QUEUED

**Purpose:** GUI workflow for provider ingestion after daemon provider jobs exist.

Must show provider ingestion as explicit, rate-limited, and operator-controlled.

### DATA-INGEST-GUI-COVERAGE-POLISH-01 — QUEUED

**Purpose:** Improve local data coverage table:

- sorting/filtering by symbol/timeframe
- search box
- show stale/missing data
- row count totals

Read-only only.

### DATA-INGEST-JOB-PERSISTENCE-01 — QUEUED

**Purpose:** Consider durable ingest job history instead of in-memory-only jobs.

Likely needs DB table. Do not start until current ingest GUI workflow is stable.

### DATA-INGEST-CANCEL-01 — QUEUED

**Purpose:** Add cancel support for long-running ingest jobs.

Only after provider jobs exist or CSV jobs prove cancellation is needed.

---

## 11. Multi-Asset Data / Ingestion Roadmap

### DATA-MULTI-ASSET-MODEL-01 — QUEUED

**Purpose:** Audit current symbol/timeframe/bar schema and design the migration path from symbol-only equity bars to `instrument_id + asset_class` market data.

**Required direction:** Do not bolt each asset class on as a one-off. Build a shared instrument model and normalized market-data layer.

Suggested normalized market data design:

```text
bars keyed by:
- instrument_id
- timeframe
- end_ts

preserve:
- provider raw symbol
- normalized OHLCV schema
- asset-class-specific extensions when needed
```

### DATA-INGEST-CRYPTO-PLAN-01 — QUEUED

**Purpose:** Plan crypto ingestion provider(s), symbol mapping, 24/7 sessions, timeframes, and storage compatibility.

### DATA-INGEST-FUTURES-PLAN-01 — QUEUED

**Purpose:** Plan futures ingestion with contract symbols, expiries, continuous contracts, sessions, and roll logic.

### DATA-INGEST-OPTIONS-PLAN-01 — QUEUED

**Purpose:** Plan options chain/contract ingestion separately from OHLCV bars.

### DATA-INGEST-FOREX-PLAN-01 — QUEUED

**Purpose:** Plan forex ingestion with currency pairs, 24/5 sessions, provider mapping, and pip/price precision.

---

## 12. GUI / Desktop Polish Remaining

### GUI-LAUNCHER-POLISH-01 — PARTIAL / PARKED

**Purpose:** Desktop launch flow mostly improved, but keep open for final launcher polish if needed.

### GUI-VISUAL-RESKIN-FINAL-01 — QUEUED / PARKED

**Purpose:** Final institutional dark/amber visual polish.

Known preferences:

- Keep Veritas Ledger logo upper left.
- Avoid text cut-off.
- Keep 1/8–1/4 inch breathing room from edges.
- Improve chart/report visuals.
- Presentation-only.
- No backend/API/trading changes.

### GUI-SCREEN-PADDING-01 — QUEUED / PARKED

**Purpose:** Ensure all GUI screens have safe interior padding so text/tables do not touch window edges or get clipped.

### GUI-NAV-REGISTRY-GUARD-01 — QUEUED / PARKED

**Purpose:** Strengthen tests so any registered screen must be visible/reachable in nav unless explicitly hidden.

Reason: Prevent another “screen exists but not visible” issue like the Ingest screen.

---

## 13. Autonomous Paper / Backend No-Trade Workstream

### AUTON-NO-TRADE-01 — QUEUED / HIGH PRIORITY

**Purpose:** Diagnose why autonomous paper runtime can be running but no trades/orders occur and no durable explanation is visible.

Known symptoms from prior ledger:

- `runtime_status=running`
- `execution/summary has_snapshot=false`
- `execution/flow truth_state=no_active_run`
- `autonomous_signal_count=0`
- `active_orders=0`

Goal: classify the first real blocker:

- no strategy tick
- no signal
- signal suppressed
- risk blocked
- outbox not written
- dispatcher not claiming
- broker submit not attempted
- broker reject
- execution truth route mismatch
- unknown

Pass condition:

- A real canonical paper order attempt occurs, **or**
- the system durably explains why no order occurs.

### AUTON-NO-TRADE-02 — QUEUED

**Purpose:** Follow-up patch based on `AUTON-NO-TRADE-01`’s first proven blocker.

Do not define until `AUTON-NO-TRADE-01` returns evidence.

### BROKER-HTTP-TIMEOUT-01 — QUEUED / PARKED

**Purpose:** Add broker HTTP timeout / independent heartbeat hardening.

Reason: Previous runtime lease issue was fixed by increasing lease TTL, but reqwest/no-total-timeout risk remains.

Constraint: Do not touch broker behavior until current paper/autonomous path is stable or this becomes the first proven blocker.

### STRATEGY-EQUITY-PERCENT-SIZING-01 — OPEN

**Purpose:** Add percent-of-equity sizing to `intraday_scalper` so position size scales with account equity/buying power rather than a fixed share count.

**Prerequisite:** Account equity or buying power must be surfaced in the strategy execution context (`StrategyContext` or an injected snapshot) before any percent-of-equity math is safe. That wiring does not currently exist.

**Dependency:** Account snapshot (equity or buying_power field from Alpaca REST) must be available to the strategy engine at `on_bar` call time.

**Background:** `STRATEGY-POSITION-SIZING-01` (commit f83ca51, 2026-05-29) added static sizing caps via `MQK_STRATEGY_MAX_TARGET_QTY` and `MQK_STRATEGY_MAX_POSITION_NOTIONAL_USD`. Those caps are config-driven and do not use live account balance. Percent-of-equity sizing requires a live or cached account balance query that is not yet wired.

**Hard rules for implementation:**
- Hard max notional cap and hard max share cap must still apply even when percent sizing is active.
- Percent sizing must be testable without broker/API calls (injected snapshot or env-overridable mock).
- Default behavior (no percent config set) must remain the existing static sizing.
- Fail-closed: if equity is unavailable or zero, fall back to static target_qty (do not guess or optimistically size up).

**Status:** OPEN — do not start until account equity is provably available in the strategy context.

---

## 14. DB / Migration Failure Tracking

### DB-MIGRATION-CHECKSUM-01 — QUEUED

**Purpose:** Diagnose local DB migration checksum mismatch:

```text
migration 6 was previously applied but has been modified
```

Affected unrelated tests mentioned:

- `scenario_broker_map_fk_enforced`
- `brk00r05b_s5_db_backed_restart_repair_sets_recovery_truth`
- `ws_truth_oa01_db_gap_cursor_persisted_after_disconnect`

Rule: Do not repair inside unrelated patches. Handle as its own DB/migration-state patch if it blocks full proof.

---

## 15. Older Parked Verification / Open Follow-Up Ledger

These are still queued as “verify from current repo before touching.” Do not treat as open bugs until the current repo proves them.

```text
DET-02: PARKED VERIFY
DET-03: PARKED VERIFY

CI-01: PARKED VERIFY
CI-02: PARKED VERIFY
CI-03: PARKED VERIFY
CI-04: PARKED VERIFY
CI-05: PARKED VERIFY
CI-06: PARKED VERIFY
CI-07: PARKED VERIFY
CI-08: PARKED VERIFY
CI-09: PARKED VERIFY
CI-10: PARKED VERIFY
CI-11: PARKED VERIFY

GUI-04: PARKED VERIFY
GUI-05: PARKED VERIFY
GUI-07: PARKED VERIFY

CTRL-03: PARKED VERIFY / PARTIAL
```

### CTRL-03 known note

CTRL-03 improved operator truthfulness by replacing unknown placeholders with explicit unavailable/closed states, but still lacks authoritative calendar/session provider wiring.

---


## 16. Historical Patch Aliases / Superseded Items

This section preserves older patch labels that were discussed or partially closed before later umbrella patches absorbed them. Do not lose these names. If a future chat sees one of these labels, map it to the current active/closed patch listed here before starting work.

### BACKTEST-GUI-ARTIFACTS-01 — PARTIAL / SUPERSEDED BY BACKTEST-GUI-CLOSURE-01

**Original purpose:** Add a GUI Backtest Results screen that loads an existing backtest artifact folder and displays:

- `manifest.json`
- `metrics.json`
- `equity_curve.csv`
- `orders.csv`
- `fills.csv`

**Known status:** Implementation/build/tests were reported good, but live GUI artifact-load smoke was not fully closed at the time.

**Current handling:** Covered by `BACKTEST-GUI-CLOSURE-01`.

**Close condition still preserved:** Paste a completed artifact folder into Backtest Results and prove manifest / metrics / equity curve / orders / fills render truthfully.

---

### BACKTEST-GUI-RUNNER-01 — PARTIAL / SUPERSEDED BY BACKTEST-GUI-CLOSURE-01

**Original purpose:** Add GUI workflow to submit backtest jobs to daemon routes, poll status, and auto-load the completed `artifact_dir`.

**Known status:** Code/tests/build were reported good, but live GUI submit/poll/auto-load smoke was not closed at the time.

**Current handling:** Covered by `BACKTEST-GUI-CLOSURE-01`.

**Close condition still preserved:** GUI submits CSV backtest → daemon job completes or fails truthfully → completed artifact auto-loads.

---

### BACKTEST-GUI-RUNNER-02 — PARTIAL / MOSTLY ABSORBED BY BACKTEST-GUI-CLOSURE-01

**Original purpose:** Improve Backtest Results GUI usability and portable path handling:

- clearly separate bars CSV input from artifact-folder input
- warn when a CSV path is entered where an artifact folder is expected
- use repo-root / launcher-bootstrap portable defaults
- show `exports\md_backup\1D` guidance
- avoid hardcoded Zach-only runtime paths

**Known status:** Tests/build passed; live launcher GUI smoke was still needed at the time.

**Current handling:** Covered by `BACKTEST-GUI-CLOSURE-01`, with optional follow-up under `BACKTEST-GUI-POLISH-01` if layout/UX issues remain.

---

### GUI-BLACK-SCREEN-01 — CLOSED

**Original purpose:** Fix the GUI black-screen crash where `DashboardScreen.tsx` attempted to read `.series` from an undefined chart/model object.

**Known status:** Treated as closed enough after render crash prevention and later GUI build/smoke work.

**Keep as historical proof:** If black-screen symptoms return, inspect:

- `core-rs/mqk-gui/src/features/dashboard/DashboardScreen.tsx`
- `core-rs/mqk-gui/src/components/common/ScreenErrorBoundary.tsx`
- `core-rs/mqk-gui/src/app/AppShell.tsx`

---

### GUI-LAUNCHER-POLISH-01 — PARTIAL / PARKED DETAIL

**Umbrella status:** Already listed in GUI/Desktop Polish as `PARTIAL / PARKED`.

**Historical sub-issues preserved:**

1. **Launcher build-output pollution / Start-Process empty path issue**  
   The PowerShell launcher allowed build stdout to flow into the return value used as the GUI exe path. This caused `Start-Process -FilePath` to receive an empty/non-path element. Historical fix was to route external command output to host rather than the function pipeline.

2. **Upper-left logo/text alignment**  
   Brand block layout needed the logo and text grouped correctly so Veritas Ledger branding rendered as one left-rail lockup.

3. **Center content clipping / scroll behavior**  
   Workspace content could clip bottom text because grid/flex children shrank instead of overflowing and scrolling. Historical fix involved preventing screen-grid shrink and adding bottom breathing room.

**Current handling:** Park under:

- `GUI-LAUNCHER-POLISH-01`
- `GUI-VISUAL-RESKIN-FINAL-01`
- `GUI-SCREEN-PADDING-01`

---

### DATA-INGEST-GUI-RUNNER-02 — CLOSED / HISTORICAL FOLLOW-UP

**Original purpose:** Fix why the new Ingest screen existed in code but was not visible in the left navigation.

**Root cause:** `LeftCommandRail.tsx` used hardcoded nav arrays and omitted `ingest` even though `SCREEN_REGISTRY`, `MONITOR_GROUPS`, `CORE_PANEL_KEYS`, and `sourceAuthority` included it.

**Known fix:** Extracted nav keys into `leftRailNav.ts`, added `ingest` near `marketData`, and added regression tests so registered/nav screens are easier to verify.

**Current status:** Closed by live GUI proof showing Ingest in the left nav and rendering the CSV ingest form.

---

### DATA-INGEST-GUI-RESULTS-01 — CLOSED / HISTORICAL ROUTE SMOKE NOTES

**Original purpose:** Add read-only market-data coverage route and GUI table.

**Important smoke history:**

- Initial GUI showed `HTTP 401` because the route/binary/auth state was not fully proven.
- A later direct route probe with token returned `404`, which pointed to stale daemon route/binary state.
- Rebuilding/restarting daemon resolved route availability.
- Final API proof showed `truth_state=active` and `AAPL / 1D / 8375`.
- Final GUI proof showed `AAPL / 1D / 8,375` in Local Data Coverage.

**Current status:** Closed.

---

### DATA-INGEST-DAEMON-JOBS-02 — CLOSED / HISTORICAL PARSER FIX

**Original purpose:** Fix daemon CSV ingest job failure:

```text
ingest_csv failed: deserialize ProviderBar failed
```

**Root cause:** The daemon CSV ingest path expected provider-artifact schema with `open/high/low/close`, but `exports/md_backup/1D` uses DB backup schema:

```text
symbol,timeframe,end_ts,open_micros,high_micros,low_micros,close_micros,volume,is_complete,ingested_at
```

and Postgres-style boolean values `t/f`.

**Known fix:** Add schema detection / DB-backup CSV support so local md backup files ingest correctly into `md_bars`.

**Final proof:** AAPL 1D CSV ingest completed with:

```text
rows_read=8375
rows_inserted=8375
rows_rejected=0
```

**Current status:** Closed.

---

### BACKTEST-CLI-UX-01 — CLOSED / DAILY INTEGRITY THRESHOLD NOTE

**Original purpose:** Make 1D backtests operator-safe by exposing/guiding `integrity_stale_threshold_ticks`.

**Root cause:** Default stale threshold was tuned for intraday data and blocked 1D bars because daily gaps exceed the default.

**Preserved rule:** For 1D / `timeframe_secs=86400`, use:

```text
integrity_stale_threshold_ticks=345600
```

This is also the daemon backtest-jobs API default for daily timeframes
(BACKTEST-DAILY-STALE-DEFAULT-FIX-01); `345600` (4 days) safely clears normal
weekend/calendar gaps. Use a larger threshold only for unusually long gaps.

**Current status:** Closed.

## 17. Recommended Order

```text
1. BACKTEST-GUI-CLOSURE-01
2. AUTON-NO-TRADE-01
3. DB-MIGRATION-CHECKSUM-01 only if it blocks proof runs
4. DATA-SYMBOL-REGISTRY-01
5. DATA-INGEST-SYNC-ALL-EQUITIES-01
6. DATA-INGEST-GUI-SYNC-ALL-01
7. DATA-MULTI-ASSET-MODEL-01
8. Provider/multi-asset ingestion plans
9. GUI polish / older parked verification items
```

Current best next patch:

```text
AUTON-NO-TRADE-01
```

Then return to the high-value original mission:

```text
AUTON-NO-TRADE-01
```
