# Market-Hours Proof Sweep — Runbook (MARKET-HOURS-PROOF-SWEEP-01)

## Purpose

This runbook covers a single real-NYSE-market-hours session used to capture
two **separate, non-interleaved** proof lanes:

1. `AUTON-NO-TRADE-02` — a canonical paper order attempt, or a durable
   market-hours no-trade explanation, for the autonomous paper path running
   in its normal default configuration (`MQK_RUNTIME_SESSION_SOURCE` unset /
   `legacy`).
2. `ASSET-CORE-05` (`ASSET-CORE-05K`) — confirmation that
   `MQK_RUNTIME_SESSION_SOURCE=v2_equity_active` drives the same real
   wall-clock session behavior as legacy, observed live rather than at
   injected fixed timestamps.

These two lanes must not be interleaved. If both are attempted in the same
market day, run lane 1 to completion (its own observation window, its own
evidence folder, its own closure doc) before starting lane 2. Lane 2 is
conditional on lane 1 finishing with time remaining in the session.

This runbook is observation/evidence-capture only. It does not change
strategy thresholds, does not force a paper order, does not enable live
routing, and does not submit a live order.

---

## Section 1 — AUTON-NO-TRADE-02 observation window

**Scope:** `AUTON-NO-TRADE-02`, the market-hours half of parent
`AUTON-NO-TRADE-01`. Prior read-only audit:
`docs/specs/auton_no_trade_02a_market_hours_preflight_audit.md`. All routes
and DB tables needed for this proof already exist — this is an observation
gap, not a code gap.

### Preconditions

- Daemon running in its existing configured paper mode (Alpaca paper
  adapter). `MQK_RUNTIME_SESSION_SOURCE` **unset** (default `legacy`) for
  this lane.
- Actual NYSE regular session in progress (9:30am–4:00pm ET, non-holiday).
- No `.env.local` edits. No new provider/broker network calls beyond the
  daemon's existing configured startup.

### Live routes to poll

```text
GET /api/v1/system/status
GET /api/v1/system/preflight
GET /api/v1/autonomous/readiness
GET /api/v1/autonomous/no-trade-diagnostics
GET /api/v1/execution/signal-evaluations
GET /api/v1/execution/summary
GET /api/v1/execution/flow
GET /api/v1/execution/orders
GET /api/v1/alerts/active
```

### DB tables (schema-discovery-first — do not assume a fixed column list)

Column lists shift as migrations land, and a runbook that hardcodes "this
table does/doesn't have column X" goes stale the moment a new migration
lands. **Before writing any ad-hoc SQL against any of these tables, always
run schema discovery first:**

```sql
SELECT column_name, data_type
FROM information_schema.columns
WHERE table_name = '<table>'
ORDER BY ordinal_position;
```

Only reference a column in a follow-up query once that discovery query has
confirmed it exists on the live target DB. If a column you expect is absent,
treat the DB as behind the migrations at HEAD (run migrations) rather than
assuming the column was never added — and if a column appears that isn't
listed below, treat this list as incomplete and update it, not the DB as
wrong.

The columns below are what this runbook's proof routes/queries currently
rely on, cross-checked against migrations committed at HEAD. This is a
known floor, not an exhaustive/authoritative ceiling — schema discovery
above is the actual source of truth for any given target DB at query time.

- `runs` — `run_id`, `engine_id`, `mode`, `started_at_utc`, `git_hash`,
  `config_hash`, `config_json`, `host_fingerprint`, `status`, plus the
  lifecycle-stage timestamp columns added by
  `core-rs/crates/mqk-db/migrations/0002_run_lifecycle.sql`:
  `armed_at_utc`, `running_at_utc`, `stopped_at_utc`, `halted_at_utc`,
  `last_heartbeat_utc`. Example schema-conditional check before relying on
  a lifecycle column:

  ```sql
  SELECT EXISTS (
    SELECT 1 FROM information_schema.columns
    WHERE table_name = 'runs' AND column_name = 'armed_at_utc'
  );
  ```

- `strategy_signal_evaluations` — `evaluation_id`, `ts_utc`, `run_id`,
  `strategy_id`, `symbol`, `timeframe`, `bar_context_source`,
  `bars_loaded`, `latest_bar_ts_utc`, `signal_generated`, `signal_qty`,
  `signal_side`, `reason_code`, `reason`, `decision_stage`, `source`.
- `autonomous_no_trade_diagnostics` — `diagnostic_id`, `observed_at_utc`,
  `run_id`, `mode`, `session_window_state`, `runtime_start_allowed`,
  `arm_state`, `overall_ready`, `reason_code`, `reason`, `stage`,
  `paper_order_attempted`, `live_order_attempted`, `source`.
