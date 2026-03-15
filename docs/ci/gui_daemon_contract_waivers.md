# GUI/Daemon Contract CI Waivers (TEST-02)

This file documents GUI-read endpoints that are intentionally not part of the TEST-02 hard gate yet.

## Enforced in CI

Gate implementation: `cargo test -p mqk-daemon --test scenario_gui_daemon_contract_gate` verifies these endpoint contracts in one stable harness.

### Canonical system surfaces

- `/api/v1/system/status` — shape + semantic truth (db_status, audit_writer_status, daemon_mode, adapter_id)
- `/api/v1/system/preflight` — shape + semantic truth (broker_config_present, db_reachable, blockers list)
- `/api/v1/system/metadata` — shape + semantic truth (build_version, api_version, broker_adapter, endpoint_status)
- `/api/v1/system/session` — shape + semantic truth (daemon_mode, adapter_id, strategy_allowed)
- `/api/v1/system/config-fingerprint` — shape + semantic truth (adapter_id, environment_profile, config_hash)
- `/api/v1/system/runtime-leadership` — shape + semantic truth (leader_node="local", leader_lease_state, generation_id non-empty, restart_count_24h=0, post_restart_recovery_state, checkpoints empty in test state)

### Execution and portfolio summaries

- `/api/v1/execution/summary` — shape check (active_orders, pending_orders, dispatching_orders, reject_count_today)
- `/api/v1/portfolio/summary` — shape check (account_equity, cash, long_market_value, buying_power)

### Risk and reconcile summaries

- `/api/v1/risk/summary` — shape check (gross_exposure, net_exposure, concentration_pct, kill_switch_active)
- `/api/v1/reconcile/status` — shape check (status, last_run_at, mismatched_positions, unmatched_broker_events)

### Strategy

- `/api/v1/strategy/summary` — shape + semantic truth (strategy_id, armed, health fields)

### Audit and operator surfaces

- `/api/v1/audit/operator-actions` — shape + backend identity (canonical_route, backend=postgres.audit_events, rows)
- `/api/v1/audit/artifacts` — shape + backend identity
- `/api/v1/ops/operator-timeline` — shape + backend identity

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

- `/api/v1/execution/orders`
- `/api/v1/portfolio/positions`
- `/api/v1/portfolio/orders/open`
- `/api/v1/portfolio/fills`
- `/api/v1/oms/overview`
- `/api/v1/metrics/dashboards`
- `/api/v1/risk/denials`
- `/api/v1/reconcile/mismatches`
- `/api/v1/alerts/active`
- `/api/v1/events/feed`
- `/api/v1/system/topology`
- `/api/v1/execution/transport`
- `/api/v1/incidents`
- `/api/v1/execution/replace-cancel-chains`
- `/api/v1/alerts/triage`
- `/api/v1/market-data/quality`
- `/api/v1/strategy/suppressions`
- `/api/v1/system/config-diffs`
