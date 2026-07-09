# AUTON-NO-TRADE-02A — Market-Hours Preflight Audit

Scope: `AUTON-NO-TRADE-02` (market-hours canonical paper order / no-trade
proof — the remaining half of parent `AUTON-NO-TRADE-01`). This document is
a read-only audit of what already exists in the current repo (HEAD
`5cde6f68`) before any live market-hours observation is attempted. It does
not change behavior and does not submit any order.

**This audit was performed outside market hours** (local wall-clock at
audit time: 2026-07-08 ~21:56 CDT / 2026-07-09 02:56 UTC — well after NYSE
regular-session close). Per this bundle's own gating rule, only Phase A
(this document) is in scope for the current turn; Phases B–D require an
actual live paper-trading session window and are deferred to a future turn
that runs during market hours.

## 1. Prior proven state (verified against current HEAD, not memory)

- `AUTON-NO-SIGNAL-OBS-01` — `CLOSED_LOCAL`. Migration
  `0043_strategy_signal_evaluations.sql` created `strategy_signal_evaluations`
  (deterministic UUIDv5 `evaluation_id`, `ON CONFLICT DO NOTHING`, nullable
  `run_id`, no FK). Read surface: `GET /api/v1/execution/signal-evaluations`
  (`mqk-daemon/src/routes/execution.rs`, `CANONICAL` const confirmed at
  line 747). Covers exactly one no-trade reason class: a strategy tick that
  ran (or was gated pre-dispatch) and produced no signal, at
  `(strategy_id, symbol, timeframe)` grain.
- `AUTON-NO-TRADE-OFFHOURS-01` — `CLOSED_LOCAL`. Migration
  `0044_autonomous_no_trade_diagnostics.sql` created
  `autonomous_no_trade_diagnostics` (deterministic minute-bucketed UUIDv5
  `diagnostic_id`, `ON CONFLICT DO NOTHING`, nullable `run_id`, no FK). Read
  surface: `GET /api/v1/autonomous/no-trade-diagnostics`
  (`mqk-daemon/src/routes/system.rs`, `CANONICAL` const confirmed at line
  1168). Every poll of `GET /api/v1/autonomous/readiness` durably snapshots
  the dominant no-trade reason via the pure `classify_no_trade_diagnostic`
  helper (confirmed at `system.rs:1250-1307`). Covers reasons reachable
  without market hours or a broker: `WS_CONTINUITY_NOT_READY`,
  `RECONCILE_NOT_READY`, `INTEGRITY_HALTED`/`ARM_NOT_READY`,
  `SIGNAL_INGESTION_NOT_CONFIGURED`, `OUTSIDE_SESSION_WINDOW`,
  `BAR_TICKER_GATE_CLOSED`/`STRATEGY_NOT_TICKED`/`NO_SIGNAL_GENERATED`/
  `RUNTIME_ALREADY_ACTIVE` (when a run is active), `STRATEGY_FLEET_EMPTY`,
  `MARKET_DATA_NOT_READY`, `NO_ACTIVE_RUN_PENDING_START`, `UNKNOWN`
  (fallback). Every row this table stores hardcodes
  `paper_order_attempted=false` and `live_order_attempted=false` by design
  (migration comment, `0044_autonomous_no_trade_diagnostics.sql:25-27`) —
  **this table can never itself prove a paper order attempt occurred; it
  only ever proves why one did not.**
- Parent `AUTON-NO-TRADE-01` — confirmed still `PARTIAL /
  MARKET-HOURS-PROOF-REMAINS` per
  `docs/specs/auton_no_trade_offhours_01e_closure_decision.md` §2 and
  `MiniQuantDesk_Master_Patch_Ledger_v2.md` §13. The offhours closure
  decision explicitly names the remaining gap: reasons 9–11 from its own
  Phase A matrix (outbox/dispatcher/broker-not-reached, broker reject,
  cross-route execution-truth mismatch) all require a live dispatch cycle
  and were explicitly deferred.

## 2. Current market-hours-relevant routes (verified against current HEAD)

