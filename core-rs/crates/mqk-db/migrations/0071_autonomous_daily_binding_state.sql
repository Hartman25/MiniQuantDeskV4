-- MULTI-STRATEGY-RUNTIME-DISPATCH-01 / autonomous completed-bar driver
-- multi-binding repair (R2A): durable per-binding health state.
--
-- Frozen operator decision (V4-STAGE-B-M2-REPAIR-02, R2): hybrid per-binding
-- fault isolation. A binding-local failure (symbol-specific no-new-bar/
-- waiting, missing/stale market data, symbol-specific readiness failure,
-- unsupported/invalid symbol/timeframe binding, or a provider failure
-- proven to affect only one binding) must quarantine only the affected
-- binding, never the whole autonomous operation. A global-critical failure
-- (ambiguous/unresolved durable dispatch claim, duplicate/conflicting
-- evaluation identity, evidence-lineage corruption, provenance/authority
-- mismatch, DB integrity/write uncertainty, runtime ownership/leadership
-- failure, account/broker identity uncertainty, or a global risk/integrity
-- halt) remains operation-wide fail-closed via the existing
-- `sys_autonomous_daily_operations.state` machine, unchanged.
--
-- This table is deliberately NOT `sys_autonomous_daily_bar_dispatches`
-- (migration 0050): that table's identity is per-BAR
-- (operation_id, local_symbol, timeframe, bar_end_ts) -- a claim record for
-- one specific completed bar, proving "this exact bar was/wasn't
-- dispatched." This table's identity is per-BINDING
-- (operation_id, symbol, strategy_id, timeframe) -- a durable health record
-- answering "is this configured symbol/strategy/timeframe assignment
-- currently progress-capable for this operation, or has it been locally
-- quarantined, and why." The two are orthogonal and both required: a
-- binding can be `active` while its most recent bar is still `claimed`
-- (in flight), and a binding can be `locally_blocked` with zero bar-claim
-- rows at all (it never got that far).
--
-- No SQL-generated timestamps or UUIDs -- every value is supplied
-- explicitly by the caller, mirroring migration 0050's convention exactly.
--
-- Foreign key to sys_autonomous_daily_operations without a destructive
-- cascade (default `NO ACTION`): a binding-state row can never survive its
-- parent operation being removed by an on-delete cascade, and an operation
-- row can never be silently deleted out from under an existing binding
-- state (identical convention to migration 0050).

create table if not exists sys_autonomous_daily_binding_state (
    operation_id      uuid        not null,
    symbol            text        not null,
    strategy_id       text        not null,
    timeframe         text        not null,
    status            text        not null,
    reason_code       text,
    last_error        text,
    updated_at_utc    timestamptz not null,

    primary key (operation_id, symbol, strategy_id, timeframe),

    constraint sys_autonomous_daily_binding_state_operation_fk
        foreign key (operation_id)
        references sys_autonomous_daily_operations (operation_id),

    constraint sys_autonomous_daily_binding_state_symbol_not_blank
        check (length(trim(symbol)) > 0),
    constraint sys_autonomous_daily_binding_state_strategy_id_not_blank
        check (length(trim(strategy_id)) > 0),
    constraint sys_autonomous_daily_binding_state_timeframe_not_blank
        check (length(trim(timeframe)) > 0),
    constraint sys_autonomous_daily_binding_state_last_error_bounded
        check (last_error is null or length(last_error) <= 4000),

    -- Closed status vocabulary. `active`: this binding is progress-capable
    -- (may be dispatched this and future ticks). `locally_blocked`: a
    -- binding-local (never global-critical) fault durably quarantines only
    -- this one binding -- other bindings in the same operation are
    -- unaffected and continue to be evaluated on their own ticks.
    constraint sys_autonomous_daily_binding_state_status_known
        check (status in ('active', 'locally_blocked')),

    -- reason_code is required exactly when locally_blocked, and absent
    -- exactly when active -- never a blocked row with no stated reason,
    -- never an active row carrying a stale reason from a prior block.
    constraint sys_autonomous_daily_binding_state_reason_requires_blocked
        check ((status = 'locally_blocked') = (reason_code is not null)),

    -- Closed, bounded reason-code vocabulary for a binding-local block --
    -- mirrors the frozen R2 operator decision's own enumerated binding-local
    -- categories exactly. An unrecognized reason can never be written; a
    -- global-critical condition must never be recorded here at all (it
    -- belongs to sys_autonomous_daily_operations' own state/blocker path).
    constraint sys_autonomous_daily_binding_state_reason_known
        check (
            reason_code is null or reason_code in (
                'binding_no_new_bar_waiting',
                'binding_market_data_missing_or_stale',
                'binding_readiness_blocked',
                'binding_unsupported_or_invalid_symbol_timeframe',
                'binding_provider_failure_isolated'
            )
        )
);

-- Supports "list every binding's current state for this operation" (the
-- aggregation read R2C's apply step needs every tick) without a table scan.
create index if not exists sys_autonomous_daily_binding_state_operation_idx
    on sys_autonomous_daily_binding_state (operation_id);
