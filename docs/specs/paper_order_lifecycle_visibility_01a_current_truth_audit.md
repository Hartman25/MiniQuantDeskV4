# Paper Order Lifecycle Visibility — 01A Current Truth Audit

Patch group: `PAPER-ORDER-LIFECYCLE-PERSISTENT-VISIBILITY-AUDIT-AND-CLOSURE-01-COMBINED`
(`PAPER-ORDER-LIFECYCLE-VIS-01A-CURRENT-TRUTH-AUDIT-01`), Phase A.

## 1. Current HEAD

`f4356c9a` (`docs: close paper daily pnl capture`), confirmed via
`git log --oneline -1` at the start of this phase. Working tree clean; no
staged files; untracked files limited to the pre-authorized
`MiniQuantDesk_Master_Patch_Ledger_v2_updated.md` and `smoke_logs/`.

## 2. Proof-02 / prior-bundle context

Confirmed by memory index cross-check against current repo state (not
trusted blindly): `PAPER-DAILY-PNL-BASELINE-CAPTURE-AND-OPERATOR-CLOSURE-01-COMBINED`
lands as commits `6aef1cda`..`f4356c9a`, all present in `git log`. This
audit does not re-verify the market-hours AAPL trade claim of
`PAPER-TRADE-LIFECYCLE-PROOF-02` (no live daemon session available
off-market); it treats that claim as prior context only, and does not rely
on it for any decision below — all decisions here are grounded in current
DB rows and current route/source code.

## 3. Existing visibility routes — active-run-only vs. durable

| Route | Source | Scoping | Restart-surviving? |
|---|---|---|---|
| `GET /api/v1/execution/summary` | `routes/execution.rs::execution_summary` | `AppState.execution_snapshot` (in-memory) | No — `has_snapshot=false` after restart until a run starts |
| `GET /api/v1/execution/orders` | `routes/execution.rs::execution_orders` | `AppState.execution_snapshot` (in-memory) | No — 503 `no_execution_snapshot` after restart |
| `GET /api/v1/execution/outbox` | `routes/execution_order_analysis.rs::execution_outbox` | DB (`oms_outbox`) scoped to `active_run_id` from `current_status_snapshot()` | Partial — DB-backed rows, but resolution requires an ARMED/RUNNING run; `no_active_run` for a STOPPED run |
| `GET /api/v1/execution/flow` | `routes/execution_flow.rs::execution_flow` | DB (`oms_outbox` + `oms_order_lifecycle_events` + `fill_quality_telemetry`); accepts explicit `run_id` query param, else falls back to `current_status_snapshot().active_run_id` | **Yes, if `run_id` is supplied explicitly** (pure DB query, any run, any status). **No** for the no-`run_id` fallback — `current_status_snapshot()` resolves `active_run_id` only for ARMED/RUNNING runs (via `mqk_db::fetch_active_run_for_engine`, filtered `status in ('ARMED','RUNNING')`); a STOPPED run (the common completed-lifecycle case) yields `no_active_run` even though its DB rows are fully present |
| `GET /api/v1/execution/replace-cancel-chains` | `routes/execution_order_analysis.rs` | DB (`oms_order_lifecycle_events`) scoped to `active_run_id` | Same partial behavior as `execution_outbox` |
| `GET /api/v1/execution/signal-evaluations` | `routes/execution.rs` (not yet inspected fully; backed by `fetch_recent_strategy_signal_evaluations`) | DB, but **global most-recent-N across all runs**, no `run_id` filter | Durable rows, but not run-scoped |
| `GET /api/v1/autonomous/no-trade-diagnostics` | backed by `fetch_recent_autonomous_no_trade_diagnostics` | DB, global most-recent-N, no `run_id` filter | Durable rows, but not run-scoped |
| `GET /api/v1/portfolio/summary` / `/positions` / `/orders/open` / `/fills` | `routes/portfolio.rs` | `AppState.broker_snapshot` (in-memory only) | **No** — every one of these routes sets `session_boundary: "in_memory_only"` explicitly in its response; `broker_snapshot` is never persisted |

## 4. Durable table map (verified against live schema, read-only)

Verified via `information_schema.columns` against `mqk-paper-postgres` /
`miniquantdesk_paper` (not guessed):