| Route | File | Confirmed |
|---|---|---|
| `GET /api/v1/autonomous/readiness` | `mqk-daemon/src/routes/system.rs::autonomous_readiness` | `routes.rs:472` |
| `GET /api/v1/autonomous/no-trade-diagnostics` | `mqk-daemon/src/routes/system.rs`, `CANONICAL` at line 1168 | `routes.rs:477` |
| `GET /api/v1/execution/signal-evaluations` | `mqk-daemon/src/routes/execution.rs`, `CANONICAL` at line 747 | `routes.rs:377` |
| `GET /api/v1/execution/summary` | `mqk-daemon/src/routes/execution.rs::execution_summary` | `routes.rs:368` |
| `GET /api/v1/execution/flow` | `mqk-daemon/src/routes/execution_flow.rs`, `CANONICAL` at line 43 | `routes.rs:371` |
| `GET /api/v1/execution/orders` | `mqk-daemon/src/routes/execution.rs::execution_orders` | `routes.rs:369` |

All six routes exist today and are already wired into the router. No new
route is required to observe market-hours behavior — Phase B is a pure
observation runbook against existing surfaces, not new code.

`truth_state` values confirmed present across these surfaces: `active`,
`no_db`, `no_active_run`, `db_unavailable`, `query_failed`, `no_rows`,
`not_applicable` (readiness-only). Every data-bearing route already
distinguishes unavailable / empty / present per `gui_rules.md`.

## 3. DB tables (schema verified against current migrations, not assumed)

- `runs` (`0001_init.sql:3-11`): `run_id`, `engine_id`, `mode`
  (`PAPER`/`LIVE`), `started_at_utc`, `git_hash`, `config_hash`,
  `config_json`, `host_fingerprint`. **No `armed_at_utc`, `running_at_utc`,
  `stopped_at_utc`, `halted_at_utc`, or `last_heartbeat_utc` columns exist.**
  Any market-hours runbook probe against `runs` must select only the
  columns that actually exist.
- `strategy_signal_evaluations` (`0043_strategy_signal_evaluations.sql`):
  `evaluation_id`, `ts_utc`, `run_id`, `strategy_id`, `symbol`, `timeframe`,
  `bar_context_source`, `bars_loaded`, `latest_bar_ts_utc`,
  `signal_generated`, `signal_qty`, `signal_side`, `reason_code`, `reason`,
  `decision_stage`, `source`.
- `autonomous_no_trade_diagnostics` (`0044_autonomous_no_trade_diagnostics.sql`):
  `diagnostic_id`, `observed_at_utc`, `run_id`, `mode`,
  `session_window_state`, `runtime_start_allowed`, `arm_state`,
  `overall_ready`, `reason_code`, `reason`, `stage`, `paper_order_attempted`,
  `live_order_attempted`, `source`.
- `oms_outbox` (`0001_init.sql:31-39`): `outbox_id`, `run_id`,
  `idempotency_key`, `order_json`, `status` (`PENDING`/`SENT`/`ACKED`/
  `FAILED`), `created_at_utc`, `sent_at_utc`. **No `claimed_at_utc`,
  `acked_at_utc`, or `rejected_at_utc` columns exist** — outcome truth for
  those states lives in `status` plus `order_json`, not separate timestamp
  columns. Any market-hours runbook probe must use the real column set.
- `oms_inbox` (`0001_init.sql:45-51`): `inbox_id`, `run_id`,
  `broker_message_id`, `message_json`, `received_at_utc`.
- `sys_arm_state` (`0006_arm_state.sql`): singleton (`sentinel_id=1`),
  `state` (`ARMED`/`DISARMED`), `reason` (nullable `DisarmReason`),
  `updated_at_utc`.

## 4. Pass/fail matrix for market-hours proof

