-- Patch A4: Crash-safe cancel/replace — persisted broker_order_map
--
-- broker_order_map stores the internal_id → broker_id mapping that would
-- otherwise live only in the in-memory BrokerOrderMap (mqk-execution/id_map.rs).
-- On daemon restart, the startup path loads this table and repopulates
-- BrokerOrderMap so cancel/replace can target the correct broker order ID.
--
-- Usage contract:
--   1. After every successful broker submit: upsert (internal_id, broker_id).
--   2. At daemon startup: load all rows and repopulate in-memory BrokerOrderMap.
--   3. When an order reaches a terminal state (filled/cancel-ack/rejected): delete row.
--
-- internal_id: the order_id from BrokerSubmitRequest (caller-generated intent ID)
-- broker_id:   the broker_order_id from BrokerSubmitResponse (broker-assigned)

create table if not exists broker_order_map (
    internal_id           text        primary key,
    broker_id             text        not null,
    registered_at_utc     timestamptz not null default now()
);

comment on table broker_order_map is
    'Persisted internal_id → broker_id mapping for crash-safe cancel/replace (Patch A4). '
    'Populated after every successful broker submit; rows deleted on terminal order state.';

comment on column broker_order_map.internal_id is
    'The order_id from BrokerSubmitRequest — internal, caller-generated intent ID.';

comment on column broker_order_map.broker_id is
    'The broker_order_id from BrokerSubmitResponse — assigned by the broker.';