- `runs` — `run_id, engine_id, mode, status, started_at_utc, armed_at_utc, running_at_utc, stopped_at_utc, halted_at_utc, last_heartbeat_utc, git_hash, config_hash, config_json, host_fingerprint`. Read helpers: `mqk_db::fetch_run(pool, run_id)`, `mqk_db::fetch_latest_run_for_engine(pool, engine_id, mode)`, `mqk_db::fetch_active_run_for_engine(pool, engine_id, mode)` — **all three already exist**, DB-backed, restart-surviving. `fetch_latest_run_for_engine` is NOT currently called by any route (confirmed via `rg`).
- `strategy_signal_evaluations` — durable (migration `0043`). Existing read helper `fetch_recent_strategy_signal_evaluations(pool, limit)` has **no `run_id` filter** — global most-recent-N only.
- `autonomous_no_trade_diagnostics` — durable (migration `0044`). Existing read helper `fetch_recent_autonomous_no_trade_diagnostics(pool, limit)` has **no `run_id` filter** — global most-recent-N only. `run_id` is nullable (no FK to `runs`) by design — a diagnostic can fire with no active run.
- `oms_outbox` — durable (migration `0001` + later ALTERs). Run-scoped read helpers already exist: `mqk_db::outbox_fetch_for_supervisor(pool, run_id)`, `outbox_list_unacked_for_run`, `outbox_load_submitted_for_run`.
- `oms_inbox` — durable. Run-scoped read helpers already exist: `mqk_db::inbox_load_all_applied_for_run(pool, run_id)`, `inbox_load_unapplied_for_run(pool, run_id)`.
- `oms_order_lifecycle_events` — durable (migration `0035`). Run-scoped helper exists: `fetch_order_lifecycle_events_for_run(pool, run_id)`.
- `fill_quality_telemetry` — durable (migration `0028`). Read via `mqk_db::fetch_execution_flow`'s internal query (run-scoped).
- No durable portfolio-position/accounting table exists anywhere in the repo. `scenario_portfolio_snapshot_durability_01.rs` tests `audit_events` rows tagged `ops.repair.portfolio_snapshot`, not a durable position ledger. Portfolio/accounting state lives only in `AppState.broker_snapshot` / `AppState.execution_snapshot`, both in-memory, explicitly labeled `"in_memory_only"` by the existing portfolio routes themselves.

## 5. Live paper-DB proof of the exact gap (read-only, current data)

```
select run_id, engine_id, mode, status, started_at_utc, stopped_at_utc
from runs order by started_at_utc desc limit 3;

15cf4309-210b-5406-8ed8-46377e093195 | mqk-daemon | PAPER | STOPPED | 2026-07-10 18:31:01 | 2026-07-10 18:49:18
2f5e0619-df6b-5907-a0f1-ad019b2dfb57 | mqk-daemon | PAPER | STOPPED | 2026-07-10 16:10:42 | 2026-07-10 18:30:34
741b421f-7e6e-5bbc-bf55-a85c3db5c559 | mqk-daemon | PAPER | STOPPED | 2026-07-09 17:53:27 | 2026-07-09 20:00:28
```

Row counts (informational, no rows mutated): `strategy_signal_evaluations`
= 19, `autonomous_no_trade_diagnostics` = 701, `oms_outbox` = 12,
`oms_inbox` = 50.

The latest PAPER run (`15cf4309-...`) is `STOPPED`, not `ARMED`/`RUNNING`.
Today, `GET /api/v1/execution/flow` called with no `run_id` returns
`truth_state = "no_active_run"` for this exact run, even though its
`oms_outbox`/`oms_inbox`/lifecycle rows are fully present in the DB and
queryable by explicit `run_id`. This is the first proven visibility gap:
**there is no route that resolves "the latest paper run" durably (via
`fetch_latest_run_for_engine`) — only "the currently active run"
(`fetch_active_run_for_engine`), which is empty once a run stops.**

## 6. First proven visibility gap (summary)

Two independent gaps, both closable without a migration:

1. **No durable "latest run" resolution.** Every existing run-scoped route
   (`execution_flow`, `execution_outbox`, `execution/replace-cancel-chains`)
   falls back to `current_status_snapshot().active_run_id`, which is
   `None` for any non-ARMED/RUNNING run. `mqk_db::fetch_latest_run_for_engine`
   already exists and is DB-backed/restart-surviving but is unused by any
   route.
2. **No single joined view across signal evaluations + no-trade
   diagnostics + outbox + inbox for one run.** `execution_flow` joins
   outbox + lifecycle-events + fills, but omits
   `strategy_signal_evaluations` and `autonomous_no_trade_diagnostics`
   entirely, and neither of those two tables' existing fetch helpers
   accept a `run_id` filter.

