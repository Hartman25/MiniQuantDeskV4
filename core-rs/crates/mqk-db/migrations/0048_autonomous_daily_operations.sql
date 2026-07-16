-- AUTONOMOUS-DAILY-PAPER-OPERATIONS-01B-DURABLE-COORDINATION: durable daily
-- autonomous-operation coordination foundation.
--
-- sys_autonomous_daily_operations is the mutable current-state row for
-- exactly one applicable autonomous PAPER daily operation per
-- (market_date, deployment_mode, adapter_id) daily slot. Neither `runs`
-- (no natural daily key, LIVE-only unique index) nor
-- `sys_autonomous_session_events` (append-only log, no CAS, no per-day
-- identity) can support idempotent daily identity or CAS-safe concurrent
-- transitions as they exist today -- this table is additive, net-new.
--
-- sys_autonomous_daily_operation_events is the append-only transition
-- history for one operation_id. Every state_version bump on the current-state
-- row corresponds to exactly one event row at transition_seq = state_version,
-- inserted in the same transaction (mqk_db::autonomous_daily_operation).
--
-- Daily-slot vs. operation identity: `unique (market_date, deployment_mode,
-- adapter_id)` is the concurrency/idempotency anchor (only one applicable
-- operation may ever exist for one broker/deployment/day). `operation_id`
-- (UUIDv5, computed in mqk-daemon over six identity components including
-- session_plan_identity/assignment_identity/runtime_binding_identity) is a
-- separate, finer-grained identity stored as the primary key; a same-day
-- identity mismatch (operator changed strategy/symbol/timeframe config and
-- restarted mid-day) is surfaced as a typed conflict by the caller, never a
-- second row for the same slot and never a silently rewritten identity
-- field.
--
-- No DEFAULT now(), no DEFAULT gen_random_uuid(), no fabricated numeric
-- DEFAULT for any not-null column -- every value (including state_version=1,
-- start_attempt_count=0, bars_observed=0, bars_dispatched=0, etc. at initial
-- creation) is supplied explicitly by the caller
-- (mqk_db::autonomous_daily_operation::create_or_recover_autonomous_daily_operation).
--
-- Foundation-only migration: no production wiring into session_controller.rs,
-- autonomous_bar_ticker.rs, the market-data scheduler, start_execution_runtime,
-- any HTTP route, or any GUI component is added by this patch.

create table if not exists sys_autonomous_daily_operations (
    operation_id                  uuid        primary key,
    market_date                   date        not null,
    deployment_mode               text        not null,
    adapter_id                    text        not null,

    session_plan_identity         text        not null,
    assignment_identity           text        not null,
    runtime_binding_identity      text        not null,

    calendar_source               text        not null,
    calendar_coverage_state       text        not null,
    schedule_source               text        not null,

    session_open_utc              timestamptz not null,
    session_close_utc             timestamptz not null,
    preopen_start_utc             timestamptz not null,
    postclose_finalize_utc        timestamptz not null,

    state                         text        not null,
    state_reason_code             text,
    state_version                 bigint      not null,

    run_id                        uuid,
    start_attempt_count           bigint      not null,
    last_start_attempt_utc        timestamptz,
    next_retry_utc                timestamptz,

    data_refresh_state            text        not null,
    last_provider_poll_utc        timestamptz,
    provider_poll_attempt_count   bigint      not null,
    provider_poll_success_count   bigint      not null,
    provider_poll_failure_count   bigint      not null,

    last_completed_bar_ts         bigint,
    last_dispatched_bar_ts        bigint,
    bars_observed                 bigint      not null,
    bars_dispatched               bigint      not null,

    started_at_utc                timestamptz,
    stopped_at_utc                timestamptz,
    finalized_at_utc              timestamptz,

    outcome                       text,
    no_trade_reason               text,
    last_error                    text,

    created_at_utc                timestamptz not null,
    updated_at_utc                timestamptz not null,

    constraint sys_autonomous_daily_operations_deployment_mode_not_blank
        check (length(trim(deployment_mode)) > 0),
    constraint sys_autonomous_daily_operations_adapter_id_not_blank
        check (length(trim(adapter_id)) > 0),
    constraint sys_autonomous_daily_operations_session_plan_identity_not_blank
        check (length(trim(session_plan_identity)) > 0),
    constraint sys_autonomous_daily_operations_assignment_identity_not_blank
        check (length(trim(assignment_identity)) > 0),
    constraint sys_autonomous_daily_operations_runtime_binding_identity_not_blank
        check (length(trim(runtime_binding_identity)) > 0),
    constraint sys_autonomous_daily_operations_calendar_source_not_blank
        check (length(trim(calendar_source)) > 0),
    constraint sys_autonomous_daily_operations_data_refresh_state_not_blank
        check (length(trim(data_refresh_state)) > 0),
    constraint sys_autonomous_daily_operations_data_refresh_state_bounded
        check (length(data_refresh_state) <= 64),

    constraint sys_autonomous_daily_operations_session_open_before_close
        check (session_open_utc < session_close_utc),
    constraint sys_autonomous_daily_operations_preopen_before_open
        check (preopen_start_utc <= session_open_utc),
    constraint sys_autonomous_daily_operations_postclose_after_close
        check (postclose_finalize_utc >= session_close_utc),

    constraint sys_autonomous_daily_operations_state_version_positive
        check (state_version >= 1),
    constraint sys_autonomous_daily_operations_start_attempt_count_nonneg
        check (start_attempt_count >= 0),
    constraint sys_autonomous_daily_operations_provider_poll_attempt_nonneg
        check (provider_poll_attempt_count >= 0),
    constraint sys_autonomous_daily_operations_provider_poll_success_nonneg
        check (provider_poll_success_count >= 0),
    constraint sys_autonomous_daily_operations_provider_poll_failure_nonneg
        check (provider_poll_failure_count >= 0),
    constraint sys_autonomous_daily_operations_bars_observed_nonneg
        check (bars_observed >= 0),
    constraint sys_autonomous_daily_operations_bars_dispatched_nonneg
        check (bars_dispatched >= 0),
    constraint sys_autonomous_daily_operations_bars_dispatched_le_observed
        check (bars_dispatched <= bars_observed),
    constraint sys_autonomous_daily_operations_completed_bar_ts_positive
        check (last_completed_bar_ts is null or last_completed_bar_ts > 0),
    constraint sys_autonomous_daily_operations_dispatched_bar_ts_positive
        check (last_dispatched_bar_ts is null or last_dispatched_bar_ts > 0),

    constraint sys_autonomous_daily_operations_last_error_bounded
        check (last_error is null or length(last_error) <= 4000),
    constraint sys_autonomous_daily_operations_no_trade_reason_bounded
        check (no_trade_reason is null or length(no_trade_reason) <= 128),
    constraint sys_autonomous_daily_operations_outcome_bounded
        check (outcome is null or length(outcome) <= 128),
    constraint sys_autonomous_daily_operations_state_reason_code_bounded
        check (state_reason_code is null or length(state_reason_code) <= 128),

    constraint sys_autonomous_daily_operations_state_known
        check (state in (
            'awaiting_preopen', 'preparing_data', 'awaiting_open',
            'preflight_blocked', 'start_retrying', 'running',
            'recovery_retrying', 'stopping', 'stop_retrying',
            'completed', 'completed_no_trade', 'completed_with_activity',
            'manual_intervention_required', 'controller_degraded',
            'calendar_unavailable', 'evidence_degraded'
        )),
    constraint sys_autonomous_daily_operations_schedule_source_known
        check (schedule_source in (
            'nyse_weekdays_heuristic', 'fixed_window_override'
        )),
    constraint sys_autonomous_daily_operations_calendar_coverage_known
        check (calendar_coverage_state in (
            'active', 'stale', 'invalid', 'out_of_range', 'unknown'
        )),

    constraint sys_autonomous_daily_operations_daily_slot_unique
        unique (market_date, deployment_mode, adapter_id)
);

