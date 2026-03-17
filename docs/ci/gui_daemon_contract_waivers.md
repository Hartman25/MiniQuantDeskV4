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
  - Tests: `gui_contract_execution_orders_503_without_snapshot` + `gui_contract_execution_orders_200_array_with_injected_snapshot`
- `/api/v1/portfolio/summary` — shape check (account_equity, cash, long_market_value, buying_power)
- `/api/v1/portfolio/positions` — structured wrapper (`snapshot_state`, `captured_at_utc`, `rows`):
  - `snapshot_state: "active"` + rows when broker snapshot is loaded; empty `rows` is authoritative (account has no positions)
  - `snapshot_state: "no_snapshot"` + empty rows when no broker snapshot; GUI checks typed field, not HTTP status string
  - Tests: `gui_contract_portfolio_positions_no_snapshot` + `gui_contract_portfolio_positions_active_snapshot`
- `/api/v1/portfolio/orders/open` — same structured wrapper pattern; `internal_order_id` = `client_order_id` from broker snapshot
  - Tests: `gui_contract_portfolio_open_orders_no_snapshot` + `gui_contract_portfolio_open_orders_active_snapshot`
- `/api/v1/portfolio/fills` — same structured wrapper pattern; `applied: true` for all fills in snapshot
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

### Strategy

- `/api/v1/strategy/summary` — shape + semantic truth (strategy_id, armed, health fields)
- `/api/v1/strategy/suppressions` — array shape; empty in test state (Vec<StrategySuppressionRow>)

### System config surfaces

- `/api/v1/system/config-diffs` — array shape; empty in test state (Vec<ConfigDiffRow>)

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

### Legacy trading surfaces (DMON-04 contract)

- `/v1/status` — shape check (state, active_run_id, integrity_armed)
- `/v1/health` — shape check (ok, service)
- `/v1/trading/account` — snapshot_state, snapshot_captured_at_utc, account; no stale has_snapshot
- `/v1/trading/positions` — snapshot_state, snapshot_captured_at_utc, positions; no stale has_snapshot
- `/v1/trading/orders` — snapshot_state, snapshot_captured_at_utc, orders; no stale has_snapshot
- `/v1/trading/fills` — snapshot_state, snapshot_captured_at_utc, fills; no stale has_snapshot

## Explicitly deferred from TEST-02 gate

These endpoints are probed by the GUI but not yet authoritative daemon contract surfaces.
Waivers are explicit so deferred coverage is visible, not silently ignored.

- `/api/v1/oms/overview`
- `/api/v1/metrics/dashboards`
- `/api/v1/alerts/active`
- `/api/v1/events/feed`
- `/api/v1/system/topology`
- `/api/v1/execution/transport`
- `/api/v1/incidents`
- `/api/v1/execution/replace-cancel-chains`
- `/api/v1/alerts/triage`
- `/api/v1/market-data/quality`
