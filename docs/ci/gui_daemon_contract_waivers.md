# GUI/Daemon Contract CI Waivers (TEST-02)

This file documents GUI-read endpoints that are intentionally not part of the TEST-02 hard gate yet.

## Enforced in CI

Gate implementation: `cargo test -p mqk-daemon --test scenario_gui_daemon_contract_gate` verifies these endpoint contracts in one stable harness.

### Canonical system surfaces

- `/api/v1/system/status` — shape + semantic truth (db_status, audit_writer_status, daemon_mode, adapter_id, broker_snapshot_source, alpaca_ws_continuity, deployment_start_allowed); AP-09: external broker truth fields proven in `gui_system_status_and_preflight_surfaces_are_semantically_truthful`
- `/api/v1/system/preflight` — shape + semantic truth (broker_config_present, db_reachable, blockers list)
- `/api/v1/system/metadata` — shape + semantic truth (build_version, api_version, broker_adapter, endpoint_status)
- `/api/v1/system/session` — shape + semantic truth (daemon_mode, adapter_id, strategy_allowed)
- `/api/v1/system/config-fingerprint` — shape + semantic truth (adapter_id, environment_profile, config_hash)
- `/api/v1/system/runtime-leadership` — shape + semantic truth (leader_node="local", leader_lease_state, generation_id non-empty, `restart_count_24h` is `null` when no DB pool (not synthetic 0) / real DB-backed count when pool present, post_restart_recovery_state, checkpoints empty in test state)

### Execution and portfolio summaries

- `/api/v1/execution/summary` — shape check (active_orders, pending_orders, dispatching_orders, reject_count_today)
- `/api/v1/execution/orders` — canonical OMS array with fail-closed semantics:
  - HTTP 503 when no execution snapshot (OMS loop not running) → endpoint lands in `missingEndpoints` → `isMissingPanelTruth` fires → execution panel blocks with `no_snapshot`
  - HTTP 200 + bare JSON array when snapshot is active; zero active orders returns `[]` (authoritative empty)
  - Legacy `/v1/trading/orders` confirmed still mounted; GUI falls through to it only on 404/network error (not on 503)
  - Row-semantic closure (exec/portfolio patch): `strategy_id`, `side`, `order_type`, `age_ms` are `null` (not fake constants "runtime"/"buy"/"market"/0) — OMS snapshot has no source for these fields
  - Tests: `gui_contract_execution_orders_503_without_snapshot` + `gui_contract_execution_orders_200_array_with_injected_snapshot`
- `/api/v1/portfolio/summary` — shape check (account_equity, cash, long_market_value, buying_power)
- `/api/v1/portfolio/positions` — structured wrapper (`snapshot_state`, `captured_at_utc`, `rows`):
  - `snapshot_state: "active"` + rows when broker snapshot is loaded; empty `rows` is authoritative (account has no positions)
  - `snapshot_state: "no_snapshot"` + empty rows when no broker snapshot; GUI checks typed field, not HTTP status string
  - Row-semantic closure: `strategy_id`, `mark_price`, `unrealized_pnl`, `realized_pnl_today`, `drift` are `null` — broker snapshot has no source for these fields; `broker_qty` is honest (equals `qty`, row IS the broker view)
  - Tests: `gui_contract_portfolio_positions_no_snapshot` + `gui_contract_portfolio_positions_active_snapshot`
- `/api/v1/portfolio/orders/open` — same structured wrapper pattern; `internal_order_id` = `client_order_id` from broker snapshot
  - Row-semantic closure: `strategy_id` is `null`; `filled_qty` is `null` — broker snapshot does not track partial fills per order
  - Tests: `gui_contract_portfolio_open_orders_no_snapshot` + `gui_contract_portfolio_open_orders_active_snapshot`
- `/api/v1/portfolio/fills` — same structured wrapper pattern; `applied: true` for all fills in snapshot
  - Row-semantic closure: `strategy_id` is `null` — broker snapshot has no strategy attribution
  - Tests: `gui_contract_portfolio_fills_no_snapshot` + `gui_contract_portfolio_fills_active_snapshot`

### Risk and reconcile summaries

