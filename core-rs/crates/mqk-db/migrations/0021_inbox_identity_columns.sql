-- FND-01R: make broker transport identity explicit and add optional economic identity.
--
-- `broker_message_id` remains the canonical transport/event identity key.
-- `broker_fill_id` is optional and captures economic fill identity when the
-- broker provides one.
-- `broker_sequence_id` and `broker_timestamp` are optional transport lineage
-- hints for adapters that can provide them.

alter table oms_inbox
    add column if not exists broker_fill_id text null,
    add column if not exists broker_sequence_id text null,
    add column if not exists broker_timestamp text null;

create index if not exists idx_inbox_run_broker_fill_id
    on oms_inbox (run_id, broker_fill_id)
    where broker_fill_id is not null;
