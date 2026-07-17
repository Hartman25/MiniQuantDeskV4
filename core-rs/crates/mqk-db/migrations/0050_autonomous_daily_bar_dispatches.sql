-- AUTONOMOUS-DAILY-PAPER-OPERATIONS-01C-COMPLETED-BAR-DATA-DRIVER: durable
-- bar-identity-keyed dispatch claim table.
--
-- Audit conclusion (recorded in the Phase C proof/ledger): the existing
-- native strategy dispatch path has NO durable idempotency keyed on bar
-- identity (symbol, timeframe, bar_end_ts). `strategy_signal_evaluations`
-- dedupes on `evaluation_id = uuidv5(run_id|strategy_id|symbol|timeframe|
-- now_tick)` -- an arbitrary in-process tick counter, not a bar timestamp --
-- and is a best-effort telemetry write, not a claim gate. `oms_outbox`
-- dedupes on `decision_id = uuidv5(run_id:strategy_id:symbol:side:qty:
-- now_micros)`, where `now_micros` is the wall-clock instant of the loop
-- tick, not derived from the bar's `end_ts`. Neither seam prevents the same
-- completed bar from being dispatched twice (e.g. after a daemon restart
-- re-evaluates the same bar, or a driver retry re-observes it). A new,
-- additive claim table is required.
--
-- `sys_autonomous_daily_bar_dispatches` is a durable "claim before dispatch"
-- record keyed on the exact bar identity within one operation. Before the
-- Phase C completed-bar driver calls the canonical native-strategy dispatch
-- path for a bar, it must first insert (claim) a row here for
-- `(operation_id, local_symbol, timeframe, bar_end_ts)`. If a row already
-- exists, the driver never dispatches again automatically:
--   - `completed`: the bar was already durably dispatched -- no redispatch.
--   - `claimed`/`uncertain`/`failed`: the outcome of a prior attempt is not
--     provably complete -- fail closed as `unresolved_dispatch_claim`, never
--     redispatch, never silently skip.
--
-- No SQL-generated timestamps, UUIDs, or operational defaults -- every value
-- is supplied explicitly by the caller
-- (mqk_db::autonomous_daily_operation::claim_autonomous_daily_bar_dispatch /
-- complete_autonomous_daily_bar_dispatch / fail_autonomous_daily_bar_dispatch).
--
-- Foreign key to sys_autonomous_daily_operations without a destructive
-- cascade (default `NO ACTION`): a dispatch-claim row can never survive its
-- parent operation being removed by an on-delete cascade, and an operation
-- row can never be silently deleted out from under an existing claim.

create table if not exists sys_autonomous_daily_bar_dispatches (
    operation_id      uuid        not null,
    local_symbol      text        not null,
    timeframe         text        not null,
    bar_end_ts        bigint      not null,
    status            text        not null,
    claimed_at_utc    timestamptz not null,
    completed_at_utc  timestamptz,
    evaluation_id     uuid,
    last_error        text,

    primary key (operation_id, local_symbol, timeframe, bar_end_ts),

    constraint sys_autonomous_daily_bar_dispatches_operation_fk
        foreign key (operation_id)
        references sys_autonomous_daily_operations (operation_id),

    constraint sys_autonomous_daily_bar_dispatches_local_symbol_not_blank
        check (length(trim(local_symbol)) > 0),
    constraint sys_autonomous_daily_bar_dispatches_timeframe_not_blank
        check (length(trim(timeframe)) > 0),
    constraint sys_autonomous_daily_bar_dispatches_bar_end_ts_positive
        check (bar_end_ts > 0),
    constraint sys_autonomous_daily_bar_dispatches_last_error_bounded
        check (last_error is null or length(last_error) <= 4000),

    constraint sys_autonomous_daily_bar_dispatches_status_known
        check (status in ('claimed', 'completed', 'uncertain', 'failed')),
    constraint sys_autonomous_daily_bar_dispatches_completed_requires_timestamp
        check (
            (status = 'completed') = (completed_at_utc is not null)
        ),
    constraint sys_autonomous_daily_bar_dispatches_evaluation_requires_completed
        check (evaluation_id is null or status = 'completed')
);

-- Supports the driver's "is this operation's most recent bar already
-- resolved" read without scanning the whole table.
create index if not exists sys_autonomous_daily_bar_dispatches_operation_idx
    on sys_autonomous_daily_bar_dispatches (operation_id, local_symbol, timeframe, bar_end_ts desc);
