-- RD-01: Durable risk denial event history.
--
-- Individual risk gate denial records persisted for operator visibility
-- across daemon restarts.
--
-- Previously denials were only held in the in-memory ring buffer inside
-- ExecutionOrchestrator (VecDeque, cap 100) and were lost on restart.
-- This table stores the authoritative denial history. The ring buffer
-- remains as an in-process supplement but no longer defines truth.
--
-- `id` is the deterministic display ID: "{denied_at_utc_micros}:{rule_code}".
-- Unique for all practical purposes. ON CONFLICT DO NOTHING gives idempotent
-- inserts so best-effort writes from the orchestrator cannot double-count.

create table if not exists sys_risk_denial_events (
    id              text        primary key,
    denied_at_utc   timestamptz not null,
    rule            text        not null,
    message         text        not null,
    symbol          text,
    requested_qty   bigint,
    limit_qty       bigint,
    severity        text        not null
);

-- Supports the most-recent-N query pattern used by the /api/v1/risk/denials
-- route (ORDER BY denied_at_utc DESC LIMIT N).
create index if not exists sys_risk_denial_events_denied_at_idx
    on sys_risk_denial_events (denied_at_utc desc);
