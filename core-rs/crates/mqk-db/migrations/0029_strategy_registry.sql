-- CC-01A: Authoritative strategy registry.
--
-- sys_strategy_registry is the canonical source of truth for known strategies.
-- Each row represents one registered strategy.  strategy_id is the natural
-- primary key — it is the stable external identity used across suppressions,
-- fleet configuration, and signal routing.
--
-- `enabled` controls whether the strategy is operationally active.
-- `kind` is an operator-assigned category string; unconstrained for forward
-- compatibility with later fleet-activation and plugin patches.
-- Timestamps are caller-injected (no DEFAULT now() per determinism rules).
-- `registered_at_utc` is set on first insert and never overwritten on upsert.
-- `updated_at_utc` is updated on every upsert.
-- `note` is optional free-text; defaults to empty string.

create table if not exists sys_strategy_registry (
    strategy_id        text        primary key,
    display_name       text        not null,
    enabled            boolean     not null,
    kind               text        not null default '',
    registered_at_utc  timestamptz not null,
    updated_at_utc     timestamptz not null,
    note               text        not null default ''
);

-- Supports fleet-activation queries (select enabled strategies).
create index if not exists sys_strategy_registry_enabled_idx
    on sys_strategy_registry (enabled);
