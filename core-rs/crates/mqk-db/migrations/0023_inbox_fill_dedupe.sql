-- DB-03: durable economic fill dedupe in oms_inbox.
--
-- Enforce run-scoped uniqueness for broker_fill_id when present so the same
-- economic fill cannot be durably recorded twice under different
-- broker_message_id values.
--
-- Fallback remains explicit: rows with broker_fill_id IS NULL are unaffected
-- and continue to dedupe by (run_id, broker_message_id) only.

drop index if exists idx_inbox_run_broker_fill_id;

create unique index if not exists uq_inbox_run_broker_fill_id
    on oms_inbox (run_id, broker_fill_id)
    where broker_fill_id is not null;
