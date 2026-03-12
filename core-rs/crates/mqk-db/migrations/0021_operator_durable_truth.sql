-- RSK-02R: Durable operator-visible truth for risk / reconcile state.
--
-- Existing durable sources:
-- - runs             -> run lifecycle status
-- - sys_arm_state    -> integrity arm/disarm + reason
--
-- Missing durable sources added here:
-- - sys_risk_block_state       -> current risk block posture + reason
-- - sys_reconcile_status_state -> current reconcile posture + watermark context

create table if not exists sys_risk_block_state (
    sentinel_id     smallint primary key check (sentinel_id = 1),
    blocked         boolean not null,
    reason          text,
    updated_at_utc  timestamptz not null
);

create table if not exists sys_reconcile_status_state (
    sentinel_id             smallint primary key check (sentinel_id = 1),
    status                  text not null check (status in ('unknown','ok','dirty','stale')),
    last_run_at_utc         timestamptz,
    snapshot_watermark_ms   bigint,
    mismatched_positions    integer not null default 0,
    mismatched_orders       integer not null default 0,
    mismatched_fills        integer not null default 0,
    unmatched_broker_events integer not null default 0,
    note                    text,
    updated_at_utc          timestamptz not null
);
