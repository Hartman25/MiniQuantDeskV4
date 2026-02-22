-- Patch L3: Outbox claim/lock semantics
--
-- Adds CLAIMED status tracking columns to oms_outbox.
-- A dispatcher MUST claim a PENDING row (PENDING → CLAIMED) before broker submit.
-- This prevents two concurrent dispatchers from both processing the same row.
--
-- State machine:
--   PENDING → CLAIMED → SENT → ACKED
--                    └→ FAILED
--
-- FOR UPDATE SKIP LOCKED in outbox_claim_batch ensures at most one dispatcher
-- holds the lock for a given row at a time.

alter table oms_outbox
  add column if not exists claimed_at_utc timestamptz null,
  add column if not exists claimed_by      text       null;

comment on column oms_outbox.status is 'PENDING | CLAIMED | SENT | ACKED | FAILED';
