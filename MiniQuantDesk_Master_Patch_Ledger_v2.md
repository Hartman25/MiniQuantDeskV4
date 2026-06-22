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

### DOCS-README-CURRENT-STATUS-20260615-01 — CLOSED

README/README_TECHNICAL were updated for the 2026-06-15 no-trade smoke, GUI dev port `1420`, GUI/CLI backtest cash micros warning, and evidence workflow wording. Doc-only patch; no code, script, config, evidence, or generated artifact changes.

---

### RISK-FLATTEN-ON-HALT-01 — CLOSED

**Closure:** Closes the `RISK-FLATTEN-ON-HALT-DESIGN-01` gap: `mqk_risk::evaluate()`
already allows `RequestKind::Flatten` while `RiskState.halted == true` (sticky
halt), but `RuntimeRiskGate` always evaluated every order with
`request = RequestKind::NewOrder` and `is_risk_reducing = false` — so a
genuine risk-reducing close/flatten order submitted while the risk engine was
sticky-halted was wrongly denied along with everything else. A verified
risk-reducing close/flatten order can now pass the sticky risk halt;
non-reducing orders remain denied exactly as before.

- New `RiskRequestContext { is_risk_reducing: bool }`
  (`mqk-execution::gateway`) carries per-order risk context into the gate.
  `RiskGate` gained a default `evaluate_gate_for_request(ctx) -> RiskDecision`
  that delegates to `evaluate_gate()` — existing gates are unaffected unless
  they override it.
- `BrokerGateway::enforce_gates(ctx)` now takes a `RiskRequestContext` and
  calls `risk.evaluate_gate_for_request(ctx)`. New
  `BrokerGateway::submit_with_context(claim, req, ctx)` contains the real
  submit logic; `submit(claim, req)` is now a thin wrapper calling
  `submit_with_context(.., RiskRequestContext::default())` — this preserves
  all existing `submit(&claim, req)` call sites across `mqk-execution` and
  `mqk-testkit` tests unchanged. `cancel`/`replace` call `enforce_gates` with
  `RiskRequestContext::default()` (unchanged behavior).
- `RuntimeRiskGate::evaluate_gate_for_request` (`mqk-runtime::runtime_risk`,
  and the parallel paper-wiring copy in `mqk-runtime::wiring_paper`) clones
  the stored `RiskInput` and overrides `request`/`is_risk_reducing` based on
  `ctx.is_risk_reducing`: `true` => `RequestKind::Flatten` +
  `is_risk_reducing: true`; `false` => `RequestKind::NewOrder` +
  `is_risk_reducing: false`. State mutation and fail-closed
  (`equity_micros <= 0` / `FailClosed`) behavior are identical to
  `evaluate_gate`.
- `ExecutionOrchestrator::dispatch_submit_claimed_outbox_row`
  (`mqk-runtime::orchestrator::dispatch`) now computes `is_risk_reducing`
  immediately before `gateway.submit_with_context(..)` via a new pure helper
  `is_submit_risk_reducing(current_qty, side, quantity)`. `current_qty` is
  read solely from the live `self.portfolio.positions` (signed; `0` if no
  position). A request is risk-reducing only when it is an exact-or-smaller
  close against an existing opposite-direction position (`Buy` reducing iff
  `current_qty < 0 && quantity <= |current_qty|`; `Sell` reducing iff
  `current_qty > 0 && quantity <= current_qty`). Never derived from
  `order_json.signal_source` or any other caller-supplied flag.

**Behavior preserved:** No change to `mqk-risk` engine math, strategy
entry/exit logic, `pre_event_flatten`, the `flatten-paper-positions` route,
the `runs.status` halt guard, `sys_arm_state`, DB schema/migrations, or any
broker adapter. No automatic close/flatten orders are generated by this
patch — `FlattenAndHalt` still produces no synthetic order. Orchestrator
phase ordering, OMS state machine transitions, and outbox/inbox discipline
are unchanged.

- Files: `core-rs/crates/mqk-execution/src/gateway.rs`,
  `core-rs/crates/mqk-execution/src/lib.rs`,
  `core-rs/crates/mqk-runtime/src/runtime_risk.rs`,
  `core-rs/crates/mqk-runtime/src/wiring_paper.rs`,
  `core-rs/crates/mqk-runtime/src/orchestrator/dispatch.rs`,
  `MiniQuantDesk_Master_Patch_Ledger_v2.md`.

**Tests:** `mqk-execution::gateway::tests::default_evaluate_gate_for_request_ignores_context`
and `submit_is_equivalent_to_submit_with_default_context` (backward
compatibility for gates/call sites that do not use the new context). 3 new
`mqk-runtime::runtime_risk::tests`:
`evaluate_gate_for_request_allows_verified_flatten_when_halted` (a verified
risk-reducing request returns `Allow` once `RiskState.halted == true`),
`evaluate_gate_for_request_denies_non_reducing_order_when_halted` (a
non-reducing request remains `Deny(RiskEngineUnavailable)` while halted), and
`evaluate_gate_for_request_matches_evaluate_gate_when_not_halted` (context-based
evaluation is unchanged when not halted). 7 new
`mqk-runtime::orchestrator::dispatch::is_submit_risk_reducing_tests` cover
long/short/flat positions, exact/partial closes, oversized closes, and
non-positive quantities. `cargo fmt --check`, `cargo test -p mqk-execution`
(65 passed), `cargo test -p mqk-runtime` (89 passed, 4 ignored —
pre-existing `MQK_DATABASE_URL`-gated tests), and
`cargo clippy -p mqk-execution -p mqk-runtime --all-targets -- -D warnings`
(zero warnings) all pass. `git diff --check` clean.

---

### MD-STALENESS-PER-TICK-GATE-01 — CLOSED / DB-PROVEN

**Closure:** Closes the `RISK-RECONCILIATION-CONTRACT-AUDIT-01` gap where
`DATA-FRESHNESS-READINESS-GATE-01` only checked market-data freshness at
startup — once a run was active, the dispatch loop never re-checked bar age,
so a stalled intraday market-data refresh could leave the runtime dispatching
strategies against stale completed bars for the rest of the session. A
fail-closed per-dispatch-tick staleness gate (cap #9, design doc §6
"per_symbol_bar_staleness_guard") is now enforced inside
`AppState::dispatch_native_strategy_for_symbol_with_bar`, on every per-symbol
dispatch (single-symbol and multi-symbol paths alike).

- New cap #9 config seam in `mqk-daemon::state::signal_intake`:
  `per_symbol_bar_staleness_secs_from_env()` reads
  `MQK_PER_SYMBOL_BAR_STALENESS_SECS`; `AppState::per_symbol_bar_staleness_secs()`
  is **always** `Some` — falling back to the existing documented default
  `market_data_freshness::MD_FRESHNESS_STALE_SECS` (4 trading days) when unset.
  Unlike caps #2/#4/#6, this gate cannot be disabled.
  `set_per_symbol_bar_staleness_secs_for_test` added for test seams.
- `dispatch_native_strategy_for_symbol_with_bar` now computes `latest_end_ts`
  from the most recent loaded `md_bars` row, calls the existing pure
  `classify_bar_staleness` helper (PER-SYMBOL-BAR-WINDOW-01) against
  `Utc::now()` and the cap #9 threshold, and returns `None` (no
  target/intent/order) when the result is `Some(true)` (stale-or-missing) —
  for both the "stale latest bar" case and the "zero completed bars" case (a
  missing bar is always stale for any cap). Each refusal is logged via
  `tracing::warn!` with structured fields `no_order_reason = "bar_data_stale"`,
  `symbol`, `timeframe`, `latest_end_ts`, `age_secs`, `staleness_cap_secs`.
- The `Err(e)` (DB query failure) arm is unchanged — out of scope for this
  patch; it continues to fall back to the single-stub context as before.
- Fresh bars (age within the cap) continue to dispatch exactly as before —
  unchanged code path beyond the new gate check.

**Behavior preserved:** No change to strategy entry/exit logic, OMS/outbox/
inbox semantics, broker adapters, or any risk/reconcile/integrity/lease/arm/
session gate. No DB migration. The gate only adds a `None`-return fail-closed
refusal *before* strategy dispatch for a stale/missing symbol/tick — it does
not weaken any existing gate. `RISK-FLATTEN-ON-HALT-01` was **OPEN** at the
time of this patch (this patch did not add flatten-on-halt behavior) and has
since been closed separately — see `RISK-FLATTEN-ON-HALT-01 — CLOSED` above.
The broader `RISK-RECONCILIATION-CONTRACT-AUDIT-01` ledger is not fully closed
by this patch (only the per-tick market-data-staleness gap is closed).

- Files: `core-rs/crates/mqk-daemon/src/state.rs`,
  `core-rs/crates/mqk-daemon/src/state/signal_intake.rs`,
  `core-rs/crates/mqk-daemon/tests/scenario_md_staleness_per_tick_gate_01.rs`,
  `MiniQuantDesk_Master_Patch_Ledger_v2.md`.

**Tests:** `scenario_md_staleness_per_tick_gate_01.rs` (5 DB-backed tests,
S01-S05 — fresh bar dispatches, stale bar blocked, missing bar blocked,
multi-symbol stale-without-blocking-fresh, +/-5s cap boundary).

**DB proof (MD-STALENESS-PER-TICK-GATE-DB-PROOF-01, 2026-06-14):** the 5
tests previously skipped gracefully (no `MQK_DATABASE_URL`). Re-run against
`mqk_test_smoke`, an already-migrated (schema at 0040, current HEAD), isolated
database on the same Postgres instance as the paper DB container
(`mqk-paper-postgres`, port 5440) — a *separate* database from
`miniquantdesk_paper`; no paper trading data read or written:

