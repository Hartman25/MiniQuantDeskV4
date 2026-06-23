-- AUTON-NO-SIGNAL-OBS-01: Durable strategy signal-evaluation journal.
--
-- One row per native-strategy bar-tick evaluation attempt against a
-- (symbol, timeframe) pair. Captures whether the strategy's `on_bar` actually
-- ran, how many bars it saw, and -- whether or not a trade signal resulted --
-- why. A no-signal row is informational, not an error: writing this table
-- never implies an oms_outbox/order/fill row exists, and this table is never
-- read by any gate, decision, or dispatch path.
--
-- `evaluation_id` is UUIDv5(NAMESPACE_DNS,
--   "mqk.signal-evaluation.v1|{run_id}|{strategy_id}|{symbol}|{timeframe}|{now_tick}")
-- -- deterministic so a duplicate write attempt for the same logical tick
-- cannot double-insert (ON CONFLICT DO NOTHING).
--
-- `run_id` is intentionally nullable with no FK to `runs`: unlike
-- fill_quality_telemetry (which only ever fires from inside an active run's
-- broker-event path), a signal evaluation can legitimately happen with no
-- active run (e.g. a test/dry-run dispatch), and this table is observability
-- only -- it is not part of the outbox/inbox/run lifecycle chain.
--
-- `decision_stage`:
--   'pre_dispatch_gate'   -- a fail-closed market-data gate (missing or stale
--                             bars) refused dispatch before the strategy's
--                             on_bar callback ran at all.
--   'strategy_evaluated'  -- on_bar ran to completion; signal_generated /
--                             signal_qty / signal_side reflect its real,
--                             unmodified output.

create table if not exists strategy_signal_evaluations (
    evaluation_id       uuid         primary key,
    ts_utc              timestamptz  not null,
    run_id              uuid,
    strategy_id         text         not null,
    symbol              text         not null,
    timeframe           text         not null,
    bar_context_source  text         not null,
    bars_loaded         bigint       not null,
    latest_bar_ts_utc   timestamptz,
    signal_generated    boolean      not null,
    signal_qty          bigint,
    signal_side         text,
    reason_code         text         not null,
    reason              text         not null,
    decision_stage      text         not null,
    source              text         not null
);

-- Most-recent-N query pattern used by GET /api/v1/execution/signal-evaluations.
create index if not exists strategy_signal_evaluations_ts_idx
    on strategy_signal_evaluations (ts_utc desc);

-- Per-run/per-symbol drill-down query pattern (operator investigating one
-- symbol's evaluation history after a restart).
create index if not exists strategy_signal_evaluations_run_symbol_idx
    on strategy_signal_evaluations (run_id, symbol, ts_utc desc);
