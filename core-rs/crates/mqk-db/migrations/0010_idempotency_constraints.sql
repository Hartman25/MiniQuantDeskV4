-- Patch D3: Enforce idempotency keys at schema level — broker_order_map.broker_id UNIQUE.
--
-- Problem (from CLAUDE_PATCH_RUNBOOK_AUDIT.md):
--   broker_order_map.broker_id has no uniqueness constraint.  Two separate internal
--   orders could be mapped to the same broker-assigned order ID.  Any routing logic
--   that looks up by broker_id (fills, cancels, replaces) would then fan-out to both
--   entries, silently corrupting position and P&L accounting.
--
-- Fix: add UNIQUE on broker_order_map.broker_id so the DB rejects a second mapping
--   to the same broker-assigned ID at the storage layer, independent of application
--   validation.
--
-- FK deferred — TODO:
--   A FK from broker_order_map.internal_id → oms_outbox(idempotency_key) would
--   guarantee that every mapped order was first admitted through the outbox.
--   This is deferred because the existing A4 integration test
--   (scenario_broker_order_map_survives_restart) calls broker_map_upsert() with
--   bare string IDs that have no corresponding outbox entries.  Adding the FK now
--   would require updating the A4 test to create outbox entries first, which is
--   outside D3 scope.  Add the FK when A4 is refactored to a full outbox-first flow.
--
-- Existing constraints already present (no action required here):
--   oms_outbox.idempotency_key   — uq_outbox_idempotency        (UNIQUE index)
--   oms_inbox.broker_message_id  — uq_inbox_broker_message_id   (UNIQUE index)
--   broker_order_map.internal_id — PRIMARY KEY                  (UNIQUE)

-- ---------------------------------------------------------------------------
-- 1. broker_order_map.broker_id
-- ---------------------------------------------------------------------------

alter table broker_order_map
    drop constraint if exists uq_broker_order_map_broker_id;

alter table broker_order_map
    add constraint uq_broker_order_map_broker_id
    unique (broker_id);