```
MQK_DATABASE_URL="postgres://postgres:postgres@127.0.0.1:5440/mqk_test_smoke?sslmode=disable" `
  cargo test -p mqk-daemon --test scenario_md_staleness_per_tick_gate_01 -- --nocapture
```

Result: `test result: ok. 5 passed; 0 failed; 0 ignored; 0 measured; 0 filtered
out`. All 5 cases (S01-S05) actually executed against the DB (confirmed via
the `MDS0*` fixture rows written to `md_bars` in `mqk_test_smoke`), not
skipped. Re-run twice for idempotency; both runs 5/5.

Targeted regression (pure, no DB required):
`scenario_multi_symbol_dispatch_loop_01` (8/8), `scenario_per_symbol_bar_window_01`
(14/14), `scenario_multi_symbol_capital_caps_01` (19/19),
`scenario_reconcile_baseline_seed_01` (7/7) — all pass. `cargo build -p mqk-daemon`
clean.

---

### RISK-ENGINE-HALTED-VISIBILITY-01 — CLOSED

**Closure:** The risk engine's sticky `RiskState.halted` flag is now surfaced
as a read-only operator signal, distinct from the transient
`sys_risk_block_state.blocked` flag (which Phase 0 of the orchestrator tick
resets every tick). Prior to this patch an operator could observe
`kill_switch_active: false` on `/api/v1/risk/summary` while the risk engine
was permanently denying all orders via sticky `RiskState.halted == true`.

- `RiskGate` trait (`mqk-execution::gateway`) gained
  `RiskEngineHaltStatus` (`Known { halted: bool }` | `Unavailable`) and a
  default `sticky_halt_status() -> Unavailable` method. Existing
  implementors (test stubs, fail-closed gates) are unaffected by the default.
- `RuntimeRiskGate::sticky_halt_status()` (`mqk-runtime::runtime_risk`) reads
  `RiskState.halted` from the held `RuntimeRiskGateState::Ready` state
  without calling `evaluate()` — read-only, no mutation.
  `RuntimeRiskGateState::FailClosed` reports `Unavailable`.
- `BrokerGateway::risk_engine_sticky_halt()` read-only passthrough added.
- `ExecutionSnapshot` gained `risk_engine_sticky_halt: RiskEngineHaltStatus`,
  populated by `ExecutionOrchestrator::snapshot()` as an in-memory overlay
  from the live gate (not DB-derived).
- `GET /api/v1/risk/summary` (`RiskSummaryResponse`) gained
  `risk_engine_halted: Option<bool>` (from the snapshot overlay; `None` when
  no snapshot exists or the gate reports `Unavailable`) and
  `risk_engine_halt_reason_code: Option<String>` (always `None` —
  `RiskState` does not track a halt reason today).
- New pure helper `sticky_halt_fault_signal()`
  (`mqk-daemon::routes::helpers`) emits a `risk.engine.sticky_halted` /
  `critical` `FaultSignal` only for `Known { halted: true }`. Wired into
  `GET /api/v1/system/status`, `GET /api/v1/alerts/active`, and
  `GET /api/v1/alerts/triage`.
- 15 pre-existing `ExecutionSnapshot` struct-literal sites across
  `mqk-daemon` tests and `state/snapshot.rs` updated mechanically to set
  `risk_engine_sticky_halt: RiskEngineHaltStatus::Unavailable` (additive
  field, no behavior change).

**Behavior preserved:** No change to risk enforcement, gate evaluation,
dispatch, OMS/outbox/inbox semantics, or broker behavior.
`risk_engine_halted` is never derived from `sys_risk_block_state.blocked`.
This is read-only observability only — **not** `RISK-FLATTEN-ON-HALT-01`
(no flatten-on-halt behavior was added; no close orders generated).

- Files: `core-rs/crates/mqk-execution/src/gateway.rs`,
  `core-rs/crates/mqk-execution/src/lib.rs`,
  `core-rs/crates/mqk-runtime/src/runtime_risk.rs`,
  `core-rs/crates/mqk-runtime/src/observability.rs`,
  `core-rs/crates/mqk-runtime/src/orchestrator.rs`,
  `core-rs/crates/mqk-daemon/src/api_types.rs`,
  `core-rs/crates/mqk-daemon/src/routes/portfolio.rs`,
  `core-rs/crates/mqk-daemon/src/routes/helpers.rs`,
  `core-rs/crates/mqk-daemon/src/routes/system.rs`,
  `core-rs/crates/mqk-daemon/src/routes/alerts_events.rs`,
  `core-rs/crates/mqk-daemon/src/state/snapshot.rs`, plus 9
  `mqk-daemon/tests/scenario_*.rs` files (mechanical `ExecutionSnapshot`
  literal fixups + new test coverage).

**Tests:** 3 new tests in `mqk-runtime::runtime_risk`
(`sticky_halt_status_known_false_for_fresh_ready_state`,
`sticky_halt_status_known_true_after_daily_loss_breach_is_sticky`,
`sticky_halt_status_unavailable_for_fail_closed_gate`); 2 new tests in
`mqk-daemon::routes::helpers` (`rehv01_known_halted_true_emits_critical_sticky_halt_signal`,
`rehv01_known_halted_false_and_unavailable_emit_no_signal`); 1 new test in
`scenario_daemon_routes.rs`
(`api_risk_summary_exposes_risk_engine_sticky_halt_state`, proves
`risk_engine_halted` is null/true/false across no-snapshot/halted/clear/unavailable
states). `cargo test -p mqk-execution -p mqk-runtime` (all pass, including
the 3 new tests) and targeted `mqk-daemon` test files
(`scenario_daemon_routes`, `scenario_gui_daemon_contract_gate`,
`scenario_daemon_order_submit`, `scenario_daemon_runtime_lifecycle`,
`scenario_monotonic_reconcile_in_run_baseline_01`, `scenario_order_trace_a5b`,
`scenario_order_timeline_a5a`, `scenario_paper_flatten_psf01`,
`scenario_runtime_start_reconcile_baseline_01`, plus `mqk-daemon --lib`) all
pass.

---

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

### DATA-SYMBOL-REGISTRY-01 — CLOSED 0d74598

**Closed:** `config/instruments/equities.json` (88 enabled equities) + `core-rs/crates/mqk-md/src/instrument_registry.rs` — `TrackedInstrument` struct with `instrument_id`, `symbol`, `asset_class`, `provider_symbol`, `venue`, `currency`, `enabled`, `timeframes`, `notes`. Schema is multi-asset ready (notes reference crypto/futures/options/forex). Tests REG-01..REG-11 in `instrument_registry.rs` + RS-03..RS-08 in `mqk-cli/src/commands/md.rs` prove: 88 enabled equities resolve, sorted, unique IDs, no empty fields, validation passes/fails correctly.

All 10 required seed symbols present and enabled: SPY, QQQ, AAPL, MSFT, META, AMZN, GOOGL, NVDA, AMD, TSLA.

### DATA-INGEST-SYNC-ALL-EQUITIES-01 — CLOSED da92040+99201f4

**Closed:** CLI `mqk md sync-provider --symbols-from-registry <path>` resolves all 88 enabled equities from the canonical registry and syncs per-symbol incremental bars. Rate-limit controls via TwelveData's bounded retry (4 retries, 65s sleep) and fixed chunk windows (1D: 8yr, 5m: 63d, 1m: 14d) prevent API-credit overruns. Per-symbol `effective_start` detection provides resume behavior. Daemon `POST /api/v1/ingest/jobs` (source=twelvedata, mode=sync_provider) accepts dry-run jobs: resolves 88 symbols, reports count/first/last, zero API calls. Tests RS-01..RS-08 prove: symbol resolution from registry, sorted order, known anchors present, conflict/error cases.

**Note:** Daemon real-provider wiring (dry_run=false) is explicitly deferred to `DATA-INGEST-PROVIDER-REAL-SYNC-01` — the daemon gate returns `not_implemented` until wired.

### DATA-INGEST-GUI-SYNC-ALL-01 — CLOSED 9c50c3a+99201f4

**Closed:** "Tracked equities" panel in `core-rs/mqk-gui/src/features/ingest/IngestScreen.tsx` shows registry truth_state, count, first/last symbol, and registry path. Panel auto-loads from `GET /api/v1/ingest/tracked-equities` on mount. Displays clear operator notice that provider sync is not enabled (points to CLI and DATA-INGEST-DAEMON-PROVIDER-JOBS-01). GUI tests: `isTrackedEquitiesActive`, `trackedEquitiesTruthLabel`, `TrackedEquitiesResponse` active/unavailable shapes (including honest error on missing registry), no provider sync fields. GUI does not imply trading readiness; no orders triggered.

GUI total test suite: 428 tests pass.

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

**Reconciliation note:** the five labels below are historical planning entries, preserved for continuity. Active multi-asset tracking now lives in §19 Multi-Asset Expansion Roadmap (full detail: `docs/audits/multi_asset_completion_audit.md`). Each label is mapped below to its corresponding §19 patch IDs. Do not start any label below directly — work against its mapped §19 patch ID instead. See `LEDGER-MULTI-ASSET-RECONCILE-01` in §19 for the closure note.

### DATA-MULTI-ASSET-MODEL-01 — RECONCILED / SUPERSEDED BY ASSET-CORE ROADMAP

**Maps to (§19):** `ASSET-CORE-01`, `ASSET-CORE-02`, `ASSET-CORE-03`, `ASSET-CORE-04`, `ASSET-CORE-05`, `BACKTEST-MULTIPLIER-MARGIN-01`. Do not start this older label directly unless intentionally reopening the roadmap design.

**Original purpose (preserved for history):** Audit current symbol/timeframe/bar schema and design the migration path from symbol-only equity bars to `instrument_id + asset_class` market data.

**Original required direction:** Do not bolt each asset class on as a one-off. Build a shared instrument model and normalized market-data layer.

Suggested normalized market data design (historical):

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

### DATA-INGEST-CRYPTO-PLAN-01 — RECONCILED / SUPERSEDED BY CRYPTO ROADMAP

**Maps to (§19, Phase 3 — Crypto Engine):** `CRYPTO-REGISTRY-01`, `CRYPTO-DATA-01`, `CRYPTO-RISK-01`, `CRYPTO-EXEC-01`, `CRYPTO-STRAT-01`. Do not start this older label directly unless intentionally reopening the roadmap design.

**Original purpose (preserved for history):** Plan crypto ingestion provider(s), symbol mapping, 24/7 sessions, timeframes, and storage compatibility.

### DATA-INGEST-FUTURES-PLAN-01 — RECONCILED / SUPERSEDED BY FUTURES ROADMAP

**Maps to (§19, Phase 1 — Futures Trading Engine):** `FUTURES-REGISTRY-01`, `FUTURES-DATA-01`, `FUTURES-DATA-02`, `FUTURES-RISK-01`, `FUTURES-EXEC-01`, `FUTURES-STRAT-01`, `FUTURES-STRAT-02`. Do not start this older label directly unless intentionally reopening the roadmap design.

**Original purpose (preserved for history):** Plan futures ingestion with contract symbols, expiries, continuous contracts, sessions, and roll logic.

### DATA-INGEST-OPTIONS-PLAN-01 — RECONCILED / SUPERSEDED BY OPTIONS ROADMAP

**Maps to (§19, Phase 2 — Options Trading Engine):** `OPTIONS-CONTRACT-01`, `OPTIONS-CHAIN-01`, `OPTIONS-RISK-01`, `OPTIONS-BACKTEST-01`, `OPTIONS-WHEEL-01`, `OPTIONS-WHEEL-02`, `OPTIONS-SPREADS-01`. Do not start this older label directly unless intentionally reopening the roadmap design.

**Original purpose (preserved for history):** Plan options chain/contract ingestion separately from OHLCV bars.

### DATA-INGEST-FOREX-PLAN-01 — RECONCILED / SUPERSEDED BY FOREX ROADMAP

**Maps to (§19, Phase 4 — Forex Engine):** `FX-FUTURES-01`, `FX-DATA-01`, `FX-RISK-01`, `FX-STRAT-01`. Do not start this older label directly unless intentionally reopening the roadmap design.

**Original purpose (preserved for history):** Plan forex ingestion with currency pairs, 24/5 sessions, provider mapping, and pip/price precision.

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

## 14. Alpha Scanner / Intraday Data / Strategy Roadmap

### INTRADAY-MD-FRESHNESS-AUTONOMOUS-01 — CLOSED

**Purpose:** Ensure autonomous paper strategies using intraday timeframes, especially 5m, do not dispatch from stale prior-session bars.

**Background:** The 2026-06-15 autonomous paper session ran safely, but AAPL/5m latest completed bar appeared to be from 2026-06-12 while the 4-day freshness threshold still marked it acceptable. That is safe from a system-fail-closed perspective, but not sufficient for a real intraday signal/trade proof.

**Requirements:**

- During active market session, intraday strategies must require current-session bars or a much tighter max-age threshold.
- For 5m bars, stale should mean no current/recent completed bar available.
- Must surface durable no-trade reason, such as `intraday_bar_not_current` or `market_data_not_refreshed`.
- Must not silently trade from old prior-session bars.
- Must not call providers in tests.
- Must not change strategy logic merely to force trades.
- Must not bypass risk/reconcile/session/arm/live-routing gates.

**Closure:** CLOSED by focused validation for
`INTRADAY-MD-FRESHNESS-AUTONOMOUS-01`; commit no longer pending. Package-scoped
clippy remains blocked by unrelated existing drift in
`core-rs/crates/mqk-daemon/src/routes/control_plane.rs:1389`
(`clippy::unnecessary_map_or`).

- Root cause: the per-dispatch staleness gate defaulted to the broad
  `MD_FRESHNESS_STALE_SECS` 345600-second / 4-day threshold for every
  timeframe, so Friday 5m bars could still be accepted on Monday.
- Intraday timeframes now use `MQK_INTRADAY_BAR_MAX_AGE_SECS`, defaulting to
  900 seconds. Daily/1D retains the 345600-second tolerance.
- Autonomous dispatch now resolves the cap by timeframe before native strategy
  invocation, so stale/missing intraday bars return no `StrategyBarResult`,
  target, intent, outbox row, or broker contact.
- Readiness/preflight market-data freshness now surfaces `reason_code`,
  `latest_completed_bar_ts`, `now_utc`, `age_seconds`, and
  `max_allowed_age_seconds`. Intraday stale/missing reason codes are
  `intraday_bar_stale` and `intraday_bar_not_current`.
- Focused proof: `scenario_intraday_md_freshness_autonomous_01`.

**Status:** CLOSED

### INTRADAY-MD-REFRESHER-01 — CLOSED

**Purpose:** Add or prove a safe intraday market-data refresh path so AAPL/5m and future watchlist symbols receive latest completed bars during the market session.

**Requirements:**

- Prefer completed bar polling/refresh first before true streaming complexity.
- Refresh cadence should align with timeframe boundaries or safe periodic polling.
- Must write to canonical `md_bars` or existing approved market-data path.
- Must expose last refresh time, latest completed bar time, provider/source, rows inserted, and failure reason.
- Must be rate-limit aware.
- Must be safe when provider credentials are missing.
- Must not spend API credits in tests.
- Must not submit orders.
- Must not modify broker/risk/order behavior.
- Must keep live routing false in any paper smoke.

**Closure:**

- Selected Option 2 (CLI/script-backed refresher): `Refresh-IntradayMarketData.ps1`
  drives `mqk-cli md sync-provider`, which writes through the canonical
  `ingest_provider_bars_to_md_bars` upsert path into `md_bars`.
- `mqk-cli` now filters provider rows before ingest so incomplete rows and
  in-progress intraday rows newer than `now_ts - timeframe_secs` are not written
  as completed strategy input.
- The Windows refresher keeps the default 300s cadence, suppresses loop
  intervals below the configured minimum, and writes provider/source, attempt,
  completion-filter, inserted/updated-row, latest-bar, and failure diagnostics
  to its intraday refresh evidence JSON.
- Readiness/preflight freshness gates remain unchanged; strategy dispatch still
  consumes only completed rows from `md_bars`.
- Focused proof: `scenario_intraday_md_refresher_01`.

**Status:** CLOSED

### INTRADAY-MD-REFRESHER-OPERATOR-SURFACE-01 — CLOSED

**Purpose:** Expose latest intraday refresh evidence through a read-only daemon/operator API surface.

**Requirements:**

- `GET /api/v1/market-data/intraday-refresh/status` returns structured evidence status.
- Missing evidence → `truth_state: "no_evidence"`, `stale_or_missing_evidence: true`.
- Malformed evidence → `truth_state: "parse_error"`, no crash.
- Valid evidence → `truth_state: "active"`, surfaces provider/symbol/timeframe/bar ts/row counts/fail_reasons.
- Evidence older than 24 h → `stale_or_missing_evidence: true`.
- No provider calls, no DB mutation, no broker calls.

**Closure:**

- `GET /api/v1/market-data/intraday-refresh/status` registered in public router.
- Handler in `routes/transport_quality.rs`: reads latest `intraday_refresh_*.json` from
  `md_refresh_evidence_dir` (env `MQK_MD_REFRESH_EVIDENCE_DIR`, default `exports/market_data`).
- `IntradayRefreshStatusResponse` + `IntradayRefreshSymbolStatus` types in `api_types.rs`.
- `md_refresh_evidence_dir` field added to `AppState`.
- 9 scenario tests (IRS-01..IRS-09) in `scenario_intraday_md_refresher_operator_surface_01.rs`; all pass.

**Status:** CLOSED

### INTRADAY-MD-REFRESHER-GUI-01 — CLOSED

**Purpose:** Add read-only GUI display for intraday refresh status exposed by INTRADAY-MD-REFRESHER-OPERATOR-SURFACE-01.

**Requirements:**

- Frontend API client/types for `GET /api/v1/market-data/intraday-refresh/status`.
- Display on existing Market Data screen (or most appropriate existing surface).
- Show: not configured/no evidence, parse error, last success, last failure, stale evidence,
  latest completed bar ts, provider/source, rows inserted/updated, filtered counts.
- Do not hide fail-closed states.
- No buttons that call providers.
- Do not change backend logic or trading behavior.

**Status:** CLOSED

**Evidence:**
- `core-rs/mqk-gui/src/features/ingest/types.ts`: `IntradayRefreshSymbolStatus` + `IntradayRefreshStatusResponse` interfaces added.
- `core-rs/mqk-gui/src/features/ingest/api.ts`: `isIntradayRefreshActive`, `intradayRefreshTruthLabel`, `fetchIntradayRefreshStatus` added.
- `core-rs/mqk-gui/src/features/ingest/IngestScreen.tsx`: "Intraday refresh status" panel appended; auto-loads on mount; shows truth_state, stale warning, produced_at, mode/source/timeframe, all_passed, per-symbol table.
- `core-rs/mqk-gui/src/features/ingest/__tests__/api.test.ts`: 15 new tests (IRA-01..IRA-15); 428/428 total pass.
- Build: `npm run build` passes (TypeScript clean; pre-existing chunking warnings only).

### DATA-STREAMING-BARS-01 — QUEUED / PARKED

**Purpose:** Evaluate true streaming or websocket bar ingestion for one or more symbols after polling/refresh is proven.

**Requirements:**

- Must define source of truth: streaming bars vs provider historical completed bars vs DB bars.
- Must handle reconnects, gaps, duplicate bars, partial/incomplete bars, and clock/session boundaries.
- Must prove that strategy dispatch only uses complete, current bars.
- Must include replay/recovery behavior after laptop sleep/network gap.
- Do not start until INTRADAY-MD-REFRESHER-01 is proven or explicitly superseded.

**Status:** QUEUED / PARKED

### STRATEGY-LAB-01 — OPEN / ROADMAP

**Goal:** Create a standardized strategy evaluation framework before adding many live strategies.

**Requirements:**

- Same interface for every strategy.
- Backtest support.
- Paper support.
- Common performance metrics:
  - win rate
  - profit factor
  - Sharpe
  - max drawdown
  - expectancy
  - trade frequency
  - exposure
- Promotion evidence must separate research/backtest results from paper/live readiness.
- No strategy should be promoted only because it trades frequently.
- Must support comparing strategies across multiple symbols and regimes.

**Status:** OPEN / ROADMAP

### MULTI-SYMBOL-SCANNER-01 — OPEN / ROADMAP

**Goal:** Monitor and score 10–50 symbols simultaneously after current data-refresh and paper lifecycle proofs are stable.

**Initial target symbols:**

- SPY
- QQQ
- AAPL
- MSFT
- META
- AMZN
- GOOGL
- NVDA
- AMD
- TSLA

**Requirements:**

- Relative volume ranking.
- ATR ranking.
- Momentum ranking.
- Liquidity filtering.
- Spread/price sanity filters if data is available.
- Dynamic watchlist generation.
- Must not trade every symbol blindly.
- Must feed the strategy router/admission framework with ranked opportunities.
- Must be proven in backtest/replay before autonomous paper promotion.

**Status:** OPEN / ROADMAP

### REGIME-DETECTION-01 — OPEN / ROADMAP

**Goal:** Classify current market state so the bot can choose strategies appropriate to conditions.

**Initial regimes:**

- Trending
- Range-bound
- Volatile
- Quiet

**Example output:**

```json
{
  "regime": "TRENDING",
  "confidence": 0.82
}
```

**Requirements:**

- Must be deterministic and explainable.
- Must work at market/index level and potentially symbol level.
- Must support backtest/replay.
- Must not be used to bypass risk or admission checks.
- Must expose confidence and reason codes.

**Status:** OPEN / ROADMAP

### STRATEGY-ROUTER-01 — OPEN / ROADMAP

**Goal:** Match strategy to symbol and regime instead of running one strategy blindly everywhere.

**Examples:**

- NVDA + Trending -> Opening Range Breakout
- SPY + Range-bound -> Mean Reversion
- TSLA + High Volatility -> Momentum

**Requirements:**

- Consume scanner scores and regime detection.
- Select strategy candidates per symbol.
- Respect risk engine, capital caps, session rules, and admission-check framework.
- Must be backtestable across many symbols.
- Must expose why a strategy/symbol pair was selected or rejected.
- Must avoid overtrading low-edge setups.

**Status:** OPEN / ROADMAP

### STRATEGY-OPENING-RANGE-BREAKOUT-01 — QUEUED / RESEARCH

**Goal:** Research and backtest Opening Range Breakout strategy.

**Candidate symbols:**

- AAPL
- NVDA
- AMD
- META
- TSLA
- SPY
- QQQ

**Concept:**

- First 15 minutes establish range.
- Break high.
- Volume confirmation.
- ATR stop.
- Risk/reward target.

**Requirements:**

- Backtest first.
- No autonomous paper promotion until performance and risk metrics are proven.
- Must handle failed breakouts and no-trade days.

**Status:** QUEUED / RESEARCH

### STRATEGY-VWAP-PULLBACK-01 — QUEUED / RESEARCH

**Goal:** Research and backtest VWAP Pullback strategy.

**Candidate symbols:**

- SPY
- QQQ
- AAPL
- MSFT

**Concept:**

- Strong trend.
- Pullback to VWAP.
- Momentum resumes.

**Requirements:**

- Needs VWAP calculation or source.
- Backtest first.
- Must define trend, pullback, confirmation, stop, and invalidation rules.
- No autonomous paper promotion until metrics are proven.

**Status:** QUEUED / RESEARCH

### STRATEGY-RELATIVE-VOLUME-MOMENTUM-01 — QUEUED / RESEARCH

**Goal:** Research relative-volume momentum as both a strategy and scanner input.

**Concept:**

- Relative volume > 2x.
- Price above VWAP.
- Breaking intraday highs.

**Requirements:**

- Requires reliable intraday volume and baseline volume calculation.
- Should feed scanner ranking.
- Backtest first.
- No autonomous paper promotion until metrics are proven.

**Status:** QUEUED / RESEARCH

### STRATEGY-GAP-AND-GO-01 — QUEUED / RESEARCH

**Goal:** Research Gap & Go strategy.

**Concept:**

- Gap > 3%.
- High premarket volume.
- Break premarket high.
- Often strongest in small caps, but small-cap risk controls must be stricter.

**Requirements:**

- Needs premarket data support before realistic testing.
- Must include liquidity/spread filters.
- Must be treated as experimental/high-risk until proven.
- No autonomous paper promotion until metrics are proven.

**Status:** QUEUED / RESEARCH

### STRATEGY-TREND-FOLLOWING-01 — QUEUED / RESEARCH

**Goal:** Research longer-horizon trend following strategy.

**Concept:**

- 20 EMA > 50 EMA.
- ADX strong.
- Volume confirmation.
- Useful for swing testing later.

**Requirements:**

- Backtest first.
- Must separate swing/longer-horizon behavior from intraday paper smoke behavior.
- No autonomous paper promotion until metrics are proven.

**Status:** QUEUED / RESEARCH

### Recommended Alpha Scanner Order

1. Finish and prove intraday market-data freshness/refresh.
2. Complete real paper order/fill lifecycle proof.
3. Build STRATEGY-LAB-01 evaluation framework.
4. Build MULTI-SYMBOL-SCANNER-01.
5. Backtest 2–3 high-quality strategies across 10–20 symbols.
6. Add REGIME-DETECTION-01.
7. Add STRATEGY-ROUTER-01.
8. Promote only top performers into autonomous paper.

**Warning:** Monitoring many symbols with no proven edge only loses money faster. The repo should eventually become many strategies + many symbols + scoring + regime-aware routing, but strategy count should not be expanded before evaluation, data quality, and paper lifecycle proof are stable.

---

## 15. DB / Migration Failure Tracking

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

## 16. Older Parked Verification / Open Follow-Up Ledger

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


## 17. Historical Patch Aliases / Superseded Items

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

## 18. Recommended Order

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

**Note:** §18 is the older, general repo-wide patch order and predates the multi-asset audit. Items 7–8 above (`DATA-MULTI-ASSET-MODEL-01`, "Provider/multi-asset ingestion plans") are superseded — see §11 for the reconciled label mapping and §19 Multi-Asset Expansion Roadmap (current next: `ASSET-CORE-01`) for active multi-asset tracking. §19 governs multi-asset sequencing where the two disagree.

Current best next patch:

```text
AUTON-NO-TRADE-01
```

Then return to the high-value original mission:

```text
AUTON-NO-TRADE-01
```

---

## 19. Multi-Asset Expansion Roadmap

**Source:** `MULTI-ASSET-COMPLETION-AUDIT-01` (docs-only audit; no code/trading-path changes).
**Full detail:** [`docs/audits/multi_asset_completion_audit.md`](docs/audits/multi_asset_completion_audit.md)

Repo trades equities only today, but already has a tested fail-closed multi-asset admission boundary (`AssetClass` enum, Gate 0 signal-admission reject, `MULTI-ASSET-ROUTING-GUARD-01` broker-submit reject, `ASSET-CAPABILITY-MATRIX-01` on `/api/v1/system/metadata` — all pre-existing, all `enabled:false` for non-equity). Nothing exists past that boundary for any non-equity class. See the audit doc for full evidence; this section is index-only and will drift if treated as the source of truth.

**Note:** ledger §11 (`DATA-MULTI-ASSET-MODEL-01` and the four `DATA-INGEST-*-PLAN-01` items) has been reconciled against this roadmap's Phase 0–4 patches under one tracking scheme via `LEDGER-MULTI-ASSET-RECONCILE-01` (closure note at the end of this section; see also audit doc §5/§13). Each old label is mapped to its corresponding §19 patch IDs directly in §11 — none were deleted.

Phase list: Phase 0 Core Foundation (`ASSET-CORE-01..05`) → Phase 5 ETF/Sector (cheapest real wins) → Phase 3 Crypto (cheapest new asset class) → Phase 7 Broker Expansion (IBKR) → Phase 1 Futures → Phase 2 Options → Phase 4 Forex / Phase 6 Rates (lowest near-term priority).

Top 20 (build order; full rationale in audit doc §8): `LEDGER-MULTI-ASSET-RECONCILE-01`, `ETF-RISK-01`, `ETF-REGISTRY-01`, `ASSET-CORE-01`, `ASSET-CORE-05`, `BACKTEST-MULTIPLIER-MARGIN-01`, `ASSET-CORE-02`, `ASSET-CORE-03`, `MULTI-ASSET-ALLOCATOR-01`, `ETF-RANKER-01`, `ETF-STRAT-01`, `MULTI-STRATEGY-CONFLICT-POLICY-01`, `PROVIDER-SWAP-CONTRACT-01`, `ASSET-CORE-04`, `CRYPTO-REGISTRY-01`, `CRYPTO-DATA-01`, `CRYPTO-RISK-01`, `CRYPTO-EXEC-01`, `CRYPTO-STRAT-01`, `BROKER-IBKR-01`.

**Status update:** positions 1–3 above (`LEDGER-MULTI-ASSET-RECONCILE-01`, `ETF-RISK-01`, `ETF-REGISTRY-01`) are now `CLOSED`. The current next foundation patch is `ASSET-CORE-01` (position 4). This list is preserved verbatim as the original audit's build-order rationale (full detail: audit doc §8) — it is not rewritten as patches close.

### ETF-FOUNDATION-01 — ETF-REGISTRY-01 CLOSED / ETF-RISK-01 PARTIAL

**`ETF-REGISTRY-01` CLOSED.** The 14 target ETFs (SPY, QQQ, IWM, DIA, XLK, XLF, XLE, XLI, XLP, XLU, TLT, IEF, SHY, GLD) are tagged in `config/instruments/equities.json` with new optional fields on `TrackedInstrument` (`core-rs/crates/mqk-md/src/instrument_registry.rs`): `instrument_kind: "etf"`, `sector` (e.g. `broad_market`, `sector_technology`, `rates_duration`), `category` (e.g. `index_equity`, `sector_equity`, `fixed_income`, `commodity`). `asset_class` stays `"equity"` for all of them — zero change to ingestion/backtest/GUI behavior, zero change to the other 74 entries. New `sector_map()` pure helper bridges registry metadata to the `HashMap<String,String>` shape `mqk_portfolio::constraints::check_sector_limits` expects. 10 new tests (REG-12..REG-19) + 9 pre-existing registry tests all pass; `cargo check`/`clippy -D warnings` clean on `mqk-md`/`mqk-cli`/`mqk-daemon`.

**`ETF-RISK-01` PARTIAL — blocked on missing live portfolio-weight plumbing, not just missing sector metadata.** Direct inspection (not the original audit's "zero callers" framing) found the real gap: no live mark-price/notional/weight computation reaches any admission boundary today, for any symbol. `mqk_risk::RiskInput`/`RiskState` carry no symbol field; `RiskRequestContext` (the struct reaching the live pre-broker-submit `RiskGate`) carries only `is_risk_reducing: bool`; the live tick-loop `PositionSnapshot`/`PortfolioSnapshot` (`mqk-runtime::observability`) carry qty + cash only, no mark price or NAV. The existing analogous per-order caps (`mqk-daemon/src/capital_policy/{position_sizing,portfolio_risk}.rs`) are single-order notional checks that explicitly punt on this same gap (`RiskUnverifiable`: "portfolio drift is not measurable at signal time without runtime portfolio state"). `cargo test -p mqk-risk sector` / `-p mqk-runtime sector` both match zero tests against HEAD, confirming no sector-aware code exists outside the dead `mqk-portfolio::constraints` module. Wiring `check_sector_limits` for real would require fabricating a price/NAV source — forbidden by `CLAUDE.md` operator-truth discipline. `mqk-portfolio`, `mqk-risk`, `mqk-runtime`, and `mqk-daemon` production code were therefore **not touched**; the dead `SectorConstraint`/`check_sector_limits` code is unchanged and still has its original 22 passing tests. Full detail: `docs/audits/multi_asset_completion_audit.md` §16.

**Next dependency patch:** `PORTFOLIO-LIVE-WEIGHTS-01` (live mark-price + NAV/weight computation reaching the decision boundary, equity-wide — not ETF-specific). Only after that exists can `ETF-RISK-01` close for real.

Recommended next patch (historical — at the time this section was written; superseded, see `LEDGER-MULTI-ASSET-RECONCILE-01` closure note at the end of §19 for the current recommendation):

```text
PORTFOLIO-LIVE-WEIGHTS-01
```

(wires already-written, zero-caller `SectorConstraint`/`check_sector_limits()` in `mqk-portfolio/src/constraints.rs` into the live risk engine — smallest diff, real risk value, no dependencies.)

### PORTFOLIO-LIVE-WEIGHTS-01 — CLOSED_LOCAL

**Purpose:** Build the missing live mark-price / NAV / portfolio-weight truth seam that `ETF-FOUNDATION-01` identified as the real blocker on `ETF-RISK-01` — without enforcing any risk limit, touching broker/order/outbox code, or fabricating a price.

**What was built:**

- `mqk_portfolio::compute_portfolio_weights` (`core-rs/crates/mqk-portfolio/src/valuation.rs`) — pure, deterministic, zero-dependency function. Inputs: `cash_micros`, `&[PositionWeightInput { symbol, signed_qty }]`, `&BTreeMap<String, PositionMark>` (explicit, attributed marks). Output: `PortfolioWeightsSnapshot` with per-symbol `market_value_micros`/`absolute_notional_micros`/`weight_bps` (all `i128`-safe) and one of three truth states — `"active"`, `"missing_marks"`, `"nav_unavailable"` — never a fabricated NAV or weight. A flat (`signed_qty == 0`) position never requires a mark. 11 scenario tests (`mqk-portfolio/tests/scenario_portfolio_live_weights_01.rs`, PW-01..PW-09) cover long/short/multi-position weights, missing marks, NAV `<= 0` (both negative and exact-zero), `i64::MAX`-magnitude overflow safety, zero-quantity predictability, and determinism.
- `GET /api/v1/portfolio/live-weights` (`core-rs/crates/mqk-daemon/src/routes/portfolio.rs`, registered in `routes.rs`) — read-only. Positions/cash come from the in-memory execution snapshot (`AppState.execution_snapshot`, runtime-ledger-derived — distinct from the broker-account-derived snapshot `portfolio_summary` uses); marks come only from the latest *completed* `md_bars` row per non-flat symbol at a `timeframe` query param (default `"1D"`), formatted as `source = "md_bars:{timeframe}:close"`. Adds a fourth, daemon-level truth state, `"db_unavailable"` (no DB pool configured at all), distinct from the pure helper's `"missing_marks"` (DB present, symbol has no completed bar) — both collapse to "no mark" but are different operator truths. 8 scenario tests (`mqk-daemon/tests/scenario_portfolio_live_weights_01.rs`, PLW-01..PLW-08), 2 of them DB-backed (seeded/unseeded `md_bars` rows against the local paper DB on port 5440) and proven for real, including an explicit assertion that the route never writes to `oms_outbox`.

**Mark source:** `md_bars` latest completed row only (`is_complete = true`, highest `end_ts`), via the existing `fetch_recent_completed_bars_for_strategy` read path. No provider call, no broker call, no live quote, no entry/order price.

**Deliberately not done (next dependency patch is still `ETF-RISK-01`):** `check_sector_limits` is not called from this patch; `mqk-risk`, `RiskRequestContext`, and the live admission/decision path are untouched. The seam now exists and is provably truthful, but nothing in the live risk/admission boundary consumes it yet.

**Validation:** `cargo check -p mqk-portfolio` / `-p mqk-runtime` (untouched) / `-p mqk-daemon` all clean. `cargo test -p mqk-portfolio --test scenario_portfolio_live_weights_01` — 11/11 pass. `cargo test -p mqk-daemon --test scenario_portfolio_live_weights_01` — 8/8 pass against the real local paper DB (port 5440), confirmed via `--nocapture` timing and a post-run `md_bars` query showing zero leftover test rows. Pre-existing, unrelated `cargo clippy -p mqk-daemon` failure at `mqk-db/src/md.rs:629` (a `clippy::clone_on_copy` lint on a file untouched by this patch — toolchain/clippy-version drift) was identified, not reproduced as caused by this patch, and left alone per the test-failure policy.

**Safety confirmation:** no broker/Alpaca submit code touched; no live routing changes; no order/outbox writes (asserted by test); no DB migrations; `.env.local` untouched; no provider/broker calls; no orders submitted; no short-entry changes; no ETF sector risk enforcement; B5/risk gates unchanged.

### ETF-RISK-CLOSURE-01 — CLOSED_LOCAL

**Purpose:** Close `ETF-RISK-01` for real, now that `PORTFOLIO-LIVE-WEIGHTS-01` provides the live mark/NAV/weight truth seam `ETF-FOUNDATION-01` identified as the blocking dependency. Sector exposure limits are default-off and use only real live weights/marks — never a fabricated price, NAV, or sector.

**What was built:**

- `mqk_portfolio::evaluate_sector_risk` (`core-rs/crates/mqk-portfolio/src/constraints.rs`) — a pure, `i64`-basis-point evaluator. Distinct from the pre-existing, unrelated `SectorConstraint`/`check_sector_limits` (`f64`-fractional weights, the research allocator's post-allocation check) — that code is untouched, still has its original tests. Recomputes `compute_portfolio_weights` before and after a candidate order using the *same* marks, sums `|weight_bps|` per sector, and returns one of seven truth states: `sector_risk_disabled`, `sector_metadata_missing`, `sector_limit_ok`, `sector_weights_missing`, `sector_nav_unavailable`, `sector_risk_reducing_allowed`, `sector_limit_exceeded`. An empty limits map short-circuits before touching marks, `sector_map`, or positions at all. 12 scenario tests (`mqk-portfolio/tests/scenario_etf_risk_closure_01.rs`, SR-01..SR-12) cover disabled/enabled, within/over cap, risk-reducing override, missing marks, NAV unavailable, unclassified symbol, no-cap-for-this-sector, opening a new position, multi-sector isolation, the exact-cap boundary, and `i128`-safe math under `i64::MIN`/`i64::MAX`-adjacent magnitudes.
- `MQK_SECTOR_EXPOSURE_LIMITS_BPS` config (`core-rs/crates/mqk-daemon/src/capital_policy/sector_risk.rs`, new sibling module alongside `portfolio_risk`/`position_sizing`/`short_entry_policy`) — comma-separated `sector=max_gross_weight_bps` entries. Unset/empty (the default) parses to an empty map, i.e. disabled. A malformed entry (missing `=`, empty sector name, non-integer or negative value, duplicate sector key) rejects the *whole* config with a named reason — fails closed for the gate evaluation only, never at daemon startup. 15 unit tests.
- **Gate 1h** in `submit_internal_strategy_decision` (`core-rs/crates/mqk-daemon/src/decision.rs`) — wired pre-outbox, immediately after the existing Gate 1g (per-symbol notional cap) and before Gate 2 (DB presence). Reuses the registry's `sector_map()` (`mqk_md::instrument_registry`) and the exact same `fetch_recent_completed_bars_for_strategy` + `compute_portfolio_weights` seam `GET /api/v1/portfolio/live-weights` already uses for marks. Disabled config, or a symbol with no sector tag, or a tagged sector with no configured cap — all pass through without ever touching the DB or the registry's md_bars lookup. When the candidate's sector *is* capped: no live execution snapshot, no DB pool, or no completed mark for a needed symbol all fail closed (`sector_weights_missing`); non-positive NAV fails closed (`sector_nav_unavailable`); a malformed env var fails closed for this decision only (`sector_config_invalid`); a risk-reducing order is allowed even while the sector remains over cap. 8 scenario tests (`mqk-daemon/tests/scenario_sector_risk_gate_etf_risk_closure_01.rs`, SRG-01..SRG-08), 4 of them DB-backed and run for real against the local paper DB (port 5440) using a synthetic `ZZSR01TECH` / `zzsr01_sector_tech` fixture (temp registry file + temp `md_bars` rows) — never a real ticker.

**Enforcement seam:** pre-outbox, on the internal/native-strategy decision path only (`decision.rs`'s `submit_internal_strategy_decision` — the funnel used by the live execution loop, the dry-run bridge, and the repair path; confirmed by checking every caller). The separate external-signal HTTP path (`routes/strategy.rs`'s own, independently-numbered Gate 1e/1f/1g/1h/1h2 sequence) was **not** wired by this patch — a known, named gap, not a hidden one. **Closed by `ETF-RISK-EXTERNAL-SIGNAL-GATE-01` below.** `mqk-execution::gateway::RiskRequestContext` (the pre-broker-submit seam the original `ETF-FOUNDATION-01` audit identified, still exactly `{ is_risk_reducing: bool }`) was deliberately left untouched: the pre-outbox seam is earlier and already sufficient, so no broker-adjacent code needed to change.

**Default-off proof:** with `MQK_SECTOR_EXPOSURE_LIMITS_BPS` unset, Gate 1h never produces a `sector_*` disposition and the decision falls through to whatever the pre-existing next gate would have said anyway (SRG-02, no DB configured, message unchanged from Gate 2's pre-existing text). An enabled config but an untagged candidate symbol also never touches the DB (SRG-03). Pure-evaluator side: an empty limits map returns `sector_risk_disabled` without consulting marks/sector_map/positions (SR-01); same for a known-but-uncapped sector (SR-08) and an unclassified symbol (SR-07).

**Enabled-sector-limit proof:** a configured cap denies a prospective breach before outbox, with the outbox row count unchanged (SRG-06, real DB); within-cap exposure is allowed (SR-02 pure).

**Missing mark / NAV unavailable proof:** enabled-for-this-sector with no DB configured fails closed distinctly from Gate 2's generic message (SRG-04, no DB); enabled with a DB but no completed `md_bars` row for the candidate denies before outbox (SRG-07, real DB); the pure evaluator fails closed on missing marks (SR-05) and on non-positive NAV (SR-06) — both only when a cap actually applies, never when disabled.

**Risk-reducing proof:** a sell that lowers an already-over-cap sector's exposure is allowed by Gate 1h even though the sector remains over cap afterward (SR-04 pure: 8000bps→7500bps against a 6000bps cap; SRG-08 real DB, same numbers, outbox row written only if accepted by a later, unrelated gate).

**Operator-visible truth states:** `InternalDecisionOutcome.disposition` carries `sector_config_invalid` / `sector_weights_missing` / `sector_nav_unavailable` / `sector_limit_exceeded` (new rows added to the existing disposition table doc comment); a denial also fires the same `signal.blocked` Discord notification shape every sibling gate (1e/1g) already uses, naming the gate (`gate_1h_sector_risk`) and the truth state.

**Safety:** no broker/Alpaca submit code touched; `mqk-execution/src/gateway.rs` untouched; no live routing changes; no DB migrations; `.env.local` untouched; no provider/broker calls; no paper/live orders submitted; no short-entry changes; B5/risk gates unchanged; sector risk remains default-off. All DB-backed tests use the synthetic `ZZSR01TECH` fixture only — verified post-run via direct query that zero `md_bars` rows and zero `oms_outbox` rows reference it, and zero temp registry files remain on disk.

**Side-finding (unrelated to this patch):** the real local paper DB's durable arm state is currently `disarmed (ReconcileDrift)` from a prior session. Two DB-backed tests initially assumed they could drive a decision all the way through to `disposition="accepted"`; rewritten to assert Gate 1h's own verdict only (absence of any `sector_*` disposition), since forcing the shared paper DB's real arm state to a particular value just to make a test pass would itself have been an unsafe, out-of-scope side effect. Full end-to-end accept-path proof for `submit_internal_strategy_decision` already exists and is out of scope here (`scenario_internal_strategy_decision.rs`).

**Validation:** `cargo check -p mqk-portfolio` / `-p mqk-md` / `-p mqk-runtime` / `-p mqk-daemon` all clean. `cargo clippy -p mqk-portfolio --all-targets -- -D warnings` and `cargo clippy -p mqk-daemon --lib -- -D warnings` both clean (no new lints; the pre-existing, unrelated `mqk-backtest::strategy_lab` lints are out of this build's lint scope and untouched). `cargo test -p mqk-portfolio` (full crate, includes the new file) — all pass, zero regressions, including the pre-existing `check_sector_limits`/`SectorConstraint` tests. `cargo test -p mqk-daemon --test scenario_sector_risk_gate_etf_risk_closure_01 -- --test-threads=1` — 8/8 pass against the real local paper DB (port 5440). `cargo test -p mqk-daemon --test scenario_internal_strategy_decision --test scenario_multi_symbol_capital_caps_01 --test scenario_multi_symbol_day_order_cap_01 --test scenario_native_strategy_bridge_b1c` — all pass, zero regression on the shared gate sequence.

### ETF-RISK-EXTERNAL-SIGNAL-GATE-01 — CLOSED_LOCAL

**Purpose:** Close the one named gap `ETF-RISK-CLOSURE-01` left open: the external-signal HTTP path (`routes/strategy.rs`'s `POST /api/v1/strategy/signal`) had no sector-exposure protection at all. Internally-generated orders (`decision.rs`) and externally-submitted signals now share the exact same gate mechanics — sector exposure risk cannot drift between the two paths.

**What was built:**

- `capital_policy::sector_risk_gate::evaluate_sector_risk_gate` (new module, `core-rs/crates/mqk-daemon/src/capital_policy/sector_risk_gate.rs`) — the registry/snapshot/marks glue that previously lived inline in `decision.rs`'s Gate 1h, extracted into one async, DB-touching function shared by both callers. The sibling `capital_policy::sector_risk` module remains pure (env parsing only, unchanged); its module doc now points to `sector_risk_gate` as the one documented async exception in the package, rather than claiming the glue lives only in `decision.rs`. Takes `(state, candidate_symbol, side, qty)` — never a caller-supplied `risk_reducing` claim — derives the signed delta from `side`/`qty` internally exactly as `decision.rs` already did, and returns a route-neutral `SectorRiskGateResult { allowed, reason_code, message, sector, current_weight_bps, prospective_weight_bps, max_weight_bps }`. Calls the same pure `mqk_portfolio::evaluate_sector_risk` evaluator `ETF-RISK-CLOSURE-01` built — that evaluator is untouched by this patch.
- `decision.rs`'s Gate 1h refactored to call the shared helper (the ~220-line inline glue collapsed to a ~45-line call + outcome mapping). Disposition values, default-off behavior, and notification shape are unchanged — proven by the full pre-existing `scenario_sector_risk_gate_etf_risk_closure_01.rs` (SRG-01..08) and `scenario_internal_strategy_decision.rs` suites passing unmodified against the real local paper DB.
- **Gate 1i** in `strategy_signal` (`core-rs/crates/mqk-daemon/src/routes/strategy.rs`) — wired immediately after the existing Gate 1h2 (earnings calendar) and before Gate 1b (WS continuity), calling the same shared helper with the already-validated `symbol`/`side`/`qty` from the signal body. `sector_limit_exceeded` (a verified breach) maps to `403 FORBIDDEN`; every other deny outcome (`sector_config_invalid`, `unavailable`, `sector_weights_missing`, `sector_nav_unavailable`) maps to `503 SERVICE_UNAVAILABLE` — the gate could not verify safety, so it fails closed without claiming a breach it never actually computed. A denial fires the same `signal.blocked` Discord notification shape every sibling external-path gate (1e/1f/1g) already uses, naming the gate `external_gate_1h_sector_risk`.
- 9 new scenario tests (`mqk-daemon/tests/scenario_external_signal_sector_risk_01.rs`, ES-01/02/04/05/06/07/08/09/10 — numbered to mirror the internal-path SRG suite, ES-03's "outbox unchanged" assertion folded into ES-02 rather than a separate test) using a synthetic `ZZEXT01TECH` / `zzext01_sector_tech` fixture distinct from the internal-path suite's `ZZSR01TECH` fixture (separate test binaries are separate OS processes; a shared fake symbol could race on `md_bars` rows if `cargo test` ran both concurrently). 6 of the 9 are DB-backed and run for real against the local paper DB (port 5440). Two cases go beyond the original `ETF-RISK-CLOSURE-01` proof matrix and close a pre-existing gap in it: ES-09 (configured cap, order within limit, daemon-wiring level — previously only proven at the pure-evaluator level, SR-02) and ES-10 (`sector_nav_unavailable` — not previously proven at the daemon-wiring level at all, on either path).

**Enforcement seam:** pre-outbox, before Gate 1b (WS continuity) and well before Gate 2 (DB presence) / Gate 3 (arm state) — a sector-risk denial on the external path never reaches arm-state, active-run, or outbox-enqueue logic, so no durable arm/run state had to be seeded or perturbed for any test in this file. Helper logic is **fully shared**, not duplicated: `decision.rs` and `routes/strategy.rs` both call `capital_policy::sector_risk_gate::evaluate_sector_risk_gate`; only the outcome-to-HTTP-status / outcome-to-disposition mapping differs per path (decision.rs has no HTTP status to choose; routes/strategy.rs splits 403 vs 503 as above).

**Config/env behavior:** identical to `ETF-RISK-CLOSURE-01` — same `MQK_SECTOR_EXPOSURE_LIMITS_BPS` env var, same parser, same default (unset/empty → disabled), shared verbatim by both paths since they call the same parsing function.

**Default-off proof:** with `MQK_SECTOR_EXPOSURE_LIMITS_BPS` unset, the external signal path never produces a `sector_*` disposition and falls through unchanged to Gate 1b (ES-01, no DB configured at all). An enabled config but an untagged candidate symbol also never touches the DB (ES-08, real default registry).

**Enabled-sector-limit proof:** a configured cap denies a prospective breach with `403 sector_limit_exceeded` before outbox, outbox row count unchanged (ES-02, real DB); an order within the same cap is allowed and falls through to Gate 1b (ES-09, real DB).

**Missing mark / missing snapshot / NAV unavailable proof:** enabled-for-this-sector with a DB but no completed `md_bars` row for the candidate denies with `503 sector_weights_missing` before outbox (ES-04, real DB); enabled with a DB but no live execution snapshot denies with `503 sector_weights_missing`, blocker text naming the snapshot distinctly (ES-05, real DB); a negative-cash/zero-position edge case denies with `503 sector_nav_unavailable` before outbox, with no `md_bars` row required at all — the *current* weights snapshot's NAV is already non-positive before any mark is ever consulted (ES-10, real DB).

**Risk-reducing proof:** identical math to `ETF-RISK-CLOSURE-01`'s SRG-08 (8000bps→7500bps against a 6000bps cap) replayed through the external HTTP path — a sell that lowers an already-over-cap sector's exposure is allowed and falls through to Gate 1b (ES-07, real DB). Risk-reducing status is derived the same honest way on both paths: from `side`/`qty` (already-validated request fields) inside the shared helper, never from a caller-supplied `risk_reducing` claim, caller-supplied marks, or order/limit price.

**Operator-visible reason codes/truth states:** `StrategySignalResponse.disposition` now carries `sector_config_invalid` / `sector_weights_missing` / `sector_nav_unavailable` / `sector_limit_exceeded` on the external path — the identical strings `decision.rs`'s `InternalDecisionOutcome.disposition` already carried, since both derive from the same `SectorRiskGateResult.reason_code`. `StrategySignalResponse`'s schema itself is unchanged (no new fields) — operator-readable detail (sector, current/prospective/max bps) is carried in the existing free-text `blockers` array, matching the internal path's existing precedent.

**Safety:** no broker/Alpaca submit code touched; `mqk-execution/src/gateway.rs` untouched; no live routing changes; no DB migrations; `.env.local` untouched; no provider/broker calls; no paper/live orders submitted; no short-entry changes; B5/risk gates unchanged; sector risk remains default-off on both paths. All new DB-backed tests use the synthetic `ZZEXT01TECH` fixture only, with `delete_test_bars`/`cleanup_dir` run after every test.

**Unrelated pre-existing finding (not caused by this patch, not fixed):** `cargo test -p mqk-daemon --test scenario_signal_to_outbox_unit_proof_01 -- --include-ignored` fails 4 of its 9 tests (`sto01`/`sto02`/`sto03`/`sto06`) when `MQK_DATABASE_URL` points at the local paper DB (port 5440 / `miniquantdesk_paper`) — that file's ignored-by-default tests go through a different connection helper (`mqk-testkit`'s `TEST-DB-SAFETY-GUARD`) that explicitly refuses to connect to any DB whose name contains `"miniquantdesk_paper"`/`"paper"`/`"live"`, by design, regardless of this patch. This file was not touched by this patch and the guard predates it; the failure reproduces identically on `main` before this patch's changes. The 5 non-ignored tests in that same file pass.

**Validation:** `cargo check -p mqk-daemon` / `-p mqk-portfolio` / `-p mqk-md` / `-p mqk-runtime` all clean. `cargo clippy -p mqk-daemon --lib -- -D warnings`, `cargo clippy -p mqk-daemon --test scenario_external_signal_sector_risk_01 -- -D warnings`, and `cargo clippy -p mqk-portfolio --all-targets -- -D warnings` all clean. `cargo fmt -p mqk-daemon -- --check` shows zero diffs in every file this patch touched (remaining repo-wide diffs are pre-existing, in files untouched by this patch). `cargo test -p mqk-daemon --test scenario_external_signal_sector_risk_01 -- --test-threads=1` — 9/9 pass against the real local paper DB (port 5440). `cargo test -p mqk-daemon --test scenario_sector_risk_gate_etf_risk_closure_01 -- --test-threads=1` — 8/8 pass (zero regression on the refactored internal path). `cargo test -p mqk-daemon --test scenario_internal_strategy_decision -- --include-ignored --test-threads=1` — 17/17 pass, including the full accept-to-outbox path. `cargo test -p mqk-daemon --test scenario_asset_class_scope_b8 --test scenario_capital_policy_tv04 --test scenario_capital_policy_tv04c --test scenario_capital_policy_tv04e --test scenario_signal_refusal_obs01 --test scenario_canonical_paper_path_pta01 -- --test-threads=1` — all pass, zero regression on the external signal route's other gates. `cargo test -p mqk-portfolio --test scenario_etf_risk_closure_01` — 12/12 pass (pure evaluator untouched).

---

### LEDGER-MULTI-ASSET-RECONCILE-01 — CLOSED_LOCAL

**Purpose:** Reconcile ledger §11's older multi-asset planning labels against this roadmap's `ASSET-CORE`/`CRYPTO`/`FUTURES`/`OPTIONS`/`FX` patch IDs, refresh `docs/specs/experimental/multi_asset_scaffold_01.md`'s stale "not yet created" status table, and update the recommended next patch now that the ETF sector-risk chain (`ETF-RISK-01` via `ETF-RISK-CLOSURE-01` + `ETF-RISK-EXTERNAL-SIGNAL-GATE-01`) is fully closed. Docs/ledger only — no Rust, GUI, config, DB, or test changes.

**What changed:**

- Ledger §11's five labels (`DATA-MULTI-ASSET-MODEL-01`, `DATA-INGEST-CRYPTO-PLAN-01`, `DATA-INGEST-FUTURES-PLAN-01`, `DATA-INGEST-OPTIONS-PLAN-01`, `DATA-INGEST-FOREX-PLAN-01`) are marked `RECONCILED / SUPERSEDED` in §11 and each mapped to its corresponding §19 patch IDs. No old label was deleted; original purpose text is preserved alongside the mapping.
- §18's numbered list items 7–8 (`DATA-MULTI-ASSET-MODEL-01`, "Provider/multi-asset ingestion plans") are annotated as superseded by §19, without rewriting §18's historical order.
- This section's own "Note:", "Top 20", and `ETF-FOUNDATION-01` "Recommended next patch" references to `PORTFOLIO-LIVE-WEIGHTS-01`/`ETF-RISK-01`/`LEDGER-MULTI-ASSET-RECONCILE-01` as forward-looking recommendations are annotated as historical/closed, pointing here for the current state.
- `docs/specs/experimental/multi_asset_scaffold_01.md`'s "Future Patch Lane IDs" table is refreshed: `ASSET-CAPABILITY-MATRIX-01` (`424f0de`), `MULTI-ASSET-ROUTING-GUARD-01` (`ff2ae59`), and `DISABLED-ASSET-GATE-TESTS-01` (`6fe1697`) are marked `SHIPPED`, independently re-verified as ancestors of `HEAD` via `git merge-base --is-ancestor` (not just copied from the audit doc). The doc's promotion-gate philosophy and hard boundaries are unchanged.
- `docs/audits/multi_asset_completion_audit.md`'s patch status table and §13 "Recommended Next Patch" are updated: `LEDGER-MULTI-ASSET-RECONCILE-01` is now `CLOSED`, and the recommended next patch is `ASSET-CORE-01`.

**Current state (confirmed against committed `HEAD`, not memory or prior chat claims):**

1. `ETF-RISK-01` is `CLOSED` — closed across both the internal decision path (`ETF-RISK-CLOSURE-01`, Gate 1h in `decision.rs`) and the external signal path (`ETF-RISK-EXTERNAL-SIGNAL-GATE-01`, Gate 1i in `routes/strategy.rs`).
2. Ledger §11's old planning labels are reconciled, not deleted — each maps to specific §19 patch IDs; a future session can map an old label forward without losing the original ask.
3. The canonical next multi-asset foundation patch is `ASSET-CORE-01` (Unified Instrument Registry v2 — also resolves the `mqk_schemas::AssetClass` vs `mqk_md::provider::ProviderAssetClass` two-enum split documented in the audit doc §2). This was already the audit's own foundational recommendation; closing `ETF-RISK-01` removed it from the "next" position without changing the underlying sequencing.
4. §18 (Recommended Order) is the older, general repo-wide patch order and predates the multi-asset audit; §19 (this section) is the active multi-asset expansion order. Where the two disagree on multi-asset sequencing, §19 governs (note added directly in §18).

**Validation:** docs-only; no `cargo`/GUI build was required or run. `git diff --check` run clean (see commit record). No Rust/GUI/config/DB files were touched; no daemon started; no provider/broker calls; no paper/live orders.

**Safety:** docs only. No source code, test, config, or DB migration changes. `MiniQuantDesk_Master_Patch_Ledger_v2_updated.md` (untracked draft) and `smoke_logs/` were not staged or touched.

### ASSET-CORE-01A — CLOSED_LOCAL / PARTIAL

**Purpose:** Start `ASSET-CORE-01` (Unified Instrument Registry v2) with its safest high-leverage slice: reconcile the two-enum split the audit doc's §2 "architecture-debt finding" identified (`mqk_schemas::AssetClass` vs `mqk_md::provider::ProviderAssetClass`), add explicit/exhaustive conversion tests, and strengthen the instrument-registry metadata seam — without changing equity behavior, enabling any non-equity class, or touching broker/order/risk-gate/DB code. Combines `ASSET-CLASS-ENUM-RECONCILE-01` + `INSTRUMENT-METADATA-SEAM-01` + `ASSET-CORE-01-LEDGER-ANCHOR-01`.

**First-decision findings (from current repo evidence, not assumption):**

1. `mqk_schemas::AssetClass` (`Equity, Option, Future, Crypto, Forex`) is already the canonical domain asset-class type — confirmed live-wired into `mqk_execution::gateway::BrokerGateway::submit_with_context` (`MULTI-ASSET-ROUTING-GUARD-01`, rejects every non-`Equity` value pre-broker) and re-exported as `mqk_execution::AssetClass`. A third independent string-keyed mapping already exists too: `mqk-runtime`'s `validated_asset_class` (outbox JSON parser) accepts `"future"`/`"futures"` and `"option"`/`"options"` as aliases and rejects both — independent confirmation that the canonical singular form (`"future"`, `"option"`) is already the repo's de facto convention.
2. ETF is correctly *not* a separate `AssetClass` variant. It is represented purely as `instrument_kind = "etf"` metadata under `asset_class = "equity"` on `TrackedInstrument` (ETF-REGISTRY-01) — this patch preserves that and makes it queryable via a new typed accessor.
3. `mqk-schemas` has zero crate-internal dependencies (only `serde`/`uuid`/`chrono`); `mqk-md` has zero dependency on `mqk-schemas`. Confirmed via direct `Cargo.toml` reads on both crates plus a repo-wide `grep -rl "mqk-schemas" --include=Cargo.toml`. A `mqk-md → mqk-schemas` edge would **not** be circular.
4. Despite (3) showing Option A (real `From<ProviderAssetClass> for AssetClass`) is dependency-graph-legal, this patch chose **Option B**: a local, pure, additive mapping inside `mqk-md` producing canonical *strings* (not the `mqk_schemas` enum type). Rationale: the mission's own strict file scope omitted every `Cargo.toml`, signaling the dependency edge should not be added in this slice; `mqk-md` is deliberately decoupled from execution-layer types (its own module doc: "does not write to the DB... no concrete provider implementations" beyond market-data concerns); and a string-level seam is sufficient to prove the mapping exhaustively without widening the dependency graph. ASSET-CORE-01B remains the natural place to revisit Option A if a real shared instrument-registry crate is built.

**What was built (all additive, zero behavior change to any existing function):**

- `mqk_md::provider::provider_asset_class_trading_class(&ProviderAssetClass) -> &str` and `provider_asset_class_instrument_kind(&ProviderAssetClass) -> Option<&'static str>` (`core-rs/crates/mqk-md/src/provider.rs`) — pure, exhaustive (no wildcard match arm in either function, so a new bare `ProviderAssetClass` variant fails the build until both are updated) mapping to the canonical singular vocabulary (`"equity"`, `"option"`, `"future"`, `"crypto"`, `"forex"`). `Etf → ("equity", Some("etf"))`. An unrecognized config-supplied `Other(raw)` passes its own string through rather than silently defaulting to `"equity"`. Re-exported from `mqk-md`'s crate root. 9 new tests (`pac_01`..`pac_09`).
- `TrackedInstrument::{is_etf, normalized_instrument_kind, normalized_sector, normalized_category, trading_asset_class}` (`core-rs/crates/mqk-md/src/instrument_registry.rs`) — `is_etf()`/`trading_asset_class()` give the ETF/equity invariant a typed accessor instead of tribal-knowledge field reads; the three `normalized_*` accessors share one pure helper (`normalize_optional_tag`) that trims and treats empty/whitespace-only as absent, defensively, independent of whether `validate_registry` has already run on the data. 8 new tests (`im_01`..`im_08`), including `im_08`: a direct cross-check that the registry's real 14 tagged ETF instruments and `provider_asset_class_trading_class`/`provider_asset_class_instrument_kind(&ProviderAssetClass::Etf)` agree exactly — the concrete "explicit conversion/mapping test between existing asset-class concepts" the mission required.
- `mqk_schemas::AssetClass` doc comment only (`core-rs/crates/mqk-schemas/src/lib.rs`) — records its canonical status, points to the `mqk-md` mapping seam, and notes ETF is deliberately not a variant. Zero code/derive/variant change.

