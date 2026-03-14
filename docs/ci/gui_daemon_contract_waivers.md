# GUI/Daemon Contract CI Waivers (TEST-02)

This file documents GUI-read endpoints that are intentionally not part of the TEST-02 hard gate yet.

## Enforced in CI


Gate implementation: `cargo test -p mqk-daemon --test scenario_gui_daemon_contract_gate` verifies these endpoint contracts in one stable harness.

The CI contract gate currently enforces the daemon surfaces the GUI depends on most directly for top-level health and operational dashboards:

- `/api/v1/system/status`
- `/api/v1/system/preflight`
- `/api/v1/execution/summary`
- `/api/v1/portfolio/summary`
- `/api/v1/risk/summary`
- `/api/v1/reconcile/status`
- `/v1/status`
- `/v1/health`
- `/v1/trading/account`
- `/v1/trading/positions`
- `/v1/trading/orders`
- `/v1/trading/fills`

Legacy trading response contract enforced by the harness follows accepted DMON-04 semantics:
- `snapshot_state` and `snapshot_captured_at_utc` metadata are present
- payload fields (`account`, `positions`, `orders`, `fills`) are present and may be `null` when snapshot truth is unavailable
- stale `has_snapshot` is not accepted

## Explicitly deferred from TEST-02 gate

The GUI probes additional detail endpoints that are not yet authoritative daemon contract surfaces and therefore remain explicitly deferred:

- `/api/v1/system/metadata`
- `/api/v1/execution/orders`
- `/api/v1/portfolio/positions`
- `/api/v1/portfolio/orders/open`
- `/api/v1/portfolio/fills`
- `/api/v1/oms/overview`
- `/api/v1/metrics/dashboards`
- `/api/v1/risk/denials`
- `/api/v1/reconcile/mismatches`
- `/api/v1/strategy/summary`
- `/api/v1/alerts/active`
- `/api/v1/events/feed`
- `/api/v1/system/topology`
- `/api/v1/execution/transport`
- `/api/v1/incidents`
- `/api/v1/execution/replace-cancel-chains`
- `/api/v1/alerts/triage`
- `/api/v1/system/session`
- `/api/v1/system/config-fingerprint`
- `/api/v1/market-data/quality`
- `/api/v1/system/runtime-leadership`
- `/api/v1/strategy/suppressions`
- `/api/v1/system/config-diffs`

These waivers are visible by design so deferred coverage is explicit instead of silently ignored.