- `/api/v1/risk/summary` — shape check (gross_exposure, net_exposure, concentration_pct, kill_switch_active)
- `/api/v1/risk/denials` — structured wrapper (`truth_state`, `snapshot_at_utc`, `denials`); denial truth is now wired:
  - `truth_state: "no_snapshot"` + empty denials + null `snapshot_at_utc` when no execution snapshot (loop not running); GUI IIFE emits ok:false → endpoint in `missingEndpoints` → `isMissingPanelTruth` fires → risk panel blocks
  - `truth_state: "active"` + authoritative `denials` array + non-null `snapshot_at_utc` when execution loop is running; rows are populated from the orchestrator's bounded denial ring buffer (`ExecutionSnapshot::recent_risk_denials`), fed only by real `RiskGate::evaluate_gate()` denials; `denials: []` truly means zero denials this session
  - `"not_wired"` is no longer returned; kept as a defensive case in the GUI IIFE
  - Tests: `gui_contract_risk_denials_no_snapshot` (loop absent → `no_snapshot`) + `gui_contract_risk_denials_active_snapshot` (loop running, empty buffer → `active` + empty rows + non-null timestamp) + `gui_contract_risk_denials_real_row_appears` (snapshot with one `RiskDenialRecord` → route serializes row with correct id/rule/symbol/severity/message/at)
- `/api/v1/reconcile/status` — shape check (status, last_run_at, mismatched_positions, unmatched_broker_events)
- `/api/v1/reconcile/mismatches` — structured wrapper (`truth_state`, `snapshot_at_utc`, `rows`):
  - `truth_state: "no_snapshot"` + empty rows + null `snapshot_at_utc` when reconcile detail truth is not authoritative yet (no reconcile snapshot, no broker snapshot, or no execution snapshot)
  - `truth_state: "stale"` + empty rows when summary/detail truth disagree or the reconcile watermark has gone stale; GUI IIFE treats this as failed probe so the Reconcile panel stays fail-closed
  - `truth_state: "active"` + rows when the daemon can derive current reconcile diffs from the active execution snapshot plus broker snapshot and the result agrees with reconcile status
  - Tests: `gui_contract_reconcile_mismatches_no_snapshot_without_authoritative_detail` + `gui_contract_reconcile_mismatches_active_with_authoritative_diff_rows`

### Strategy truth surfaces (CC-01 and CC-02)

- `/api/v1/strategy/summary` — `StrategySummaryResponse` wrapper; truth state is conditional on
  whether `MQK_STRATEGY_IDS` is configured; backend is `daemon.strategy_fleet`:
  - `truth_state="not_wired"` + empty `rows` when no strategy fleet is configured
    (`MQK_STRATEGY_IDS` not set). GUI IIFE emits `ok:false` on `not_wired` → `"strategies"` in
    `mockSections` → panel authority collapses to `"placeholder"` → StrategyScreen hard-blocks.
  - `truth_state="active"` + rows when fleet is configured; `rows` may be empty if
    `MQK_STRATEGY_IDS` contains no entries (authoritative empty — fleet is configured but vacant).
  - The former synthetic `daemon_integrity_gate` surrogate row has been removed.
  - CC-01 closed: conditional truth path is wired; `not_wired` is not a permanent state.

- `/api/v1/strategy/suppressions` — `StrategySuppressionsResponse` wrapper; truth state is
  conditional on DB pool availability; backend is `postgres.sys_strategy_suppressions`
  (migration 0027):
  - `truth_state="no_db"` + empty `rows` when no DB pool is configured. The source IS wired
    (postgres); the pool is just unavailable. GUI renders an honest "unavailable" notice.
    `"not_wired"` is not returned by this route.
  - `truth_state="active"` + rows when DB pool is present; `rows` may be empty (authoritative
    zero suppressions) or populated with real durable suppression records.
  - CC-02 closed: durable suppression persistence exists; this is not a permanently not_wired surface.

- Contract gate: `gui_contract_not_wired_surfaces_declare_truth_state` proves, in the no-fleet
  no-DB test state:
  - `strategy/suppressions` → `truth_state="no_db"` + `backend="postgres.sys_strategy_suppressions"`;
    confirms the source is wired but the DB pool is unavailable (not permanently not_wired).
  - `strategy/summary` → `truth_state="not_wired"` (no `MQK_STRATEGY_IDS` in test state);
    the active-fleet path is not exercised by this no-fleet test state.
  - `config-diffs` → `truth_state="not_wired"` in no-DB/no-run state (see System config surfaces
    section below for the full conditional truth path including the `active` path).

### System config surfaces — conditional truth

