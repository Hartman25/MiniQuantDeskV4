-- DATA-INGEST-JOB-PERSISTENCE-01: durable daemon ingest job history.
--
-- Stores operator-visible ingest job status/progress so list/status survive a
-- daemon process restart when Postgres is configured.
--
-- No DEFAULT now() or DEFAULT gen_random_uuid() -- callers supply all fields.

CREATE TABLE sys_ingest_jobs (
    job_id                       uuid                     NOT NULL,
    source                       text                     NOT NULL,
    mode                         text,
    status                       text                     NOT NULL,
    timeframe                    text                     NOT NULL,
    csv_path                     text,
    registry_path                text,
    provider_registry_path       text,
    symbols_source               text,
    asset_class                  text                     NOT NULL,
    dry_run                      boolean                  NOT NULL,
    provider_api_calls_allowed   boolean                  NOT NULL,
    api_calls_made               bigint                   NOT NULL,
    symbols_count                bigint,
    symbols_completed            bigint,
    symbols_failed               bigint,
    planned_first_symbol         text,
    planned_last_symbol          text,
    rows_read                    bigint,
    rows_inserted                bigint,
    rows_rejected                bigint,
    quality_report_path          text,
    error                        text,
    provider_enabled             boolean,
    provider_verification_status text,
    created_at_utc               timestamp with time zone NOT NULL,
    started_at_utc               timestamp with time zone,
    completed_at_utc             timestamp with time zone,
    updated_at_utc               timestamp with time zone NOT NULL,
    CONSTRAINT sys_ingest_jobs_pkey PRIMARY KEY (job_id),
    CONSTRAINT sys_ingest_jobs_status_check CHECK (
        status IN (
            'queued',
            'running',
            'completed',
            'dry_run_completed',
            'failed',
            'refused',
            'partial'
        )
    ),
    CONSTRAINT sys_ingest_jobs_counts_nonnegative CHECK (
        api_calls_made >= 0
        AND (symbols_count IS NULL OR symbols_count >= 0)
        AND (symbols_completed IS NULL OR symbols_completed >= 0)
        AND (symbols_failed IS NULL OR symbols_failed >= 0)
        AND (rows_read IS NULL OR rows_read >= 0)
        AND (rows_inserted IS NULL OR rows_inserted >= 0)
        AND (rows_rejected IS NULL OR rows_rejected >= 0)
    )
);

CREATE INDEX idx_sys_ingest_jobs_updated_at
    ON sys_ingest_jobs (updated_at_utc DESC, created_at_utc DESC, job_id);
