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

**Update (2026-06-22):** A market-hours paper proof classified the first real blocker as legitimate `NO_SIGNAL_GENERATED` (strategy conditions not met / price movement below threshold) — not a wiring bug. That reason was only visible live, via in-memory readiness counters; it did not survive a daemon restart. `AUTON-NO-SIGNAL-OBS-01` below closes that durability gap.

### AUTON-NO-SIGNAL-OBS-01 — CLOSED_LOCAL

**Purpose:** Persist autonomous strategy signal-evaluation outcomes durably — especially legitimate no-signal decisions — so a market-hours run that evaluates real bars but produces no order leaves DB evidence after daemon restart. Observability only: no strategy threshold, gate, or broker/order-path logic changed.

**Where the no-signal reason lived before this patch:** readiness-only formatting, computed live from in-memory `AtomicI64`/`AtomicU64` counters (`AppState::last_bar_signal_qty`, `bar_tick_dispatch_count`, `last_bar_context_bars`) and rendered as a `NO_SIGNAL_GENERATED (...)` blocker string in `routes/system.rs` on every `/api/v1/autonomous/readiness` poll. Nothing about a no-signal tick was written to the DB.

**What was built:**

- Migration `0043_strategy_signal_evaluations.sql` — new additive `strategy_signal_evaluations` table (no existing table had the needed structured columns; `sys_autonomous_session_events` lacks symbol/timeframe/bars_loaded, `audit_events` is the hash-chained replay-determinism ledger and the wrong semantic fit for high-frequency per-tick telemetry). `evaluation_id` is `UUIDv5(NAMESPACE_DNS, "mqk.signal-evaluation.v1|{run_id}|{strategy_id}|{symbol}|{timeframe}|{now_tick}")` — deterministic, `ON CONFLICT DO NOTHING`. `run_id` is nullable with no FK (observability, not part of the outbox/inbox/run lifecycle chain).
- `mqk-db/src/strategy.rs` — `InsertStrategySignalEvaluationArgs` / `StrategySignalEvaluationRecord` / `insert_strategy_signal_evaluation` / `fetch_recent_strategy_signal_evaluations`, mirroring the `fill_quality_telemetry` (TV-EXEC-01) idiom exactly.
- `mqk-daemon/src/state.rs` — `AppState::record_signal_evaluation`, a best-effort, non-fatal write (warn + swallow on DB error, mirrors the orchestrator's fill-quality telemetry pattern) called from three points inside `dispatch_native_strategy_for_symbol_with_loaded_bars`: (1) the existing MD-STALENESS-PER-TICK-GATE-01 "no completed bars" fail-closed return (`bar_context_source="no_bars_available"`, `bars_loaded=0`, `decision_stage="pre_dispatch_gate"`); (2) its "stale bar" fail-closed return (`bar_context_source="stale_bars"`, real stale bar's own timestamp, `decision_stage="pre_dispatch_gate"`); (3) immediately after `invoke_native_strategy_on_bar_from_window` returns (`bar_context_source="db_loaded"`, `decision_stage="strategy_evaluated"`, `signal_qty`/`signal_generated` from the strategy's real target sum, `reason_code`/`reason` reused from the already-live `STRATEGY-DECISION-OBSERVABILITY-01` diagnostic). `strategy_id` comes from `NativeStrategyBootstrap::active_strategy_id()`; `run_id` from `self.status.read().await.active_run_id` — no new function-signature plumbing, so no unrelated call sites needed changes. None of the three existing gate decisions (return value, control flow) were altered — only an additive side-effect was added after each was already made.
- `GET /api/v1/execution/signal-evaluations` (`routes/execution.rs`, registered in `routes.rs`) — read-only, **not** scoped to the active run (unlike `execution_fill_quality`) so a restart-surviving no-signal row stays visible with no run active. `truth_state`: `active` / `no_rows` / `db_unavailable` / `query_failed`.

**Tests:** new `mqk-daemon/tests/scenario_signal_evaluation_journal_auton_no_signal_obs_01.rs` (SO-01..SO-06, 7 tests including a route split). All run for real against the local paper DB (port 5440) after applying migration 0043 via `mqk db migrate --yes`: SO-01 drives the real production dispatch path (`tick_strategy_dispatch_for_symbol`) end-to-end and proves the persisted row's `strategy_id`/`symbol`/`timeframe`/`bars_loaded`/`latest_bar_ts_utc`/`signal_qty`/`signal_generated`/`reason_code`/`reason` are bit-for-bit truthful against the strategy's actual output, **and** that zero new `oms_outbox` rows are created; SO-02/SO-03 prove the two pre-dispatch-gate paths persist with the gate's own (unchanged) verdict; SO-04 proves `run_id` links via the daemon's own `establish_db_backed_active_run_for_test` seam; SO-05 proves the `mqk_db` insert/fetch round-trip is idempotent under a duplicate `evaluation_id` and orders newest-first; SO-06 proves the route's `active` and `db_unavailable` truth states. 7/7 pass.

**Regression:** `scenario_md_staleness_per_tick_gate_01` (5/5, the closest sibling — shares the exact code path this patch instruments), `scenario_intraday_md_freshness_autonomous_01` (6/6), `scenario_multi_symbol_dispatch_loop_01` (8/8), `scenario_route_contract_rt01` (2/2, GUI route contract gate) — all pass unchanged. `cargo check -p mqk-db -p mqk-daemon` and `cargo clippy -p mqk-db -p mqk-daemon --lib -- -D warnings` both clean.

**Pre-existing, unrelated finding:** `scenario_fill_quality_tv_exec01::fq05_read_surface_returns_active_truth_with_exact_rows` fails on this local paper DB. Root cause confirmed via direct `runs` table inspection: `current_status_snapshot()` resolves `active_run_id` from the single *latest* run row for `(engine_id, mode)`, and FQ-05's fixture seeds its run with a hardcoded `started_at_utc = 2020-01-01` — so it loses that lookup to any real `mqk-daemon`/`PAPER` run created later by actual daemon/CLI usage (a `HALTED` run timestamped earlier today, well before this session's test work, was the culprit). Not caused by this patch's code (verified: the failure reproduces identically whether or not this patch's test file is run first; this patch added no `PAPER`-mode run rows) and not reproducible in isolation on a fresh DB. Left alone — out of this patch's file scope.