- `/api/v1/system/config-diffs` — `ConfigDiffsResponse` wrapper; truth state is conditional on DB
  and run availability:
  - `truth_state="not_wired"` + empty `rows` when no DB is configured OR no latest durable daemon
    run exists. GUI IIFE emits `ok:false` on `not_wired` → `"configDiffs"` in `mockSections` →
    ConfigScreen renders honest "not wired" notice, not empty table.
  - `truth_state="active"` + config diff rows when a latest durable run exists in DB; backend is
    `postgres.runs+daemon.runtime_selection`. Rows reflect diffs between the latest durable run's
    config hash and the daemon's current runtime selection (adapter, mode, config hash, host).
  - There is no separate config-diff persistence table. Diffs are computed live from the `runs`
    table and current daemon state. The `not_wired` contract gate test exercises the no-DB path;
    DB-backed active-truth path requires a live Postgres instance and a prior durable run row.

### Audit and operator surfaces

- `/api/v1/audit/operator-actions` — wrapper shape + backend identity proven; GUI fetch/map layer (IIFE) unwraps `{canonical_route, backend=postgres.audit_events, rows}` and maps `audit_event_id→audit_ref`, `ts_utc→at`, `requested_action→action_key`, `disposition→result_state`; row-level field contracts require DB integration test
- `/api/v1/audit/artifacts` — wrapper shape + backend identity proven; GUI fetch/map layer constructs `ArtifactRegistrySummary` from `{canonical_route, backend=postgres.runs, rows}` (one `run_config` artifact per run); row-level field contracts require DB integration test
- `/api/v1/ops/operator-timeline` — wrapper shape + backend identity proven; GUI fetch/map layer maps `ts_utc→at`, `kind→category`, `detail→title+summary`, `provenance_ref→timeline_event_id`; row-level field contracts require DB integration test
- Contract gate: `gui_contract_operator_history_endpoints_declare_correct_backends` (new in REC-02) proves wrapper shape, `canonical_route` self-identity, and exact backend sources in no-DB test state

### Operator action dispatcher and catalog

- `/api/v1/ops/action` — POST; dispatch semantics proven:
  - `arm-execution` → 200, accepted=true, disposition="applied"
  - `disarm-execution` → 200, accepted=true, disposition="applied"
  - `change-system-mode` → 409 CONFLICT, accepted=false, disposition="not_authoritative" (mode transition requires daemon restart; this action key is intentionally not authoritative via API)
  - unknown key → 400 BAD_REQUEST, accepted=false

- `/api/v1/ops/catalog` — GET; daemon-authoritative Action Catalog:
  - Returns `canonical_route` + `actions` array (exactly 5 entries in test state)
  - Each entry: `action_key`, `label`, `level`, `description`, `requires_reason`, `confirm_text`, `enabled`, `disabled_reason`
  - State-correct availability proven: disarmed → arm-execution=true, disarm-execution=false; idle → start-system=true, stop-system=false; not-halted → kill-switch=true
  - `change-system-mode` is absent (would return 409 from dispatcher)

Note: `/api/v1/ops/change-mode` is intentionally NOT mounted. Mode transitions require a controlled restart with configuration reload. The GUI disables mode-change buttons and surfaces a panel notice. The `change-system-mode` action key through `/api/v1/ops/action` returns 409 as a defense-in-depth rejection.

### Alert and event feed surfaces (CC-06)

- `/api/v1/alerts/active` — `ActiveAlertsResponse` canonical active-alert surface:
  - `truth_state="active"` always — computed from live in-memory daemon state at
    request time; no DB required.  Empty `rows` means the daemon has no current
    fault conditions (genuinely healthy state, not absence of source).
  - Source: `build_fault_signals(StatusSnapshot, ReconcileStatusSnapshot, risk_blocked)`.
    Produces one row per active fault signal.  No ack lifecycle, no persistent
    alert IDs.  `alert_id` equals `class` (stable slug, not a UUIDv4).
  - `backend="daemon.runtime_state"` — in-memory computation, not DB-backed.
    Risk-blocked component uses a DB query but falls back to `false` without DB
    (matching the behaviour of `GET /api/v1/system/status`).
  - Row fields proven: `alert_id`, `severity`, `class`, `summary`, `detail`,
    `source`.  `alert_count` must equal `rows.len()`.
  - Proof: `gui_contract_alerts_active_wrapper_semantics` (contract gate) +
    `cc06_01_alerts_active_clean_state_empty_rows` +
    `cc06_02_alerts_active_dirty_reconcile_emits_critical_alert` (scenario file).
  - CC-06 closed for alerts/active.

