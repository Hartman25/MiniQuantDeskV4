-- CC-02: Durable strategy suppression persistence.
--
-- Suppression rows record operator- or system-initiated strategy-level
-- suppression events.  Each suppression is keyed by a caller-provided UUID
-- (suppression_id) and tracks strategy identity, suppression reason, lifecycle
-- state, and timestamps.
--
-- `state` is constrained to 'active' | 'cleared'.
-- `started_at_utc` is provided by the daemon caller (no DEFAULT now() per
-- determinism rules; TimeSource-injected at the call site).
-- `cleared_at_utc` is null until the suppression is cleared.
-- `note` is optional free-text; defaults to empty string.

create table if not exists sys_strategy_suppressions (
    suppression_id   uuid        primary key,
    strategy_id      text        not null,
    state            text        not null check (state in ('active', 'cleared')),
    trigger_domain   text        not null,
    trigger_reason   text        not null,
    started_at_utc   timestamptz not null,
    cleared_at_utc   timestamptz,
    note             text        not null default ''
);

-- Supports per-strategy queries (which suppressions affect this strategy?).
create index if not exists sys_strategy_suppressions_strategy_id_idx
    on sys_strategy_suppressions (strategy_id);

-- Supports the primary route query: ordered by started_at, optionally filtered by state.
create index if not exists sys_strategy_suppressions_state_started_idx
    on sys_strategy_suppressions (state, started_at_utc desc);