-- Supports deterministic recent-operations listing
-- (market_date desc, created_at_utc desc, operation_id desc).
create index if not exists sys_autonomous_daily_operations_recent_idx
    on sys_autonomous_daily_operations (market_date desc, created_at_utc desc, operation_id desc);

create table if not exists sys_autonomous_daily_operation_events (
    operation_id       uuid        not null,
    transition_seq     bigint      not null,
    from_state         text        not null,
    to_state           text        not null,
    reason_code        text,
    occurred_at_utc    timestamptz not null,
    run_id             uuid,
    bounded_detail     text        not null,

    primary key (operation_id, transition_seq),

    constraint sys_autonomous_daily_operation_events_transition_seq_positive
        check (transition_seq >= 1),
    constraint sys_autonomous_daily_operation_events_reason_code_bounded
        check (reason_code is null or length(reason_code) <= 128),
    constraint sys_autonomous_daily_operation_events_bounded_detail_len
        check (length(bounded_detail) <= 4000),
    constraint sys_autonomous_daily_operation_events_to_state_known
        check (to_state in (
            'awaiting_preopen', 'preparing_data', 'awaiting_open',
            'preflight_blocked', 'start_retrying', 'running',
            'recovery_retrying', 'stopping', 'stop_retrying',
            'completed', 'completed_no_trade', 'completed_with_activity',
            'manual_intervention_required', 'controller_degraded',
            'calendar_unavailable', 'evidence_degraded'
        )),
    -- 'none' is legal only as the initial transition's from_state (enforced
    -- at the application layer: mqk_db::autonomous_daily_operation only ever
    -- writes from_state='none' for transition_seq=1).
    constraint sys_autonomous_daily_operation_events_from_state_known
        check (from_state = 'none' or from_state in (
            'awaiting_preopen', 'preparing_data', 'awaiting_open',
            'preflight_blocked', 'start_retrying', 'running',
            'recovery_retrying', 'stopping', 'stop_retrying',
            'completed', 'completed_no_trade', 'completed_with_activity',
            'manual_intervention_required', 'controller_degraded',
            'calendar_unavailable', 'evidence_degraded'
        ))
);

-- No FK from events to the current-state table (matches the
-- 0032/0043 precedent for observability/history tables in this codebase):
-- an operation row is never deleted, so referential integrity is
-- structurally preserved without a formal constraint, and this keeps the
-- events table's own insert path independent of the current-state table's
-- lock/update path.
