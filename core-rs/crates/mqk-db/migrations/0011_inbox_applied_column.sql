-- Patch D2: Journal inbox "apply" status for crash-safe fill recovery.
--
-- Problem (from CLAUDE_PATCH_RUNBOOK_AUDIT.md):
--   inbox_insert_deduped() inserts a broker fill and returns true on first
--   insert.  The caller then applies the fill to the in-process portfolio.
--   If the process crashes after the DB insert but before the portfolio
--   mutation completes, the fill is permanently lost:
--     - On restart, inbox_insert_deduped returns false (already in DB, deduped)
--       → apply gate closed → fill never applied.
--
-- Fix: add `applied_at_utc timestamptz null` to oms_inbox.
--   The caller stamps this column via inbox_mark_applied() immediately after a
--   successful portfolio apply.  A new recovery helper,
--   inbox_load_unapplied_for_run(), returns all rows with
--   applied_at_utc IS NULL — fills that were received (DB-inserted) but whose
--   apply step did not complete before a crash.  The recovery path replays
--   these fills in inbox_id order.  The apply function itself must be idempotent
--   so that re-applying a partially-applied fill is safe.
--
-- Caller contract after D2:
--   Normal path:
--     1. inserted = inbox_insert_deduped(pool, run_id, msg_id, json)
--     2. if inserted { apply_fill(json); inbox_mark_applied(pool, msg_id) }
--   Recovery path (at startup):
--     3. rows = inbox_load_unapplied_for_run(pool, run_id)
--     4. for each row: apply_fill(row.message_json)
--                      inbox_mark_applied(pool, row.broker_message_id)
--
-- Note: existing rows will have applied_at_utc = NULL after this migration.
--   For a fresh deployment this is safe (no prior fills to replay).
--   For a system with existing inbox data, back-fill before enabling the
--   recovery path:
--     UPDATE oms_inbox SET applied_at_utc = received_at_utc
--       WHERE applied_at_utc IS NULL;
--   Only do this if all historical fills are known to have been applied.

alter table oms_inbox
    add column if not exists applied_at_utc timestamptz null;

comment on column oms_inbox.applied_at_utc is
    'Stamped by inbox_mark_applied() after the fill has been applied to the '
    'in-process portfolio ledger.  NULL means the fill was received (deduped '
    'in DB) but the apply step did not complete — these rows are returned by '
    'inbox_load_unapplied_for_run() for crash-recovery replay (Patch D2).';

-- Partial index: only scans NULL rows, keeping recovery queries fast
-- even on large inbox tables.
create index if not exists idx_inbox_run_unapplied
    on oms_inbox (run_id, inbox_id)
    where applied_at_utc is null;
