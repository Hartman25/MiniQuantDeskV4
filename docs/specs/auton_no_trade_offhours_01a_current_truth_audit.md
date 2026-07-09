# AUTON-NO-TRADE-OFFHOURS-01A — Current Truth Audit

Scope: `AUTON-NO-TRADE-OFFHOURS-01` (non-market-hours durable no-trade
explanation). This document is a read-only audit of what already exists in
the current repo (HEAD `1e1ce999`) before any code is written. It does not
change behavior.

## 1. Relevant commits / prior patches

- `AUTON-NO-TRADE-01` — QUEUED / HIGH PRIORITY (`MiniQuantDesk_Master_Patch_Ledger_v2.md`
  section 13). Diagnose why autonomous paper runtime can be running with no
  trades and no durable explanation. Pass condition: a real canonical paper
  order attempt, **or** a durable explanation for why no order occurs.
- **Update (2026-06-22)**: a market-hours paper proof classified the first
  real blocker as legitimate `NO_SIGNAL_GENERATED` — not a wiring bug. That
  reason was only visible live via in-memory readiness counters and did not
  survive a daemon restart.
- `AUTON-NO-SIGNAL-OBS-01` — CLOSED_LOCAL. Added migration
  `0043_strategy_signal_evaluations.sql`, `mqk-db::strategy` insert/fetch
  helpers, `AppState::record_signal_evaluation` (best-effort, non-fatal),
  and `GET /api/v1/execution/signal-evaluations`. This closed the durability
  gap for exactly one no-trade reason: **no signal generated after a
  strategy tick actually ran** (and the two pre-dispatch market-data gate
  refusals that precede it: `no_bars_available`, `stale_bars`).
- `AUTON-NO-TRADE-02` — QUEUED, not yet defined; blocked on
  `AUTON-NO-TRADE-01` evidence.

`AUTON-NO-SIGNAL-OBS-01` did **not** touch the readiness surface itself
(`GET /api/v1/autonomous/readiness`, `mqk-daemon/src/routes/system.rs`,
`autonomous_readiness()`), which remains entirely in-memory.

## 2. Current no-trade explanation matrix

| # | Reason | Current source of truth | Route/API/DB surface | Durable after restart? | Provable non-market-hours? | Close in this prompt? |
|---|--------|--------------------------|------------------------|:---:|:---:|:---:|
| 1 | Outside session window | `AutonomousSessionSchedule::is_in_session` (live calendar/env check), surfaced as `session_in_window` / `session_window_state` | `GET /api/v1/autonomous/readiness` (in-memory only) | No | Yes | **Yes** |
| 2 | Runtime start not allowed (a locally-owned run already exists) | `AppState::locally_owned_run_id()` (in-memory `Option<Uuid>`) | `GET /api/v1/autonomous/readiness` (`runtime_start_allowed`) | No | Yes (can be simulated without market) | **Yes** |
| 3 | No active run (session-controller has not started one yet) | Same `locally_owned_run_id()` combined with `session_in_window` | `GET /api/v1/autonomous/readiness`; also `execution/flow truth_state=no_active_run` | No | Yes | **Yes** |
| 4 | Strategy did not tick | `AppState::bar_tick_dispatch_count()` (in-memory `AtomicU64`, session-scoped) | `GET /api/v1/autonomous/readiness` (`bar_tick_dispatch_count`) | No | Partially (requires a run + no bar deposit, achievable via test seam) | **Yes**, folded into the same snapshot as reasons 1–3 |
| 5 | Strategy ticked but no signal | `strategy_signal_evaluations` table (`AUTON-NO-SIGNAL-OBS-01`) | `GET /api/v1/execution/signal-evaluations` — **already durable** | **Yes (already closed)** | Requires a real dispatch (bars + strategy tick) — market-hours-flavored, but the DB table itself does not require market hours to prove idempotency/shape | Reference only, no new write path |
| 6 | Stale/missing market data | `evaluate_md_freshness_status_for_symbols` (live DB read of `md_bars`, computed per request) | `GET /api/v1/autonomous/readiness` (`market_data_readiness`, blockers) | No (verdict is recomputed every request; `md_bars` rows themselves are durable but the *verdict* is not journaled) | Yes | **Yes**, snapshot the verdict |
| 7 | Arm/execution not ready | `AppState.integrity` (in-memory) + `mqk_db::load_arm_state` (DB-backed `sys_arm_state`, singleton, already durable) | `GET /api/v1/autonomous/readiness` (`arm_state`, `arm_ready`) | Arm state itself: yes (`sys_arm_state`). The *readiness snapshot combining it with other gates*: no | Yes | **Yes**, snapshot the combined verdict |
| 8 | Risk/integrity/kill-switch halt | `AppState.integrity.halted` (in-memory) | `GET /api/v1/autonomous/readiness` (`arm_state="halted"`) | No (in-memory flag; DB `sys_arm_state` reason may independently persist) | Yes | **Yes**, folded into reason 7's snapshot |
| 9 | Outbox/dispatcher/broker path not reached | `execution_snapshot` (in-memory OMS snapshot) | `GET /api/v1/execution/summary`, `/execution/orders` | No | Requires a live dispatch cycle; out of clean off-hours scope | **No** — deferred to a market-hours proof |
| 10 | Broker reject | Broker event via inbox → `oms_outbox`/`oms_inbox` (already durable, part of the canonical lifecycle chain) | `execution/orders`, inbox tables | Yes (already durable, out of this patch's scope — it's lifecycle truth, not diagnostic) | No (requires a real broker interaction) | **No** — out of scope, already durable via canonical chain |
| 11 | Execution truth route mismatch | N/A — cross-route consistency, not a single gate | N/A | N/A | N/A | **No** — not a single-value diagnostic, out of scope |
| 12 | Unknown/unclassified | N/A today | N/A | N/A | Yes | **Yes**, as an explicit fallback classification |

