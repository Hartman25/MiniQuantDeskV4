-- EB-4: Broker Map FK → oms_outbox — enforce outbox-first at schema level.
--
-- Invariant enforced:
--   Every row in broker_order_map must correspond to an oms_outbox entry.
--   This guarantees that every tracked broker order was admitted through
--   the outbox (outbox-first dispatch, EB-3), making it structurally
--   impossible to create a broker mapping from outside the sanctioned
--   dispatch path.
--
-- ON DELETE RESTRICT:
--   broker_order_map rows must be removed (via broker_map_remove) before the
--   corresponding outbox row can be deleted.  This enforces correct cleanup
--   ordering: terminal-state map cleanup → outbox archive/purge.
--
-- Referenced column:
--   oms_outbox.idempotency_key carries UNIQUE index uq_outbox_idempotency
--   (created in 0001_init.sql), which satisfies the FK referencing requirement.
--
-- Deferred context (0010_idempotency_constraints.sql):
--   This FK was deferred in D3 because scenario_broker_order_map_survives_restart
--   called broker_map_upsert with bare IDs that had no outbox parent.  That test
--   has been updated in EB-4 to create the required run + outbox entries first.

alter table broker_order_map
    drop constraint if exists fk_broker_map_outbox_idempotency;

alter table broker_order_map
    add constraint fk_broker_map_outbox_idempotency
    foreign key (internal_id)
    references oms_outbox(idempotency_key)
    on delete restrict;