| Evidence | Durable source | Route | Provable today without new code? |
|---|---|---|---|
| Paper order attempt reached outbox | `oms_outbox` row exists for the session's `run_id` | `GET /api/v1/execution/orders`, `GET /api/v1/execution/summary` (has_snapshot) | Yes — already durable lifecycle chain per `execution_rules.md` |
| Dispatcher claimed outbox | `oms_outbox.status` transitions from `PENDING` | `GET /api/v1/execution/orders` | Yes |
| Broker submit attempted | `oms_outbox.status = SENT`, `sent_at_utc` populated | `GET /api/v1/execution/orders` | Yes |
| Broker reject/ack/fill recorded | `oms_inbox` row + `oms_outbox.status` terminal | `GET /api/v1/execution/orders`, `GET /api/v1/execution/flow` | Yes |
| No signal generated durably recorded | `strategy_signal_evaluations` row, `signal_generated=false` | `GET /api/v1/execution/signal-evaluations` | Yes (`AUTON-NO-SIGNAL-OBS-01`, closed) |
| Stale/missing market data durably recorded | `strategy_signal_evaluations` row, `bar_context_source in (no_bars_available, stale_bars)` **or** `autonomous_no_trade_diagnostics` row, `reason_code=MARKET_DATA_NOT_READY` | Both signal-evaluations and no-trade-diagnostics routes | Yes (both closed) |
| No active run durably recorded | `autonomous_no_trade_diagnostics` row, `reason_code=NO_ACTIVE_RUN_PENDING_START` | `GET /api/v1/autonomous/no-trade-diagnostics` | Yes (`AUTON-NO-TRADE-OFFHOURS-01`, closed) |
| Arm/session/risk/reconcile blocker durably recorded | `autonomous_no_trade_diagnostics` row, `reason_code in (ARM_NOT_READY, INTEGRITY_HALTED, OUTSIDE_SESSION_WINDOW, RECONCILE_NOT_READY, WS_CONTINUITY_NOT_READY)` | `GET /api/v1/autonomous/no-trade-diagnostics` | Yes (closed) |

**The remaining unproven cell is the first four rows during an actual
live market-hours session** — not because the routes or DB tables are
missing, but because nobody has yet observed a real market-hours tick that
either (a) produces a signal and reaches `oms_outbox`, or (b) confirms the
dominant `autonomous_no_trade_diagnostics`/`strategy_signal_evaluations`
reason during an actual open session rather than in a test harness or an
off-hours poll. This is an observation gap, not a code gap.

## 5. What this prompt may observe but not force

- May observe: live polls of the six routes above during an actual NYSE
  regular session, and read-only DB queries against the six tables above,
  while the daemon is running in its existing configured paper mode.
- May not: change strategy thresholds, submit a live order, fabricate a
  signal/order/fill/bar, bypass any risk/session/arm/reconcile/staleness/
  broker gate, weaken fail-closed behavior, add new provider/broker network
  calls beyond what the daemon's existing configured paper workflow already
  performs, or write to `.env.local`/config outside existing startup
  scripts.
- **No live order will be submitted. No paper order will be forced.** A
  paper order attempt is acceptable evidence only if it is naturally
  produced by the existing autonomous dispatch path during a real session;
  a durable no-trade explanation via `strategy_signal_evaluations` or
  `autonomous_no_trade_diagnostics` is equally acceptable closure evidence
  per the parent's original pass condition.

## 6. Phase C scope preview (conditional, not decided here)

Phase A finds no evidence yet of a *missing* durable seam — every route
and table needed to classify a market-hours no-trade outcome already
exists and is already closed by `AUTON-NO-SIGNAL-OBS-01` and
`AUTON-NO-TRADE-OFFHOURS-01`. Phase C should only be exercised if a live
Phase B observation surfaces a genuine gap (for example: a field the
runbook needs that no current route exposes, or a DB column the runbook
needs that no current schema provides). Absent that live evidence, Phase C
is expected to be skipped.

## 7. Next step

Phase B (`AUTON-NO-TRADE-02B-LIVE-PAPER-OBSERVATION-RUNBOOK-AND-GUARD-01`)
and any live observation must run during an actual NYSE regular session
window. This turn stops after Phase A because the current wall-clock time
is outside market hours.
