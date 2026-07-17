-- AUTONOMOUS-DAILY-PAPER-OPERATIONS-01B-BOUNDARY-MODEL-REPAIR-01: separate
-- exchange-schedule truth from effective-autonomous-operation-window truth
-- on sys_autonomous_daily_operations. Additive only; does not modify
-- migration 0048_autonomous_daily_operations.sql.
--
-- The pre-existing session_open_utc/session_close_utc columns remain
-- physically present and are, going forward, explicitly defined as the
-- *effective operation* boundaries (mapped in Rust as
-- effective_operation_open_utc/effective_operation_close_utc) -- they are
-- not renamed, and they are never conflated with exchange truth.
--
-- New exchange_* columns carry the authoritative exchange-calendar facts a
-- completed-bar driver (Phase C) will need alongside the effective operation
-- window: the exchange session open/close, whether that exchange session
-- was an early close, and the previous trading date. All four are nullable
-- for legacy-row compatibility only (no backfill is performed or possible
-- without fabricating calendar truth for any pre-existing row) -- every new
-- row created through the current mqk_db::autonomous_daily_operation store
-- API supplies all four as non-null values.

alter table sys_autonomous_daily_operations
    add column if not exists exchange_session_open_utc  timestamptz,
    add column if not exists exchange_session_close_utc timestamptz,
    add column if not exists exchange_is_early_close     boolean,
    add column if not exists previous_trading_date       date;

comment on column sys_autonomous_daily_operations.session_open_utc is
    'Effective autonomous-operation open boundary (mapped in Rust as effective_operation_open_utc). Equals exchange_session_open_utc when no fixed-window override is configured; otherwise the validated override boundary. Not authoritative exchange-session truth -- see exchange_session_open_utc.';

comment on column sys_autonomous_daily_operations.session_close_utc is
    'Effective autonomous-operation close boundary (mapped in Rust as effective_operation_close_utc). Equals exchange_session_close_utc when no fixed-window override is configured; otherwise the validated override boundary. Not authoritative exchange-session truth -- see exchange_session_close_utc.';

comment on column sys_autonomous_daily_operations.exchange_session_open_utc is
    'Authoritative exchange-calendar session open (never affected by a fixed-window override). Null only for a legacy row created before this migration.';

comment on column sys_autonomous_daily_operations.exchange_session_close_utc is
    'Authoritative exchange-calendar session close, including early-close truth (never affected by a fixed-window override). Null only for a legacy row created before this migration.';

comment on column sys_autonomous_daily_operations.exchange_is_early_close is
    'Authoritative exchange-calendar early-close truth for market_date. Null only for a legacy row created before this migration.';

comment on column sys_autonomous_daily_operations.previous_trading_date is
    'Authoritative exchange-calendar previous trading date, per MarketSessionSchedule::previous_trading_date. Null only for a legacy row created before this migration.';

alter table sys_autonomous_daily_operations
    add constraint sys_autonomous_daily_operations_exchange_fields_together
        check (
            (exchange_session_open_utc is null) = (exchange_session_close_utc is null)
            and (exchange_session_open_utc is null) = (exchange_is_early_close is null)
            and (exchange_session_open_utc is null) = (previous_trading_date is null)
        ),
    add constraint sys_autonomous_daily_operations_exchange_open_before_close
        check (
            exchange_session_open_utc is null
            or exchange_session_close_utc is null
            or exchange_session_open_utc < exchange_session_close_utc
        ),
    add constraint sys_autonomous_daily_operations_previous_trading_before_market_date
        check (
            previous_trading_date is null
            or previous_trading_date < market_date
        );