- `/api/v1/events/feed` — `EventsFeedResponse` canonical recent-event feed:
  - `truth_state="active"` + `backend="postgres.runs+postgres.audit_events"` when
    DB pool is present; rows contain at most 50 recent events (runtime transitions
    from `runs` table + operator actions from `audit_events`, sorted newest-first).
  - `truth_state="backend_unavailable"` + `backend="unavailable"` + empty `rows`
    when no DB pool.  Empty rows in this state must NOT be treated as authoritative
    empty history.
  - Row fields: `event_id`, `ts_utc`, `kind` (`"runtime_transition"` |
    `"operator_action"`), `detail`, `run_id` (optional), `provenance_ref`.
  - Same durable source as `operator-timeline` but capped at 50 rows (feed semantics).
  - No fake historical backfill.  No synthetic rows.
  - Proof: `gui_contract_events_feed_no_db_backend_unavailable` (contract gate) +
    `cc06_03_events_feed_no_db_is_backend_unavailable` +
    `cc06_04_events_feed_canonical_route_identity` +
    `cc06_05_events_feed_db_backed_positive_path_real_rows` (DB-backed, #[ignore];
    seeds deterministic `runs` + `audit_events` rows and validates exact field
    mapping for both `runtime_transition` CREATED and `operator_action` kinds,
    plus newest-first ordering).
  - CC-06 closed for events/feed.

### Metrics dashboard surface (CC-05)

- `/api/v1/metrics/dashboards` — `MetricsDashboardResponse` canonical metrics/KPI dashboard:
  - Portfolio panel (`portfolio_snapshot_state`, `account_equity`, `long_market_value`,
    `short_market_value`, `cash`, `buying_power`) from `broker_snapshot`; `"no_snapshot"` + null
    values when absent. `daily_pnl` is always None — not derivable from current sources.
  - Risk panel (`risk_snapshot_state`, `gross_exposure`, `net_exposure`, `concentration_pct`,
    `kill_switch_active`, `active_breaches`) from `broker_snapshot` positions + runtime state.
    `drawdown_pct` and `loss_limit_utilization_pct` always None — no source exists.
  - Execution panel (`execution_snapshot_state`, `active_order_count`, `pending_order_count`,
    `dispatching_order_count`, `reject_count_today`) from `execution_snapshot`; `"no_snapshot"`
    when execution loop has not started.
  - Reconcile panel (`reconcile_status`, `reconcile_last_run_at`, `reconcile_total_mismatches`)
    always present; `"unknown"` before first reconcile tick.
  - Route is read-only (public sub-router) — no auth required.
  - Proof: `cargo test --test scenario_metrics_dashboards_cc05 -p mqk-daemon` (6 tests; CC05-01..CC05-06)

### OMS overview surface (CC-04)

- `/api/v1/oms/overview` — `OmsOverviewResponse` canonical overview composed from mounted truth:
  - `runtime_status`, `integrity_armed`, `kill_switch_active`, `daemon_mode`, `fault_signal_count`
    from StatusSnapshot (runtime lane — always present).
  - `account_snapshot_state` / `account_equity` / `account_cash` from `broker_snapshot.account`;
    `"no_snapshot"` + null values when no broker snapshot loaded (not fake zeros).
  - `portfolio_snapshot_state` / `position_count` / `open_order_count` / `fill_count` from
    `broker_snapshot` counts; `"no_snapshot"` when absent.
  - `execution_has_snapshot` / `execution_active_orders` / `execution_pending_orders` from
    `execution_snapshot` (OMS internal); `false` + zeros when execution loop has not started.
  - `reconcile_status` / `reconcile_last_run_at` / `reconcile_total_mismatches` from reconcile
    snapshot (always present; defaults to `"unknown"` before first reconcile tick).
  - Route is read-only (public sub-router) — no auth required.
  - Proof: `cargo test --test scenario_oms_overview_cc04 -p mqk-daemon` (5 tests; CC04-01..CC04-05)

### Legacy trading surfaces (DMON-04 contract)

- `/v1/status` — shape check (state, active_run_id, integrity_armed)
- `/v1/health` — shape check (ok, service)
- `/v1/trading/account` — snapshot_state, snapshot_captured_at_utc, account; no stale has_snapshot
- `/v1/trading/positions` — snapshot_state, snapshot_captured_at_utc, positions; no stale has_snapshot
- `/v1/trading/orders` — snapshot_state, snapshot_captured_at_utc, orders; no stale has_snapshot
- `/v1/trading/fills` — snapshot_state, snapshot_captured_at_utc, fills; no stale has_snapshot

### Discord outbound notification signal (OPS-NOTIFY-01 / OPS-NOTIFY-01B)