**Asset-class treatment (unchanged from current repo behavior, now explicitly tested):**

- Equity: tradable today, unchanged.
- ETF: not a separate executable asset class; `instrument_kind = "etf"` under `asset_class = "equity"`, proven via `is_etf()`/`trading_asset_class()` against all 14 real registry entries (SPY, QQQ, IWM, DIA, XLK, XLF, XLE, XLI, XLP, XLU, TLT, IEF, SHY, GLD).
- Option/Future/Crypto/Forex: remain disabled; `provider_asset_class_trading_class` maps them to canonical singular labels (`"option"`, `"future"`, `"crypto"`, `"forex"`) for future reference, but labeling is explicitly documented as not enablement — the only live enforcement boundary (`GateRefusal::AssetClassDisabled`) is untouched and re-proven via regression.

**Deliberately not done:** no `Cargo.toml` edited anywhere (no new dependency edge); no change to `mqk_schemas::AssetClass`'s variants/derives; no change to `gateway.rs`, `order_router.rs`, `types.rs`, `routes/strategy.rs`, `routes/system.rs`, or any capability-matrix code; no change to `config/instruments/equities.json` or `config/providers/providers.json`; no DB migration. `ASSET-CORE-01` remains `PARTIAL` — a real unified instrument-registry v2 schema/loader (multi-provider, multi-asset-class-aware, replacing today's equities-only `equities.json` + string `asset_class` field) does not exist yet.