A third condition is **not** closable within this bundle's no-migration
constraint: durable, restart-surviving portfolio/position/P&L-by-run
visibility does not exist anywhere in the repo today (see §4). This
bundle will report that stage's `portfolio_truth_state` /
`pnl_truth_state` honestly as `in_memory_only_not_restart_surviving`
rather than fabricate a reconstruction from `oms_inbox` fills (which
would require re-deriving position/accounting logic outside the
established `mqk-portfolio`/`mqk-reconcile` seams — out of scope, real new
surface, not a "route contract" change).

## 7. Chosen route contract

Single route, following `execution_flow`'s existing query-param
convention (smallest shape matching current router style — no separate
`/latest` sub-path):

```
GET /api/v1/execution/paper-lifecycle?run_id=<uuid>
```

- `run_id` optional. When absent, resolves the latest run for
  `(engine_id="mqk-daemon", mode="PAPER")` via
  `mqk_db::fetch_latest_run_for_engine` — durable, restart-surviving,
  independent of ARMED/RUNNING status.
- When present, must parse as a UUID (400 `invalid_request` otherwise),
  then resolved via `mqk_db::fetch_run(pool, run_id)`.
- Response joins: `runs` (run truth) + `strategy_signal_evaluations`
  (new run-scoped fetch) + `autonomous_no_trade_diagnostics` (new
  run-scoped fetch) + `oms_outbox` (existing `outbox_fetch_for_supervisor`)
  + `oms_inbox` (existing `inbox_load_all_applied_for_run` +
  `inbox_load_unapplied_for_run`).
- Explicit `run_truth_state`, `signal_truth_state`, `no_trade_truth_state`,
  `outbox_truth_state`, `inbox_truth_state`, `portfolio_truth_state`,
  `pnl_truth_state`, `overall_lifecycle_state`, `blockers`, `warnings` per
  mission spec.
- Read-only: no INSERT/UPDATE/DELETE anywhere in the handler or its DB
  helpers. No broker/provider/network call. No order submission.

## 8. Is a migration needed?

**No.** Every table this route reads already exists and is durable. The
only new code is: (a) two new run-scoped DB read helpers
(`fetch_strategy_signal_evaluations_for_run`,
`fetch_autonomous_no_trade_diagnostics_for_run`) mirroring the exact
pattern of the existing global fetch helpers, added to
`core-rs/crates/mqk-db/src/strategy.rs`; (b) the new response model and
route handler in `mqk-daemon`. No new column, table, or index is
required — the existing `strategy_signal_evaluations_run_symbol_idx`
index already covers the new query's `run_id` filter pattern; the
`autonomous_no_trade_diagnostics` query will do a sequential scan filtered
by `run_id` (table is small — 701 rows today — no index needed for this
read-only operator surface).

## 9. Exact tests planned

- `mqk-db`: DB-backed scenario test proving both new run-scoped fetch
  helpers return only rows for the given `run_id`, return an empty `Vec`
  (not an error) when no rows match, and never write.
- `mqk-daemon`: DB-backed scenario test proving the new route's every
  truth-state branch: `no_db`, `invalid_request` (malformed `run_id`),
  `not_found` (no matching run), `no_rows` (no runs exist at all),
  latest-run resolution when `run_id` omitted, explicit-`run_id`
  resolution, signal-only lifecycle, no-trade-only lifecycle,
  outbox-only lifecycle (`order_submitted_fill_pending`),
  outbox+inbox-fill lifecycle (`order_filled_position_visible` /
  `order_filled_pnl_pending`), and zero DB writes to `oms_outbox` /
  `oms_inbox` / `runs` across every call.

## 10. Non-goals

- No order submission, no broker/provider/network call anywhere in route
  or tests.
- No trading behavior change; no strategy/risk logic change.
- No gate weakening; no change to any existing route's behavior.
- No fabricated lifecycle rows — every field traces to a real DB row or
  an explicit absence/truth-state label.
- No DB migration.
- Portfolio/P&L visibility for this route is reported honestly as
  in-memory-only / not-restart-surviving; this bundle does not attempt to
  reconstruct positions or P&L from `oms_inbox` fills.

`PAPER-ORDER-LIFECYCLE-PERSISTENT-VISIBILITY-AUDIT-AND-CLOSURE-01-COMBINED`:
audit complete. Proceeding to Phase B
(`PAPER-ORDER-LIFECYCLE-VIS-01B-PERSISTENT-DB-MODEL-AND-ROUTE-CONTRACT-01`).
