-- DET-02 follow-on: align durable inbox schema with enriched insert contract.
--
-- The write path persists these fields today:
--   - internal_order_id
--   - broker_order_id
--   - event_kind
--   - event_ts_ms
--
-- Older databases created before these columns existed fail with
-- "column internal_order_id does not exist".
--
-- This migration is additive and preserves existing rows by backfilling
-- sensible defaults from existing durable data.

alter table oms_inbox
    add column if not exists internal_order_id text,
    add column if not exists broker_order_id text,
    add column if not exists event_kind text,
    add column if not exists event_ts_ms bigint;

update oms_inbox
   set internal_order_id = coalesce(internal_order_id, broker_message_id),
       broker_order_id = coalesce(broker_order_id, internal_order_id, broker_message_id),
       event_kind = coalesce(event_kind, 'UNKNOWN'),
       event_ts_ms = coalesce(event_ts_ms, 0)
 where internal_order_id is null
    or broker_order_id is null
    or event_kind is null
    or event_ts_ms is null;

alter table oms_inbox
    alter column internal_order_id set not null,
    alter column broker_order_id set not null,
    alter column event_kind set not null,
    alter column event_ts_ms set not null;