- `oms_outbox` — `outbox_id`, `run_id`, `idempotency_key`, `order_json`,
  `status`, `created_at_utc`, `sent_at_utc`, retry-state columns
  (`dispatch_attempt_count`, `next_dispatch_after_utc`,
  `last_dispatch_error`), plus `claimed_at_utc` added by
  `core-rs/crates/mqk-db/migrations/0005_outbox_claim.sql`. Confirm
  presence via `information_schema.columns` before relying on
  `claimed_at_utc` or any other column, the same as for `runs` above.
- `oms_inbox` — `inbox_id`, `run_id`, `broker_message_id`, `message_json`,
  `received_at_utc`.
- `sys_arm_state` — singleton row (`sentinel_id=1`), `state`, `reason`,
  `updated_at_utc`.

Always re-confirm live via `information_schema.columns` before writing any
ad-hoc SQL beyond what's listed above — this list is a snapshot cross-checked
against migrations at HEAD at the time this runbook was last updated, not a
substitute for a schema check on the actual target DB.

### Evidence folder

```text
exports/market_hours_proof_sweep/auton_no_trade_02/
```

Untracked/generated only — do not stage.

### Closure doc

`docs/specs/auton_no_trade_02b_market_hours_observation_summary.md`, then
(conditionally) `docs/specs/auton_no_trade_02c_market_hours_closure_decision.md`.

---

## Section 2 — ASSET-CORE-05 v2-equity-active observation window

**Scope:** `ASSET-CORE-05K`, a conditional live proof that
`MQK_RUNTIME_SESSION_SOURCE=v2_equity_active` produces the same real
wall-clock session behavior as legacy. Prior preparation:
`docs/runbooks/v2_equity_session_active_market_hours_proof.md` and the
read-only collector `scripts/windows/Collect-V2EquitySessionActiveProof.ps1`.

This lane only runs if:

- The Section 1 window is fully captured and documented first.
- Enough market time remains in the current session for a clean, separate
  second observation window.

### Preconditions

- A **fresh daemon process** started with the temporary process-scoped
  environment override:

  ```powershell
  $env:MQK_RUNTIME_SESSION_SOURCE = "v2_equity_active"
  ```

- This override is **not** persisted globally and **not** written to
  `.env.local`. It applies only to the terminal session that launches this
  proof-window daemon process.
- No live routing. No live orders. No forced paper orders. No non-equity
  trading enabled. No strategy threshold changes.

### Live routes to poll

```text
GET /api/v1/system/status
GET /api/v1/system/preflight
GET /api/v1/autonomous/readiness
GET /api/v1/execution/summary
GET /api/v1/execution/flow
GET /api/v1/alerts/active
GET /api/v1/market-data/intraday-refresh/status
```

### Evidence folder

```text
smoke_logs/
```

(produced by `Collect-V2EquitySessionActiveProof.ps1`) — untracked/generated
only, do not stage.

### Closure doc

`docs/specs/asset_core_05k_v2_equity_active_market_hours_proof.md`
(conditional — only if this lane runs).

---

## Hard safety rules (both lanes)

- Do not enable live routing.
- Do not submit live orders.
- Do not force a paper order.
- Do not change strategy thresholds.
- Do not fabricate signals, orders, fills, market data, snapshots,
  execution-flow rows, or artifacts.
- Do not bypass risk, integrity, reconcile, broker, lease, arm, session,
  DB, or staleness gates.
- Do not weaken fail-closed behavior.
- Do not enable crypto/futures/options/forex/rates trading.
- Do not change config flags except the explicit temporary process env var
  `MQK_RUNTIME_SESSION_SOURCE=v2_equity_active` during the isolated
  Section 2 proof window.
- Do not persist that env var globally. Do not edit `.env.local`.
- Do not stage generated evidence, smoke logs, exports, raw provider
  responses, or `MiniQuantDesk_Master_Patch_Ledger_v2_updated.md`.

## Network rule

Normal existing paper broker/runtime connectivity may be observed only if
the daemon is already configured for paper mode. Do not add new provider or
broker network behavior. Do not call Kraken/TwelveData/Alpaca provider data
endpoints unless a pre-existing operator startup/runbook already requires it
and the operator intentionally starts it. No live broker/order network.

## DB rule

Read-only DB probes are allowed against the paper DB for observation.
Normal existing runtime writes are allowed if produced by the daemon
naturally. Do not manually insert/update/delete proof rows. Do not mutate
DB manually except normal migration/startup flows already required by the
repo.

## Paper order rule

A paper order attempt is acceptable evidence only if naturally produced
through the canonical existing autonomous paper path. A no-trade result is
acceptable if a durable market-hours explanation is recorded.
