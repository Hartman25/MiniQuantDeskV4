# AUTONOMOUS-DAILY-PAPER-OPERATIONS-01A — Current-Truth Audit and Binding Contract

Patch ID: `AUTONOMOUS-DAILY-PAPER-OPERATIONS-01A-AUDIT-AND-CONTRACT`
Bundle: `AUTONOMOUS-DAILY-PAPER-OPERATIONS-01-COMBINED`
Starting HEAD: `ee026f5f31511304d9b226d464f3df0e6526ddad` ("docs: close daily data readiness")
Scope: audit only. No production code, test code, or migration is changed by this patch.

---

## 1. Executive disposition

The current daemon already runs a real, unattended, 30-second polling control loop
(`session_controller.rs`) that auto-arms and auto-starts a PAPER execution runtime through the
canonical `AppState::start_execution_runtime` gate chain, and auto-stops it when the session
window closes and the controller believes it owns the run. That much of "autonomous daily
operation" is real today, not aspirational.

What is missing is **durability, exactly-once dispatch, typed retry discipline, and canonical
truth surfaces** around that loop:

- There is no durable, restart-safe "daily operation" identity or state machine. All ownership
  and retry state (`locally_started`) lives in a single `bool` on the controller task's stack —
  it is lost on daemon restart and shared by nothing else.
- The completed-bar dispatch path (`autonomous_bar_ticker.rs`) is a blind 60-second timer, not a
  bar-completeness-driven trigger. It does not check whether a new canonical bar actually closed
  before depositing a dispatch trigger.
- The market-data latest-closed-bar scheduler is fully decoupled from session lifecycle — it is
  operator-route-started/stopped only, with no preopen-start/postclose-stop coordination.
- Retry behavior on a refused start has no typed transient/terminal classification and no
  backoff: a refused start (and, separately, an unexpected run end) triggers a fresh Discord
  alert on every 30-second tick with no dedup, for as long as the blocker persists.
- Task supervision is inconsistent: only the session-controller task has an exit watchdog (and
  it only observes/records death, never restarts). The WS transport task and the bar-ticker task
  have no watchdog at all. The reconcile-tick task's `JoinHandle` is discarded outright.
- No route or GUI surface exposes a canonical current/historical daily-operation state. The
  closest existing thing, `autonomous_paper_status`'s `readiness_classification` field, is a
  coarse three-value readiness label (`blocked` / `ready_for_market_smoke` /
  `market_proof_pending`), not a per-trading-day lifecycle with reason codes, retry counts, or a
  durable outcome.
