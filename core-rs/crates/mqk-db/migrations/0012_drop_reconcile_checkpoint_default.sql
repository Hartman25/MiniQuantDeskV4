-- D1-4: Drop DEFAULT now() from sys_reconcile_checkpoint.created_at_utc.
--
-- sys_reconcile_checkpoint.created_at_utc is a SEMANTICS-BEARING column:
-- reconcile_checkpoint_load_latest() orders by it (DESC) to select the latest
-- checkpoint passed to arm_preflight.  Supplying the timestamp via DB DEFAULT
-- now() is non-deterministic and non-injectable.  After this migration,
-- reconcile_checkpoint_write() must bind created_at_utc explicitly (the
-- D1-3 pattern: caller injects now: DateTime<Utc>).
--
-- BOOKKEEPING BASELINE (not changed here):
-- The following columns retain DEFAULT now() as acceptable ops metadata.
-- They are NOT in any enforcement or capital-decision path:
--   oms_outbox.created_at_utc         (0001)
--   oms_inbox.received_at_utc         (0001)
--   runs.started_at_utc               (0001, app-bound at insert_run)
--   md_bars.ingested_at               (0003)
--   run_events.ts                     (0003)
--   corporate_events.ingested_at      (0003)
--   md_quality_reports.created_at     (0004)
--   sys_arm_state.updated_at_utc      (0006, also app-managed via SQL now())
--   broker_order_map.registered_at_utc (0007)
--
-- Guard note: check_unsafe_patterns.{sh,ps1} [N] block checks only migrations
-- numbered >= 0012_; these baseline migrations are the D1-4 legacy whitelist.

ALTER TABLE sys_reconcile_checkpoint
    ALTER COLUMN created_at_utc DROP DEFAULT;