**Validation:** `cargo check -p mqk-md -p mqk-schemas` clean; `cargo check -p mqk-daemon -p mqk-execution` clean (downstream dependents recompile cleanly). `cargo test -p mqk-md` — 180/180 pass (163 pre-existing + 17 new: 9 `pac_*` + 8 `im_*`; full crate run, zero regressions), doc-tests 1/1 pass. `cargo test -p mqk-md instrument_registry` — 27/27 pass (19 pre-existing `reg_*` + 8 new `im_*`). `cargo test -p mqk-md pac_` — 9/9 pass (the mission's suggested `provider_asset_class` filter only matched 1 test by substring coincidence; the named-module filter found all 9, per the "inspect names and run the exact test module" fallback instruction). `cargo test -p mqk-schemas` — 0 tests (pre-existing condition; crate had zero tests before this patch and still has none — only a doc comment was added). `cargo clippy -p mqk-md --all-targets -- -D warnings` and `cargo clippy -p mqk-schemas --all-targets -- -D warnings` — both clean. `cargo fmt -p mqk-md -- --check` / `-p mqk-schemas -- --check` — both zero diffs. Regression (disabled-asset gates, unmodified): `cargo test -p mqk-daemon --test scenario_asset_class_scope_b8` — 12/12 pass; `cargo test -p mqk-execution --test scenario_asset_class_guard_multi_asset_routing_guard_01 --features testkit` — 8/8 pass (cargo itself required `--features testkit`, a pre-existing crate requirement unrelated to this patch).

**Safety confirmation:** no broker submit changes; no Alpaca adapter changes; no live routing changes; no order/outbox writes; no DB migrations; `.env.local` not read or touched; no provider/broker network calls; no paper/live orders submitted; no non-equity asset class enabled; disabled-asset gates not weakened (re-proven by the unmodified regression suites above). Daemon was not started at any point. `MiniQuantDesk_Master_Patch_Ledger_v2_updated.md` and `smoke_logs/` were not staged or touched.

**Recommended next slice:** `ASSET-CORE-01B` — canonical instrument-registry v2 schema/loader (the real multi-provider, multi-asset-class-aware registry this slice's mapping seam was built to feed into). Should also be the place to revisit Option A (a real `mqk-md → mqk-schemas` dependency and `From<ProviderAssetClass> for AssetClass` impl) if a shared registry crate makes that dependency direction natural.
