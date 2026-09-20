-- MULTI-STRATEGY-RUNTIME-DISPATCH-01 / autonomous completed-bar driver
-- same-symbol multi-strategy repair (V4-BULK-CODE-COMPLETION-STAGE-B-M2-01):
-- durable bar-identity-keyed dispatch claim table, scoped by strategy_id.
--
-- `sys_autonomous_daily_bar_dispatches` (migration 0050) is keyed
-- (operation_id, local_symbol, timeframe, bar_end_ts) -- no strategy_id.
-- That identity is correct and sufficient for the legacy single-active-
-- engine autonomous path (every claim under one operation shares the same
-- one bootstrapped strategy by construction), but it is NOT sufficient once
-- an approved watchlist-v3 fleet binds two different strategies to the same
-- (symbol, timeframe) -- e.g. AAPL/strategy_A and AAPL/strategy_B. Two such
-- bindings' claims would collide on the 0050 identity and incorrectly
-- treat one binding's dispatch as covering the other's.
--
-- This table is deliberately a NEW, separate table rather than an ALTER of
-- 0050 (mirrors the 0050-vs-0071 precedent exactly): 0050's existing rows
-- and primary key are historical, deployed identity for the legacy path and
-- must not be renumbered or have a new NOT NULL key column retrofitted onto
-- them. This table is used exclusively for bindings the autonomous
-- completed-bar driver routes through the approved watchlist-v3 host-pool
-- dispatch path (see `autonomous_completed_bar_driver.rs`'s
-- `HostPoolDispatchRoute`) -- the legacy single-engine path continues to use
-- 0050 unchanged.
--
-- No SQL-generated timestamps or UUIDs -- every value is supplied
-- explicitly by the caller, mirroring migration 0050's convention exactly.

create table if not exists sys_autonomous_daily_binding_bar_dispatches (
    operation_id      uuid        not null,
    symbol            text        not null,
    strategy_id       text        not null,
    timeframe         text        not null,
    bar_end_ts        bigint      not null,
    status            text        not null,
    claimed_at_utc    timestamptz not null,
    completed_at_utc  timestamptz,
    evaluation_id     uuid,
    last_error        text,

    primary key (operation_id, symbol, strategy_id, timeframe, bar_end_ts),

    constraint sys_autonomous_daily_binding_bar_dispatches_operation_fk
        foreign key (operation_id)
        references sys_autonomous_daily_operations (operation_id),

    constraint sys_autonomous_daily_binding_bar_dispatches_symbol_not_blank
        check (length(trim(symbol)) > 0),
    constraint sys_autonomous_daily_binding_bar_dispatches_strategy_id_not_blank
        check (length(trim(strategy_id)) > 0),
    constraint sys_autonomous_daily_binding_bar_dispatches_timeframe_not_blank
        check (length(trim(timeframe)) > 0),
    constraint sys_autonomous_daily_binding_bar_dispatches_bar_end_ts_positive
        check (bar_end_ts > 0),
    constraint sys_autonomous_daily_binding_bar_dispatches_last_error_bounded
        check (last_error is null or length(last_error) <= 4000),

    constraint sys_autonomous_daily_binding_bar_dispatches_status_known
        check (status in ('claimed', 'completed', 'uncertain', 'failed')),
    constraint sys_autonomous_daily_binding_bar_dispatches_completed_requires_timestamp
        check (
            (status = 'completed') = (completed_at_utc is not null)
        ),
    constraint sys_autonomous_daily_binding_bar_dispatches_evaluation_requires_completed
        check (evaluation_id is null or status = 'completed')
);

create index if not exists sys_autonomous_daily_binding_bar_dispatches_operation_idx
    on sys_autonomous_daily_binding_bar_dispatches (operation_id, symbol, strategy_id, timeframe, bar_end_ts desc);
