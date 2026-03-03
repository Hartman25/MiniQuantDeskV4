-- RT-5: Add DISPATCHING status to close the crash window between broker submit
-- and outbox_mark_sent.
--
-- Problem (W4 crash window):
--   Current state machine: PENDING → CLAIMED → SENT → ACKED.
--   After claiming a row, the dispatcher calls gateway.submit() while the row
--   is still CLAIMED.  If a crash occurs between submit() and outbox_mark_sent(),
--   outbox_reset_stale_claims (which only resets CLAIMED rows) silently requeues
--   the order to PENDING — the next dispatcher double-submits to the broker.
--
-- Fix:
--   Insert DISPATCHING between CLAIMED and SENT.  The dispatcher writes
--   DISPATCHING to the DB BEFORE calling gateway.submit().
--   outbox_reset_stale_claims only touches CLAIMED rows; DISPATCHING rows
--   survive restart without being requeued.
--   A DISPATCHING row on startup signals: "submit was attempted — do not
--   requeue without operator review."
--
-- New state machine:
--   PENDING → CLAIMED → DISPATCHING → SENT → ACKED
--                    └→ FAILED (pre-dispatch error)
--                                  └→ FAILED (post-dispatch broker reject)
--
-- Application changes (mqk-db/src/lib.rs):
--   New function outbox_mark_dispatching:
--     CLAIMED → DISPATCHING (call immediately before gateway.submit()).
--   outbox_mark_sent:
--     WHERE status = 'CLAIMED'  →  WHERE status IN ('CLAIMED', 'DISPATCHING')
--     (backward-compat: tests that skip DISPATCHING still work; production
--      path goes DISPATCHING → SENT).
--   outbox_mark_failed:
--     Same broadening — accept both CLAIMED and DISPATCHING.
--   outbox_list_unacked_for_run:
--     Add 'DISPATCHING' to the status IN list.
--
-- Application changes (mqk-runtime/src/orchestrator.rs):
--   Phase 1: call outbox_mark_dispatching before gateway.submit();
--   on submit failure: call outbox_mark_failed (not outbox_release_claim).

alter table oms_outbox
    add column if not exists dispatching_at_utc  timestamptz null,
    add column if not exists dispatch_attempt_id text        null;

comment on column oms_outbox.dispatching_at_utc  is 'Timestamp when dispatch was initiated (RT-5). Set before gateway.submit().';
comment on column oms_outbox.dispatch_attempt_id is 'Dispatcher identity written at DISPATCHING time (RT-5). Informational for crash recovery audit.';
comment on column oms_outbox.status              is 'PENDING | CLAIMED | DISPATCHING | SENT | ACKED | FAILED';