**Migration governance:** `scripts/guards/check_migration_governance.sh`'s manifest-diff check (run manually via `python` since the script's hardcoded `python3` triggers the Windows Store shim on this box) confirms migration `0043` is correctly paired in `manifest.json`. The only drift it reports is a pre-existing, unrelated Windows path-separator artifact on the historical `hold/0017_...` entry (`/` vs `\` from `pathlib.relative_to` on Windows) — not introduced by this patch.

**Safety confirmation:** no broker/Alpaca submit code touched; no live routing changes; no paper/live orders submitted by tests; no strategy threshold/entry logic changed; no gate bypassed (both pre-dispatch gates' `return None` control flow is untouched — the journal write is an additive side-effect after the verdict, not part of it); no fabricated data (a no-signal row's `latest_bar_ts_utc`/`signal_qty` are honestly `None` when no bar/strategy-run exists, never a default); `.env.local` not read or modified; no provider/broker calls; smoke logs and the untracked ledger draft untouched.

**Recommended next patch:** `AUTON-NO-TRADE-02` can now be defined for real — the durable evidence this patch added is the missing input it was blocked on.

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

### INTRADAY-MD-PROVIDER-FRESHNESS-TRUTH-01-COMBINED — CLOSED_LOCAL

**Purpose:** Close the stale intraday/provider freshness truth gap found during the market-hours short proof attempt: the provider sync completed with rows read/updated, but AAPL/5m still had only prior-session completed bars while the operator evidence/status surface could still report `all_passed=true`.

**Root cause found in current repo:**

- The Windows intraday refresher wrote evidence under `exports/market_data/intraday_refresh_*.json`.
- `all_passed` was computed in `scripts/windows/Refresh-IntradayMarketData.ps1` from provider/config success plus completed-row and staleness checks, but its default `-MaxStalenessMinutes` was `1440`, wider than the daemon intraday gate's `MQK_INTRADAY_BAR_MAX_AGE_SECS` default of `900` seconds.
- `GET /api/v1/market-data/intraday-refresh/status` in `mqk-daemon/src/routes/transport_quality.rs` read only the latest evidence file and relayed `all_passed`; it did not independently recompute stale-after-refresh truth from the evidence fields.
- No concrete TwelveData request-parameter bug was proven in this patch. Current request construction uses interval `5min`, `start_date`, `end_date`, `timezone=UTC`, and date-window chunking. The live provider may still return stale data; this patch makes that condition visible and fail-closed in evidence/status.

**Closure:**

- `Refresh-IntradayMarketData.ps1` now derives the default freshness cap from timeframe: intraday uses `MQK_INTRADAY_BAR_MAX_AGE_SECS` or `900` seconds; daily keeps the existing 4-day tolerance. The script writes per-symbol post-refresh verdict fields: `latest_completed_bar_age_secs`, `max_allowed_age_secs`, `freshness_truth_state`, `reason_code`, and `passed`.
- New/used reason codes include `fresh_after_refresh`, `provider_returned_stale_intraday_data`, `latest_bar_stale_after_refresh`, `latest_completed_bar_missing`, `provider_returned_no_rows`, `provider_error`, and `refresh_failed`.
- `IntradayRefreshSymbolStatus` now surfaces provider row counts plus the freshness verdict fields.
- The read-only status route recomputes a conservative symbol verdict from evidence fields. If any symbol is stale or otherwise failed, response `all_passed` is forced false even when an older evidence file claims `all_passed=true`.
- Tests remain fixture-only/pure: no TwelveData, Alpaca, yfinance, Polygon, broker, order, autonomous runtime, or market-hours proof calls.

**Tests:**

- `scenario_intraday_md_refresher_01`: added RF-05 proving provider rows can be present while all usable completed rows are dropped, leaving intraday freshness missing/fail-closed.
- `scenario_intraday_md_refresher_operator_surface_01`: added IRS-10/IRS-11 proving stale-after-refresh and provider-success/no-rows evidence force `all_passed=false` with explicit reason codes.
- Regression proof also includes `scenario_intraday_md_freshness_autonomous_01`, `scenario_premarket_data_readiness_gate_01`, and the closest existing market-data coverage target `scenario_md_coverage_data_ingest_gui_results_01`.

**Validation:** Focused tests and compile/clippy checks passed. `cargo fmt -p mqk-daemon -p mqk-cli -p mqk-md -p mqk-db -- --check` still fails on pre-existing unrelated formatting drift in files outside this patch scope; patch-owned Rust files were formatted locally. No DB migration.

**Remaining requirement before retrying market-hours short proof:** Run a fresh allowed market-hours paper proof only after a real provider refresh produces current-session completed 5m bars and `/api/v1/market-data/intraday-refresh/status` reports `all_passed=true` with per-symbol `passed=true` and `fresh_after_refresh`.

**Status:** CLOSED_LOCAL

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

### ASSET-CORE-01B — CLOSED_LOCAL / PARTIAL

**Purpose:** Build the real instrument-registry v2 schema, loader, and validator `ASSET-CORE-01A` deferred — a model that can represent stocks, ETFs, crypto pairs, futures, options, and forex — without switching any production code path over to it. Combines `INSTRUMENT-REGISTRY-V2-SCHEMA-01` + `INSTRUMENT-REGISTRY-V2-LOADER-01` + `INSTRUMENT-REGISTRY-V2-COMPAT-PROOF-01` + `ASSET-CORE-01B-LEDGER-ANCHOR-01`.

**First-decision findings (from current repo evidence, not assumption):**

1. `mqk_schemas::Instrument`/`ContractSpec`/`AssetClass`/`OrderSpec` are confirmed **live execution-path types** — `mqk-execution/src/order_router.rs` builds `Instrument`/`ContractSpec::Equity` values directly and `mqk-execution/src/types.rs` re-exports `AssetClass` for the broker-submit gate (`BrokerGateway::submit_with_context`, `MULTI-ASSET-ROUTING-GUARD-01`). Extending or reshaping these types to fit a registry schema would touch the live broker-submit/risk boundary — out of this patch's scope by `CLAUDE.md` and the mission's own forbidden-files list.
2. `mqk-md → mqk-schemas` remains dependency-graph-legal (re-confirmed via direct `Cargo.toml` reads: `mqk-schemas` depends only on `serde`/`uuid`/`chrono`; `mqk-md` depends on `anyhow`/`chrono`/`serde`/`serde_json`/`tokio`/`async-trait`/`reqwest` — no edge either direction), but this patch again chose **not** to add it, for the same reason finding (1) gives new weight to: the existing `mqk_schemas` types are narrower than what a real multi-asset registry needs (no `instrument_id`, no `provider_symbols` map, no `enabled`/`paper_trading_enabled`/`live_trading_enabled`, no metadata, no `schema_version`, and `ContractSpec::Crypto` carries no base/quote) — reusing them would mean either widening a live execution type or building a second model anyway. `ASSET-CORE-01A`'s Option B precedent (additive, string-keyed, `mqk-md`-local) was followed again.
3. `TrackedInstrument` (v1) is consumed by `mqk-daemon` (`capital_policy/sector_risk_gate.rs`, `routes/ingest.rs`, `api_types.rs`) and `mqk-cli` (`commands/md.rs`), confirmed via repo-wide grep for `TrackedInstrument|load_instrument_registry|enabled_equities(|validate_registry(`. None of these were touched; v2 has zero consumers by construction.
4. `config/instruments/equities.json` has exactly 88 entries, all `enabled: true`, 14 of them `instrument_kind="etf"` (verified by direct parse, not assumption) — matches `reg_08`'s `88` constant exactly.

**What was built (all additive, zero behavior change to any existing function):**

- `core-rs/crates/mqk-md/src/instrument_registry_v2.rs` (new file, ~520 lines incl. tests) — registered as `pub mod instrument_registry_v2;` in `mqk-md/src/lib.rs` (one line added, no re-exports at crate root — matches how v1's `instrument_registry` module is wired). No `Cargo.toml` touched.
  - `InstrumentRegistryV2 { schema_version: u32, instruments: Vec<InstrumentDefinitionV2> }`.
  - `InstrumentDefinitionV2`: `instrument_id`, `symbol`, `asset_class: String` (canonical singular: `equity`/`option`/`future`/`crypto`/`forex`, matching `provider_asset_class_trading_class`'s output exactly), `instrument_kind: Option<String>`, `venue: Option<String>`, `currency`, `quote_currency: Option<String>`, `provider_symbols: BTreeMap<String,String>`, `enabled`, `paper_trading_enabled`, `live_trading_enabled` (both independent of `enabled`), `timeframes: Vec<String>`, `contract: Option<ContractDefinitionV2>`, `metadata: InstrumentMetadataV2 { sector, category, tags }`, `notes: Option<String>`, and `allow_enabled_non_equity_for_testing: bool` — an explicit, documented test/fixture-only escape hatch (see Validation rules below).
  - `ContractDefinitionV2` (internally-tagged enum, `#[serde(tag = "kind")]`): `Equity`, `Etf`, `CryptoPair { base, quote }`, `Future { root, expiry, multiplier, tick_size_micros }`, `Option { underlying, expiry, strike_micros, right, multiplier }`, `ForexPair { base, quote }`.
  - `load_instrument_registry_v2(path) -> Result<InstrumentRegistryV2>` — pure parse, mirrors v1's `load_instrument_registry`. No production file exists for this schema; proven via a temp-file round-trip test, not a committed fixture.
  - `validate_registry_v2(&InstrumentRegistryV2) -> Result<()>` — checks (in order): supported `schema_version` (currently `{1}`); non-empty `instrument_id`/`symbol`/`asset_class`/`currency`; canonical `asset_class` membership; unique `instrument_id`; unique `symbol` (no alias policy — none is proven anywhere in the repo, so v2 matches v1's strict-uniqueness behavior exactly); non-empty optional tags when present; ETF (`instrument_kind="etf"`) requires `asset_class=equity` **and** non-empty `sector`+`category` metadata (mirrors v1's existing ETF-REGISTRY-01 rule verbatim); non-ETF equity cannot carry `contract=Etf`; `future`/`option`/`crypto`/`forex` each require their matching `ContractDefinitionV2` variant with all-positive numeric fields and non-empty string fields (option `right` restricted to `call`/`put`); **`enabled=true` on any non-`equity` row fails unless `allow_enabled_non_equity_for_testing=true`** — and that flag only changes what this validator accepts, since no production path reads `InstrumentRegistryV2` at all; `enabled=true` requires non-empty `provider_symbols` (disabled rows may have none).
  - `convert_tracked_instrument_to_v2(&TrackedInstrument) -> InstrumentDefinitionV2` and `convert_v1_registry_to_v2(&[TrackedInstrument]) -> InstrumentRegistryV2` — pure, no IO. ETF detection uses `TrackedInstrument::is_etf()` (ASSET-CORE-01A) to pick `contract=Etf` vs `contract=Equity`; `provider_symbols` carries exactly the one provider identity v1 already proves (`{provider: provider_symbol}`, e.g. `{"twelvedata": "AAPL"}`); `sector`/`category` use the existing `normalized_sector()`/`normalized_category()` accessors; `paper_trading_enabled`/`live_trading_enabled` are always `false` (v1 has no such fields to preserve); neither function reads or writes `config/instruments/equities.json`.

**Tests (26 new, all in `instrument_registry_v2.rs`):**

- Schema/loader (`v2_01`..`v2_17`): a hand-written JSON document parses one equity, one ETF, and one disabled instance each of future/option/crypto/forex and validates cleanly (`v2_01`, the literal "registry v2 parses X" proof via the wire format); the loader round-trips a registry through a real temp file (`v2_02`); invalid `asset_class` (`v2_03`); duplicate `instrument_id`/`symbol` (`v2_04`/`v2_05`); unsupported `schema_version` (`v2_06`); missing contract entirely for each of the four derivative classes (`v2_07`); per-field contract violations for future/option/crypto/forex (`v2_08`..`v2_11`, table-driven); ETF missing sector or category (`v2_12`); non-ETF equity with `contract=Etf` (`v2_13`); enabled non-equity fails without the allow flag and passes with it explicitly set (`v2_14`/`v2_15`); enabled-requires-`provider_symbols`, disabled does not (`v2_16`); all four disabled derivative fixtures validate together (`v2_17`).
- V1↔V2 compatibility (`compat_01`..`compat_09`, all against the real `config/instruments/equities.json`): v1 loader/validator unaffected, 88 entries (`compat_01`); count preserved through conversion (`compat_02`); `enabled_equities()` symbol set preserved through conversion (`compat_03`); all 14 real tagged ETFs convert with `contract=Etf` + `instrument_kind=etf` (`compat_04`); a real non-ETF stock (AAPL) converts to plain equity (`compat_05`); provider/timeframes/venue/currency carry through for AAPL (`compat_06`); sector/category carry through for SPY (`compat_07`); **the entire real 88-row production registry, once converted, passes `validate_registry_v2` cleanly** (`compat_08` — the core "v2 is compatible with real production data" proof); paper/live flags default `false` for every converted row (`compat_09`).

**Deliberately not done:** no `config/instruments/experimental_instruments_v2.example.json` or any other config file added — the mission's own stated preference ("prefer no config file for this slice if tests can cover the schema") was followed, since all required fixtures (equity/ETF/future/option/crypto/forex, enabled and disabled) are fully covered by embedded Rust/JSON-literal tests; no `Cargo.toml` edited anywhere; no change to `mqk_schemas::Instrument`/`ContractSpec`/`AssetClass`; no change to `equities.json`, `providers.json`, `mqk-daemon`, or `mqk-cli`; no daemon started; no DB migration. `ASSET-CORE-01` remains `PARTIAL` — registry v2 exists and is proven compatible with current production data in memory, but nothing yet consumes it, and no non-equity class is enabled anywhere.

**Validation:** `cargo check -p mqk-md` clean. `cargo test -p mqk-md instrument_registry_v2` — 26/26 pass. `cargo test -p mqk-md instrument_registry` — 53/53 pass (27 pre-existing v1 `reg_*`/`im_*` + 26 new). `cargo test -p mqk-md pac_` — 9/9 pass (ASSET-CORE-01A regression, untouched). `cargo test -p mqk-md` (full crate) — 206/206 pass (180 pre-existing + 26 new), zero regressions; doc-test 1/1 pass. `cargo check -p mqk-daemon` and `cargo check -p mqk-cli` (downstream dependents) both clean. `cargo clippy -p mqk-md --all-targets -- -D warnings` clean. `cargo fmt -p mqk-md -- --check` clean (one pre-existing-style formatting pass applied to the new file before commit; zero diffs after). `mqk-schemas` was not touched, so its `cargo check`/`clippy`/`test` were not re-run (mission's own conditional: only required "if `mqk-schemas` touched beyond docs").

**Safety confirmation:** no broker submit changes; no Alpaca adapter changes; no live routing changes; no order/outbox writes; no DB migrations; `.env.local` not read or touched; no provider/broker network calls; no paper/live orders submitted; no non-equity asset class enabled anywhere (the one positive `enabled=true` non-equity test case exists only to prove the validator's own explicit escape hatch, entirely inside `#[cfg(test)]`, with zero production reader); disabled-asset gates untouched (no shared asset-class code outside `mqk-md` was modified, so the mission's conditional regression suites were not required and were not run). Daemon was not started at any point. `MiniQuantDesk_Master_Patch_Ledger_v2_updated.md` and `smoke_logs/` were not staged or touched.

**Recommended next slice:** `ASSET-CORE-01C` — either a read-only production v2 registry status surface (e.g. a daemon route that reports the v1→v2 conversion result for operator visibility, still never read by any trading path), or a controlled production v2 loader integration sitting behind an explicit, default-off flag. Either way, `ASSET-CORE-01` stays `PARTIAL` until something beyond tests actually reads `InstrumentRegistryV2`.

### ASSET-CORE-01C — CLOSED_LOCAL / PARTIAL

**Purpose:** Give `InstrumentRegistryV2` (`ASSET-CORE-01B`) its first real, non-test consumer: a read-only production/operator surface that loads the configured v1 registry, converts it to v2 in memory, validates it, and reports the result — without making v2 a trading/ingestion/backtest/GUI/risk/broker input anywhere. Combines `INSTRUMENT-REGISTRY-V2-STATUS-ROUTE-01` + `INSTRUMENT-REGISTRY-V2-CLI-PROBE-01` + `INSTRUMENT-REGISTRY-V2-OPERATOR-CONTRACT-01` + `ASSET-CORE-01C-LEDGER-ANCHOR-01`.

**First-decision findings (from current repo evidence, not assumption):**

1. `mqk-daemon/src/routes/system.rs` already owns the precedent surface for this exact shape: `system_metadata` builds a static, read-only `asset_capability_matrix` from compile-time constants with zero DB/provider/broker dependency, mounted at `/api/v1/system/*` alongside `system_status`/`system_preflight`/`system_runtime_leadership`/`system_session`/`system_topology`/`system_config_fingerprint`/`system_config_diffs`. The new route follows that file and that `/api/v1/system/*` placement rather than `routes/ingest.rs`.
2. `mqk-daemon/src/routes/ingest.rs::tracked_equities_list` (`GET /api/v1/ingest/tracked-equities`) is the exact existing precedent for "load `AppState::instrument_registry_path`, classify failure as missing-file vs parse-failure, return a `truth_state` envelope, never error the HTTP layer itself" — the new handler reuses this exact failure-classification shape (`path.exists()` distinguishes `unavailable` from `v1_load_failed`) instead of inventing a new one. `tracked_equities_list` itself was left untouched.
3. `AppState::instrument_registry_path: String` (defaulting to `config/instruments/equities.json`, overridable via `MQK_INSTRUMENT_REGISTRY_PATH`) is the one field every production registry reader already shares; the new route reads only this field and adds no new state, no query-parameter path override (unlike the CLI probe, which takes an explicit `--registry` argument as a local operator tool, not a network-exposed surface).
4. `convert_v1_registry_to_v2` and `validate_registry_v2` (`ASSET-CORE-01B`) are both pure/infallible-or-`Result`-returning with no IO, so the route needs no panic guard — `v2_conversion_failed` is documented as a reserved-but-currently-unreachable `truth_state` for this reason, rather than fabricated as a real code path.
5. `scenario_route_contract_rt01.rs`'s `GUI_PROBE_MANIFEST` and `scenario_gui_daemon_contract_gate.rs` are one-directional gates (every GUI-probed route must be mounted); neither requires every mounted daemon route to appear in the GUI's probe manifest. Since this patch explicitly does not touch the GUI, the new route was not added to either list — both regression suites were run unmodified and pass.

**What was built (additive; zero behavior change to any existing route, registry, or CLI command):**

- `core-rs/crates/mqk-daemon/src/api_types.rs` — `InstrumentRegistryV2StatusResponse` (`truth_state`, `registry_path`, `schema_version: Option<u32>`, `v1_count`, `v2_count`, `validation_passed`, `validation_errors: Vec<String>`, `asset_class_counts`/`instrument_kind_counts`/`contract_kind_counts: BTreeMap<String, usize>`, `enabled_count`, `etf_count`, `non_equity_count`, `enabled_non_equity_count`, `paper_trading_enabled_count`, `live_trading_enabled_count`, `production_cutover_enabled: bool` (always `false`), `trading_uses_v2: bool` (always `false`), `notes: Vec<String>`).
- `core-rs/crates/mqk-daemon/src/routes/system.rs` — `system_instrument_registry_v2_status` handler (+ pure helpers `contract_kind_label`, `instrument_registry_v2_failure_response`): loads v1 via `mqk_md::instrument_registry::load_instrument_registry(&st.instrument_registry_path)`, converts via `convert_v1_registry_to_v2`, tallies the response's counts/maps over the converted `InstrumentRegistryV2`, then calls `validate_registry_v2` to decide `active` vs `v2_validation_failed`. `truth_state` values: `active`, `v1_load_failed`, `v2_validation_failed`, `unavailable` (`v2_conversion_failed` reserved/unreachable — see finding 4).
- `core-rs/crates/mqk-daemon/src/routes.rs` — mounted `GET /api/v1/system/instrument-registry-v2/status` on the public (no-auth) router, next to `/api/v1/system/runtime-leadership`.
- `core-rs/crates/mqk-cli/src/commands/md.rs` + `src/main.rs` — `mqk md registry-v2-status --registry <path>` (`MdCmd::RegistryV2Status`): same load→convert→validate pipeline, prints a flat `key=value` status report, returns `Err` (nonzero exit) on v1-load or v2-validation failure. No DB connection, no provider/broker call — the function's only inputs are the registry path and its only effects are `println!`.

**Tests (16 new; 13 daemon route + 3 CLI):**

- `core-rs/crates/mqk-daemon/tests/scenario_instrument_registry_v2_status_asset_core_01c.rs` (13 tests, `arc01c_01`..`arc01c_13`): real production registry → `active`, `v1_count == v2_count == 88`, `etf_count == 14`, `enabled_non_equity_count == 0`, `paper_trading_enabled_count == live_trading_enabled_count == 0`, `production_cutover_enabled == trading_uses_v2 == false` (both on the active path and on every failure path); `asset_class_counts`/`instrument_kind_counts`/`contract_kind_counts` match the real registry shape exactly (`equity: 88`, `etf: 14`, plain-equity contract: 74); the route is idempotent/side-effect-free across repeated calls; missing registry path → `unavailable`; malformed (unparseable) JSON → `v1_load_failed`; a v1 fixture that parses but converts to an ETF row missing `sector`/`category` → `v2_validation_failed` with the violation message surfaced; the route succeeds with no DB pool configured (proof it requires none and, having none, cannot write through one) and is mounted publicly with no auth.
- `core-rs/crates/mqk-cli/src/commands/md.rs` (3 tests, `rv2_01`..`rv2_03`): `registry-v2-status` succeeds on the real canonical registry; a missing file returns a clear `Err` mentioning "v1 load failed"; an ETF-missing-sector fixture returns a clear `Err` mentioning "v2 validation failed".

**Deliberately not done:** no GUI change of any kind; no change to `tracked_equities_list`, `system_metadata`, or any other existing route's response shape; no change to `config/instruments/equities.json` or any other registry file; no production v2 registry file created; no env flag added to switch any production path to v2; no `Cargo.toml` edited anywhere (both `mqk-daemon` and `mqk-cli` already depend on `mqk-md`); no change to `mqk-md/src/instrument_registry_v2.rs` or `instrument_registry.rs` (both remained read-only for this patch — the small "contract kind" labeling helper was written directly in `routes/system.rs` instead of added to `mqk-md`, keeping that crate untouched); no DB migration; no daemon started for proof (all proof is route/unit-level via `axum::Router::oneshot`, matching the mission's own "testable without starting the daemon" requirement); `.env.local` not read or touched; no provider/broker network calls; no paper/live orders. `ASSET-CORE-01` remains `PARTIAL` — v1 is still the only registry any production trading/ingestion/backtest/GUI path reads.

**Validation:** `cargo test -p mqk-daemon --test scenario_instrument_registry_v2_status_asset_core_01c` — 13/13 pass. `cargo test -p mqk-cli rv2_` — 3/3 pass. `cargo test -p mqk-md instrument_registry_v2` — 26/26 pass (regression, untouched). `cargo test -p mqk-daemon --test scenario_gui_daemon_contract_gate` — 23/23 pass (regression). `cargo test -p mqk-daemon --test scenario_route_contract_rt01` — 2/2 pass (regression; new route correctly absent from the GUI probe manifest, per finding 5). `cargo test -p mqk-daemon --test scenario_ingest_jobs_data_ingest_daemon_01` — 45/45 pass (regression; `tracked_equities_list` untouched). `cargo check -p mqk-md` / `-p mqk-daemon` / `-p mqk-cli` all clean. `cargo clippy -p mqk-daemon --lib -- -D warnings` clean. `cargo clippy -p mqk-daemon --test scenario_instrument_registry_v2_status_asset_core_01c -- -D warnings` clean. `cargo clippy -p mqk-cli --bin mqk-cli -- -D warnings` clean. **Pre-existing, unrelated failure (not fixed, out of scope):** `cargo clippy -p mqk-cli --all-targets -- -D warnings` fails on 3 `clippy::ptr_arg` (`&PathBuf` vs `&Path`) findings in `crates/mqk-cli/tests/scenario_cli_strategy_lab_evaluate.rs` (lines 14/42/67) — a file last modified in commits `4306c8f`/`2ca6aa9`, long before this patch, with zero diff against HEAD from this session. `cargo test -p mqk-cli` (plain, non-clippy) compiles and runs that same file with 0 failures, confirming this is a clippy-lint-only, pre-existing condition unrelated to `ASSET-CORE-01C`.

**Safety confirmation:** no broker submit changes; no Alpaca adapter changes; no live routing changes; no order/outbox writes; no DB migrations; `.env.local` not read or touched; no provider/broker network calls; no paper/live orders submitted; no non-equity asset class enabled anywhere; disabled-asset gates untouched; daemon was not started at any point (all proof is `axum::Router::oneshot` route-level testing); production still reads only the v1 registry (`config/instruments/equities.json` via `AppState::instrument_registry_path`) for every trading/ingestion/backtest/GUI decision. `MiniQuantDesk_Master_Patch_Ledger_v2_updated.md` and `smoke_logs/` were not staged or touched.

**Recommended next slice:** `ASSET-CORE-05` (market calendar/session generalization) — this patch's own read-only status surface is sufficient proof-of-consumption for `InstrumentRegistryV2`; a controlled default-off v2 loader integration (`ASSET-CORE-01D`) has no current operator-facing motivation now that the conversion/validation truth is observable, and would be the right next step only once a real second registry-consuming feature (not just status reporting) is scoped.

### ASSET-CORE-05A — CLOSED_LOCAL / PARTIAL

**Commit:** `37a6440` "daemon: add multi-asset session classification seam"

**Built:**
- additive multi-asset session-classification seam (`mqk-daemon/src/state/market_calendar.rs`)
- equity US regular profile (real, backed by the existing `MarketCalendarProvider`/`MarketSessionState`)
- crypto continuous model-only profile
- futures regular / extended / overnight / closed model-only profile
- forex weekday-continuous / weekend-closed model-only profile
- tests proving Unknown/fail-closed behavior remains intact
- current `MarketCalendarProvider` and `MarketSessionState` preserved unchanged

**Validation:**
- `cargo test -p mqk-daemon --test scenario_market_calendar_session_provider_01` — 20/20 pass
- `cargo test -p mqk-daemon --test scenario_gui_daemon_contract_gate` — 23/23 pass
- `cargo check -p mqk-daemon` — clean
- `cargo clippy -p mqk-daemon --lib -- -D warnings` — clean

**Not done:**
- no production runtime cutover
- no authoritative new holiday/early-close expansion
- no per-instrument session routing
- no DB migration
- no daemon smoke
- no provider/broker calls
- no crypto/futures/forex enablement

`ASSET-CORE-05` remains `PARTIAL`.

**Recommended next slice:** `ASSET-CORE-05B` — authoritative equity calendar / holiday / early-close provider.

### ASSET-CORE-05B-COMBINED — CLOSED_LOCAL / PARTIAL

**Commit:** single combined local commit, message `"daemon: strengthen equity calendar session profiles"` (code + tests + this ledger/audit update, per this patch's explicit one-commit instruction). Hash intentionally not hardcoded here — this entry is part of that commit's own tree, so it cannot self-reference its own resulting hash; see `git log --oneline -1` in the repo for the exact hash.

**Built (Part A — equity calendar authority audit):**
- corrected a stale doc-comment on `NyseWeekdaysProvider` (`mqk-daemon/src/state/market_calendar.rs`) that claimed 2023–2026 coverage; the underlying table in `mqk-integrity::calendar` has covered 2023–2028 since `NYSE-CALENDAR-EXTENSION-AND-EXCHANGE-PROVIDER-01` — this was a documentation-only correction, no behavior change
- audited the existing holiday/early-close table (`core-rs/crates/mqk-integrity/src/calendar.rs`, untouched — out of this patch's file scope): 60 holiday entries (10 named US market holidays × 6 years) and 10 early-close entries (day-after-Thanksgiving every year; Christmas Eve in 2024/2025/2026; Independence Day Eve in 2024) across 2023–2028
- added `EQCAL01`/`EQCAL02` contract tests to `scenario_market_calendar_session_provider_01.rs`: one known full-day holiday and one known early-close date per covered year (2023–2028), closing a real test-coverage gap — 2023 and 2025 previously had no holiday/early-close-specific assertion in this file. All dates reused verbatim from the existing table; none invented.

**Built (Part B — session-profile resolution seam):**
- `SessionProfileResolutionTruth` (`Active` / `UnsupportedAssetClass` / `Unknown`) and `SessionProfileResolution` types, plus pure fn `resolve_session_profile_for_instrument_metadata(asset_class: &str, instrument_kind: Option<&str>) -> SessionProfileResolution` in `mqk-daemon/src/state/market_calendar.rs` (ASSET-CORE-05B section, additive to ASSET-CORE-05A)
- equity (bare or `instrument_kind="etf"`) resolves to `MarketSessionProfile::EquityUsRegular` with `truth_state=Active` (the real, wired profile)
- crypto / `"future"`/`"futures"` / forex resolve to their ASSET-CORE-05A model-only profiles with `truth_state=UnsupportedAssetClass` — known shape, not wired into any runtime or trading path
- `"option"`/`"options"` resolve to `UnsupportedAssetClass` with `profile=None` — no options session calendar is invented; documented as likely to inherit the underlying equity session in a future patch
- unknown and blank/whitespace `asset_class` fail closed to `Unknown` with `profile=None`
- exported from `mqk-daemon::state` re-exports; 8 new `ACS05B01`–`ACS05B08` tests
- skipped the optional read-only metadata route exposure (explicitly optional in the mission) to keep file scope minimal — pure helper + tests only

**Validation:**
- `cargo test -p mqk-daemon --test scenario_market_calendar_session_provider_01` — 30/30 pass (22 pre-existing + 8 new: `EQCAL01`, `EQCAL02`, `ACS05B01`–`ACS05B06` [`ACS05B04`/`06` are table-driven over 2 cases each], `ACS05B07`, `ACS05B08`)
- `cargo test -p mqk-daemon --test scenario_gui_daemon_contract_gate` — 23/23 pass (no regression)
- `cargo check -p mqk-daemon` — clean
- `cargo clippy -p mqk-daemon --lib -- -D warnings` — clean
- `cargo clippy -p mqk-daemon --test scenario_market_calendar_session_provider_01 -- -D warnings` — clean (after factoring a 6-tuple into a named `EarlyCloseCoverageCase` type alias per clippy's `type_complexity` lint)

**Not done:**
- `mqk-integrity/src/calendar.rs` (the actual holiday/early-close table) was not modified — out of this patch's file scope; Part A is audit/contract-proof + doc correction only, not table expansion
- no production runtime cutover; `session_controller.rs`'s `AutonomousSessionSchedule::NyseRegularSession` still calls `mqk_integrity::CalendarSpec::NyseWeekdays.classify_market_session` directly and was not touched
- no per-instrument session routing — `resolve_session_profile_for_instrument_metadata` is not called from any route, gate, or runtime path
- no DB migration, no daemon smoke, no provider/broker calls, no crypto/futures/forex/options enablement

`ASSET-CORE-05` remains `PARTIAL` pending true per-instrument runtime session routing and authoritative non-equity session providers.

**Audit finding (honesty note, not a defect to fix in this patch):** the `MarketCalendarProvider` trait and its implementors (`NyseWeekdaysProvider`, `FixedWindowOverrideProvider`, `ExchangeSourcedCalendarProvider`) in `mqk-daemon/src/state/market_calendar.rs` are consulted only by their own test files — production runtime gating (`session_controller.rs`) depends directly on `mqk_integrity::CalendarSpec::NyseWeekdays`, not on this trait/seam. Both call sites converge on the same underlying holiday/early-close table in `mqk-integrity`, so there is no truth drift today, but the `MarketCalendarProvider` seam itself remains an unconsumed abstraction in production code.

**Recommended next slice:** wire `resolve_session_profile_for_instrument_metadata` as a read-only diagnostic (e.g., a status route) once a second real consumer exists beyond status reporting — or, if multi-asset trading is actually prioritized next, scope true per-instrument runtime session routing as its own patch with its own proof standard (this is explicitly NOT what ASSET-CORE-05B-COMBINED did).

### BACKTEST-MULTIPLIER-MARGIN-01-COMBINED — CLOSED_LOCAL / PARTIAL

**Commit:** single combined local commit, message `"backtest: add multiplier-aware economics seam"` (code + tests + this ledger/audit update, per this patch's explicit one-commit instruction). Hash intentionally not hardcoded here — this entry is part of that commit's own tree, so it cannot self-reference its own resulting hash; see `git log --oneline -1` in the repo for the exact hash.

**Repo evidence found before writing any code:**
- backtest realized P&L is not computed in `mqk-backtest` at all — `BacktestEngine::run` calls `mqk_portfolio::apply_fill` (`mqk-portfolio/src/accounting.rs`'s `buy_fifo`/`sell_fifo`), which computes `(price_a - price_b) * qty` with multiplier implicitly `1`
- backtest unrealized/equity-curve P&L is likewise delegated: `compute_equity_micros` / `compute_exposure_micros` / `compute_unrealized_pnl_micros` (`mqk-portfolio/src/metrics.rs`), same implicit multiplier-1 `qty * mark` math
- `mqk-portfolio` is the **same accounting engine used by the live/paper path** — `mqk-runtime/src/orchestrator.rs` and `orchestrator/apply.rs` call the identical `apply_fill`/`compute_equity_micros` functions — so any change to existing `mqk-portfolio` function bodies would be a live-accounting change, not a backtest-only one
- `ContractDefinitionV2::Future`/`Option` already carry a validated `multiplier: i64` field (`mqk-md/src/instrument_registry_v2.rs`, from `ASSET-CORE-01B`), but it is registry-validation-only — zero consumers in any P&L, notional, or accounting path
- no existing margin concept anywhere in the repo (the only prior "margin" hits are Alpaca's unrelated `marginable: Option<bool>` asset flag)

**Built:**
- new pure module `core-rs/crates/mqk-backtest/src/economics.rs` (not `mqk-portfolio` — kept structurally unreachable from the live path, since `mqk-runtime` does not depend on `mqk-backtest`)
- `BacktestInstrumentEconomics { contract_multiplier: i64, initial_margin_micros: Option<i64>, maintenance_margin_micros: Option<i64> }`; `::equity()` (multiplier=1, infallible) and `::new(multiplier, initial_margin, maintenance_margin) -> Result<Self, EconomicsError>` (fails closed on multiplier <= 0)
- three pure helpers, each saturating through `i128` (no panic/wrap on extreme i64 inputs — a 3-factor `qty * price * multiplier` product can exceed `i128` range, unlike the existing 2-factor `qty * price` math): `notional_micros`, `mark_to_market_value_micros`, `realized_pnl_micros`
- margin fields are carried as metadata only — no function reads them; `bmm05` proves this explicitly
- re-exported from `mqk-backtest::lib` (`mod economics;` + `pub use economics::{...}`); zero existing files modified inside `mqk-backtest` or `mqk-portfolio`
- **not wired into `BacktestEngine`** — this is a standalone, additively-tested foundation only; the engine's existing inline notional clamp and all `mqk-portfolio` calls are untouched, so there is no possible behavior drift to prove against

**Equity multiplier=1 preservation proof (`bmm01`):**
- `notional_micros`/`realized_pnl_micros`/`mark_to_market_value_micros` at multiplier=1 are checked against the exact fixture values from `mqk-portfolio/tests/scenario_pnl_partial_fills_fifo.rs` (sell 5 @ 120 closing a 100-entry long → realized 100; 15 shares @ mark 115 → market value 1,725 → equity 100,225) — same numbers, same results

**Synthetic futures/options multiplier proof (`bmm02`/`bmm03`):**
- futures-style multiplier 50: notional, long-side realized P&L, short-side realized P&L (sign-convention proof), and mark-to-market all scale correctly
- options-style multiplier 100: notional and realized P&L scale correctly

**Validation:**
- `cargo test -p mqk-backtest --lib bmm` — 14/14 new tests pass
- `cargo test -p mqk-backtest` — full crate suite passes, no regression (includes unchanged `scenario_allocation_cap_enforced.rs`, which exercises the same notional-clamp shape this patch left untouched)
- `cargo test -p mqk-portfolio` — full crate suite passes (untouched crate, confirmed unaffected)
- `cargo check -p mqk-backtest` / `cargo check -p mqk-portfolio` — clean
- `cargo clippy -p mqk-backtest --all-targets -- -D warnings` / `cargo clippy -p mqk-portfolio --all-targets -- -D warnings` — clean

**Not done:**
- no wiring into `BacktestEngine` (no per-symbol economics config, no `BacktestConfig` field, no API/CLI change)
- no futures/options registry entries, no contract roll logic, no options assignment/expiration
- no margin enforcement (margin is `Option<i64>` metadata only, read by nothing)
- no live/paper portfolio accounting change — `mqk-portfolio` was not modified
- no broker/provider/DB/daemon path touched; no non-equity asset class enabled

`BACKTEST-MULTIPLIER-MARGIN-01` remains `PARTIAL` — this combined patch is the foundation seam only (pure helpers + proof), not engine wiring or asset-class enablement.

**Recommended next slice:** wire `BacktestInstrumentEconomics` into `BacktestEngine` for a single explicitly-flagged synthetic instrument path (e.g. an opt-in per-run economics override defaulting to `equity()`), proven against a full synthetic futures-style backtest run rather than pure-helper unit tests alone — only after that wiring exists does per-instrument multiplier selection (reading `ContractDefinitionV2::multiplier` from the registry) become a meaningful next step.

### BACKTEST-MULTIPLIER-RUN-WIRE-01-COMBINED — CLOSED_LOCAL / PARTIAL

**Commit:** single combined local commit, message `"backtest: wire multiplier economics into run path"` (code + tests + this ledger/audit update). Hash intentionally not hardcoded here — this entry is part of that commit's own tree; see `git log --oneline -1` for the exact hash.

**Repo evidence found before writing any code:**
- `BacktestEngine::run` calls `mqk_portfolio::apply_fill` (cash + FIFO realized P&L) and `compute_equity_micros`/`compute_exposure_micros` (equity curve, allocation-cap exposure) directly — confirmed still the same functions the live/paper path calls via `mqk-runtime::orchestrator`; no change was made to any of them.
- The allocation-cap notional check (PATCH 13, in the per-intent loop) computed `qty * fill_price` inline with no multiplier — this is the one place pre-existing engine code computed notional itself rather than delegating to `mqk-portfolio`.
- `BacktestConfig` looked like a clean place for an optional `economics` field, but one exhaustive struct literal outside this patch's allowed scope — `mqk-daemon/tests/scenario_backtest_jobs_01.rs:690` (every field named, no `..base` spread) — would fail to compile the moment a new field was added. (`mqk-testkit/tests/scenario_corp_act_01.rs` and every in-scope `mqk-backtest/tests/*` literal use `..BacktestConfig::test_defaults()` and would have been fine.)
- `BacktestReport` has the same problem one level worse: `mqk-artifacts/src/lib.rs`'s own test module exhaustively constructs `BacktestReport { .. }` literals (`test_report_with_orders`, `make_report_no_fills`), and `mqk-artifacts` is not even in this patch's read-only-unless-necessary list.
- Conclusion: neither config-side nor report-side struct gains a field without an undisclosed out-of-scope edit. Per this patch's own fallback clause ("if no clean config exists, add a minimal constructor or builder method"), economics is wired as an opt-in builder method on `BacktestEngine` instead — `BacktestConfig` and `BacktestReport` are both untouched, byte-for-byte, by this patch.

**Built (all inside `mqk-backtest`):**
- `BacktestEconomicsLedger` (new, crate-private, in `economics.rs`): a backtest-only shadow ledger that mirrors `mqk_portfolio::accounting::{apply_fill, buy_fifo, sell_fifo}` 1:1 in control flow (same FIFO consumption order, same cash/lot shape), but every notional/P&L term goes through the already-proven `notional_micros`/`realized_pnl_micros`/`mark_to_market_value_micros` helpers instead of raw `qty * price`. Never reads from or writes to `mqk_portfolio::PortfolioState`.
- `BacktestEngine::with_economics(self, economics: BacktestInstrumentEconomics) -> Self` — opt-in builder, called as `BacktestEngine::new(cfg).with_economics(econ)`. Engines that never call it default to `BacktestInstrumentEconomics::equity()` (multiplier=1), reproducing today's behavior exactly.
- `BacktestEngine::economics()`, `::economics_equity_curve()`, `::economics_realized_pnl_micros()` — read-only accessors, callable after `run()` returns.
- `BacktestEngine::validate_economics()` (mirrors the existing `validate_stress_profile()` pattern) + new `BacktestError::InvalidEconomics { multiplier }` variant, checked at the very top of `run()` — before any bar is touched or any `BacktestReport` produced. Defense-in-depth: catches an invalid multiplier even if a caller bypasses the validating `BacktestInstrumentEconomics::new()` constructor via direct struct literal (its fields are public).
- The PATCH 13 allocation-cap notional check now calls `economics::notional_micros(intent.qty, fill_price, &self.economics)` instead of the old inline `qty * fill_price` clamp. Exactly equal to the old formula at multiplier=1 (proven directly in `economics.rs`'s pre-existing `bmm01` tests); scales correctly above 1.
- Both fill-application sites (intent fills in the main loop, and risk-halt flatten fills in `flatten_all`) now also call `self.economics_ledger.apply_fill(...)` with the same symbol/side/qty/price/fee passed to the existing `mqk_portfolio::apply_fill` call — `self.portfolio` itself is untouched by the new code.
- Tests: 5 new ledger-level unit tests in `economics.rs` (`bmw_ledger_*`) + 8 new engine-level scenario tests in `tests/scenario_multiplier_run_wire_01.rs` (`bmw01`, `bmw01b`, `bmw02`, `bmw02b`, `bmw02c`, `bmw03`, `bmw03b`, `bmw04`).

**Multiplier-aware outputs (proven):**
- Allocation-cap enforcement notional — `bmw02c`: qty=100 @ $100 ($10,000 notional) fits a $20,000 cap at multiplier=1 (fill recorded) but breaches the same $20,000 cap at multiplier=50 for the identical share count (order rejected, zero fills).
- `BacktestEngine::economics_realized_pnl_micros()` — `bmw02`/`bmw02b`: a 10-share buy@$4,500/sell@$4,510 round trip realizes exactly $100 at multiplier=1, $5,000 at multiplier=50, $10,000 at multiplier=100.
- `BacktestEngine::economics_equity_curve()` — parallel curve; the final-bar equity gain over initial cash scales identically ($100 / $5,000 / $10,000).
- `bmw_ledger_matches_portfolio_state_at_multiplier_one` (in `economics.rs`) directly replays the same fills through both `mqk_portfolio::apply_fill`/`compute_equity_micros` and `BacktestEconomicsLedger` and asserts byte-identical cash, realized P&L, and equity at multiplier=1 — the strongest available proof that the new seam is a true superset of existing behavior, not an approximation of it.

**Outputs still multiplier=1 / explicitly unwired:**
- `BacktestReport.equity_curve` — the field daemon/CLI/`mqk-artifacts` actually read — is still produced by `mqk_portfolio::compute_equity_micros` against the real, un-multiplied `PortfolioState`, unchanged by design (`mqk-portfolio` was not modified). The multiplier-aware curve exists only on the engine object (`engine.economics_equity_curve()`), not on `BacktestReport`.
- `BacktestReport` gained zero new fields (deliberately, per the repo evidence above), so `metrics.json`/`manifest.json`/`report.md` (written by `mqk-artifacts::write_backtest_report`) do not surface `contract_multiplier` or margin metadata anywhere yet. No artifact reports economics metadata at all — truthfully or otherwise — in this patch.
- `BacktestConfig::config_id()` / `BacktestReport::run_id` do not encode economics (economics lives on `BacktestEngine` via `with_economics`, not on `BacktestConfig`). Two runs over identical bars/config/strategy but different economics currently produce the **same** `run_id`/`config_id` — a real determinism/identity gap, not just a metadata one.
- Margin fields remain metadata-only — carried faithfully through `BacktestEngine::economics()` (round-trip proven by `bmw04`) but read by nothing; no enforcement, same status as the parent patch.
- No daemon route, CLI command, or config/report field was touched, so there is no operator-facing (CLI/GUI) way to run a multiplier-aware backtest yet — only a direct Rust caller using `BacktestEngine::new(cfg).with_economics(econ)` can reach this seam.

**Validation:**
- `cargo test -p mqk-backtest --test scenario_multiplier_run_wire_01` — 8/8 new engine-level tests pass.
- `cargo test -p mqk-backtest` — full crate suite (28 lib unit tests + 26 scenario/integration test binaries) passes, zero regressions, including the now-modified `scenario_allocation_cap_enforced.rs` path.
- `cargo check -p mqk-backtest` — clean.
- `cargo clippy -p mqk-backtest --all-targets -- -D warnings` — clean.
- Daemon/CLI checks were intentionally **not** run — neither `BacktestConfig`, `BacktestReport`, nor any daemon/CLI file was touched (see repo evidence above), so per this patch's own instruction ("if no daemon/CLI touched, do not run those just for noise") they would add no signal.

**Not done (explicit):**
- No `BacktestConfig`/`BacktestReport` field, no daemon route, no CLI command, no `mqk-artifacts` change — economics is reachable only via `BacktestEngine::with_economics` today.
- No margin enforcement (still `Option<i64>` metadata only, read by nothing).
- No live/paper portfolio accounting change — `mqk-portfolio` was not modified.
- No futures/options registry, broker, execution, or DB/migration change; no non-equity asset class enabled.
- No `run_id`/`config_id` sensitivity to economics.

`BACKTEST-MULTIPLIER-MARGIN-01` remains `PARTIAL` — this patch wires the engine seam and proves full-run multiplier-aware notional/P&L/equity behavior, but the artifact/report/config-identity layer (`metrics.json`, `manifest.json`, `run_id`) is still entirely multiplier=1/unaware, and there is still no CLI/daemon/GUI path to use it.

**Recommended next slice:** thread `with_economics` through to an operator-facing surface — either (a) fix the one out-of-scope exhaustive `BacktestConfig` literal in `mqk-daemon/tests/scenario_backtest_jobs_01.rs` as its own tiny, explicitly-scoped patch so a `BacktestConfig.economics` field becomes safe to add, or (b) add a narrow CLI flag (e.g. `--contract-multiplier`) that calls `.with_economics(...)` directly without touching `BacktestConfig` at all — then fold `contract_multiplier`/margin metadata into `BacktestReport`/`metrics.json`/`manifest.json` and into `config_id()`/`run_id` so replay identity is sensitive to economics.

### BACKTEST-ECONOMICS-CONFIG-READY-01 — CLOSED_LOCAL / PARTIAL

**Commit:** single local commit, message `"backtest: make economics config additions safe"` (code + tests + this ledger/audit update). Hash intentionally not hardcoded here — this entry is part of that commit's own tree; see `git log --oneline -1` for the exact hash.

**Mission:** close the exact blocker `BACKTEST-MULTIPLIER-RUN-WIRE-01-COMBINED` identified directly above: exhaustive `BacktestConfig`/`BacktestReport` struct literals outside `mqk-backtest` that would fail to compile the moment either struct gained a new field. This patch adds no economics field — it only removes/isolates the literals that were blocking one, per its own explicit no-bundling instruction.

**Repo evidence found before writing any code (current HEAD, not prior session claims):**
- `BacktestConfig::test_defaults()` and `BacktestConfig::conservative_defaults()` already exist (`mqk-backtest/src/types.rs`) and are already used correctly via `..BacktestConfig::test_defaults()` spread syntax in `mqk-backtest/tests/scenario_corporate_action_policy.rs` and `mqk-testkit/tests/scenario_corp_act_01.rs` — these were false leads, not blockers.
- Two **additional, previously undocumented** exhaustive `BacktestConfig { .. }` literals (all 19 fields named, no `..` spread) were found in `mqk-backtest/tests/`: `scenario_backtest_live_semantics_alignment.rs`'s `alignment_config()` and `scenario_stale_data_stops_execution.rs`'s `config_with_integrity()`. The prior patch's note above ("every in-scope `mqk-backtest/tests/*` literal use `..BacktestConfig::test_defaults()`") was incomplete — repo evidence overrides that claim per this repo's audit rules.
- The exact blocker the prior patch named — `mqk-daemon/tests/scenario_backtest_jobs_01.rs:690`, a 19-field exhaustive `BacktestConfig` literal inside `run_daily_weekend_gap_with_stale_threshold()` — was confirmed present and unchanged at current HEAD.
- `BacktestReport` has **no** constructor, builder, or `Default` impl at all (unlike `BacktestConfig`). Its sole production constructor is the one real, fully-explicit literal in `mqk-backtest/src/engine.rs::run()` — correct and left untouched.
- The two exhaustive `BacktestReport { .. }` test-fixture literals the prior patch named in `mqk-artifacts/src/lib.rs`'s own `#[cfg(test)] mod tests` (`test_report_with_orders()`, `make_report_no_fills()`) were confirmed present.
- **New finding, outside this patch's strict file scope:** `mqk-promotion/tests/` contains **seven** additional exhaustive `BacktestReport { .. }` literals across six files (`scenario_fail_below_threshold.rs` ×2, `scenario_promotion_requires_partial_fill_stress.rs` ×3, `scenario_pass_above_threshold.rs`, `scenario_backtest_to_promotion_pipeline.rs`, `scenario_golden_artifact_hash_lock.rs`). `mqk-promotion` was not named in this patch's allowed/read-only/forbidden file lists, so per "minimal scope only" / "do not touch files outside the patch's stated scope" it was **left untouched**. This is now the single largest remaining blocker to adding a `BacktestReport` field — see Recommended next slice.

**Built:**
- `BacktestReport::test_fixture() -> Self` added to `mqk-backtest/src/types.rs`, mirroring `BacktestConfig::test_defaults()`'s convention exactly: a named, explicitly-documented "test only, never production" constructor, not a `std::default::Default` impl — avoiding any implicit/silent empty-report default ever being reachable from non-test code, consistent with this repo's no-fabricated-truth posture. Returns zero/empty/nil values for all 14 fields. Automatically available wherever `BacktestReport` is already used (inherent impl on an already-`pub use`-exported type; no `lib.rs` change needed).
- Converted the two newly found `mqk-backtest/tests/` `BacktestConfig` literals and the one `mqk-daemon/tests/scenario_backtest_jobs_01.rs` literal to `..BacktestConfig::test_defaults()` spread syntax, keeping only each test's actually-overridden fields. Trimmed now-unused local imports (`StressProfile` in two `mqk-backtest` test files; `CommissionModel`, `CorporateActionPolicy`, `StrategySizingConfig`, `StressProfile` from the daemon test's function-scoped `use`).
- Converted both `mqk-artifacts/src/lib.rs` `BacktestReport` literals to `..BacktestReport::test_fixture()` spread syntax, keeping only each fixture's actually-distinguishing fields.
- Field-by-field diff confirms every rewritten literal produces byte-identical values to the original — this is a mechanical de-sugaring, not a behavior change.

**Construction/default pattern added:** Option B from this patch's own preferred-design list (functional-update spread against an explicit named constructor), applied symmetrically to `BacktestConfig` (already had `test_defaults()`) and `BacktestReport` (gained `test_fixture()`). No `std::default::Default` trait impl was added to either type, by design.

**Proof existing behavior is unchanged:**
- `cargo test -p mqk-backtest` — full suite, 28 lib unit tests + every scenario/integration binary, 0 failures, including `scenario_backtest_live_semantics_alignment` (9 tests) and `scenario_stale_data_stops_execution` (6 tests) — the two rewritten files.
- `cargo test -p mqk-daemon --test scenario_backtest_jobs_01` — 12/12 pass, including `bj_d04_daily_weekend_gap_not_blocked_by_default_stale_threshold` (the test exercising the rewritten literal).
- `cargo test -p mqk-artifacts` — 32/32 pass (13 lib + 6 e2e + 13 strategy_lab_artifact), including both CSV-header tests that consume the rewritten fixtures.
- `cargo check -p mqk-backtest` / `cargo check -p mqk-daemon` / `cargo check -p mqk-artifacts` — all clean.
- `cargo clippy -p mqk-backtest --all-targets -- -D warnings` — clean.
- `cargo clippy -p mqk-artifacts --lib -- -D warnings` — clean (isolates the touched `src/lib.rs`). `cargo clippy -p mqk-artifacts --all-targets -- -D warnings` fails on a **pre-existing, unrelated** `clippy::ptr_arg` finding in `tests/strategy_lab_artifact.rs` (lines 22/56, `&PathBuf` should be `&Path`) — confirmed via `git diff --name-only` that this file was not touched by this patch; flagged separately as its own follow-up, not fixed here.

**Proof no economics output behavior changed:** zero lines in `mqk-backtest/src/engine.rs`, `mqk-backtest/src/economics.rs`, or any economics code path were touched. No field was added to `BacktestConfig` or `BacktestReport`. `BacktestEngine::with_economics`/`economics()`/`economics_equity_curve()`/`economics_realized_pnl_micros()` are all unmodified.

**Proof no daemon/CLI request JSON shape changed:** `mqk-daemon/src/routes/backtests.rs` and `mqk-cli/src/commands/*` were not modified and do not appear in `git diff --name-only`. The only daemon file touched is a `#[cfg(test)]`-only integration test (`tests/scenario_backtest_jobs_01.rs`), which has no bearing on any API/CLI request or response shape.

**Unrelated/pre-existing failures and handling:** `clippy::ptr_arg` in `mqk-artifacts/tests/strategy_lab_artifact.rs:22,56` — pre-existing (file untouched by this patch), unrelated to backtest config/report construction safety. Flagged as a standalone follow-up task rather than fixed inline, to keep this patch's diff to its stated scope.

**Not done (explicit):**
- No economics field added to `BacktestConfig`, `BacktestReport`, `metrics.json`, `manifest.json`, or `config_id()`/`run_id` — this patch is construction-safety preparation only, exactly as scoped.
- `mqk-promotion/tests/` (7 exhaustive `BacktestReport` literals across 6 files) was **not** de-risked — it was outside this patch's strict file scope and is now the largest concrete blocker remaining before a `BacktestReport` field can be added without an additional, separately-scoped patch.
- No daemon route, CLI command, GUI, DB migration, broker/provider call, or live/paper trading path was touched.
- `config_id()`/`run_id` are byte-for-byte unchanged in formula.

`BACKTEST-MULTIPLIER-MARGIN-01` remains `PARTIAL`. This patch only removes the construction-safety blocker that `BACKTEST-MULTIPLIER-RUN-WIRE-01-COMBINED` identified for `mqk-backtest`/`mqk-daemon`/`mqk-artifacts`; it does not add economics fields, and a comparably-sized blocker now confirmed in `mqk-promotion/tests/` still stands between today's repo and a safe `BacktestReport` field addition.

**Recommended next slice:** either (a) a narrowly-scoped follow-up patch that applies the same `..BacktestReport::test_fixture()` treatment to the seven literals now found in `mqk-promotion/tests/` (mechanical, same pattern, low risk — this is the last remaining `BacktestReport` blocker), after which a `contract_multiplier`/margin field could be added to `BacktestReport` with reasonable confidence, or (b) proceed directly to adding the `BacktestConfig.economics`-equivalent field now that `mqk-backtest`/`mqk-daemon` are de-risked, accepting that `mqk-promotion`'s `BacktestReport` literals would need fixing in the same patch that adds a `BacktestReport` field (not before).

**Full detail, exact test names, and validation commands:** this entry (above) is the full detail; see `git log --oneline -1` in the repo for the exact commit hash.

### BACKTEST-REPORT-FIXTURE-READY-01-COMBINED — CLOSED_LOCAL / PARTIAL

**Commit:** single local commit, message `"test: make promotion backtest report fixtures future-safe"` (test-only diff + this ledger update). Hash intentionally not hardcoded here — this entry is part of that commit's own tree; see `git log --oneline -1` for the exact hash.

**Mission:** close the exact blocker `BACKTEST-ECONOMICS-CONFIG-READY-01` identified directly above — the remaining exhaustive `BacktestReport { .. }` literals in `mqk-promotion/tests/` — by routing them through `BacktestReport::test_fixture()`. Mechanical test-surface cleanup only; no economics field added, no promotion/backtest behavior changed.

**Repo evidence found before writing any code (current HEAD, not the prior entry's count):**
- The prior entry's count ("seven... across six files", naming five files) does not match current HEAD. Direct `rg`/`grep` enumeration found **nine** exhaustive `BacktestReport { .. }` literals across **six** files: `scenario_golden_artifact_hash_lock.rs` ×1, `scenario_tie_break_correctness.rs` ×1 (not named in the prior entry at all), `scenario_backtest_to_promotion_pipeline.rs` ×1, `scenario_pass_above_threshold.rs` ×1, `scenario_promotion_requires_partial_fill_stress.rs` ×3, `scenario_fail_below_threshold.rs` ×2. Per this repo's audit rules, current repo state overrides the prior doc's claim; the higher count is what was actually fixed.
- `BacktestReport::test_fixture()` (added by `BACKTEST-ECONOMICS-CONFIG-READY-01`) was confirmed present in `mqk-backtest/src/types.rs` and already reachable from `mqk-promotion/tests` via the existing `use mqk_backtest::{..., BacktestReport}` imports in every affected file — no new import needed.
- Every one of the nine literals followed the same shape: a handful of meaningful, test-specific fields (`strategy_name`, computed `run_id`/`config_id`/`input_data_hash`, `equity_curve`, and in some cases `fills`/`halted`/`halt_reason` threaded through as function parameters) plus a fixed tail of fields whose literal value was identical to `BacktestReport::test_fixture()`'s default (`halted: false`, `halt_reason: None`, `orders: vec![]`, sometimes `fills: vec![]`, `last_prices: BTreeMap::new()`, `execution_blocked: false`, `first_bar_open_micros: None`, `last_bar_close_micros: None`, `sizing: StrategySizingConfig::default_sizing()`).
- No literal asserted behavior that depended on a field being absent/defaulted in a way the spread would change — every dropped field's value is byte-identical to the fixture default it now inherits.

**Built:**
- Rewrote all nine literals (in `scenario_golden_artifact_hash_lock.rs`, `scenario_tie_break_correctness.rs`, `scenario_backtest_to_promotion_pipeline.rs`, `scenario_pass_above_threshold.rs`, `scenario_promotion_requires_partial_fill_stress.rs` ×3, `scenario_fail_below_threshold.rs` ×2) to keep only each test's actually-distinguishing fields plus `..BacktestReport::test_fixture()`. Dropped fields whose literal value matched the fixture default exactly; kept every field whose value was computed, parametrized, or otherwise meaningful to the test.
- Removed the now-unused `use std::collections::BTreeMap;` import from all six files (its only use in each file was the dropped `last_prices: BTreeMap::new()` field).
- No new helper functions added; existing per-file helpers (`good_report()`, `report_with_provenance()`, `make_report_with_provenance()`) were edited in place.

**Construction/default pattern used:** same Option B pattern as `BACKTEST-ECONOMICS-CONFIG-READY-01` (functional-update spread against `BacktestReport::test_fixture()`), applied to the six `mqk-promotion/tests` files that patch left untouched by scope.

**Proof existing behavior is unchanged:**
- `cargo test -p mqk-promotion` — 0 lib tests + 40 integration tests across all 7 test binaries (3 + 2 + 11 + 14 + 1 + 6 + 3), 0 failures. Every test in every file containing a rewritten literal passes unchanged.
- `cargo check -p mqk-promotion` — clean.
- `cargo clippy -p mqk-promotion --all-targets -- -D warnings` — clean (no `clippy::needless_update`, confirming every rewritten literal still has at least one field genuinely sourced from the spread).
- `cargo check -p mqk-backtest` / `cargo test -p mqk-backtest` / `cargo clippy -p mqk-backtest --all-targets -- -D warnings` — all clean (upstream fixture constructor itself untouched by this patch; re-run only to confirm it still resolves correctly from a downstream crate).
- `mqk-artifacts` was not touched (confirmed via `git diff --name-only`) and its suite was not re-run, per this patch's own instruction to avoid noise.

**Proof no `BacktestReport` fields were added:** `mqk-backtest/src/types.rs` (the struct definition and `test_fixture()` impl) does not appear in `git diff --name-only` for this patch — read-only, as scoped.

**Proof no economics output behavior changed:** zero lines in `mqk-backtest/src/engine.rs` or `mqk-backtest/src/economics.rs` were touched; this patch's diff is entirely contained in `mqk-promotion/tests/*`.

**Proof no promotion logic changed:** `mqk-promotion/src/*` does not appear in `git diff --name-only` — only `mqk-promotion/tests/*` files were edited.

**Proof no artifact/config_id/run_id behavior changed:** every rewritten literal still computes `run_id`/`config_id`/`input_data_hash` via the same `derive_run_id`/`derive_input_data_hash`/`BacktestConfig::test_defaults().config_id()` calls as before, unmodified; `mqk-artifacts/src/lib.rs` was not touched.

**Unrelated/pre-existing findings:** none newly found. The previously-flagged `clippy::ptr_arg` in `mqk-artifacts/tests/strategy_lab_artifact.rs` was already fixed in a separate prior commit (`test: fix strategy lab artifact clippy ptr arg lint`) before this patch started, confirmed via the clean-tree precondition check.

**Not done (explicit):**
- No `BacktestReport` field added.
- No `BacktestConfig` field added.
- No change to `metrics.json` / `manifest.json` / `report.md` output format, `config_id()`, or `run_id` behavior.
- No promotion scoring/gating/ranker/artifact logic changed.
- No daemon, CLI, GUI, runtime, broker, DB, or live/paper trading path touched.

`BACKTEST-MULTIPLIER-MARGIN-01` remains `PARTIAL`. This patch closes the last concrete `BacktestReport` construction-safety blocker (`mqk-promotion/tests/`); a `contract_multiplier`/margin field can now be added to `BacktestReport` without breaking any known exhaustive-literal call site in the workspace.

**Recommended next slice:** add the actual economics/margin field to `BacktestReport` (e.g. `contract_multiplier` or equivalent), now that both `mqk-backtest`/`mqk-daemon`/`mqk-artifacts` (via `BACKTEST-ECONOMICS-CONFIG-READY-01`) and `mqk-promotion/tests` (via this patch) are de-risked. That follow-up should re-grep the full workspace for any exhaustive `BacktestReport { .. }` literal once more immediately before adding the field, since this repo's audit rules require verifying current HEAD rather than trusting either ledger entry's literal count.

**Full detail, exact test names, and validation commands:** this entry (above) is the full detail; see `git log --oneline -1` in the repo for the exact commit hash.

---

### BACKTEST-REPORT-ECONOMICS-ARTIFACT-01-COMBINED — CLOSED_LOCAL / PARTIAL

**Commit:** single local commit, message `"backtest: surface economics in reports"` (code + tests + this ledger update). Hash intentionally not hardcoded here — this entry is part of that commit's own tree; see `git log --oneline -1` for the exact hash.

**Mission:** add the first official backtest report/artifact economics surface, now that `BACKTEST-REPORT-FIXTURE-READY-01-COMBINED` removed every exhaustive `BacktestReport { .. }` literal blocking a new field. Make multiplier/margin economics visible and identity-sensitive in backtest reports/artifacts while preserving default equity output exactly. Backtest-only; no live/paper/runtime/broker/DB/non-equity path touched.

**Repo evidence found before writing any code (current HEAD):**
- `BacktestEngine::run` (`mqk-backtest/src/engine.rs`) built `BacktestReport` via one exhaustive field-by-field literal (the only exhaustive `BacktestReport { .. }` construction site in the whole workspace — confirmed by `rg -n "BacktestReport \{"` across `core-rs/crates`). Every other construction site (`mqk-artifacts/src/lib.rs` test helpers, all six `mqk-promotion/tests/*` files, all `mqk-backtest/tests/*` helpers) used `..BacktestReport::test_fixture()`.
- `report.equity_curve` was assigned directly from `self.equity_curve` (the `mqk_portfolio`-driven, multiplier-unaware curve) — confirmed the "known remaining gap" from `BACKTEST-MULTIPLIER-RUN-WIRE-01-COMBINED`. `self.economics_equity_curve` (multiplier-aware, computed in parallel since that patch) was tracked on the engine but never read by `run()`'s report construction.
- Two existing tests in `mqk-backtest/tests/scenario_multiplier_run_wire_01.rs` (`bmw02_multiplier_50_scales_full_run_outputs`, `bmw02b_multiplier_100_scales_full_run_outputs`) asserted `report1.equity_curve == report50.equity_curve` / `== report100.equity_curve` — i.e. they actively pinned the gap as "current correct behavior". Closing the gap required updating these two assertions; documented and proven below, not silently bypassed.
- `BacktestConfig::config_id()` (`mqk-backtest/src/types.rs`) is a pure function of `BacktestConfig` fields only; economics lives on `BacktestEngine` (via `with_economics`), not on `BacktestConfig` (a deliberate prior-patch boundary — see `BACKTEST-MULTIPLIER-RUN-WIRE-01-COMBINED` memory). Widening `BacktestConfig` itself was out of scope and unnecessary.
- `mqk-artifacts::write_backtest_report` (not `init_run_artifacts`) is the only artifact writer that receives the full `BacktestReport` after `engine.run()` completes; `metrics.json` and `report.md` are entirely computed inside it. `manifest.json` is written earlier by `init_run_artifacts`, called from `mqk-cli/src/commands/bkt.rs` with only `run_id`/`config_hash`/`git_hash`/`host_fingerprint` — adding economics there would require new `InitRunArtifactsArgs` fields and CLI call-site changes, which the patch's own mission text discourages absent proof of necessity.
- `mqk-promotion/src/evaluator.rs` and `mqk-backtest/src/sweep.rs` only ever read named `BacktestReport` fields (`report.equity_curve`, `report.fills`, `report.run_id`, etc.) — no exhaustive destructuring (`let BacktestReport { .. } = ...`) exists anywhere in the workspace, confirmed by a dedicated `rg` pattern search that returned zero matches.

**Built:**
- `mqk-backtest/src/economics.rs`: added `BacktestInstrumentEconomics::is_default_equity(&self) -> bool` (`pub(crate)`, `*self == Self::equity()`); added `BacktestEconomicsReport` struct (`contract_multiplier`, `initial_margin_micros`, `maintenance_margin_micros`, `realized_pnl_micros`, `margin_enforced: bool`, always `false` — margin remains metadata-only, matching the module's existing no-enforcement contract) with `equity()` and `from_run(economics, realized_pnl_micros)` constructors. Added 7 new unit tests (`bmm07`/`bmm07b`/`ber01`–`ber03`).
- `mqk-backtest/src/types.rs`: added `derive_run_id_with_economics(strategy_name, config_id, input_data_hash, economics) -> Uuid` next to `derive_run_id`. Returns `derive_run_id(...)` unchanged (byte-identical `v2` UUID) when `economics.is_default_equity()`; otherwise hashes a distinct `mqk-bkt.run.v3|...|mult=..|im=..|mm=..` string, which can never collide with a `v2` digest because the version prefix itself differs. Added `economics: BacktestEconomicsReport` field to `BacktestReport`, defaulted to `BacktestEconomicsReport::equity()` in `test_fixture()`.
- `mqk-backtest/src/engine.rs`: `run()`'s only exhaustive `BacktestReport { .. }` literal now (a) derives `run_id` via `derive_run_id_with_economics`, (b) selects `equity_curve` as `self.equity_curve.clone()` when `contract_multiplier == 1` (byte-identical to the pre-existing path, unconditionally, every time) or `self.economics_equity_curve.clone()` otherwise, and (c) sets the new `economics` field via `BacktestEconomicsReport::from_run(&self.economics, self.economics_realized_pnl_micros())`.
- `mqk-backtest/src/lib.rs`: re-exported `BacktestEconomicsReport` and `derive_run_id_with_economics`.
- `mqk-artifacts/src/lib.rs`: added `EconomicsSection` (serde `Serialize`, mirrors the existing `SizingSection`/`BenchmarkSection` pattern) with the same 5 fields as `BacktestEconomicsReport`; added `economics: EconomicsSection` to `BacktestMetrics` (additive, `schema_version` stays `1`); populated truthfully from `report.economics.*` in `write_backtest_report`; added an "## Instrument Economics" section to `build_report_md` (multiplier, margins, realized P&L, enforcement flag, plus a one-line note on which equity-curve semantics apply). `manifest.json` was deliberately **not** touched — see evidence above.
- `mqk-backtest/tests/scenario_multiplier_run_wire_01.rs`: extended `bmw01b_explicit_multiplier_one_equals_default` with `run_id`/`config_id`/`economics` equality assertions; extended `bmw04_economics_metadata_round_trips_truthfully_and_margin_is_inert` to capture and assert the report-level margin round-trip and `run_id` divergence between bare and margin-bearing economics at the same multiplier; **fixed** `bmw02_multiplier_50_scales_full_run_outputs` and `bmw02b_multiplier_100_scales_full_run_outputs` to assert the new intended behavior (`report.equity_curve == engine.economics_equity_curve()`, and `report1.equity_curve != report50/100.equity_curve`) instead of the old gap-pinning equality.
- New `mqk-backtest/tests/scenario_report_economics_artifact_01.rs`: `brea01_default_equity_report_preserves_output`, `brea02_multiplier_50_report_curve_is_economics_aware`, `brea02b_multiplier_100_report_curve_is_economics_aware`, `brea04_config_identity_includes_non_default_economics`, `brea05_backtest_report_literals_are_fixture_safe`.
- New `mqk-artifacts/tests/scenario_report_economics_artifact_metadata.rs`: `brea03a_default_equity_metrics_json_economics_is_truthful`, `brea03b_multiplier_50_metrics_json_economics_is_truthful`, `brea03c_metrics_json_schema_version_unchanged`.

**Identity design decision (explicit):** `config_id` stays a pure function of `BacktestConfig` only — unchanged, not economics-sensitive, because economics is not (and was deliberately not made) a `BacktestConfig` field. `run_id` (the full replay-identity aggregate) *is* economics-sensitive: any economics value other than exactly `BacktestInstrumentEconomics::equity()` (multiplier=1, both margins `None`) produces a different `run_id` than the default-equity report, including margin-only changes that never alter P&L/equity math — because the reported economics metadata genuinely differs and two runs with different reported metadata must not share a replay identity. Proven by `brea04` (default vs multiplier=50 vs margin-only-at-multiplier=1, all three mutually distinct; same non-default economics re-run reproduces the same `run_id`).

**Proof default equity behavior is unchanged:** `brea01_default_equity_report_preserves_output` and `bmw01b_explicit_multiplier_one_equals_default` (extended) assert `report.equity_curve`, `run_id`, `config_id`, and `economics` are identical between an unconfigured engine and one explicitly given `BacktestInstrumentEconomics::new(1, None, None)`. `brea03c_metrics_json_schema_version_unchanged` proves `schema_version` stays `1` (additive field, not a schema bump).

**Synthetic multiplier=50/100 proof:** `brea02`/`brea02b` (report-level) and `bmw02`/`bmw02b` (engine-level, updated) prove `report.equity_curve` now equals `engine.economics_equity_curve()` and differs from the multiplier=1 curve; `report.economics.realized_pnl_micros` scales by exactly 50x/100x ($100 → $5,000 / $10,000 on the shared 4-bar buy-hold-sell fixture). `brea03b` proves the same truthfully in `metrics.json` end-to-end through `write_backtest_report`, including non-`None` margin fields.

**Invalid multiplier fail-closed proof:** unchanged pre-existing `bmw03_invalid_multiplier_zero_fails_closed` / `bmw03b_invalid_multiplier_negative_fails_closed` already prove `engine.run()` returns `Err(BacktestError::InvalidEconomics)` before any bar/report/artifact is produced — no report exists to inspect on that path, so no new test was needed; re-ran to confirm still passing after this patch's changes.

**Margin metadata/enforcement status:** unchanged from `BACKTEST-MULTIPLIER-MARGIN-01-COMBINED` — `margin_enforced` is hardcoded `false` everywhere (`BacktestEconomicsReport::equity()` and `::from_run()` both set it; no code path in `mqk-backtest` or `mqk-artifacts` reads margin fields to gate, block, or alter behavior). `bmw04` (extended) and `ber02`/`brea03b` prove margin values round-trip truthfully into the report and `metrics.json` without affecting P&L/equity.

**Proof no live/shared portfolio accounting changed:** `mqk-portfolio` does not appear in `git diff --name-only` for this patch (confirmed below). `self.portfolio` (the `mqk_portfolio::PortfolioState` driving fills/risk gating) is untouched by this patch; the `BacktestEconomicsLedger` parallel-shadow design from `BACKTEST-MULTIPLIER-RUN-WIRE-01-COMBINED` is unchanged.

**Proof no broker/provider/DB/runtime path changed:** `git diff --name-only` for this patch touches only `core-rs/crates/mqk-backtest/{src,tests}` and `core-rs/crates/mqk-artifacts/{src,tests}` files (6 modified, 2 new). No `mqk-daemon`, `mqk-cli`, `mqk-broker-alpaca`, `mqk-runtime`, `mqk-db`, or GUI file appears in the diff. No daemon was started; no DB connection was opened; no network call was made.

**Tests/checks run and exact results:**
- `cargo test -p mqk-backtest` — 199 passed, 0 failed, across 28 binaries (1 lib-unittest binary incl. the new `bmm07`/`bmm07b`/`ber01`–`ber03` tests, 26 integration test files incl. the 2 new/updated files, 1 doctest binary with 0 tests).
- `cargo check -p mqk-backtest` — clean.
- `cargo clippy -p mqk-backtest --all-targets -- -D warnings` — clean.
- `cargo test -p mqk-artifacts` — 35 passed, 0 failed, across 5 binaries (1 lib-unittest, 3 integration files incl. the new metadata test file, 1 doctest binary).
- `cargo check -p mqk-artifacts` — clean.
- `cargo clippy -p mqk-artifacts --all-targets -- -D warnings` — clean.
- `cargo test -p mqk-promotion` — 40 passed, 0 failed, across 9 binaries — run because `BacktestReport` itself gained a field; zero fixture edits were required (every `mqk-promotion/tests/*` literal already used `..BacktestReport::test_fixture()`, confirming `BACKTEST-REPORT-FIXTURE-READY-01-COMBINED`'s closure claim holds).
- `cargo check -p mqk-promotion --tests` and `cargo clippy -p mqk-promotion --all-targets -- -D warnings` — both clean.
- `mqk-daemon` / `mqk-cli` were not touched (confirmed via `git diff --name-only`) and were not built or run, per this patch's own instruction.

**Unrelated/pre-existing findings:** none newly found. The sqlx-postgres future-incompatibility warning printed by every `cargo` invocation in this workspace is pre-existing and unrelated to this patch.

**Not done (explicit):**
- `manifest.json` does not carry economics metadata (only `metrics.json` and `report.md` do) — would require new `InitRunArtifactsArgs` fields and `mqk-cli/src/commands/bkt.rs` call-site changes, not proven necessary for this patch's minimum required behavior (which only requires `metrics.json` *and/or* `manifest.json`).
- No daemon/CLI/API input field for economics was added — `BacktestEngine::with_economics` remains reachable only from Rust code (tests today; a future daemon/CLI patch would need to thread it through `BacktestConfig`-adjacent request shapes or an explicit engine-builder seam).
- No `InstrumentRegistryV2` multiplier lookup was added or consumed — `BacktestInstrumentEconomics` values are still caller-supplied, not registry-derived.
- No futures/options trading enablement, no contract registry lookups, no non-equity asset class enabled.
- `mqk-portfolio`, broker/provider code, OMS/outbox/inbox, risk gates, DB schema/migrations, GUI, and runtime startup flow were not touched.

`BACKTEST-MULTIPLIER-MARGIN-01` remains `PARTIAL`. This patch closes the report/artifact/identity gap `BACKTEST-MULTIPLIER-RUN-WIRE-01-COMBINED` left open (`report.equity_curve` is now genuinely multiplier-aware; reports/artifacts carry truthful economics; `run_id` is economics-sensitive). Still open before that parent can close: a registry-derived (rather than caller-supplied) multiplier source, and any daemon/CLI/GUI path to actually configure non-default economics for a real run.

**Recommended next slice:** thread `BacktestInstrumentEconomics` through one real entry point (CLI flag or daemon backtest-job request field) behind an explicit opt-in, now that the engine, report, and artifact layers are all economics-aware and proven. Re-verify current HEAD (not this entry) before starting, per this repo's audit rules.

**Full detail, exact test names, and validation commands:** this entry (above) is the full detail; see `git log --oneline -1` in the repo for the exact commit hash.

### BACKTEST-ECONOMICS-CLI-ENTRY-01-COMBINED — CLOSED_LOCAL / PARTIAL

**Commit:** single local commit, message `"cli: add backtest economics flags"` (code + tests + this ledger update). Hash intentionally not hardcoded here — this entry is part of that commit's own tree; see `git log --oneline -1` for the exact hash.

**Mission:** add the first real operator entry point for backtest economics — CLI opt-in flags that pass `BacktestInstrumentEconomics` into the already-proven `BacktestEngine::with_economics(...)` path. Default equity CLI behavior must remain unchanged. Daemon/GUI/API request shapes, registry-derived multipliers, and live/paper trading are explicitly out of scope.

**Repo evidence found before writing any code (current HEAD):**
- Three CLI commands build a `BacktestEngine`: `BacktestCmd::Csv` → `run_backtest_csv` (CSV file, no DB/provider), `BacktestCmd::CsvSweep` → `run_sweep_csv` (parameter sweep over CSV), and `BacktestCmd::Db` → `run_backtest_db` (loads bars from Postgres via `mqk_db::connect_from_env`). Only `Csv` requires neither DB nor network, matching this patch's own test constraint ("do not rely on provider calls or DB") and the mission's singular framing ("run **a** local backtest artifact"). Scope was narrowed to `BacktestCmd::Csv`/`run_backtest_csv` only — `CsvSweep` and `Db` are untouched.
- `BacktestInstrumentEconomics` and `EconomicsError` were already `pub use`-exported from `mqk-backtest` (`mqk-backtest/src/lib.rs`), and `BacktestEngine::with_economics(self, economics) -> Self` was already a public builder method (`mqk-backtest/src/engine.rs`) — both proven by the prior `BACKTEST-MULTIPLIER-RUN-WIRE-01-COMBINED` patch but reachable only from test code before this patch.
- `report.economics` (a `BacktestEconomicsReport`) was already surfaced into `metrics.json`'s `economics` section and `report.md`'s "## Instrument Economics" section by `BACKTEST-REPORT-ECONOMICS-ARTIFACT-01-COMBINED` — confirmed by reading `mqk-artifacts/src/lib.rs` and re-running `scenario_report_economics_artifact_metadata.rs`. This patch needed zero changes to `mqk-backtest/src` or `mqk-artifacts/src` — the entire remaining gap was CLI plumbing.
- `BacktestInstrumentEconomics::new(multiplier, init_margin, maint_margin) -> Result<Self, EconomicsError>` already fails closed on `multiplier <= 0` (`EconomicsError::InvalidMultiplier`), and `EconomicsError` already implements `std::error::Error` (so `anyhow::Context::with_context` composes directly) — no new validation primitive was needed, only a CLI call site that invokes the existing constructor before `engine.run()`/`init_run_artifacts` are ever called.
- Clap v4 (this workspace's pinned major version) rejects a bare `--contract-multiplier -5` as an ambiguous flag-like token (`error: unexpected argument '-5' found`); confirmed by direct `cargo run` reproduction. The `--contract-multiplier=-5` (`=`-joined) form parses correctly. This is a CLI-parsing fact, not a defect in this patch's flags — documented here so the negative-multiplier test uses the `=` form deliberately, not by accident.

**Built:**
- `core-rs/crates/mqk-cli/src/main.rs`: added three optional fields to `BacktestCmd::Csv` — `contract_multiplier: Option<i64>`, `initial_margin_micros: Option<i64>`, `maintenance_margin_micros: Option<i64>` (all `#[arg(long)]`, no default — absent means "do not touch economics"). Threaded all three through the `BacktestCmd::Csv` match arm into `run_backtest_csv(...)`. No other `BacktestCmd` variant, and no other `Commands` variant, was touched.
- `core-rs/crates/mqk-cli/src/commands/bkt.rs`: added `BacktestInstrumentEconomics` to the `mqk_backtest` import list. `run_backtest_csv` gained the same three new `Option<i64>` parameters. Immediately after `let mut engine = BacktestEngine::new(cfg);` and before `engine.add_strategy(...)`: if none of the three flags are `Some`, the block is skipped entirely (engine keeps its default `BacktestInstrumentEconomics::equity()`, byte-identical to pre-patch behavior). If any flag is `Some`, `multiplier = contract_multiplier.unwrap_or(1)` and `BacktestInstrumentEconomics::new(multiplier, initial_margin_micros, maintenance_margin_micros)` is called, propagating any `EconomicsError` via `.with_context(...)?` — which returns before `engine.add_strategy`, `engine.run`, and `init_run_artifacts` ever execute, so no artifact directory is created on rejection. On success, `engine = engine.with_economics(economics)`. Also added two `println!` lines (`economics_contract_multiplier=`, `economics_margin_enforced=`) alongside the existing `run_id=`/`strategy=`/`git_hash=`/`config_hash=` stdout lines, so the new flags' effect is visible on stdout even without `--out-dir`.
- New `core-rs/crates/mqk-cli/tests/scenario_cli_backtest_economics.rs`: 7 tests (`backtest_csv_economics_default_preserves_equity`, `backtest_csv_economics_multiplier_50_appears_in_artifacts`, `backtest_csv_economics_multiplier_100_appears_in_artifacts`, `backtest_csv_economics_margin_metadata_not_enforced`, `backtest_csv_economics_zero_multiplier_fails_closed`, `backtest_csv_economics_negative_multiplier_fails_closed`, `backtest_csv_economics_margin_only_defaults_multiplier_one`). Named with the repo's `backtest_csv_*` convention (matching `scenario_cli_backtest_integrity_calendar.rs`) so `cargo test -p mqk-cli backtest` picks them up automatically. Each test invokes the compiled `mqk-cli` binary via `assert_cmd`, against a temp-file CSV bars fixture (3 bars, 300s apart, matching `intraday_scalper`'s required `timeframe_secs=300`) and a fresh never-created temp output directory — no DB, no provider, no network.

**Default/no-flag behavior proof:** `backtest_csv_economics_default_preserves_equity` runs `mqk backtest csv` with no economics flags and asserts `economics_contract_multiplier=1` / `economics_margin_enforced=false` on stdout, plus `metrics.json`'s `economics.contract_multiplier == 1`, both margins `null`, `margin_enforced == false`, and `report.md` containing `Contract Multiplier | 1`. The two pre-existing tests in `scenario_cli_backtest_integrity_calendar.rs` (unmodified, still calling `mqk backtest csv` with no economics flags) continue to pass unchanged, confirming no regression to the no-flag path.

**Multiplier=50/100 CLI proof:** `backtest_csv_economics_multiplier_50_appears_in_artifacts` and `_multiplier_100_appears_in_artifacts` pass `--contract-multiplier 50` / `100` and assert the stdout line, `metrics.json`'s `economics.contract_multiplier`, and `report.md`'s `Contract Multiplier | 50` / `| 100` line all agree.

**Margin metadata proof:** `backtest_csv_economics_margin_metadata_not_enforced` passes `--contract-multiplier 50 --initial-margin-micros 10000000000 --maintenance-margin-micros 5000000000` and asserts both margin values round-trip into `metrics.json`'s `economics` object and `margin_enforced` stays `false` in both `metrics.json` and `report.md` (`Margin Enforced | false`) — proving the margin scaffold is recorded but never enforced, matching `BacktestEconomicsReport`'s existing contract. `backtest_csv_economics_margin_only_defaults_multiplier_one` additionally proves that supplying only `--initial-margin-micros` (no `--contract-multiplier`) defaults the multiplier to `1`, per the mission's explicit rule.

**Invalid multiplier fail-closed proof:** `backtest_csv_economics_zero_multiplier_fails_closed` (`--contract-multiplier 0`) and `backtest_csv_economics_negative_multiplier_fails_closed` (`--contract-multiplier=-5`) both assert: (a) the CLI process exits non-zero, (b) stderr contains `contract_multiplier must be positive` (the exact `EconomicsError::Display` message, surfaced through the `.with_context` chain), and (c) the target `--out-dir` path does **not** exist on disk afterward (`!out_dir.exists()`) — proving the rejection happens before `init_run_artifacts` (which would create `out_dir/<run_id>/`) is ever reached.

**Artifact outputs affected:** `metrics.json` (`economics` section) and `report.md` ("## Instrument Economics" section) now receive operator-supplied values end-to-end from the CLI, using the section/format `BACKTEST-REPORT-ECONOMICS-ARTIFACT-01-COMBINED` already built. `manifest.json` is unchanged (still no economics fields, same as the parent patch left it) — `init_run_artifacts`/`InitRunArtifactsArgs` were not touched.

**Proof daemon/GUI/API request shape unchanged:** `git diff --name-only` (below) touches only `core-rs/crates/mqk-cli/src/main.rs`, `core-rs/crates/mqk-cli/src/commands/bkt.rs`, the new CLI test file, and this ledger file. No `mqk-daemon`, `mqk-gui`, route, or API response-type file appears in the diff.

**Proof no live/shared portfolio accounting changed:** `mqk-portfolio` does not appear anywhere in the diff. `run_backtest_db` (the DB-backed command) is byte-for-byte unmodified — it still calls `BacktestEngine::new(cfg)` with no economics wiring, so the only CLI path that touches a live Postgres connection is completely untouched by this patch.

**Proof no broker/provider/DB/runtime path changed:** no `mqk-broker-alpaca`, `mqk-runtime`, `mqk-db`, or OMS/outbox/inbox file appears in the diff. No daemon was started; no DB connection was opened; no network call was made; `.env.local` was not touched.

**Tests/checks run and exact results:**
- `cargo test -p mqk-cli backtest` — 9 passed, 0 failed (7 new in `scenario_cli_backtest_economics.rs` + 2 pre-existing in `scenario_cli_backtest_integrity_calendar.rs`); all other CLI test binaries report `0 ... ; N filtered out` under this name filter (expected — their test names don't contain "backtest").
- `cargo test -p mqk-cli strategy_lab` — 7 passed, 0 failed, all pre-existing, unaffected.
- `cargo check -p mqk-cli` — clean.
- `cargo clippy -p mqk-cli --all-targets -- -D warnings` — clean.
- `cargo test -p mqk-backtest` — all passing (33+ unit tests plus 27 integration binaries, 0 failed) — unaffected, since `mqk-backtest/src` was not modified by this patch.
- `cargo check -p mqk-backtest` / `cargo clippy -p mqk-backtest --all-targets -- -D warnings` — both clean.
- `cargo test -p mqk-artifacts` — all passing (13+6+3+13 = 35 tests, 0 failed) — unaffected, since `mqk-artifacts/src` was not modified by this patch.
- `cargo check -p mqk-artifacts` / `cargo clippy -p mqk-artifacts --all-targets -- -D warnings` — both clean.
- Additionally (beyond the mission's required commands, for extra confidence): `cargo test -p mqk-cli` (whole crate) — all passing, 0 failed.

**Unrelated/pre-existing findings:** none newly found. The sqlx-postgres future-incompatibility warning printed by every `cargo` invocation in this workspace is pre-existing and unrelated to this patch.

**Not done (explicit):**
- `BacktestCmd::CsvSweep` and `BacktestCmd::Db` do not have economics flags — only `BacktestCmd::Csv` does. A sweep or DB-backed economics entry point would be a separate, equally-scoped patch.
- No daemon/API/GUI input field for economics was added — `mqk-daemon` backtest jobs remain default-equity-only.
- No `InstrumentRegistryV2` multiplier lookup was added or consumed — the multiplier is still entirely caller-supplied via CLI flags, never inferred from the symbol.
- `manifest.json` still carries no economics fields.
- No futures/options trading enablement, no contract registry lookups, no non-equity asset class enabled.
- `mqk-portfolio`, broker/provider code, OMS/outbox/inbox, risk gates, DB schema/migrations, GUI, and runtime startup flow were not touched.

`BACKTEST-MULTIPLIER-MARGIN-01` remains `PARTIAL`. This patch adds the first real operator-facing entry point (CLI-only) for backtest economics, closing the exact gap `BACKTEST-REPORT-ECONOMICS-ARTIFACT-01-COMBINED` left open. Still open before that parent can close: daemon/GUI/API entry points, a registry-derived (rather than caller-supplied) multiplier source, and CLI flags on the sweep/DB-backed commands.

**Recommended next slice:** either (a) extend the same opt-in flags to `BacktestCmd::Db` (the only other commonly-used real-bars entry point) behind the same fail-closed validation, or (b) begin the daemon backtest-job request-shape design for economics, behind an explicit opt-in field, now that the CLI proof-of-concept is closed. Re-verify current HEAD (not this entry) before starting, per this repo's audit rules.

**Full detail, exact test names, and validation commands:** this entry (above) is the full detail; see `git log --oneline -1` in the repo for the exact commit hash.

### BACKTEST-ECONOMICS-DB-CLI-ENTRY-01-COMBINED — CLOSED_LOCAL / PARTIAL

**Commit:** single local commit, message `"cli: add db backtest economics flags"` (code + tests + this ledger update). Hash intentionally not hardcoded here — this entry is part of that commit's own tree; see `git log --oneline -1` for the exact hash.

**Mission:** extend the already-proven `mqk backtest csv` economics flags (`BACKTEST-ECONOMICS-CLI-ENTRY-01-COMBINED`) to `mqk backtest db`, so an operator can run a DB-backed backtest against existing `md_bars` data with an explicit contract multiplier and optional margin metadata. Default DB-backed equity behavior must remain unchanged. Daemon/GUI/API request shapes, registry-derived multipliers, and live/paper trading are explicitly out of scope.

**Repo evidence found before writing any code (current HEAD):**
- `BacktestCmd::Db` (`core-rs/crates/mqk-cli/src/main.rs`) and its handler `run_backtest_db` (`core-rs/crates/mqk-cli/src/commands/bkt.rs`) had no economics flags at all — `run_backtest_db` built `BacktestEngine::new(cfg)` and never called `.with_economics(...)`, so every DB-backed run was hardcoded to the default equity economics regardless of instrument.
- `run_backtest_db` connects via `mqk_db::connect_from_env()` (reads `MQK_DATABASE_URL`) and loads rows via `mqk_db::md::load_md_bars_for_backtest_symbols`, then converts them to `BacktestBar` and runs the same `BacktestEngine`/`mqk_artifacts::write_backtest_report` path `run_backtest_csv` already uses. No DB schema, no new query, and no new artifact-writing code was needed — only the same opt-in economics wiring `run_backtest_csv` already had, applied to the second call site.
- `report.economics` already flows into `metrics.json`'s `economics` section and `report.md`'s "## Instrument Economics" section purely from `BacktestReport.economics`, independent of how the bars were sourced (confirmed by reading `mqk-artifacts/src/lib.rs`) — so no `mqk-artifacts` change was needed once `run_backtest_db` calls `.with_economics(...)`.
- No existing test in `mqk-cli/tests/` exercised `mqk backtest db` at all (`scenario_cli_backtest_economics.rs` and `scenario_cli_backtest_integrity_calendar.rs` are both CSV-only); the closest precedent for a DB-gated `mqk-cli` integration test is `scenario_cli_db_migrate_requires_yes_when_live_active.rs`, which skips gracefully (prints `SKIP: ...`, returns `Ok(())`) when `MQK_DATABASE_URL` is unset or unreachable rather than failing the suite.

**Built:**
- `core-rs/crates/mqk-cli/src/commands/bkt.rs`: extracted the CSV economics block into a new shared helper, `build_backtest_economics_from_cli_flags(contract_multiplier, initial_margin_micros, maintenance_margin_micros) -> Result<Option<BacktestInstrumentEconomics>>`. Returns `None` (caller keeps engine default) when all three flags are absent; otherwise defaults an absent multiplier to `1` and calls `BacktestInstrumentEconomics::new(...)`, propagating `EconomicsError` via the same `.with_context(|| format!("invalid --contract-multiplier {}", multiplier))` used before. `run_backtest_csv` now calls this helper instead of its old inline block — behavior is byte-identical (proven by all 7 pre-existing CSV economics tests passing unchanged). `run_backtest_db` gained the same three new parameters and calls the helper **before** `mqk_db::connect_from_env()` / before loading any bars — so an invalid `--contract-multiplier` fails closed without a wasted DB round trip, never mind an artifact directory. After `let mut engine = BacktestEngine::new(cfg);`, `if let Some(economics) = economics { engine = engine.with_economics(economics); }` mirrors the CSV wiring. Two new `println!` lines (`economics_contract_multiplier=`, `economics_margin_enforced=`) were added to `run_backtest_db`'s stdout output, matching `run_backtest_csv`'s existing lines.
- `core-rs/crates/mqk-cli/src/main.rs`: added the same three optional fields (`contract_multiplier`, `initial_margin_micros`, `maintenance_margin_micros`, all `#[arg(long)]`, no default) to `BacktestCmd::Db`, with doc comments copied verbatim from `BacktestCmd::Csv`. Threaded all three through the `BacktestCmd::Db` match arm into `run_backtest_db(...)`. No other `BacktestCmd` variant (`CsvSweep`, `StrategyLabEvaluate`, `StrategyLabRank`, `RegimeDetect`) and no other `Commands` variant was touched.
- New `core-rs/crates/mqk-cli/tests/scenario_cli_backtest_db_economics.rs`: 5 tests. Three are DB-backed and skip gracefully (matching `scenario_cli_db_migrate_requires_yes_when_live_active.rs`'s pattern) without a working `MQK_DATABASE_URL`: `backtest_db_economics_default_preserves_equity`, `backtest_db_economics_multiplier_50_appears_in_artifacts`, `backtest_db_economics_margin_metadata_not_enforced`. Two require no DB at all, because the economics validation in `run_backtest_db` now runs before any DB connection attempt: `backtest_db_economics_zero_multiplier_fails_closed`, `backtest_db_economics_negative_multiplier_fails_closed`. The DB-backed tests seed three `md_bars` rows (same OHLCV values as `scenario_cli_backtest_economics.rs`'s CSV fixture, 300s apart) under a fresh `Uuid`-suffixed symbol via direct `sqlx` insert, run the compiled `mqk-cli` binary against them, then delete the seeded rows.

**Default/no-flag DB behavior proof:** `backtest_db_economics_default_preserves_equity` runs `mqk backtest db` with no economics flags against seeded `md_bars` rows and asserts `bars_loaded=3`, `economics_contract_multiplier=1`, `economics_margin_enforced=false` on stdout, plus `metrics.json`'s `economics.contract_multiplier == 1`, both margins `null`, `margin_enforced == false`, and `report.md` containing `Contract Multiplier | 1`. **DB-backed; see closure honesty note below.**

**Multiplier=50 DB proof:** `backtest_db_economics_multiplier_50_appears_in_artifacts` passes `--contract-multiplier 50` and asserts the stdout line, `metrics.json`'s `economics.contract_multiplier`, and `report.md`'s `Contract Multiplier | 50` line all agree. **DB-backed; see closure honesty note below.**

**Margin metadata DB proof:** `backtest_db_economics_margin_metadata_not_enforced` passes `--contract-multiplier 50 --initial-margin-micros 10000000000 --maintenance-margin-micros 5000000000` and asserts both margin values round-trip into `metrics.json`'s `economics` object and `margin_enforced` stays `false` in both `metrics.json` and `report.md` (`Margin Enforced | false`). **DB-backed; see closure honesty note below.**

**Invalid multiplier fail-closed DB proof:** `backtest_db_economics_zero_multiplier_fails_closed` (`--contract-multiplier 0`) and `backtest_db_economics_negative_multiplier_fails_closed` (`--contract-multiplier=-5`) both assert: (a) the CLI process exits non-zero, (b) stderr contains `contract_multiplier must be positive`, (c) the target `--out-dir` path does not exist afterward. These two ran for real (no `MQK_DATABASE_URL` needed, by design — see "Built" above) and passed.

**Closure honesty note (DB-backed tests):** in this session's environment, `MQK_DATABASE_URL` was unset, so the three DB-backed tests above hit the pre-existing graceful-skip path (`SKIP: MQK_DATABASE_URL not set`, confirmed via `--nocapture`) rather than exercising a live Postgres connection. Three local Docker Postgres containers were found running (`mqk-test-postgres:5433`, `mqk-live-postgres:5432`, `mqk-paper-postgres:5440`); per this patch's safety rules, live (5432) and paper (5440) were never touched. The isolated test container (5433) was attempted with its own `docker inspect`-reported init credentials and this repo's documented `mqk`/`mqk` proof-DB convention; both attempts failed with a real Postgres `password authentication failed` error (a pre-existing local credential mismatch unrelated to this patch — the data volume was evidently initialized with different credentials than its current `POSTGRES_PASSWORD` env). No further credential attempts were made. Confidence that the multiplier/margin wiring itself is correct rests on: (1) the DB path reuses the exact same `build_backtest_economics_from_cli_flags` helper and `BacktestEngine::with_economics`/`BacktestReport.economics`/`mqk_artifacts::write_backtest_report` plumbing already proven end-to-end by the CSV tests, with no new logic specific to DB beyond flag plumbing and two `println!` lines; (2) `cargo check`/`clippy -D warnings` are clean; (3) the skip path itself was proven to fire correctly and harmlessly. This is **not** a substitute for a real DB-backed pass — the next session should re-run `scenario_cli_backtest_db_economics.rs` with a working `MQK_DATABASE_URL` (e.g. after fixing or recreating the local test container) before treating the DB-backed proof as fully closed.

**CSV regression proof:** `cargo test -p mqk-cli --test scenario_cli_backtest_economics` — all 7 pre-existing tests pass unchanged, proving the `build_backtest_economics_from_cli_flags` extraction preserved `run_backtest_csv`'s behavior byte-for-byte (same stdout lines, same `metrics.json`/`report.md` content, same fail-closed error text).

**Artifact outputs affected:** `metrics.json` (`economics` section) and `report.md` ("## Instrument Economics" section) now receive operator-supplied values end-to-end from `mqk backtest db`, using the exact section/format `BACKTEST-REPORT-ECONOMICS-ARTIFACT-01-COMBINED` already built. `manifest.json` is unchanged (still no economics fields) — `init_run_artifacts`/`InitRunArtifactsArgs` were not touched.

**Proof daemon/GUI/API request shape unchanged:** `git diff --name-only` (below) touches only `core-rs/crates/mqk-cli/src/main.rs`, `core-rs/crates/mqk-cli/src/commands/bkt.rs`, the new CLI test file, this ledger file, and the audit doc. No `mqk-daemon`, `mqk-gui`, route, or API response-type file appears in the diff.

**Proof no live/shared portfolio accounting changed:** `mqk-portfolio` does not appear anywhere in the diff. `mqk-backtest/src` and `mqk-artifacts/src` are untouched (read-only, confirmed by `git diff --name-only`).

**Proof no broker/provider/runtime path changed:** no `mqk-broker-alpaca`, `mqk-runtime`, OMS/outbox/inbox, or risk-gate file appears in the diff. No daemon was started; no production/paper DB was mutated (live/paper Docker containers were identified but never connected to); no network/provider call was made; `.env.local` was not touched.

**Tests/checks run and exact results:**
- `cargo test -p mqk-cli backtest` — 14 passed, 0 failed (5 new in `scenario_cli_backtest_db_economics.rs`, 7 pre-existing in `scenario_cli_backtest_economics.rs`, 2 pre-existing in `scenario_cli_backtest_integrity_calendar.rs`); 3 of the 5 new tests took the graceful DB-unavailable skip path (see closure honesty note above).
- `cargo test -p mqk-cli strategy_lab` — 7 passed, 0 failed, all pre-existing, unaffected.
- `cargo check -p mqk-cli` — clean.
- `cargo clippy -p mqk-cli --all-targets -- -D warnings` — clean.
- `cargo test -p mqk-backtest` — all passing (33+ unit tests plus 27 integration binaries, 0 failed) — unaffected, `mqk-backtest/src` not modified.
- `cargo check -p mqk-backtest` / `cargo clippy -p mqk-backtest --all-targets -- -D warnings` — both clean.
- `cargo test -p mqk-artifacts` — all passing (13+6+3+13 = 35 tests, 0 failed) — unaffected, `mqk-artifacts/src` not modified.
- `cargo check -p mqk-artifacts` / `cargo clippy -p mqk-artifacts --all-targets -- -D warnings` — both clean.
- Additionally (beyond the mission's required commands, for extra confidence): `cargo test -p mqk-cli` (whole crate) — all passing, 0 failed.

**Unrelated/pre-existing findings:** the local `mqk-test-postgres` Docker container's current password does not match its own `docker inspect`-reported `POSTGRES_PASSWORD` init env var (see closure honesty note) — a pre-existing local environment issue, not introduced by this patch, out of scope to fix here. The sqlx-postgres future-incompatibility warning printed by every `cargo` invocation in this workspace is pre-existing and unrelated to this patch.

**Not done (explicit):**
- No daemon/API/GUI input field for economics was added — `mqk-daemon` backtest jobs remain default-equity-only.
- No `InstrumentRegistryV2` multiplier lookup was added or consumed — the multiplier is still entirely caller-supplied via CLI flags, never inferred from the symbol.
- `BacktestCmd::CsvSweep` still has no economics flags.
- `manifest.json` still carries no economics fields.
- No futures/options trading enablement, no contract registry lookups, no non-equity asset class enabled.
- `mqk-portfolio`, broker/provider code, OMS/outbox/inbox, risk gates, DB schema/migrations, GUI, and runtime startup flow were not touched.
- ~~The three DB-backed tests were not proven against a live Postgres connection in this session~~ — **superseded, see addendum below: now proven for real, 5/5 pass.**

`BACKTEST-MULTIPLIER-MARGIN-01` remains `PARTIAL`. This patch extends the CLI-only operator entry point for backtest economics from `mqk backtest csv` to `mqk backtest db`, closing the exact gap `BACKTEST-ECONOMICS-CLI-ENTRY-01-COMBINED` left open for the DB-backed command. Still open before that parent can close: daemon/GUI/API entry points and a registry-derived (rather than caller-supplied) multiplier source.

**Recommended next slice:** ~~re-run `scenario_cli_backtest_db_economics.rs` against a working `MQK_DATABASE_URL`...~~ — done; see addendum below. Next: daemon request-shape design for economics is the next logical scope.

**Addendum (same-day follow-up, commit `267442a` + a later re-verification — supersedes the "Closure honesty note," the DB-backed-tests bullet under "Not done," and the "Recommended next slice" above):** the `mqk-test-postgres` credential mismatch was root-caused as a stale Docker Desktop host-port-forward tied to host port 5433, not a genuine credential problem — the same password authenticated successfully via `docker exec` (both Unix-socket and TCP-from-inside-container) and via Docker's internal bridge network; only the Windows-host-published port path failed. Recreating the container on host port 5434 (same credentials, same data) fixed it immediately, with no credential change needed. Re-running `cargo test -p mqk-cli --test scenario_cli_backtest_db_economics -- --nocapture --test-threads=1` against `postgresql://postgres:postgres@127.0.0.1:5434/mqk_test` now passes **5/5, 0 skipped**: `backtest_db_economics_default_preserves_equity`, `_multiplier_50_appears_in_artifacts`, and `_margin_metadata_not_enforced` all ran for real (no `SKIP` line, ~0.7-1.9s runtime consistent with real DB round trips), in addition to the two DB-independent fail-closed tests. This closes the one proof gap this entry originally left open — `mqk backtest db`'s economics wiring is now proven end-to-end against a real Postgres connection, not just by code-reuse argument. `README_TECHNICAL.md` and `scripts/reset-mqk-testdb.ps1` were separately updated (commit `267442a`) to caution that this machine's persistent `mqk-live-postgres`/`mqk-paper-postgres` containers occupy two of the three documented default proof-DB ports (5432, 5440), so a documented default must be checked against `docker ps` before being trusted, not assumed free.

**Full detail, exact test names, and validation commands:** this entry (above) is the full detail; see `git log --oneline -1` in the repo for the exact commit hash.

### BACKTEST-ECONOMICS-DAEMON-JOB-REQUEST-01-COMBINED — CLOSED_LOCAL / PARTIAL

**Mission:** add an explicit, opt-in daemon backtest-job economics request surface so daemon-submitted backtests can pass caller-supplied `contract_multiplier`, `initial_margin_micros`, and `maintenance_margin_micros` into the existing `BacktestEngine::with_economics(...)` path. GUI controls, registry-derived multipliers, live/paper runtime, broker/provider paths, OMS/outbox/inbox, risk gates, DB schema, and `mqk-portfolio` remain untouched.

**Repo evidence found:** `POST /api/v1/backtests/jobs` is the daemon job route. `BacktestJobRequest` in `mqk-daemon/src/api_types.rs` is the request type. `mqk-daemon/src/routes/backtests.rs` constructs `BacktestConfig` and `BacktestEngine` in both CSV and `md_bars` worker paths; both paths call `mqk_artifacts::write_backtest_report(...)`, so `BacktestReport.economics` already reaches `metrics.json` and `report.md` once the engine is configured. Jobs are in-memory but can be CSV-backed or `md_bars`-backed. Serde has no `deny_unknown_fields`, and existing request fields already use additive defaults where needed.

**Built:** `BacktestEconomicsRequest { contract_multiplier: Option<i64>, initial_margin_micros: Option<i64>, maintenance_margin_micros: Option<i64> }` and `BacktestJobRequest.economics: Option<BacktestEconomicsRequest>` were added. Omitted `economics` preserves the daemon's previous default-equity path exactly. Present `economics` defaults an omitted multiplier to `1`, validates through `BacktestInstrumentEconomics::new(...)`, and calls `BacktestEngine::with_economics(...)` before `engine.run(...)`. Invalid non-positive multipliers fail the queued job with a truthful error before artifact directories are created.

**Daemon proof:** `scenario_backtest_jobs_01` now proves (a) no `economics` writes default `contract_multiplier=1`, null margins, and `margin_enforced=false` to `metrics.json` and `report.md`; (b) `economics.contract_multiplier=50` plus margin metadata reaches `metrics.json` and `report.md`; and (c) `contract_multiplier=0` leaves the job in `failed` state with no artifact paths and an empty temp output directory.

**Artifact outputs affected:** only existing `metrics.json` economics metadata and `report.md` "Instrument Economics" output receive daemon-supplied values. `manifest.json` still carries no economics metadata.

**Validation:** required focused checks passed: `cargo test -p mqk-daemon --test scenario_backtest_jobs_01`, `cargo test -p mqk-daemon --test scenario_gui_daemon_contract_gate`, `cargo check -p mqk-daemon`, `cargo clippy -p mqk-daemon --lib -- -D warnings`, `cargo test -p mqk-cli backtest`, `cargo check -p mqk-cli`, `cargo clippy -p mqk-cli --all-targets -- -D warnings`, `cargo test -p mqk-backtest`, `cargo check -p mqk-backtest`, `cargo clippy -p mqk-backtest --all-targets -- -D warnings`, `cargo test -p mqk-artifacts`, `cargo check -p mqk-artifacts`, and `cargo clippy -p mqk-artifacts --all-targets -- -D warnings`. The recurring `sqlx-postgres` future-incompatibility warning is pre-existing.

**Safety confirmation:** no daemon live/paper runtime was started; no production or paper DB was mutated; no broker/provider calls; no live routing; no paper/live orders; no runtime startup behavior change; no non-equity trading enablement; no DB migration; `.env.local`, GUI, broker adapters, runtime, OMS/outbox/inbox, risk gates, `mqk-portfolio`, smoke logs, and untracked ledger draft were untouched.

`BACKTEST-MULTIPLIER-MARGIN-01` remains `PARTIAL`. This patch closes the daemon request/worker entry point. Still open before the parent can close: GUI controls and registry-derived multiplier wiring.

**Recommended next slice:** GUI backtest economics controls or registry-derived multiplier lookup, as a separate opt-in patch with the same default-equity behavior preserved.

### BACKTEST-ECONOMICS-GUI-REGISTRY-01-COMBINED — CLOSED_LOCAL / PARTIAL

**Mission:** add operator-facing GUI support for the daemon backtest economics request object and add a safe registry-derived economics suggestion path where current repo evidence supports it.

**Repo evidence found:** the GUI builds `POST /api/v1/backtests/jobs` in `core-rs/mqk-gui/src/features/backtests/BacktestResultsScreen.tsx` via `submitBacktestJob` in `api.ts`; the TypeScript request type is `BacktestJobRequest` in `types.ts`. Form state and validation live inside `BacktestResultsScreen`. `metrics.json` is parsed by `parseMetrics` and displayed in `MetricsSection`, but economics metadata was not displayed before this patch. GUI submit request-shape tests live in `core-rs/mqk-gui/src/features/backtests/__tests__/api.test.ts`. The daemon had a read-only registry-v2 status route, not a symbol lookup route; it reported counts/status only, so a minimal backtest-only read-only suggestion route was added.

**Built:** the Backtest Results submit form gained optional Instrument economics fields: `contract_multiplier`, `initial_margin_micros`, and `maintenance_margin_micros`. Blank fields preserve the old request shape by omitting `economics`. Populated fields send the nested daemon `economics` object. Client-side validation rejects non-integer text and rejects `contract_multiplier <= 0` before submit. Metrics display now renders the `metrics.json.economics` fields truthfully, and old artifacts without economics show "not reported" rather than fabricating multiplier `1`.

**Registry suggestion status:** added `GET /api/v1/backtests/economics-suggestion?symbol=<SYMBOL>`, a public read-only backtest/operator route. It loads the configured v1 registry, converts it to `InstrumentRegistryV2`, validates it, and returns `active` with `contract_multiplier=1` for current equity/ETF registry symbols, or truthful `not_found` / `registry_unavailable` / `unsupported` / `no_contract_economics` states. The GUI calls it only when the operator clicks "Load registry economics"; it does not auto-query on keystrokes and does not infer from symbol strings.

**Validation:** `npm test -- --run` in `core-rs/mqk-gui` passed 536/536. `npm run build` passed (Vite emitted existing-style chunk/dynamic-import warnings only). Backend validation passed: `cargo test -p mqk-daemon --test scenario_backtest_economics_registry_suggestion` 5/5, `cargo test -p mqk-daemon --test scenario_gui_daemon_contract_gate` 23/23, `cargo test -p mqk-daemon --test scenario_route_contract_rt01` 2/2, `cargo check -p mqk-daemon` clean, `cargo clippy -p mqk-daemon --lib -- -D warnings` clean, and extra `cargo clippy -p mqk-daemon --test scenario_backtest_economics_registry_suggestion -- -D warnings` clean. The recurring `sqlx-postgres` future-incompatibility warning is pre-existing.

**Safety confirmation:** daemon backtest worker behavior is unchanged except for receiving the existing optional economics request object from the GUI. CLI behavior is untouched. No live/paper runtime, broker adapter, OMS/outbox/inbox, risk gate, shared portfolio accounting, DB migration, provider call, broker call, live routing, paper/live order, `.env.local`, smoke log, or untracked ledger draft was touched. Non-equity trading remains disabled.

`BACKTEST-MULTIPLIER-MARGIN-01` remains `PARTIAL`: manifest economics metadata remains incomplete, and the registry suggestion is currently limited to the converted v1 equity/ETF registry default multiplier because no production v2 non-equity registry data exists.

### BACKTEST-ECONOMICS-REGISTRY-MANIFEST-01-COMBINED — CLOSED_LOCAL / PARTIAL

**Mission:** close the two remaining gaps the prior entry's closing paragraph named: (1) `manifest.json` carries no economics metadata even though `metrics.json`/`report.md` do; (2) the registry suggestion route can only ever see converted-v1 equity/ETF data, so it has no way to represent or prove an explicit non-default multiplier/margin. Backtest/artifact/operator metadata only — no live/paper trading, broker, OMS, risk, or shared-portfolio code touched; no non-equity trading enabled.

**Repo evidence found:** `mqk_artifacts::init_run_artifacts` writes `manifest.json` (via `RunManifest`) *before* the engine result is known at every call site that creates an output directory; `mqk_artifacts::write_backtest_report` runs immediately afterward and already receives the full `BacktestReport` (including `report.economics: BacktestEconomicsReport`) at all six call sites (`mqk-cli` CSV/DB/sweep, daemon CSV-blocking/md_bars-blocking, plus this crate's own tests) — so `write_backtest_report` is the one place that can correct the manifest's economics to the run's real value without touching any other call site, including the live/paper `run` CLI command (which calls `init_run_artifacts` but never `write_backtest_report`, confirmed via `rg write_backtest_report` — only backtest paths call it). `mqk-md::instrument_registry_v2::InstrumentDefinitionV2` already has a `multiplier` field nested inside `ContractDefinitionV2::Future`/`::Option`, but nothing uniform across asset classes (equity/ETF contracts carry no multiplier field at all) and no margin field anywhere. The daemon's `backtest_economics_suggestion` route gated on `instrument.asset_class != "equity"` and returned a `"unsupported"` truth_state for anything else — untested and unreachable in production today, since the only registry data path (`load_instrument_registry` → `convert_v1_registry_to_v2`) only ever produces equity/ETF entries.

**Built — Part A (manifest economics):** added `mqk_artifacts::ManifestEconomics { contract_multiplier: i64, initial_margin_micros: Option<i64>, maintenance_margin_micros: Option<i64>, margin_enforced: bool, source: String }` and a new `RunManifest.economics: ManifestEconomics` field (`#[serde(default)]` so manifests written before this field existed still parse, defaulting to `ManifestEconomics::default_equity()`). `init_run_artifacts` (shared with the live `run` CLI) always writes `default_equity` — it has no backtest-specific economics input by design. `write_backtest_report` now does a read-parse-merge-write on `manifest.json`'s `economics` key using `ManifestEconomics::from_backtest_economics(&report.economics)` — the same pattern the daemon's existing `augment_manifest_with_md_bars_provenance` already uses for `source`/`symbol`/`start`/`end`/`bar_count`. The merge is skipped (not fabricated) when no `manifest.json` exists yet, which is the case for this crate's own bare-temp-dir unit tests that call `write_backtest_report` directly without `init_run_artifacts`. `source` is `"default_equity"` when the values equal the implicit default (multiplier=1, no margin) and `"explicit_request"` otherwise — pure value-equality, mirroring `BacktestInstrumentEconomics::is_default_equity`'s existing semantics (not a record of operator intent). **No call site outside `mqk-artifacts/src/lib.rs` needed to change** — every CLI/daemon backtest path already calls `write_backtest_report` with the real report, so all of them gained truthful manifest economics for free, and the live/paper `run` CLI (which never calls `write_backtest_report`) is completely unaffected.

**Built — Part B (registry-v2 economics metadata seam):** added `InstrumentEconomicsMetadataV2 { contract_multiplier: Option<i64>, initial_margin_micros: Option<i64>, maintenance_margin_micros: Option<i64> }` and `InstrumentDefinitionV2.economics: Option<InstrumentEconomicsMetadataV2>` (additive, defaults `None`; `convert_tracked_instrument_to_v2` leaves it `None` since v1 has no such data — no fabrication). `validate_registry_v2` now calls a new `validate_economics_v2` per instrument: rejects `contract_multiplier <= 0` and rejects negative margins when present; absent `economics` always validates (opt-in metadata, independent of `enabled`/asset class). Added a pure helper, `backtest_economics_suggestion_for_instrument(&InstrumentDefinitionV2) -> BacktestEconomicsSuggestionV2`: explicit `economics.contract_multiplier` wins (`"active"`, `reason="registry_v2_explicit"`) regardless of asset class; otherwise equity/ETF instruments fall back to `"active"`/multiplier=1/`"equity_default"`; otherwise `"no_contract_economics"` truthfully (never a fabricated multiplier). This is metadata-only: nothing reads `enabled`/`paper_trading_enabled`/`live_trading_enabled`, and the helper's own test (`sug05`) proves output is unchanged when those flags are flipped.

**Built — Part C (suggestion route hardened):** `backtest_economics_suggestion` in `mqk-daemon/src/routes/backtests.rs` now calls `backtest_economics_suggestion_for_instrument` instead of its old inline `asset_class`/`contract` match, and `BacktestEconomicsSuggestionResponse`'s margin fields are now actually populated from the helper instead of being hardcoded `None`. The dead `"unsupported"` truth_state (never reachable or tested against real data) was removed in favor of the helper's more specific `"no_contract_economics"`. The route remains read-only, untouched DB/provider/broker behavior, and its real-data behavior for the only registry path that exists today (converted v1, equity/ETF only) is unchanged byte-for-byte — proven by all 5 pre-existing `scenario_backtest_economics_registry_suggestion` tests passing unmodified.

**Built — Part D (GUI):** `BacktestManifest.economics?: BacktestEconomicsMetadata | null` added to the TS type (passthrough already handled by `parseManifest`'s existing cast — no parser logic change needed). `ManifestSection` ("Run summary", manifest.json-only panel) gained two always-rendered rows, "Contract multiplier" and "Margin enforced", both falling back to `"not reported"` via optional chaining for old manifests. The existing `MetricsSection` "Instrument economics" panel (sourced from `metrics.json`) was **not** touched, so there is exactly one truthful source feeding each panel — no possibility of contradictory labels.

**Honest PARTIAL — production non-equity economics still does not exist:** the economics-suggestion route's only data path is `load_instrument_registry` (v1, `config/instruments/equities.json`, equity/ETF only) → `convert_v1_registry_to_v2`, which never sets `economics` (v1 has no such data) on any instrument, and never produces a non-equity instrument at all. So in production today the route still only ever returns the equity-default branch — identical to before this patch. The explicit-registry-v2-economics and non-equity-no-contract-economics branches are proven **only** via direct unit tests against hand-built `InstrumentDefinitionV2` fixtures (`sug01`-`sug04` in `instrument_registry_v2.rs`), exactly as the patch's own guidance anticipated ("add a test-only seam or helper rather than broad route architecture" when the route can't safely load v2 fixtures). No production futures/options/crypto/forex registry file was created, and none should be until a real provider/data source exists.

**Tests added:** `mqk-artifacts/src/lib.rs` — `mre01`-`mre04` (default/explicit-multiplier/margin/old-manifest-parses). `mqk-md/src/instrument_registry_v2.rs` — `econ01`-`econ05` (validation) and `sug01`-`sug05` (suggestion helper). `mqk-cli/tests/scenario_cli_backtest_economics.rs` — `backtest_csv_economics_default_manifest_is_truthful`, `backtest_csv_economics_multiplier_50_reaches_manifest`. `mqk-cli/tests/scenario_cli_backtest_db_economics.rs` — `backtest_db_economics_multiplier_50_reaches_manifest`. `mqk-daemon/tests/scenario_backtest_jobs_01.rs` — `bj15`/`bj16` (default/explicit manifest economics via daemon job). `mqk-gui/src/features/backtests/__tests__/parsers.test.ts` — two `parseManifest` economics-passthrough/absence tests.

**Validation (all green):** `cargo test -p mqk-artifacts` 39/39, `cargo check -p mqk-artifacts` clean, `cargo clippy -p mqk-artifacts --all-targets -- -D warnings` clean. `cargo test -p mqk-backtest` all passing (untouched), `cargo check`/`clippy` clean. `cargo test -p mqk-md instrument_registry_v2` 36/36, `cargo test -p mqk-md instrument_registry` 63/63, `cargo check -p mqk-md` / `cargo clippy -p mqk-md --all-targets -- -D warnings` clean. `cargo test -p mqk-cli backtest` 17/17 (DB-backed tests ran for real against `MQK_DATABASE_URL`, not the skip path). `cargo check -p mqk-cli` / `cargo clippy -p mqk-cli --all-targets -- -D warnings` clean. `cargo test -p mqk-daemon --test scenario_backtest_jobs_01` 17/17, `--test scenario_backtest_economics_registry_suggestion` 5/5, `--test scenario_gui_daemon_contract_gate` 23/23, `--test scenario_route_contract_rt01` 2/2, `cargo check -p mqk-daemon` / `cargo clippy -p mqk-daemon --lib -- -D warnings` clean. `npm test -- --run` in `core-rs/mqk-gui` 538/538. `npm run build` succeeded (pre-existing chunk-size/dynamic-import warnings only). The recurring `sqlx-postgres` future-incompatibility warning is pre-existing and unrelated.

**Safety confirmation:** no daemon live/paper runtime was started; no DB was mutated (the DB-backed CLI test only reads/writes its own isolated `md_bars` test rows, deleted before and after); no broker/provider calls; no live routing; no paper/live orders; no runtime startup behavior changed (`mqk-cli/src/commands/run.rs` was never touched, and `init_run_artifacts`'s signature is unchanged so every other caller is unaffected); no non-equity trading enabled (`enabled`/`paper_trading_enabled`/`live_trading_enabled` are never read by the new economics code); `config/instruments/equities.json` untouched; no DB migration added; smoke logs and the untracked ledger draft were untouched.

`BACKTEST-MULTIPLIER-MARGIN-01` remains `PARTIAL`. Manifest economics and the registry-v2 economics metadata seam are now both real, tested, and proven additive. Still open before that parent can close: a real (non-v1-converted) v2 registry data source so the suggestion route's explicit-economics branch is reachable in production, and any decision about whether/how operators should be able to author such data outside of test fixtures.

**Recommended next slice:** if/when a real multi-asset data source exists, wire it as an explicit, separate v2 registry *file* (never replacing `config/instruments/equities.json`) and prove the suggestion route against it end-to-end; until then, this patch's helper-level proof is the honest ceiling.

### INSTRUMENT-REGISTRY-V2-SOURCE-01-COMBINED — CLOSED_LOCAL / PARTIAL

**Mission:** close the gap the prior entry's "Recommended next slice" named: add a separate, explicit `InstrumentRegistryV2` source the daemon's economics-suggestion route can read, so explicit non-equity/multiplier metadata can be proven end-to-end through the route — without replacing `config/instruments/equities.json`, without enabling any non-equity trading, and without touching broker/runtime/risk/OMS/DB code.

**Repo evidence found:** `backtest_economics_suggestion` (`mqk-daemon/src/routes/backtests.rs`) only ever read `AppState::instrument_registry_path` (v1, default `config/instruments/equities.json`), converted it to `InstrumentRegistryV2` in memory via `convert_v1_registry_to_v2`, and searched that. No env var or config path pointed at a real `InstrumentRegistryV2` file anywhere in the daemon; `load_instrument_registry_v2`/`validate_registry_v2`/`backtest_economics_suggestion_for_instrument` (all already proven by `BACKTEST-ECONOMICS-REGISTRY-MANIFEST-01-COMBINED`'s fixture-level tests) were unused by any route. No production trading path reads `InstrumentRegistryV2` today; `mqk-execution`, `mqk-runtime`, `mqk-risk`, `mqk-portfolio`, and broker adapters were not touched and remain untouched.

**Built:** added `AppState::instrument_registry_v2_path: Option<String>` (`mqk-daemon/src/state.rs`), sourced only from `MQK_INSTRUMENT_REGISTRY_V2_PATH`; `None` when unset — there is no fixed-path fallback, so a committed example fixture can never silently change route behavior without an explicit env var. `backtest_economics_suggestion` now: (1) if the v2 path is configured, loads+validates it first — a missing/unparseable file returns `registry_unavailable`, a parseable-but-invalid registry returns `validation_failed`, and **neither falls back to v1** (a bad explicit config must never look like quiet success); (2) if the symbol is found in a healthy configured v2 source, returns its suggestion (explicit economics, equity-default, or `no_contract_economics`) immediately; (3) otherwise (v2 unconfigured, or configured-and-healthy-but-symbol-absent) falls through to the pre-existing, byte-for-byte-unchanged v1 lookup. `BacktestEconomicsSuggestionResponse` gained `asset_class`/`enabled`/`paper_trading_enabled`/`live_trading_enabled: Option<...>` fields, populated truthfully whenever an instrument is matched (v1 or v2) and `null` otherwise, so an operator can never mistake a disabled/non-equity suggestion for trading permission. Added a committed reference fixture, `config/instruments/instruments_v2.backtest_suggestions.example.json` (schema_version 1, three disabled/non-tradable instruments — `ES_TEST`/`MES_TEST` futures with explicit `contract_multiplier`/margin economics, `BTCUSD_TEST` crypto pair with `contract_multiplier=1`), proven by two new `mqk-md` tests (`ex01`/`ex02`) that load+validate the real committed file and confirm every entry stays `enabled=false`/non-equity with explicit economics. The example file is **not** a default fallback path — it only affects route behavior in a deployment that explicitly sets `MQK_INSTRUMENT_REGISTRY_V2_PATH` to point at it.

**GUI:** `BacktestEconomicsSuggestionResponse` (TS) mirrors the four new fields. A new pure helper, `describeEconomicsSuggestionTradability` (`mqk-gui/src/features/backtests/parsers.ts`), returns a truthful "not enabled for trading (suggestion only)" warning when `enabled === false` and `null` when enablement is unknown or `true` — wired into the existing "Load registry economics" hint in `BacktestResultsScreen.tsx` alongside the already-present `asset_class` echo. The button's handler (`handleLoadEconomicsSuggestion`) was not touched: it already only sets local display state and never writes into the multiplier/margin form inputs, so the suggestion still cannot be auto-submitted. Three new `parsers.test.ts` unit tests cover enabled/disabled/unknown. No component-render test harness exists in this codebase (no `testing-library`/`jsdom` dependency) — display-logic proof is via the extracted pure function, matching this file's existing `describeNoTradeActivity`/`describeExecutionWarnings` pattern, not a rendered-DOM assertion.

**Tests added:** `mqk-daemon/tests/scenario_backtest_economics_registry_suggestion.rs` — `ber06`-`ber12` (no-v2-path-unchanged-plus-new-fields, explicit v2 economics, known-instrument-without-economics, missing-v2-path-fails-closed-without-v1-fallback, invalid-v2-registry-`validation_failed`, v2-symbol-absent-falls-back-to-v1, symbol-absent-from-both-`not_found`). `mqk-md/src/instrument_registry_v2.rs` — `ex01`/`ex02` (committed example fixture loads/validates/stays disabled, and yields `active`/`registry_v2_explicit` suggestions). `mqk-gui/src/features/backtests/__tests__/parsers.test.ts` — 3 new `describeEconomicsSuggestionTradability` tests.

**Validation (all green):** `cargo test -p mqk-daemon --test scenario_backtest_economics_registry_suggestion` 12/12, `--test scenario_gui_daemon_contract_gate` 23/23, `--test scenario_route_contract_rt01` 2/2, `cargo check -p mqk-daemon` clean, `cargo clippy -p mqk-daemon --lib -- -D warnings` clean. `cargo test -p mqk-md instrument_registry_v2` 38/38, `cargo test -p mqk-md instrument_registry` 65/65, `cargo check -p mqk-md` / `cargo clippy -p mqk-md --all-targets -- -D warnings` clean. `npm test -- --run` in `core-rs/mqk-gui` 541/541. `npm run build` succeeded (pre-existing chunk-size/dynamic-import warnings only). The recurring `sqlx-postgres` future-incompatibility warning is pre-existing and unrelated. `cargo test --workspace`, daemon live smoke, and provider/broker scripts were not run — none were required to prove this patch, consistent with the mission's own scoping.

**Safety confirmation:** no daemon live/paper runtime was started (all proof is `axum::Router::oneshot` in-process, matching the existing test pattern in this file); no DB was touched (this route has never taken a DB pool); no broker/provider network calls; no live routing; no paper/live orders; no runtime startup behavior changed; no non-equity trading enabled anywhere (the new v2 source is read-only, suggestion-only, and the committed example fixture's three instruments are all `enabled=false`/`paper_trading_enabled=false`/`live_trading_enabled=false`); `config/instruments/equities.json` untouched; no DB migration added; `mqk-execution`/`mqk-runtime`/`mqk-risk`/`mqk-portfolio`/broker adapters untouched; smoke logs and the untracked ledger draft were untouched.

`ASSET-CORE-01` / `BACKTEST-MULTIPLIER-MARGIN-01` status: still `PARTIAL`. The registry-v2 economics metadata seam can now be proven end-to-end through the daemon route from a real, separate, committed file — closing the specific gap `BACKTEST-ECONOMICS-REGISTRY-MANIFEST-01-COMBINED` left open — but `InstrumentRegistryV2` is still never read by any trading/execution/risk/OMS/ingestion path, and no production (non-test) non-equity data source exists. The v1 registry remains the sole source of trading truth.

**Recommended next slice:** if/when a real non-equity provider or data source exists, decide whether operators should be able to author production v2 registry entries through an authoring/ingestion workflow (still suggestion-only, still never trading-enabled) rather than hand-edited JSON; until then, this patch's separate-source proof is the honest ceiling.

### ASSET-CORE-01D-REGISTRY-V2-STATUS-01-COMBINED — CLOSED_LOCAL / PARTIAL

**Mission:** make `INSTRUMENT-REGISTRY-V2-SOURCE-01-COMBINED`'s separate v2 source (`MQK_INSTRUMENT_REGISTRY_V2_PATH`) and the existing static asset-capability matrix operator-visible through read-only daemon/GUI surfaces — without using `InstrumentRegistryV2` for trading, without replacing `config/instruments/equities.json`, and without enabling any non-equity asset class. Combines `INSTRUMENT-REGISTRY-V2-STATUS-API-01` + `INSTRUMENT-REGISTRY-V2-GUI-VISIBILITY-01` + `INSTRUMENT-REGISTRY-V2-CONTRACT-TESTS-01` + `ASSET-CAPABILITY-MATRIX-GUI-VISIBILITY-01` + `ASSET-CORE-01D-LEDGER-ANCHOR-01`.

**Repo evidence found:** `/api/v1/system/instrument-registry-v2/status` (`ASSET-CORE-01C`) already existed but answers a different question — it always reads `AppState::instrument_registry_path` (v1, `equities.json`) and reports the v1→v2 *conversion* diagnostic; it never reads `AppState::instrument_registry_v2_path` and is unaffected by `MQK_INSTRUMENT_REGISTRY_V2_PATH`. No route anywhere reported whether that *separate* v2 source was configured, valid, or disabled-for-trading — the only place its health was observable was indirectly, per-symbol, through `GET /api/v1/backtests/economics-suggestion`. Separately, `/api/v1/system/metadata` already returns a real, tested `asset_capability_matrix` (`ASSET-CAPABILITY-MATRIX-01`, static, all non-equity classes `enabled: false`), but the GUI's `MetadataSummary` TS type omitted the field entirely — `fetchOperatorModel` fetched it every cycle and silently dropped it before it ever reached a screen. The "Backtest Results" screen (`features/backtests/BacktestResultsScreen.tsx`) was the only screen with both an existing rendered UI for the v1/v2 registry economics path and in-scope file access for this patch; there is no dedicated "System" screen, and `features/system/*` in this repo is a pure data/helper layer with no `.tsx` rendering components of its own (`SettingsScreen.tsx`, which renders `model.metadata` today, sits in `features/settings/*`, outside this patch's file scope).

**Built — backend:** added `GET /api/v1/system/instrument-registry-v2-source/status` (`mqk-daemon/src/routes/system.rs::system_instrument_registry_v2_source_status`), deliberately a *different* path from `ASSET-CORE-01C`'s route to avoid colliding two semantically distinct surfaces. Reads only `AppState::instrument_registry_v2_path`; `None` → `truth_state="not_configured"`; `Some(path)` not on disk or unparseable → `"registry_unavailable"`; parses but fails `validate_registry_v2` → `"validation_failed"`; otherwise → `"configured_valid"` with full counts. No DB pool, no provider/broker calls, no writes — mirrors `ASSET-CORE-01C`'s proof shape exactly. `used_for_trading`/`enabled_for_live_trading`/`enabled_for_paper_trading` are hardcoded `false` on every branch (never derived from the configured file's own enablement flags), since the only production reader of this path remains the read-only economics-suggestion route. Added a pure helper, `mqk_md::instrument_registry_v2::summarize_instrument_registry_v2_status(&InstrumentRegistryV2) -> InstrumentRegistryV2Summary` (total/asset-class counts, enabled/paper/live counts, `non_equity_present`, `non_equity_all_disabled` — vacuously `true` with zero non-equity entries, `has_economics_metadata`, and up to `SAMPLE_SYMBOLS_LIMIT=10` sample symbols in registry order) — the route calls this rather than duplicating the counting logic inline, so the same counting code is unit-tested in `mqk-md` independent of axum/daemon plumbing.

**Built — GUI:** `MetadataSummary` (`features/system/types/system.ts`) gained `asset_capability_matrix?: AssetCapabilityMatrix` (optional — absent on the daemon-unreachable fallback object rather than fabricated) plus the matching `AssetCapabilityEntry`/`AssetCapabilityMatrix` interfaces; this alone closes the silent-drop bug (the field was already on the wire, just untyped). A new pure-helper module, `features/system/assetCapability.ts`, adds `assetCapabilityMatrixStatusLabel`, `nonEquityAssetClassState` (`"all_disabled" | "some_enabled" | "unknown"` — independently re-derives the answer from `entries`, not from trusting the backend's own `disabled_asset_classes` array verbatim), `describeNonEquityAssetClassState`, and `describeAssetCapabilityEntry`. `features/backtests/{types,api,parsers}.ts` gained the new route's response type, `getInstrumentRegistryV2SourceStatus()`, and three render helpers (`instrumentRegistryV2SourceStatusLabel`, `describeInstrumentRegistryV2SourceTradingUse`, `describeInstrumentRegistryV2SourceNonEquity`) — the trading-use/non-equity helpers derive their language from the response's own booleans (with a loud `WARNING:` branch) rather than hardcoding a "never used for trading" string that could go stale if the invariant were ever violated. Two new self-fetching panels, `InstrumentRegistryV2SourceStatusPanel` and `AssetCapabilityMatrixPanel`, were added to `BacktestResultsScreen.tsx` directly above the "B — Run a new backtest" panel (the same screen that already owns the per-symbol "Load registry economics" control fed by the same `MQK_INSTRUMENT_REGISTRY_V2_PATH` source); both fetch on mount via plain GET (`getInstrumentRegistryV2SourceStatus()` and `/api/v1/system/metadata` respectively), render an honest "Checking…" loading state, and fail closed to an explicit unavailable notice — never a fake healthy/empty render — confirmed live in the Vite dev-server preview with no daemon running (see Validation).

**Honest PARTIAL on placement:** per this patch's own strict file scope, the capability-matrix and registry-v2-source visibility live on the Backtest Results screen rather than a dedicated System/Settings screen, because `features/settings/*` (where `model.metadata` is rendered today) was outside the allowed GUI file list and no dedicated "System" screen exists in this repo. The data/helper/type layer (`features/system/types/system.ts`, `features/system/assetCapability.ts`) is fully wired and tested independent of this placement choice, so relocating the rendered panels to a future dedicated System screen — if one is added — would not require redoing the underlying logic.

**Tests added:** `mqk-daemon/tests/scenario_instrument_registry_v2_source_status_asset_core_01d.rs` — 10 new tests (`arc01d_01`-`arc01d_10`: not_configured, configured_valid against the committed `instruments_v2.backtest_suggestions.example.json` fixture with exact count/sample-symbol assertions, registry_unavailable, validation_failed, independence from `instrument_registry_path` (v1), idempotency, public/no-auth mounting, no-DB-pool). `mqk-md/src/instrument_registry_v2.rs` — 9 new tests (`v2status_01`-`v2status_06`, including one against the committed example fixture proving the exact mission-documented shape: 2 future + 1 crypto, all disabled, all carrying economics, sample symbols `["ES_TEST","MES_TEST","BTCUSD_TEST"]`). `features/system/assetCapability.test.ts` (new file; registered in `package.json`'s `test` script, the one out-of-strict-scope mechanical edit this patch made, justified because an unregistered `node:test` file under this repo's hardcoded `tsx --test <file-list>` runner would never execute) — 11 tests, including an adversarial fixture proving `nonEquityAssetClassState` inspects `entries` rather than trusting a stale `disabled_asset_classes` array. `features/backtests/__tests__/parsers.test.ts` — 12 new tests for the three new render helpers, including adversarial `used_for_trading: true` / `non_equity_all_disabled: false` fixtures proving the warning branches actually fire. `features/backtests/__tests__/api.test.ts` — 3 new tests mocking `globalThis.fetch` (the established pattern from `features/system/api.test.ts`, not previously used in this file) for not_configured, configured_valid, and 404-not-mounted.

**Validation (all green):** `cargo test -p mqk-daemon --test scenario_instrument_registry_v2_source_status_asset_core_01d` 10/10, `--test scenario_backtest_economics_registry_suggestion` 12/12, `--test scenario_gui_daemon_contract_gate` 23/23, `--test scenario_route_contract_rt01` 2/2, `--test scenario_instrument_registry_v2_status_asset_core_01c` 13/13 (regression, untouched route). `cargo check -p mqk-daemon` clean, `cargo clippy -p mqk-daemon --lib -- -D warnings` clean. `cargo test -p mqk-md instrument_registry_v2` 47/47, `cargo test -p mqk-md instrument_registry` 74/74, `cargo check -p mqk-md` / `cargo clippy -p mqk-md --all-targets -- -D warnings` clean. `npm test -- --run` in `core-rs/mqk-gui` 564/564. `npm run build` succeeded (pre-existing chunk-size/dynamic-import warnings only). Live GUI proof: started the Vite dev server via the repo's existing `.claude/launch.json` `mqk-gui-dev` config (no daemon process started), navigated to the Backtest Results screen, and confirmed via DOM snapshot that both new panels render their titles/subtitles immediately and transition from "Checking…" to an explicit `unavailable`/`Failed to fetch` notice once the unreachable-daemon fetch settles — no crash, no fabricated healthy state. The recurring `sqlx-postgres` future-incompatibility warning is pre-existing and unrelated. `cargo test --workspace`, daemon live smoke, and provider/broker scripts were not run — none were required to prove this patch.

**Safety confirmation:** no daemon live/paper runtime was started for backend proof (all daemon-side proof is `axum::Router::oneshot` in-process); the GUI live-preview check ran only the Vite dev server, not the daemon, and only ever observed honest "daemon unreachable" / "Failed to fetch" states; no DB touched (neither route has ever taken a DB pool); no broker/provider network calls; no live routing; no paper/live orders; no runtime startup behavior changed; no non-equity trading enabled anywhere; `config/instruments/equities.json` and `instruments_v2.backtest_suggestions.example.json` untouched (read-only); no DB migration added; `mqk-execution`/`mqk-runtime`/`mqk-risk`/`mqk-portfolio`/broker adapters untouched; `.env.local` not read or touched; smoke logs and the untracked ledger draft were untouched.

`ASSET-CORE-01` status: still `PARTIAL`. The separate v2 source's configuration/validity/disabled-non-equity status and the static asset-capability matrix are now both genuinely operator-visible (daemon route + GUI panels), closing the specific visibility gap this patch targeted — but `InstrumentRegistryV2` is still never read by any trading/execution/risk/OMS/ingestion path, the v1 registry remains the sole source of trading truth, and the new GUI surfaces live on the Backtest Results screen rather than a dedicated System screen (see "Honest PARTIAL on placement" above).

**Recommended next slice:** if a dedicated System/Settings screen is ever brought into a patch's file scope, move (not duplicate) the two new panels there and let `BacktestResultsScreen.tsx` link out to it instead; until then, the Backtest Results placement is the honest ceiling for this patch's scope.

### GUI-SYSTEM-STATUS-SURFACE-01-COMBINED — CLOSED_LOCAL

**Mission:** close `ASSET-CORE-01D-REGISTRY-V2-STATUS-01-COMBINED`'s GUI placement partial by moving the read-only InstrumentRegistryV2 source status and Asset Capability Matrix visibility off Backtest Results and onto the existing operator System/Status surface.

**Repo evidence found:** `Settings / Operations` already exists in `core-rs/mqk-gui/src/features/screens/screenRegistry.tsx`, is registered in the operator monitor group, is reachable from the left rail, and already renders daemon endpoint plus operations metadata from `model.metadata`. That is the current repo-native System/Status operator surface; no duplicate System screen was added.

**Built:** `InstrumentRegistryV2SourcePanel` and `AssetCapabilityMatrixPanel` were moved into `features/system/*` and rendered from `SettingsScreen`. Registry-v2 source status types and the read-only `GET /api/v1/system/instrument-registry-v2-source/status` GUI client now live under `features/system`, with compatibility re-exports left for the existing backtest tests. Backtest Results no longer renders or owns those unrelated system-status panels. The registry-v2 status helpers still label `not_configured`, `configured_valid`, `registry_unavailable`, and `validation_failed` truthfully; the asset capability helpers still fail closed on absent metadata and independently prove non-equity classes disabled from `entries`.

**Safety confirmation:** backend files were not changed; daemon routes were not changed; no trading/runtime/broker/risk/portfolio/OMS/outbox/inbox/DB migration path was changed; no daemon live/paper runtime was started; no broker/provider calls; no paper/live orders; no live routing; no non-equity trading was enabled; smoke logs and the untracked ledger draft were untouched.

**Validation:** GUI validation only: `npm test -- --run` and `npm run build` in `core-rs/mqk-gui`.

**Relation to parent:** this closes the GUI placement partial from `ASSET-CORE-01D-REGISTRY-V2-STATUS-01-COMBINED`. Registry source health and disabled non-equity capability truth are now visible from the operator System/Status surface rather than relying on Backtest Results placement or API-only inspection.

### ASSET-CORE-05-MARKET-CALENDAR-GENERALIZE-01-COMBINED — CLOSED_LOCAL / PARTIAL

**Mission:** add truthful, read-only session-profile diagnostics on top of the existing ASSET-CORE-05A/05B market-calendar/session model without changing live/paper trading behavior, broker behavior, risk gates, OMS/outbox/inbox, portfolio accounting, DB schema, or autonomous startup behavior.

**Built:** `mqk-daemon/src/state/market_calendar.rs` now has typed `SessionAuthority` (`authoritative`, `fallback`, `configured_override`, `unavailable`), `SessionProfileStatus`, and deterministic `supported_session_profiles()` over the repo-native profiles `equity_us_regular`, `crypto_continuous`, `futures_globex`, and `forex_24x5`. `/api/v1/system/session` gained additive fields: `session_profile`, `session_authority`, `session_profile_is_open`, `session_profile_reason_code`, `session_profile_message`, and `supported_session_profiles`. Current active behavior remains `equity_us_regular`; paper/backtest always-on calendar policy reports `configured_override`, while the current NYSE weekdays heuristic reports `fallback`.

**GUI:** `Settings / Operations` now renders a read-only Session profile panel showing profile, authority, open truth, reason, message, and supported profile labels. No session-profile controls were added, and the GUI fields are optional for compatibility with older daemon responses.

**Behavior unchanged:** existing `market_session`, `exchange_calendar_state`, and `calendar_spec_id` still come from the same `CalendarSpec` path as before. Autonomous readiness/session-window decisions still use the pre-existing `autonomous_session_schedule_from_env()` path with fixed UTC override when configured. The crypto/futures/FX profiles remain diagnostic/model-only scaffolds and are not used for admission, routing, broker submission, risk, OMS/outbox/inbox, portfolio accounting, or runtime startup.

`ASSET-CORE-05` remains `PARTIAL`: this closes the read-only diagnostics slice, but true per-instrument session routing, authoritative non-equity calendars, maintenance-break modeling by product/exchange, and any use of non-equity profiles in trading/admission remain deliberately unwired.

### ASSET-CORE-02-ORDER-INTENT-V2-FOUNDATION-01-COMBINED — CLOSED_LOCAL / PARTIAL

**Mission:** harden the existing inert `OrderIntentV2` / `ExecutionIntentV2` scaffold with a pure validation/routability contract and model-level multi-asset fixtures, without wiring v2 intents into live/paper trading or replacing the current equity order path.

**Repo evidence found:** `OrderIntentV2`, `ExecutionIntentV2`, and `equity_instrument()` were defined only in `core-rs/crates/mqk-execution/src/types.rs` and publicly re-exported from `mqk-execution/src/lib.rs` under the existing `RESEARCH-NON-EQ-01` warning. Targeted search found no runtime, daemon, OMS, broker, risk, portfolio, or strategy consumer of these v2 types. The live/paper dispatch path still uses `BrokerSubmitRequest` plus `BrokerGateway::submit_with_context`, where `MULTI-ASSET-ROUTING-GUARD-01` rejects every non-`Equity` `AssetClass` before any broker adapter can be invoked.

**Built:** `OrderIntentV2` gained additive inert model fields for `instrument_id`, v2-local `IntentV2Contract`, order type, limit/stop prices, time in force, strategy/source metadata, and a `research_only` marker. Existing `OrderIntentV2::new(instrument, side, qty)` remains and defaults to a market DAY model intent. New pure helpers return `IntentV2Validation { valid, routability, reason_code, message }` with `IntentV2Routability::{ResearchOnly, EquityRoutableCandidate, DisabledAssetClass, Invalid}`. Structural validation fails closed for missing symbol/currency, non-positive quantity, missing required limit/stop prices, and contract-shape mismatches. Equity and ETF-as-equity can validate as model-level candidates only; crypto/future/option/forex can validate structurally but return `DisabledAssetClass`. A caller-supplied routing request flag is intentionally ignored by `validate_model_with_caller_routing_request`, proving disabled non-equity cannot become routable through caller intent. `ExecutionIntentV2` gained the same pure validation surface for its wrapped `OrderSpec`.

**Tests added:** `core-rs/crates/mqk-execution/tests/scenario_order_intent_v2_foundation_01.rs` covers stock equity, ETF-as-equity (`asset_class=Equity`, `instrument_kind=etf`), disabled crypto spot, disabled futures contract, disabled option contract, disabled forex pair, invalid quantity, missing limit price, caller flag cannot make crypto routable, existing `equity_instrument()` behavior, and `ExecutionIntentV2::market` validation.

**Validation:** `cargo test -p mqk-execution order_intent_v2` - 9/9 focused v2 tests passed. `cargo test -p mqk-execution execution_intent_v2` - 1/1 passed. `cargo test -p mqk-execution --test scenario_asset_class_guard_multi_asset_routing_guard_01 --features testkit` - 8/8 passed. `cargo check -p mqk-execution` clean. `cargo clippy -p mqk-execution --all-targets --features testkit -- -D warnings` clean. `cargo test -p mqk-daemon --test scenario_asset_class_scope_b8` - 12/12 passed. The recurring `sqlx-postgres` future-incompatibility warning is pre-existing and unrelated.

**Deliberately not done:** no daemon/API/GUI status surface was added because the safe, smallest patch was pure model/tests/docs; existing `/api/v1/system/metadata` and GUI capability matrix remain unchanged. No broker adapters, Alpaca adapter, runtime startup, OMS/outbox/inbox schema or lifecycle, risk gates, `mqk-portfolio`, DB migrations, provider scripts, `.env.local`, `config/instruments/equities.json`, or InstrumentRegistryV2 trading path were touched.

`ASSET-CORE-02` remains `PARTIAL`: the v2 model contract is now better specified and tested, but v2 intents remain research/foundation-only and are still not production order types. Remaining gaps include a shared production-ready order schema decision, bracket/OCO/multi-leg extensions, and any future scope-reviewed bridge from model candidate to a guarded production path.

**Safety confirmation:** no daemon live/paper runtime was started; no DB was mutated; no broker/provider calls; no live routing; no paper/live orders; no runtime startup behavior changed; no non-equity trading was enabled; smoke logs and the untracked ledger draft were untouched.

### SHORT-SIDE-EXTERNAL-SIGNAL-WIRING-01-COMBINED — CLOSED_LOCAL / PARTIAL

**Mission:** wire the existing short-entry policy into the external `strategy_signal` path, add a read-only broker asset shortable-preflight status surface, and extend operator paper flatten so canonical close orders cover short positions. This patch does not enable live shorting by default and does not submit orders during proof.

**Repo evidence found:** short-entry policy and intent classification already lived in `mqk-daemon/src/capital_policy/short_entry_policy.rs`. The internal/native decision path already blocked short-open intents through that policy, but the external signal route did not. `flatten-paper-positions` rejected short positions even though the canonical helper already generated `side="buy"` for negative `net_qty`. Alpaca asset shortability parsing was already fixture-tested, but no daemon route exposed a read-only preflight status.

**Built:** `routes/strategy.rs` now evaluates short-entry policy before external signals can reach outbox enqueue. It classifies the signal delta against the current execution snapshot, treats missing snapshot state as flat for fail-closed sell-from-flat behavior, rejects ambiguous duplicate same-symbol snapshot rows, reads the existing capital-policy JSON from `MQK_CAPITAL_POLICY_PATH`, and denies short opens when the policy is absent/disabled, unavailable, or the broker preflight cannot prove tradable/shortable/easy-to-borrow truth. The read-only route `GET /api/v1/broker/assets/:symbol/shortable-preflight` reports `active`, `not_configured`, `unsupported_adapter`, `symbol_not_found`, `broker_unavailable`, or `query_failed` without exposing submit/cancel/replace behavior. `mqk-broker-alpaca` gained only `GET /v2/assets/{symbol}` asset fetch plumbing. `flatten-paper-positions` now closes both long and short paper positions through canonical market close JSON, with duplicate/blank-symbol snapshot ambiguity rejected before enqueue.

**Tests added/updated:** `scenario_short_side_external_signal_gate_01.rs` proves default-off sell-from-flat denial, enabled-policy denial when preflight is unavailable/not-shortable, allowed pass-through to the existing downstream WS gate when shortability is proven, sell-to-reduce-long unchanged, and sell-beyond-long following short-entry policy. `scenario_shortable_preflight_route_01.rs` proves not-configured, active shortable true/false, query-failed, broker-unavailable, and symbol-not-found route truth states. `scenario_paper_flatten_psf01.rs` now proves a short paper position enqueues a canonical buy-to-cover close order and ambiguous duplicate position snapshots fail closed.

**Validation:** targeted Rust validation only. The default target directory could not be reused because an existing local `mqk-daemon.exe` process held `core-rs/target/debug/mqk-daemon.exe` open, so all subsequent proof used `C:\tmp\mqk-target-short-side`. Passed: `cargo fmt -p mqk-daemon -p mqk-execution -p mqk-broker-alpaca`; `cargo check -p mqk-daemon -p mqk-execution -p mqk-broker-alpaca`; `cargo test -p mqk-daemon --test scenario_short_entry_policy_gates_01`; `cargo test -p mqk-daemon --test scenario_short_side_intent_model_01`; `cargo test -p mqk-daemon --test scenario_short_side_shortable_preflight_01`; `cargo test -p mqk-daemon --test scenario_short_side_flatten_proof_01`; `cargo test -p mqk-daemon --test scenario_short_side_external_signal_gate_01`; `cargo test -p mqk-daemon --test scenario_shortable_preflight_route_01`; `cargo test -p mqk-daemon --test scenario_paper_flatten_psf01`; `cargo test -p mqk-portfolio --test scenario_short_position_lifecycle_01`; `cargo test -p mqk-reconcile --test scenario_short_position_reconcile_01`; `cargo test -p mqk-broker-alpaca --test scenario_alpaca_asset_shortable_preflight_01`; `cargo test -p mqk-daemon --test scenario_asset_class_scope_b8`; `cargo test -p mqk-execution --test scenario_asset_class_guard_multi_asset_routing_guard_01 --features testkit`; `cargo test -p mqk-execution --test scenario_asset_risk_router_foundation_01`; `cargo clippy -p mqk-daemon --lib -- -D warnings`; `cargo clippy -p mqk-execution --all-targets --features testkit -- -D warnings`; `cargo clippy -p mqk-broker-alpaca --all-targets -- -D warnings`; `cargo fmt -p mqk-daemon -p mqk-execution -p mqk-broker-alpaca -- --check`.

**Safety confirmation:** no daemon live/paper runtime was started by this patch's proof, no provider/live/paper smoke was run, no broker submit/cancel/replace path was invoked, no DB migration was added, no full workspace test was run, and no paper/live order was submitted. Existing recurring `sqlx-postgres` future-incompatibility warnings are unrelated.

**Residual risk / PARTIAL:** local short-side wiring and canonical flatten coverage are closed. Market-hours proof remains blocked by the separate stale intraday data/provider freshness gap called out before this patch; retrying provider or market proof is deliberately out of scope here.

### ASSET-CORE-03-RISK-ROUTER-FOUNDATION-01-COMBINED — CLOSED_LOCAL / PARTIAL

**Mission:** add a pure asset-aware risk-router foundation that defines static per-asset-class policy truth without wiring that router into live/paper order execution, broker submit, daemon strategy routes, `mqk-risk`, OMS/outbox/inbox, portfolio accounting, DB schema, or runtime startup.

**Repo evidence found:** disabled-asset Gate 0 remains in `mqk-daemon/src/routes/strategy.rs` and is regression-covered by `scenario_asset_class_scope_b8`; the broker-submit routing guard remains in `mqk-execution/src/gateway.rs::BrokerGateway::submit_with_context`, rejecting non-`Equity` `AssetClass` before any broker adapter is invoked; `/api/v1/system/metadata` still builds the static asset capability matrix in `mqk-daemon/src/routes/system.rs::static_asset_capability_matrix`; ETF sector-risk policy lives in `mqk-portfolio::evaluate_sector_risk` with daemon glue in `capital_policy::sector_risk_gate`; `OrderIntentV2` validation/routability lives in `mqk-execution/src/types.rs`. No existing asset-aware risk policy model beyond binary disabled/equity-only gates was found, and `mqk-risk` was not touched because doing so would risk altering live enforcement behavior.

**Built:** added `mqk_execution::asset_risk_policy`, a pure/static model with `AssetRiskPolicyState::{Enabled, Disabled, ResearchOnly, Unsupported}`, `AssetRiskPolicy`, `AssetRiskRouteDecision::{AllowedEquity, DisabledAssetClass, ResearchOnly, Unsupported, Invalid}`, and `AssetRiskRouteEvaluation`. `default_asset_risk_policies()` now summarizes equity, ETF-as-equity, crypto, future, option, forex, and rates/fixed-income scaffolds. `evaluate_asset_risk_for_order_intent_v2()` validates `OrderIntentV2` first, returns `Invalid` before policy routing for structural failures, maps equity and ETF-as-equity to model-level `AllowedEquity`, and maps crypto/future/option/forex to `DisabledAssetClass`. `evaluate_asset_risk_for_order_intent_v2_with_caller_routing_request()` intentionally ignores the caller flag, proving caller intent cannot promote disabled non-equity to routable. Static constants state `ASSET_RISK_PRODUCTION_ENFORCEMENT_ENABLED=false` and `ASSET_RISK_NON_EQUITY_ROUTING_ENABLED=false`.

**Tests added:** `core-rs/crates/mqk-execution/tests/scenario_asset_risk_router_foundation_01.rs` covers static policy source/model-only flags, default policy summaries, ETF-as-equity modeling, equity and ETF model-level allowed candidates, crypto/future/option/forex disabled decisions, invalid quantity, missing limit/stop prices, caller-flag resistance, non-equity dependency metadata, and unsupported policy lookup.

**Deliberately not done:** no daemon/API/GUI status surface was added in this slice; the existing `/api/v1/system/metadata` capability matrix and GUI surfaces remain unchanged. No broker adapters, Alpaca adapter, live routing, runtime startup behavior, OMS/outbox/inbox schema or lifecycle, `RiskRequestContext`, `mqk-risk` live enforcement, `mqk-portfolio` accounting, DB migrations, provider calls, broker calls, `.env.local`, `config/instruments/equities.json`, or InstrumentRegistryV2 trading path were touched.

**Validation:** `cargo fmt --all` hit a Windows path-length error (`os error 206`), so the touched package was formatted with `cargo fmt -p mqk-execution` successfully. `cargo test -p mqk-execution order_intent_v2` passed (5/5 new-bridge-filtered tests plus 9/9 existing OrderIntentV2-filtered tests). `cargo test -p mqk-execution execution_intent_v2` passed (1/1). `cargo test -p mqk-execution asset_risk` passed (2/2 filtered). `cargo test -p mqk-execution --test scenario_asset_risk_router_foundation_01` passed (13/13). `cargo test -p mqk-execution --test scenario_order_intent_v2_foundation_01` passed (11/11). `cargo test -p mqk-execution --test scenario_asset_class_guard_multi_asset_routing_guard_01 --features testkit` passed (8/8). `cargo check -p mqk-execution` clean. `cargo clippy -p mqk-execution --all-targets --features testkit -- -D warnings` clean after replacing two constant runtime assertions with const assertions. `cargo test -p mqk-daemon --test scenario_asset_class_scope_b8` passed (12/12). The recurring `sqlx-postgres` future-incompatibility warning is pre-existing and unrelated.

`ASSET-CORE-03` remains `PARTIAL`: a pure model/router foundation now exists and is tested, but it is not production enforcement, not operator-surfaced by daemon/GUI metadata yet, and not wired into any live/paper execution path. Existing fail-closed disabled-asset gates remain the active enforcement boundary.

**Safety confirmation:** no daemon live/paper runtime was started; no DB was mutated; no broker/provider calls; no live routing; no paper/live orders; no runtime startup behavior changed; no non-equity trading was enabled; smoke logs and the untracked ledger draft were untouched.
