-- FULL-AUDIT-FAIL-012 / INGEST-JOB-CANCEL-STATUS-CONSTRAINT-REPAIR-01:
-- sys_ingest_jobs_status_check was defined in migration 0041, before
-- IngestJobStatus::Cancelled and the ingest-cancel route existed. It rejects
-- 'cancelled', so DB-backed cancellation returns 503 instead of 202.
--
-- Replaces the constraint (same name, so it remains the single authoritative
-- status constraint on this table) with a vocabulary that matches
-- IngestJobStatus::as_str()/from_str() exactly, including 'cancelled'. All
-- previously-legal statuses are preserved unchanged; no other status is
-- added or removed.

ALTER TABLE sys_ingest_jobs
    DROP CONSTRAINT sys_ingest_jobs_status_check;

ALTER TABLE sys_ingest_jobs
    ADD CONSTRAINT sys_ingest_jobs_status_check CHECK (
        status IN (
            'queued',
            'running',
            'completed',
            'dry_run_completed',
            'failed',
            'refused',
            'partial',
            'cancelled'
        )
    );