- No test in the existing suite proves a DB-backed session-controller tick driving
  `start_execution_runtime` to a successful (`Ok`) result. `scenario_autonomous_paper_day_lifecycle_auton12.rs`'s
  own header explicitly disclaims this ("That path requires a DB, which is proven by the gate
  chain reaching the DB fault in AL-02"). Every `start_execution_runtime()` call in
  `scenario_daily_data_readiness_start_gate_01.rs` (18 call sites) asserts `.expect_err(...)`.

All 16 "current source truths" enumerated in the bundle prompt were checked against source and
are confirmed **true** (see §12 gap table for citations). No correction to the Phase A audit
leads was required. This document freezes the binding contract in §13 for Phases B–G, refines
the durability model in §14 against actual schema (§8), and reconciles the retry/no-trade
contract in §15–§16 against actual reason-code surfaces already in the codebase.

Disposition: proceed to Phase B on the contract below.

---

## 2. Current process/task startup graph

`main()` (`core-rs/crates/mqk-daemon/src/main.rs`) runs strictly sequentially — there is no
supervisory framework:

1. Load env, init tracing (`main.rs:18-53`).
2. `mqk_db::connect_from_env()` — mandatory; boot fails without DB (`main.rs:55-57`).
3. `AppState::new_with_db(db)` (`main.rs:59`).
4. `shared.seed_ws_continuity_from_db().await` (`main.rs:64`) — must precede WS task start.
5. `shared.seed_broker_baseline_from_db().await` (`main.rs:69`) — must precede `spawn_reconcile_tick`.
6. Operator-auth-mode logging (`main.rs:71-84`).
7. `state::spawn_heartbeat(bus, 1s)` (`main.rs:85`).
8. `shared.try_ws_gap_auto_recovery().await` — one-shot startup REST gap-fill (`main.rs:92-111`).
9. `state::spawn_alpaca_paper_ws_task(...)` → `_alpaca_ws_handle` (`main.rs:115`). Handle held,
   **never awaited, no watchdog** ("kept alive for the lifetime of the daemon" per comment).
10. `state::spawn_autonomous_session_controller(...)` → `session_controller_handle` (`main.rs:119`).
11. `state::spawn_autonomous_bar_ticker(...)` → `_bar_ticker_handle` (`main.rs:125`). Same as (9):
    held, never awaited, **no watchdog**.
12. Startup-outcome consistency log — warns if WS started but controller/ticker did not
    (`main.rs:134-156`).
13. Session-controller **exit watchdog** — the only task with one (`main.rs:158-193`; see §10).
14. Build axum router, bind, serve with graceful shutdown calling `stop_for_shutdown()` on Ctrl-C
    (`main.rs:195-213`).

Nothing restarts a crashed task. The session-controller watchdog only observes and records death
(`AutonomousSessionTruth::ControllerExited`); it does not respawn `run_session_controller`.

---

## 3. Current session-controller state machine

`session_controller.rs`, `SESSION_POLL_INTERVAL: Duration = Duration::from_secs(30)`
(`session_controller.rs:58`), driven by `tokio::time::interval` in `run_session_controller`
(`session_controller.rs:193,197`).

**Ownership flag**: `locally_started: bool`, a plain local variable on the controller task's
stack (`session_controller.rs:194`), passed `&mut` into `run_session_controller_tick`. It is
process-local and in-memory only — not persisted, not derived from DB truth, and lost on daemon
restart. It is distinct from `has_active_run` (`state.locally_owned_run_id().await.is_some()`,
`session_controller.rs:209`), which is daemon-wide truth about whether *any* non-finished
execution-loop handle exists in-process, regardless of who started it.

**State machine** (`run_session_controller_tick`, `session_controller.rs:202-249`), matched on
`(in_session, locally_started, has_active_run)`:

| in_session | locally_started | has_active_run | Action |
|---|---|---|---|
| true | true | true | no-op (steady state) |
| true | true | false | run died unexpectedly: reset `locally_started=false`, set `RunEndedUnexpectedly`, fire Discord alert — **every tick this holds**, no dedup guard (`session_controller.rs:213-236`) |
| true | false | — | `attempt_auto_start` (`session_controller.rs:237-238`) |
| false | true | true | `attempt_auto_stop` (`session_controller.rs:240-241`) |
| false | false | true | no-op — operator-managed run outside session window, untouched |
| false | — | false | reset `locally_started=false`, clear truth (`session_controller.rs:244-247`) |

**Refused start retried next tick?** Yes — `attempt_auto_start` (`session_controller.rs:251-323`)
is re-entered every 30s while `(in_session, locally_started=false)` holds, because a failed
start never sets `locally_started = true`.

**Dedup/backoff on repeated identical refusals?** None for the alert channel.
`attempt_auto_start`'s `Err` branch (`session_controller.rs:299-320`) unconditionally calls both
`state.set_autonomous_session_truth(StartRefused{...})` and
`state.discord_notifier.notify_critical_alert(...)` on every failed tick.
`set_autonomous_session_truth` (`state.rs:1422-1429`) short-circuits the **DB event write** only
when the new truth value is byte-identical to the current one (`if current == truth { return; }`)
— but the Discord alert fires unconditionally regardless of that dedup, every 30s a refusal
recurs. The `(true, true, false)` `RunEndedUnexpectedly` branch has the same unguarded-alert
behavior.

**Stop only when controller believes it locally started the run**: confirmed exactly.
`attempt_auto_stop` is only reachable from the `(false, true, true)` arm
(`session_controller.rs:211,240-241`). If `locally_started == false` (operator-started run, or
attribution lost across a controller-task restart), the `(false, false, true)` arm is a pure
no-op (`session_controller.rs:243`), matching the module doc's stated contract
(`session_controller.rs:36-37`).

---

## 4. Current session/calendar authority

A typed authority exists: `market_calendar.rs` defines `MarketSessionState`
(`RegularOpen|PreMarket|AfterHours|Closed|Holiday|EarlyClose|Unknown`,
`market_calendar.rs:52-67`), `MarketSessionTruth`, and the `MarketCalendarProvider` trait with
`NyseWeekdaysProvider`, `FixedWindowOverrideProvider`, and `ExchangeSourcedCalendarProvider`
implementations (`market_calendar.rs:96-258,345+`). It is genuinely used elsewhere:
`routes/system.rs:1995`, `routes/portfolio.rs:304`, `routes/control_plane.rs:1838`,
`state/runtime_session_source.rs:208,551,1034,1036`.

**It is not consulted uniformly.** The two components that actually gate autonomous execution
each bypass it with their own raw calendar check:

- `session_controller.rs`'s `AutonomousSessionSchedule::is_in_session` calls
  `CalendarSpec::NyseWeekdays.classify_market_session(ts) == "regular"` directly
  (`session_controller.rs:87-88`), not `NyseWeekdaysProvider::session_for`.
- `autonomous_bar_ticker.rs` independently re-does the same string-comparison check
  (`autonomous_bar_ticker.rs:114-115`).

Both today resolve to the same underlying function
(`NyseWeekdaysProvider::session_for` itself wraps `CalendarSpec::NyseWeekdays.classify_market_session`,
`market_calendar.rs:178`), so there is currently no observed disagreement — but there are three
independent call sites for what should be one authority (the third being `state.rs:3761,3861`).
This is exactly the duplication the bundle's binding safety contract ("do not maintain separate
session-open calculations") forbids going forward.

`AutonomousSessionSchedule::is_in_session` additionally layers the ASSET-CORE-05E
`RUNTIME_SESSION_SOURCE` hook: `legacy`/`v2_equity_shadow` pass the legacy boolean through
unchanged; only explicit `v2_equity_active` substitutes
`evaluate_runtime_session_source_active_decision(...).in_session_fail_closed()`, fail-closed to
`false` on a v2 registry-load failure (`session_controller.rs:90-102`).

The strict daily-data-readiness evaluator (`daily_data_readiness.rs`, Bundle 2) is a **separate,
third** calendar/session composition, consumed by `market_data_readiness.rs`'s
`compute_daily_data_readiness_response` and shared by `system/preflight`,
`autonomous/readiness`, and `market-data/ingest-plan` (see §6, §11). It is not consulted by
`session_controller.rs` or `autonomous_bar_ticker.rs` at all today.

---

## 5. Current data-refresh lifecycle

The market-data latest-closed-bar scheduler (`MarketDataFeedSchedulerRuntimeState`,
`state.rs:201-249`) is started/stopped **exclusively** via two operator HTTP routes:
`POST /api/v1/market-data/feed/scheduler/start` (`routes/ingest.rs:1282`, wired `routes.rs:767`)
and `POST /api/v1/market-data/feed/scheduler/stop` (`routes/ingest.rs:1422`, wired `routes.rs:771`).

No file in `main.rs`, `session_controller.rs`, or `lifecycle.rs` references this scheduler
(confirmed by grep — only `daily_data_readiness.rs`, `api_types.rs`, `state.rs` struct def,
`routes/ingest.rs`, `routes.rs` match). There is no auto-start-on-preopen or auto-stop-on-postclose
logic. `MarketDataFeedSchedulerRuntimeState::default()` is `running: false` at boot
(`state.rs:224-249`) — off by default, untouched by daemon boot.

The provider-call logic itself is not extracted into a reusable internal seam: the
`POST /api/v1/market-data/feed/poll-once` handler, `market_data_feed_poll_once`
(`routes/ingest.rs:518-1054`, ~535 lines), does validation → provider registry load → instrument
registry load/validate → provider client construction → per-symbol admission → provider
`fetch_latest_closed_bar()` calls → `mqk_db::md::ingest_provider_bars_to_md_bars_with_provider_metadata()`
writes, all inline in the one Axum handler. The scheduler's own poll loop,
`execute_scheduler_poll_once` (`ingest.rs:1150-1176`), calls this handler function directly (not
over real HTTP) but still marshals through the full `State`/`Json` extractor signature and
`axum::body::to_bytes` + `serde_json::from_slice` against the `Response`/JSON boundary — an
in-process call disguised as an HTTP round-trip, not a decoupled coordinator function. Phase C's
"reuse the internal scheduler/coordinator seam" requirement is a real extraction, not a trivial
rename.

`market_data_readiness.rs`'s `compute_daily_data_readiness_response` (Bundle 2) already performs
bounded `md_bars` reads and is genuinely read-only (module doc `market_data_readiness.rs:12-18`:
"No provider call, no broker call, no ingest job, no scheduler start, no runtime start, no arm,
no run/outbox row, no readiness event persisted") — confirmed by the closure doc
`docs/specs/daily_data_readiness_01e_closure_decision.md` §15.

---

## 6. Current completed-bar dispatch lifecycle

`autonomous_bar_ticker.rs`, default cadence 60s (`DEFAULT_BAR_INTERVAL_SECS = 48`... actually
`= 60`, `autonomous_bar_ticker.rs:48`), overridable via `MQK_STRATEGY_BAR_INTERVAL_SECS`
(`autonomous_bar_ticker.rs:51-58`), `tokio::time::interval` with `MissedTickBehavior::Skip`
(`autonomous_bar_ticker.rs:88-89`).

**Does not check for a genuinely new completed bar.** `run_bar_tick`
(`autonomous_bar_ticker.rs:100-147`) gates on exactly three conditions — WS continuity `Live`
(`:102-110`), NYSE session `"regular"` via the raw calendar check (`:112-121`, see §4), and
`day_signal_limit_exceeded()` false (`:123-131`) — none of which is bar completeness or newness.
If all three pass, it unconditionally deposits `StrategyBarInput { now_tick, end_ts: session_ts
("now"), limit_price: None, qty }` (`:135-142`). The module doc is explicit: "Fabricate market
data or historical price bars — `limit_price` is always `None`" (`:21-24`). This is a blind
timer trigger, not a canonical-bar-close trigger.

**Stub fallback** happens downstream, not in the ticker: `AppState::dispatch_native_strategy_for_symbol_with_bar`
(`state.rs:2422-2496`) tries a real DB-backed bar window via
`mqk_db::fetch_recent_completed_bars_for_strategy` (`:2433-2439`) first; on DB error or unset
`symbol`/`timeframe` it falls through to an empty-stub context (`state.rs:2478-2496`,
"B1B legacy path", safe-by-construction since empty bars → strategies return hold/flat). When
real DB bars *are* found but stale/missing, the separate `MD-STALENESS-PER-TICK-GATE-01` gate
blocks outright (`state.rs:2125-2226`) rather than falling to the stub — two distinct fail-closed
branches, neither triggered by anything in the ticker itself checking "is this a new bar."

---

## 7. Current runtime start/stop/recovery lifecycle

`AppState::start_execution_runtime(self: &Arc<Self>) -> Result<StatusSnapshot, RuntimeLifecycleError>`
(`lifecycle.rs:47-49`) traverses, in exact order:

1. `lifecycle_op` mutex lock (`:50`)
2. `reap_finished_execution_loop()` (`:51`)
3. `deployment_readiness().start_allowed` (`:53-62`)
4. `integrity.is_execution_blocked()` (`:64-70`)
5. LiveCapital real-operator-token requirement (`:72-81`)
6. Already-owned-run conflict via `active_owned_run_id()` (`:83-88`)
7. BRK-00R-04 Paper+Alpaca WS-continuity-proven gate (`:90-120`)
8. BRK-09R reconcile-truth gate (dirty/stale refused) (`:122-156`)
9. Live-capital WS-continuity-proven gate (`:158-184`)
10. TV-01/TV-02C artifact intake + deployability gate (`:186-260`)
11. TV-03C parity evidence gate + artifact-id cross-validation (`:262-340`)
12. TV-04F live-capital explicit capital policy requirement (`:342-372`)
13. TV-04A capital allocation policy gate (`:374-414`)
14. TV-04D deployment economics gate (`:416-459`)
15. B1A native strategy bootstrap gate incl. STRATEGY-DORMANCY-01 (`:461-545`)
16. DAILY-DATA-READINESS-01C-ENFORCEMENT-01 strict daily-data readiness gate + pre-start evidence
    persistence (`:547-701`)
17. `db_pool()?` (`:703`)
18. B2A DB strategy-registry gate (`:705-755`)
19. PREMARKET-DATA-READINESS-GATE-01 multi-symbol market-data freshness gate (`:757-804`)
20. `create_or_reuse_run_for_start(&db)` (`:806`, conflict rules `:950-1043`)
21. `daily_data_readiness::advance_run_to_active(...)` via `ProductionRuntimeStartEffects`
    (`:808-871`, effects impl `:1330-1519`)
22. `spawn_reconcile_tick(...)` (`:873-927`) — **`JoinHandle` discarded, no supervision** (see §10)
23. Build/publish running `StatusSnapshot` (`:929-939`)

This is the canonical start path Phase B–G must continue to route every autonomous start
attempt through, unmodified, per the bundle's binding safety contract.

---

## 8. Current durable evidence

No `daily_operation` concept exists anywhere in the daemon (`grep "daily_operation"` across
`src/` returns zero matches). What exists:

- `sys_autonomous_session_events` (migration `0032_autonomous_session_events.sql:11-19`) — an
  **append-only transition log**, not a stateful current-state table. Columns: `id text PK`,
  `ts_utc`, `event_type`, `resume_source`, `detail`, `run_id`, `source`. Written via
  `AppState::persist_autonomous_session_truth_event` (`state.rs:1559-1599`) every time the
  in-memory `AutonomousSessionTruth` value changes, `ON CONFLICT (id) DO NOTHING`
  (`arm_state.rs:283`). No unique-per-day constraint, no CAS semantics — module comment states
  explicitly it is "history/evidence, not the current active-alert surface"
  (`0032_autonomous_session_events.sql:4-5`). On write failure, `autonomous_history_degraded`
  sticks true for the process lifetime (`state.rs:1563-1567,1595-1605`).
- `runs` (`0001_init.sql` + `0002_run_lifecycle.sql`) — `run_id uuid PK`, `status` (CREATED/
  ARMED/RUNNING/STOPPED/HALTED), lifecycle timestamps. No natural daily/date key; only guard is
  a partial unique index `uq_live_engine_active_run on (engine_id) where mode='LIVE' and status
  in ('ARMED','RUNNING')` — LIVE-only, not applicable to PAPER. `arm_run`'s check-then-update
  (`runs.rs:424-452`) is not atomic CAS at the SQL level for PAPER/BACKTEST.
- Neither table can support idempotent daily identity, restart-safe current-state projection
  keyed by trading day, or concurrent-transition CAS protection as-is. A new durable model is
  required (§14).

**Deterministic-ID convention already established** — reuse this, do not invent a new one.
Canonical helper: `derive_event_id` in `core-rs/crates/mqk-audit/src/lib.rs:301-306`:

```rust
fn derive_event_id(prev_hash: Option<&str>, payload: &Value, seq: u64) -> Result<Uuid> {
    let payload_canonical = canonical_json_line(payload)?;
    let prev = prev_hash.unwrap_or("");
    let data = format!("mqk-audit.event.v1|{}|{}|{}", prev, payload_canonical, seq);
    Ok(Uuid::new_v5(&Uuid::NAMESPACE_DNS, data.as_bytes()))
}
```

The same `Uuid::new_v5(NAMESPACE_DNS, "mqk.<domain>.v1|...")` pattern is used by
`strategy_signal_evaluations.evaluation_id` (`0043_strategy_signal_evaluations.sql:10-13`) and
`fill_quality_telemetry.telemetry_id` (`0028_fill_quality_telemetry.sql:6`). Phase B's
`operation_id` must follow this same `mqk.autonomous-daily-operation.v1|{market_date}|{...}`
style, not `Uuid::new_v4()`.

**Migration numbering**: highest applied migration is `0047_strategy_promotion_transition_lineage.sql`.
A Phase B migration, if required, is `0048_...`. Every migration file has a corresponding entry
in `core-rs/crates/mqk-db/migrations/manifest.json` (enforced by that file's own validation
rule) — a new migration must add one.

**`DEFAULT now()` / `DEFAULT gen_random_uuid()`**: confirmed present only in migrations numbered
`< 0012` (whitelisted "bookkeeping baseline," per `0012_drop_reconcile_checkpoint_default.sql:14-23`
and `scripts/guards/check_unsafe_patterns.ps1:16,99-104,279-312`, which only checks migrations
`>= 0012`). Zero occurrences of `gen_random_uuid()` as an actual default anywhere. Any new Phase
B migration must supply `created_at_utc`/`updated_at_utc`/ID values from the caller, matching
every migration `>= 0018` in the repo.

---

## 9. Current no-trade observability

`autonomous_no_trade_diagnostics` (`state.rs:2370-2411`) is a per-minute-bucket diagnostic
snapshot journal, deduplicated via `Uuid::new_v5` keyed on `(reason_code, stage, minute_bucket)`
(`:2380-2387`), written from the `GET /api/v1/autonomous/readiness` handler
(`autonomous_readiness`, `routes/system.rs:1091-1128`) on every call — **a genuine DB write
triggered by a GET request**, an intentional, code-commented deviation (tag
`AUTON-NO-TRADE-OFFHOURS-01B`) from strict GET/read-only semantics, best-effort/non-fatal on
failure. This is useful, minute-resolution diagnostic telemetry, but it is not an end-of-day
outcome classification: there is no `completed_no_trade` / `completed_with_activity` rollup, no
evidence-hierarchy distinction between "no bar" vs "bar with no signal" vs "signal with no
accepted decision" vs "outbox without submission" vs "submission without fill" surfaced as a
single daily verdict anywhere today.

`strategy_signal_evaluations` (`0043_strategy_signal_evaluations.sql`) durably journals one row
per dispatch attempt (`evaluation_id` UUIDv5 of `run_id|strategy_id|symbol|timeframe|now_tick`,
`ON CONFLICT DO NOTHING`) via `AppState::dispatch_native_strategy_for_symbol_with_bar` →
`tick_strategy_dispatch_for_symbol`, proven by `scenario_signal_evaluation_journal_auton_no_signal_obs_01.rs`
(SO-01..SO-06) to persist correctly and to create zero `oms_outbox` side effects on its own —
this is the durable per-evaluation building block Phase E's evidence hierarchy should read from,
but nothing today aggregates it into a daily outcome.

---

## 10. Current task supervision

Only `session_controller` has an exit watchdog (`EXEC-OBS-LIVENESS-01`, `main.rs:158-193`) — it
`.await`s the controller's `JoinHandle`, logs on exit/panic, and calls
`set_autonomous_session_truth(ControllerExited{...})` so `/api/v1/autonomous/readiness` can
surface the death. **It does not restart the controller.**

- WS transport task (`_alpaca_ws_handle`): no watchdog at all; handle held, never awaited.
- Bar-ticker task (`_bar_ticker_handle`): no watchdog at all; same pattern.
- Execution loop task: no dedicated watchdog. Only indirect, narrow, ≤30s-latency detection via
  the session controller's own poll (`has_active_run` check, §3) — and only when
  `locally_started == true`. An operator-started run's unexpected death is invisible to the
  controller entirely.
- Reconcile-tick task (`spawn_reconcile_tick`, `loop_runner.rs:1158-1278`): `JoinHandle`
  **discarded outright** at the call site (`lifecycle.rs:920-926` does not bind the return
  value; the function itself returns `()`, not a handle). Zero detection of this task's death —
  no watchdog, no reap, no indirect signal.

Phase D's task-supervision requirement is a real gap-close, not a formalization of something
that already mostly works.

---

## 11. Current API/GUI surfaces

**Routes** (`routes.rs:333-684` public router; `routes/system.rs`, `routes/autonomous_paper_status.rs`):

| Route | Handler | Response type | `truth_state`? |
|---|---|---|---|
| `GET /api/v1/autonomous/readiness` | `system::autonomous_readiness` | `AutonomousPaperReadinessResponse` | yes (`api_types.rs:740`) — **but this GET also writes a DB row**, see §9 |
| `GET /api/v1/autonomous/no-trade-diagnostics` | `system::autonomous_no_trade_diagnostics` | `NoTradeDiagnosticsResponse` | yes |
| `GET /api/v1/autonomous/paper-status` | `autonomous_paper_status::autonomous_paper_status` | `AutonomousPaperStatusResponse` | yes (`api_types.rs:899`) |
| `GET /api/v1/system/preflight` | `system_preflight` | `PreflightStatusResponse` | **no top-level field** — per-subsystem states only |
| `GET /api/v1/system/status` | `system_status` | `SystemStatusResponse` | **no top-level field** — per-subsystem states only |
| `GET /api/v1/market-data/readiness` | `market_data_readiness_status` | `DailyDataReadinessResponse` | **no top-level field** — has `applicability`/per-assignment `readiness_state` instead |

No `truth_state`-bearing route or field named `daily_operation`, `operation_id`, or equivalent
exists anywhere in `api_types.rs` today (grep-confirmed).

`AutonomousPaperStatusResponse` carries `readiness_classification: String`
(`autonomous_paper_status.rs:358-376`), computed as:

```rust
let readiness_classification = if fatal_blockers_count > 0 || kill_switch_active {
    "blocked"
} else if blockers.is_empty() {
    "ready_for_market_smoke"
} else {
    "market_proof_pending"
};
```

This is the closest existing analog to a "daily operation state" — a coarse three-value
readiness-to-start classification, not a per-trading-day lifecycle with `operation_id`,
`market_date`, reason codes, retry counts, run linkage, or durable outcome. There is no
`GET /api/v1/autonomous/daily-operation` or `GET /api/v1/autonomous/daily-operations` route
today; Phase E adds both net-new.

**GUI**: `core-rs/mqk-gui/src/features/system/` consumes `autonomous_paper_status` and related
system fields (`DashboardScreen.tsx`, `StrategyScreen.tsx`, `api.ts`) but has no
`AutonomousDailyOperationPanel.tsx` or equivalent, and no `daily-operation`/`daily_operation`
reference anywhere in GUI source (grep-confirmed). Phase F adds this panel net-new.

**Stale doc found**: `docs/proofs/premarket_ingest_plan_proof.md` §1 claims the
`market_data_ingest_plan` handler "takes no `State<AppState>` parameter at all — it cannot reach
the DB... even if it tried." This is now false at current HEAD: the handler
(`routes/ingest.rs:3209`) does take `State(st)` and calls
`compute_daily_data_readiness_response`, which performs bounded, read-only `md_bars` reads
(added later by the `DAILY-DATA-READINESS-01C-ENFORCEMENT-01` bundle). The *substantive* safety
property (no mutation, no provider/broker call) still holds — only the specific structural claim
in the older proof doc is stale and should not be re-cited as current architecture without this
correction.

---

## 12. Exact gaps blocking unattended daily operation

For each of the 16 "current source truths" named in the bundle prompt: confirmed true, with the
operational consequence and the phase that closes it.

| # | Current source | Current test proof | Missing proof/behavior | Operational consequence | Phase |
|---|---|---|---|---|---|
| 1 | `session_controller.rs:58` polls every 30s | `scenario_autonomous_paper_day_*` (various) | n/a — behavior confirmed as designed | Acceptable poll cadence; not itself a gap | — |
| 2 | `locally_started: bool`, process-local (`session_controller.rs:194`) | none — not persisted, nothing to test for restart-survival | Durable, restart-safe operation identity | Daemon restart loses ownership attribution; controller may re-attempt a start that's actually already running under operator control, or vice versa | B, D |
| 3 | Refused start retried every tick (`session_controller.rs:251-323`) | `scenario_autonomous_gate_parity_auton11.rs` (blocked-case coverage only) | Typed transient/terminal classification + backoff | Terminal blockers (e.g. disarmed, halted) retried every 30s forever with no backoff | D |
| 4 | Discord alert fires every tick on repeated refusal, no dedup (`session_controller.rs:299-320`) | none | Notification dedup keyed on `operation_id + event class + blocker signature` | Alert spam under any persistent daytime blocker | D |
| 5 | Stop only when `locally_started==true` (`session_controller.rs:211,240-241`) | `scenario_autonomous_paper_session_hygiene_01.rs` (SH01-04) | n/a — confirmed correct as designed | Operator-managed runs correctly untouched; not itself a gap | — |
| 6 | Session controller/bar ticker bypass `MarketCalendarProvider`, use raw `CalendarSpec::NyseWeekdays` directly (`session_controller.rs:87-88`, `autonomous_bar_ticker.rs:114-115`) | none testing the divergence itself | Single authoritative session-plan composition consumed uniformly | Three independent call sites for one authority; latent drift risk if any one is changed without the others | B, D |
| 7 | No durable daily-operation record persisted anywhere (`grep daily_operation` = 0 hits) | none | `sys_autonomous_daily_operations`-equivalent durable, CAS-safe table | No canonical "what happened today" truth survives a restart | B |
| 8 | `scenario_autonomous_paper_day_lifecycle_auton12.rs` does not prove a full DB-backed start | file's own header, lines 19-25, explicitly disclaims it; AL-02 asserts `StartRefused` | A DB-backed test proving `start_execution_runtime()` returns `Ok` from a controller tick | Full unattended start-to-running path is untested end-to-end | D |
| 9 | Latest-bar scheduler is mutation-route-driven, process-local (`state.rs:201-249`, ops routes only) | `scenario_market_data_latest_bar_scheduler_01.rs` (route correctness only) | Preopen-start / postclose-stop coordination with session lifecycle | Operator must manually start/stop the scheduler around every session; no autonomous data-refresh | C |
| 10 | Bar ticker fires on blind 60s timer regardless of bar state (`autonomous_bar_ticker.rs:100-147`) | none testing bar-newness gating (ticker has none) | Exactly-once dispatch keyed on new completed `end_ts` | Duplicate/non-bar-aligned dispatch triggers possible; no true "new bar" semantics | C |
| 11 | Dispatch path falls back to empty-stub bar context (`state.rs:2478-2496`) | `scenario_per_symbol_bar_window_01.rs` (proves stub is safe-by-construction) | Fail-closed refusal instead of silent stub fallback for applicable autonomous PAPER operation | Reachable in production for any symbol lacking `MQK_STRATEGY_MD_TIMEFRAME`/DB rows; currently safe (hold/flat) but silently degrades rather than blocking | C |
| 12 | Market-data scheduler fully independent of session lifecycle (`grep` confirms zero cross-references) | `scenario_market_data_latest_bar_scheduler_01.rs` (isolated route tests) | Coordinated start/stop tied to preopen/postclose | Same as #9 | C |
| 13 | Only controller has an exit watchdog; it doesn't restart (`main.rs:158-193`) | `scenario_startup_truth_boot_valid01.rs` (spawn-presence only, not liveness) | Uniform liveness truth + bounded restart for all critical tasks | WS/bar-ticker/reconcile task death is invisible; execution-loop death only visible when controller-owned | D |
| 14 | `sys_autonomous_session_events` is a transition log, not a daily result (`0032_autonomous_session_events.sql`) | none — module doc itself says "not the current active-alert surface" | Durable, finalized-once daily outcome record | No single row answers "what happened on 2026-07-16" | B, E |
| 15 | No-trade diagnostics are per-minute snapshots, not an end-of-day classification (`state.rs:2370-2411`) | none aggregating to a daily verdict | Evidence-hierarchy-based no-trade reason classification, finalized once per day | Can't distinguish "no bar" from "signal with no accepted decision" from "already at target" as a durable daily fact | E |
| 16 | No canonical daily-operation state/history surface exists (`readiness_classification` is the closest, and it's coarse) | `scenario_autonomous_paper_status_summary_01.rs` (proves the 3-value classification, not a lifecycle) | `GET /api/v1/autonomous/daily-operation[s]` + GUI panel | Operator cannot see current/historical daily-operation truth in one place | E, F |

---

## 13. Final binding contract

This freezes, for Phases B–G, the contract already specified in the bundle prompt, confirmed
against the audit above with no corrections required:

- **Canonical start path**: every autonomous start attempt (current and future) must continue to
  route through `AppState::start_execution_runtime` (§7's 23-gate chain) unmodified in ordering
  or content. The daily coordinator decides *when* to call it; it must never create a second,
  simplified start gate.
- **Canonical stop path**: `stop_execution_runtime` / `halt_execution_runtime` remain the only
  stop entry points (`lifecycle.rs:1049,1109`).
- **Canonical session/calendar authority**: Phase B/D must converge `session_controller.rs` and
  `autonomous_bar_ticker.rs` onto one authoritative session-plan composition (reusing Bundle 2's
  `daily_data_readiness.rs` calendar fields and/or `market_calendar.rs`'s
  `MarketCalendarProvider`), eliminating the three-call-site duplication in §4/§12#6. It must not
  introduce a *fourth* calculation.
- **Deterministic operation identity**: `operation_id` must be `Uuid::new_v5(NAMESPACE_DNS,
  "mqk.autonomous-daily-operation.v1|{market_date}|{deployment_mode}|{adapter_id}|{session_plan_identity}|{assignment_identity}|{runtime_binding_identity}")`
  or equivalent, following the established convention in §8 — not `Uuid::new_v4()`.
- **Durable daily operation record**: required net-new (§14) since neither `runs` nor
  `sys_autonomous_session_events` can support idempotent daily identity, restart-safe
  current-state projection, or CAS-safe concurrent transitions as they exist today (§8, §12#7/#14).
- **PAPER-only boundary, provider-call authorization, no-synthetic-lifecycle, and all other
  constraints from the bundle prompt's "BINDING SAFETY CONTRACT" / "PAPER-ONLY BOUNDARY" /
  "DATA-CALL AUTHORIZATION" sections** are adopted verbatim — nothing in the audit contradicts
  or requires loosening any of them.
- **Task supervision**: Phase D must give the WS task, bar-ticker task, and reconcile-tick task
  the same liveness-truth treatment the controller already has (§10, §12#13), plus bounded
  restart for pure-coordination/data tasks only — execution-loop restart must always re-enter
  through the canonical start gate, never bypass it.
- **Read-only API additions**: `GET /api/v1/autonomous/daily-operation` and
  `GET /api/v1/autonomous/daily-operations?limit=` are net-new (§11, §12#16); existing GET routes
  keep their current shapes, extended additively only.
- **One known, accepted, pre-existing deviation carried forward**: `GET /api/v1/autonomous/readiness`
  performs a deduplicated, best-effort DB write (§9, §11) under tag `AUTON-NO-TRADE-OFFHOURS-01B`.
  This audit does not require fixing it (out of scope for this bundle) but Phase B–G must not
  model new GET routes on this pattern — new daily-operation GET routes must be strictly
  read-only per the bundle's explicit contract.

---

## 14. Proposed durability model

Neither `runs` (no natural daily key, LIVE-only uniqueness index, non-atomic check-then-update
for PAPER) nor `sys_autonomous_session_events` (append-only log, no CAS, no per-day identity) can
trustworthily provide race-safe daily coordination (§8, §12#7/#14). A new additive table is
required, matching the bundle prompt's proposed shape:

```
sys_autonomous_daily_operations (new, migration 0048_...)
  operation_id            uuid        primary key   -- UUIDv5 per §13
  market_date             date        not null
  deployment_mode         text        not null
  adapter_id              text        not null
  assignment_identity     text        not null
  session_open_utc        timestamptz not null
  session_close_utc       timestamptz not null
  state                   text        not null       -- typed state machine, see bundle prompt's list
  state_reason_code       text
  run_id                  uuid                        -- nullable, no FK per sys_autonomous_session_events precedent (0043:15-19 style)
  start_attempt_count     bigint      not null default 0   -- caller-managed, no DB default per DB rules
  last_start_attempt_utc  timestamptz
  next_retry_utc          timestamptz
  started_at_utc          timestamptz
  stopped_at_utc          timestamptz
  finalized_at_utc        timestamptz
  outcome                 text
  no_trade_reason         text
  last_error              text
  created_at_utc          timestamptz not null   -- caller-injected, no DEFAULT now() per db_rules.md
  updated_at_utc          timestamptz not null   -- caller-injected
  unique (market_date, deployment_mode, adapter_id, assignment_identity)  -- idempotent daily identity + CAS anchor
```

Uniqueness on `(market_date, deployment_mode, adapter_id, assignment_identity)` gives the
"repeated ticks / restart / concurrent create" idempotency the bundle requires: `INSERT ... ON
CONFLICT (market_date, deployment_mode, adapter_id, assignment_identity) DO NOTHING` followed by
a `SELECT` gives exactly-once creation; transitions use a `WHERE state = $expected` compare-and-set
`UPDATE`, matching the pattern already partially present in `runs.arm_run`'s check-then-update
(`runs.rs:424-452`) but made atomic via the `WHERE` clause instead of read-then-write. This is a
Phase B implementation task, not performed in this Phase A patch. No production code or migration
is added by Phase A.

---

## 15. Retry classification and policy

Confirmed from source that today's retry behavior has **no typed classification at all** — every
refusal reason is treated identically (§3, §12#3/#4): retried every 30s, alerted every 30s. The
bundle prompt's proposed transient/terminal split maps cleanly onto reason strings already
produced by the existing gate chain (§7) and readiness surfaces (§6, §9, §11):

- Transient (already-observed reason strings/equivalents in current code):
  `service_unavailable` (DB, `scenario_autonomous_paper_day_lifecycle_auton12.rs` AL-02),
  WS `ColdStartUnproven`/`GapDetected` (`scenario_autonomous_gate_parity_auton11.rs` AP-01/AP-02),
  arm-pending (`scenario_auton_no_trade_diagnosis_01.rs` ND-01), session-window-not-open
  (`autonomous_paper_status.rs` blockers).
- Terminal/manual (already-observed): integrity halted (AP-03), reconcile dirty (AP-04, BRK-09R),
  durable disarm with no DB path to auto-clear (AP-05, ND-02/03) — matching the bundle's binding
  safety contract ("autonomous re-arm only when DB arm state == ARMED AND not halted AND no kill
  switch active").

No correction to the bundle's proposed backoff schedule (30s → 60s → 120s → 300s cap) is
required; nothing in current source contradicts it. This is a Phase D implementation task.

---

## 16. No-trade evidence hierarchy

`strategy_signal_evaluations` (§9) already durably records, per dispatch attempt: `bars_loaded`,
`signal_generated`, `signal_qty`, `signal_side`, `reason_code`, `decision_stage` — this is real
evidence that can distinguish "bar observed, no signal" from "no bar observed" today. What is
missing is the aggregation layer: nothing rolls these rows, plus `oms_outbox`/`oms_inbox`/fill
evidence, into a single per-day outcome. The bundle's proposed evidence hierarchy (bars observed
→ bars dispatched → strategy evaluations → nonzero target decisions → accepted decisions →
outbox enqueues → broker submissions → acknowledgements → fills → position changes) is directly
constructible from existing tables (`strategy_signal_evaluations`, `oms_outbox`, `oms_inbox`,
`fill_quality_telemetry`) with no new evidence source needed — only a new aggregation/finalization
step (Phase E). No correction to the bundle's proposed reason-code list is required.

---

## 17. Phase B–G implementation map

- **Phase B** — `sys_autonomous_daily_operations` table (§14) + deterministic `operation_id` +
  typed state machine + restart-safe create/load + CAS transitions + shared read-only projection.
  No provider-call or controller-behavior changes.
- **Phase C** — Extract a reusable internal provider-poll coordinator seam out of
  `market_data_feed_poll_once` (§5, §12#9); wire it to preopen-start/postclose-stop; make
  `autonomous_bar_ticker.rs` (or its replacement) dispatch exactly-once per genuinely-new
  completed `end_ts` instead of blind-timer (§6, §12#10/#11).
- **Phase D** — Migrate `session_controller.rs` off the process-local `locally_started` bool onto
  the Phase B durable operation record; add typed retry/backoff (§15); converge the
  three-call-site calendar duplication (§4, §12#6) onto one authority; add liveness watchdogs for
  WS/bar-ticker/reconcile tasks with bounded restart for pure-coordination tasks only (§10, §12#13).
- **Phase E** — Daily outcome finalization from the evidence hierarchy (§16);
  `GET /api/v1/autonomous/daily-operation[s]` routes (§11, §12#16); additive summary fields on
  existing `readiness`/`paper-status`/`preflight` routes.
- **Phase F** — GUI panel (net-new, §11); runbook corrections (including the stale claim in §11);
  read-only Windows evidence-capture script.
- **Phase G** — Closure audit, focused test matrix (§18), ledger reconciliation.

---

## 18. Test matrix

Existing coverage confirmed sufficient as a regression baseline for Phases B–G (all 17 files
audited exist, none make real network calls, per the test-coverage audit):

| Area | Existing regression file(s) | Confirmed gap this bundle must close |
|---|---|---|
| Controller tick / start gating | `scenario_autonomous_paper_day_lifecycle_auton12.rs`, `scenario_autonomous_gate_parity_auton11.rs`, `scenario_autonomous_paper_day_auton01.rs` | None prove a successful (`Ok`) DB-backed start — Phase D adds this |
| Session hygiene / reset semantics | `scenario_autonomous_paper_session_hygiene_01.rs` | No runtime start attempted anywhere in file — expected, out of its scope |
| Readiness/status surfaces | `scenario_autonomous_paper_status_summary_01.rs`, `scenario_auton_no_trade_diagnosis_01.rs`, `scenario_auton_no_trade_followup_02.rs` | No daily-operation state exposed — Phase E adds routes |
| Signal-evaluation journal | `scenario_signal_evaluation_journal_auton_no_signal_obs_01.rs` | No daily aggregation — Phase E adds it |
| Startup task presence | `scenario_startup_truth_boot_valid01.rs` | Only proves spawn Some/None, not liveness once running — Phase D adds watchdog tests |
| Long-run stability | `scenario_runtime_longrun_01.rs` | Iteration-based, not restart-based — Phase B/D add restart-recovery tests |
| Data scheduler | `scenario_market_data_latest_bar_scheduler_01.rs`, `scenario_market_data_latest_bar_poll_01.rs` | Fake-provider route correctness only, unrelated to session lifecycle — Phase C adds coordination tests |
| Readiness evaluator | `scenario_daily_data_readiness_01.rs`, `scenario_daily_data_readiness_start_gate_01.rs` | Every real `start_execution_runtime()` call in the latter asserts `.expect_err` — Phase D/B add a passing-path DB-backed test |
| Intraday/staleness gates | `scenario_intraday_md_freshness_autonomous_01.rs`, `scenario_md_staleness_per_tick_gate_01.rs`, `scenario_per_symbol_bar_window_01.rs` | Confirmed correct as-is; reused unmodified as regression baseline |

New test files anticipated (named per bundle prompt, not created in this Phase A patch):
`scenario_autonomous_daily_operation_store_01.rs`, `scenario_autonomous_daily_operation_identity_01.rs`
(Phase B); `scenario_autonomous_completed_bar_driver_01.rs` (Phase C);
`scenario_autonomous_daily_session_controller_01.rs`, `scenario_autonomous_daily_task_supervision_01.rs`
(Phase D); `scenario_autonomous_daily_outcome_01.rs`, `scenario_autonomous_daily_operation_api_01.rs`
(Phase E).

---

## 19. Explicit non-goals

Unchanged from the bundle prompt's PRIORITY section, confirmed as still correctly out of scope
given current source state: per-symbol strategy bootstrap, broader multi-symbol expansion, `1h`
provider support, Alpaca `1D` timestamp research, futures, options, forex, crypto expansion,
live-capital operation, strategy research, new strategies, strategy optimization, repo lean-out,
broad GUI redesign, automatic historical backfill, realized/unrealized P&L calculation, the
10–20-session soak itself, and Bundle 4 (`DURABLE-PAPER-PORTFOLIO-AND-PNL-01-COMBINED`).

Additionally out of scope for this bundle specifically (found during audit, not pre-existing
bundle exclusions): fixing the `GET /api/v1/autonomous/readiness` DB-write deviation (§9, §11,
§13) — noted and carried forward, not remediated here; correcting the stale
`docs/proofs/premarket_ingest_plan_proof.md` claim (§11) — flagged, not edited, since this Phase
A patch's allowed-files list does not include that doc.

---

## 20. Safety boundaries

All boundaries from the bundle prompt's "BINDING SAFETY CONTRACT," "PAPER-ONLY BOUNDARY," and
"GLOBAL SAFETY BOUNDARIES" sections apply unchanged and are reaffirmed by this audit with no
loosening: no bypass of `AppState::start_execution_runtime`'s 23-gate chain (§7, §13); no
second/simplified start gate; no automatic clearing of halt, kill switch, or durable disarm; no
live-capital enablement; no synthetic broker lifecycle events; no fabricated bars, signals,
orders, fills, positions, or daily outcomes; zero real network/provider/broker calls from any
test or from this Phase A patch itself. This Phase A patch makes no production-code, test-code,
or migration change — it is documentation and a guard script only.