## 3. Exact first durable gap to close

The entire `/api/v1/autonomous/readiness` computation — every gate boolean
(`ws_continuity_ready`, `reconcile_ready`, `arm_ready`, `session_in_window`,
`runtime_start_allowed`, `signal_ingestion_configured`, `strategy_fleet_empty`,
`md_readiness.start_allowed`, `bar_ticker_gate`) and the resulting
`overall_ready` / `blockers` — is computed **fresh from in-memory `AppState`
and live DB reads on every HTTP request**
(`mqk-daemon/src/routes/system.rs::autonomous_readiness`, lines ~670–1112).
Nothing about *why* the system did not attempt an order on a given tick is
written anywhere. If the daemon restarts, or if no operator happens to poll
`/api/v1/autonomous/readiness` at the moment a particular no-trade reason was
true, that explanation is gone forever — even though every one of the
underlying facts it draws from (arm state, session window math, market-data
freshness) is itself either durable or deterministically recomputable from
durable state.

This is the first safe, non-market-hours-dependent durable gap: **a snapshot
of the readiness verdict itself is never journaled.** Reasons 1–4, 6, 7, 8,
and 12 above (all reachable without market hours or a broker) can be closed
by durably recording that verdict. Reasons 9–11 require a live dispatch/
broker cycle and are explicitly deferred.

## 4. Is a DB migration needed?

Yes. No existing table is a semantically correct fit:

- `strategy_signal_evaluations` (0043) is scoped to one `(symbol, timeframe)`
  strategy-tick evaluation attempt — it requires `strategy_id`/`symbol`/
  `timeframe` and a `decision_stage` of `pre_dispatch_gate` /
  `strategy_evaluated`. Off-hours reasons (outside session window, arm not
  ready, no active run) are not per-symbol strategy-tick facts and would
  force fabricated or misleading values into those NOT NULL columns.
- `audit_events` is the hash-chained replay-determinism ledger
  (`.claude/rules/audit_repo_truth_rules.md`); it is the wrong semantic fit
  for high-frequency operator-observability telemetry, exactly as
  `AUTON-NO-SIGNAL-OBS-01` already decided for the sibling table.
- `sys_autonomous_session_events` lacks the gate-level columns
  (`session_window_state`, `runtime_start_allowed`, `arm_state`,
  `overall_ready`) needed to reconstruct which specific gate blocked.

A new, small, additive table (`autonomous_no_trade_diagnostics`) is
justified, following the exact `0043` idiom: deterministic UUIDv5 primary
key, `ON CONFLICT DO NOTHING`, no `DEFAULT now()`/`DEFAULT gen_random_uuid()`,
nullable `run_id` with no FK (observability only, not part of the
outbox/inbox/run lifecycle chain).

## 5. Is a read-only route needed?

Yes — `GET /api/v1/autonomous/no-trade-diagnostics`, mirroring
`GET /api/v1/execution/signal-evaluations`: not scoped to the active run, so
a restart-surviving off-hours diagnostic stays visible with no run active.

## 6. What this patch will not do

- No paper order will be submitted.
- No live order will be submitted.
- No provider (Alpaca/Kraken/TwelveData) call will be made.
- No strategy threshold, gate, or admission behavior will change.
- No existing gate will be weakened, bypassed, or reordered.
- No fabricated `run_id`, symbol, strategy, signal, outbox, or broker state —
  every diagnostic field is read from already-live `AppState`/DB truth, or
  is honestly `None`.
- No change to `start_execution_runtime`, the session controller's actual
  start/stop decisions, or any broker adapter.
- Reasons 9–11 (outbox/dispatcher/broker-not-reached, broker reject,
  cross-route mismatch) are **not** closed by this prompt — they require a
  live dispatch cycle and remain for a future market-hours proof.

## 7. Off-hours vs market-hours boundary (explicit)

`AUTON-NO-TRADE-OFFHOURS-01` closing does **not** close parent
`AUTON-NO-TRADE-01`. The parent's pass condition (`a real canonical paper
order attempt occurs, OR the system durably explains why no order occurs`)
still has an unproven market-hours half: durably recording that the
*outbox/dispatcher/broker path was reached* (or wasn't) during a live
session requires a real dispatch, which this off-hours prompt explicitly
must not force.
