-- AUTON-PAPER-02: Durable autonomous-session supervisor history.
--
-- Stores restart-safe autonomous supervisor events for the canonical
-- Paper+Alpaca path. This is history/evidence, not the current active-alert
-- surface. Current operator attention remains sourced from daemon state.
--
-- The table is intentionally small and append-only. It records autonomous
-- supervisor lifecycle truth that must survive daemon restart/crash so the
-- operator can audit a full-day soak after the fact.

create table if not exists sys_autonomous_session_events (
    id              text        primary key,
    ts_utc          timestamptz not null,
    event_type      text        not null,
    resume_source   text,
    detail          text        not null,
    run_id          uuid,
    source          text        not null
);

create index if not exists sys_autonomous_session_events_ts_idx
    on sys_autonomous_session_events (ts_utc desc);