- `DiscordNotifier` — best-effort outbound signal rail; NOT source of truth:
  - Configured via `DISCORD_WEBHOOK_URL` env var; no-op when absent (fail-closed).
  - Only fires on accepted, applied control actions — never from read-only GET routes.
  - Delivery failure is swallowed (logged as `warn!`); primary daemon action result unchanged.
  - Payload fields: `action_key`, `disposition`, `environment`, `ts_utc`, `provenance_ref`, `run_id`.
  - `provenance_ref` is exact and durable (OPS-NOTIFY-01B):
    - Run paths (run.start / run.stop / run.halt): `"audit_events:<uuid>"` when a durable row was
      written; `null` when no DB or no run anchor (honest).
    - Arm/disarm (control.arm / control.disarm): `"audit_events:<uuid>"` from `/control/arm`
      (which calls `write_control_operator_audit_event` and threads the exact UUID); `"sys_arm_state"`
      label for `integrity_arm` / `integrity_disarm` / `ops/action` arm paths (those write
      `sys_arm_state`, not `audit_events`).
  - Proof (in-process, no real Discord): `scenario_notify_ops01.rs` — N01..N05:
    - N01: configured notifier fires; payload has correct `action_key`, `disposition`, `ts_utc`,
      `content`; `provenance_ref` is null for arm-execution (no DB, no audit_events row).
    - N02: missing config → no delivery, action result unchanged.
    - N03: delivery failure (bad URL) → primary result unchanged (200/applied).
    - N04: GET `/api/v1/alerts/active` does NOT fire the notifier.
    - N05 (`#[ignore]`, DB-backed): POST `/control/arm` with seeded run anchor → `provenance_ref`
      is `"audit_events:<uuid>"` matching the `audit_event_id` in the response; UUID exists in DB.
  - CI command: `cargo test -p mqk-daemon --test scenario_notify_ops01`

## Routes resolved by GUI-CONTRACT-01/02/03 (Batch 6)

These 11 routes were previously in the deferred list but are now resolved. They are
no longer probed by the GUI's `fetchOperatorModel` batch. The GUI uses `notProbed()`
stubs for the 6 static surfaces and simply sets the 5 per-order detail fields to null
(no HTTP request). Panel authority still degrades correctly for the 6 static surfaces
because `notProbed()` returns `ok: false`, causing `useObject`/`useArray` to push the
key into `usedMockSections` as before — but without the 404 HTTP request.

### Static surfaces — all graduated (Batch A2 + A3/A4)

All previously-deferred static surfaces are now mounted on the daemon and wired
in the GUI. `NOT_MOUNTED` is empty. `notProbed()` stubs removed.

| Route                                     | Batch | truth_state / notes                          |
|-------------------------------------------|-------|----------------------------------------------|
| `/api/v1/execution/transport`             | A2    | `"active"` / `"no_snapshot"` — in-memory    |
| `/api/v1/market-data/quality`             | A2    | `"active"` — in-memory WS + source state    |
| `/api/v1/system/topology`                 | A3    | `"active"` — 5 local service nodes          |
| `/api/v1/incidents`                       | OPS-01 | `"no_db"` / `"active"` — DB-backed incident lifecycle; POST create + linked_incident_id in triage wired |
| `/api/v1/execution/replace-cancel-chains` | A4    | `"not_wired"` — no chain lineage tracking   |
| `/api/v1/alerts/triage`                   | A4    | `"no_db"` / `"active"` — source real; ack lifecycle wired (OPS-02) |

### Per-order detail surfaces — mounted, not batch-probed (static null in api.ts)

These five routes are mounted on the daemon but are not in the `fetchOperatorModel`
batch probe manifest.  They are always `null` in the `SystemModel`.  Dedicated
exported functions (`fetchExecutionTimeline`, etc.) are used for screen-level calls.

Note: paths use the `/orders/:order_id/` prefix, not a flat `/{id}` structure.

- `/api/v1/execution/orders/:order_id/timeline`
- `/api/v1/execution/orders/:order_id/trace`
- `/api/v1/execution/orders/:order_id/replay`
- `/api/v1/execution/orders/:order_id/chart`
- `/api/v1/execution/orders/:order_id/causality`

### ROUTE-TRUTH-01 CI gate

`scenario_route_contract_rt01.rs` provides the regression lock:

- `rt01_all_gui_probed_routes_are_mounted` — every route in `GUI_PROBE_MANIFEST` must
  return non-404 from the daemon. Fails CI if a mounted route is removed without
  updating the GUI or if a new GUI probe is added without mounting the daemon route.

- `rt01_known_not_mounted_routes_stay_404_until_explicitly_promoted` — every route
  in `NOT_MOUNTED` must still return 404. Fails CI if a previously-deferred route is
  mounted without completing the promotion checklist above.
