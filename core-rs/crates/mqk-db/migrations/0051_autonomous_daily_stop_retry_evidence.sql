-- AUTONOMOUS-DAILY-PAPER-OPERATIONS-01D2-DURABLE-SESSION-COORDINATOR: add
-- durable stop-attempt evidence to sys_autonomous_daily_operations. Additive
-- only; does not modify migrations 0048, 0049, or 0050.
--
-- The existing schema has durable start-attempt evidence
-- (start_attempt_count, last_start_attempt_utc) but no equivalent for the
-- stop path. D2's canonical stop sequence (session close -> stopping ->
-- stop_execution_runtime -> stopped_at_utc) needs the same durable,
-- restart-safe attempt counter so a stop failure can be retried with bounded
-- backoff and an unresolved stop failure can be distinguished from a stop
-- that was never attempted.
--
-- Both new columns are nullable for legacy-row compatibility only: a row
-- created before this migration has unknown stop-attempt history (never
-- fabricated as zero). Every row created through the current
-- mqk_db::autonomous_daily_operation::create_or_recover_autonomous_daily_operation
-- API supplies an explicit stop_attempt_count of 0 at insert time -- no SQL
-- DEFAULT fabricates it.

alter table sys_autonomous_daily_operations
    add column if not exists stop_attempt_count   bigint,
    add column if not exists last_stop_attempt_utc timestamptz;

comment on column sys_autonomous_daily_operations.stop_attempt_count is
    'Count of canonical stop_execution_runtime call attempts for this operation. Null only for a legacy row created before this migration -- never fabricated as zero. Every row created after this migration supplies an explicit 0 at insert time.';

comment on column sys_autonomous_daily_operations.last_stop_attempt_utc is
    'Timestamp of the most recent canonical stop_execution_runtime call attempt for this operation. Null when no stop attempt has ever been recorded.';

alter table sys_autonomous_daily_operations
    add constraint sys_autonomous_daily_operations_stop_attempt_count_nonneg
        check (stop_attempt_count is null or stop_attempt_count >= 0);
