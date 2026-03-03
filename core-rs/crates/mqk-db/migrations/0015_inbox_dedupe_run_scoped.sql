-- RT-3: Scope oms_inbox dedupe by (run_id, broker_message_id).
--
-- Problem:
--   uq_inbox_broker_message_id is a UNIQUE INDEX on oms_inbox(broker_message_id).
--   This scopes deduplication globally: the same broker_message_id cannot appear
--   in more than one run.  In practice, broker message IDs are only guaranteed
--   unique within a single broker session (connection lifetime).  After a crash
--   and restart, a broker may reuse the same message ID in the new session.
--   The global uniqueness constraint would then silently discard the new run's
--   insert via ON CONFLICT DO NOTHING, preventing the fill from being applied.
--
-- Fix:
--   Drop the global unique index and replace it with a composite unique index
--   on (run_id, broker_message_id).  The same broker_message_id can now appear
--   in multiple runs, but cannot appear twice within the same run.
--
-- Application changes (mqk-db/src/lib.rs):
--   inbox_insert_deduped:
--     ON CONFLICT (broker_message_id)        → ON CONFLICT (run_id, broker_message_id)
--   inbox_mark_applied:
--     WHERE broker_message_id = $1           → WHERE run_id = $1 AND broker_message_id = $2
--     (run_id parameter added at position 1)

drop index if exists uq_inbox_broker_message_id;

create unique index if not exists uq_inbox_run_message
    on oms_inbox (run_id, broker_message_id);
