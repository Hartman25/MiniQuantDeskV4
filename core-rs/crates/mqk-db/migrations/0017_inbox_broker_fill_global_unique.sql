-- 0017_inbox_broker_fill_global_unique.sql
-- Enforce global uniqueness of broker_fill_id (not run-scoped).

BEGIN;

-- Drop any prior run-scoped uniqueness if it exists.
-- (Names may differ depending on how the earlier migration was written.)
ALTER TABLE oms_inbox
    DROP CONSTRAINT IF EXISTS oms_inbox_run_id_broker_fill_id_key;
ALTER TABLE oms_inbox
    DROP CONSTRAINT IF EXISTS oms_inbox_run_id_broker_fill_id_uniq;
DROP INDEX IF EXISTS oms_inbox_run_id_broker_fill_id_key;
DROP INDEX IF EXISTS oms_inbox_run_id_broker_fill_id_idx;

-- Now enforce global uniqueness.
ALTER TABLE oms_inbox
    ADD CONSTRAINT oms_inbox_broker_fill_id_uniq UNIQUE (broker_fill_id);

COMMIT;