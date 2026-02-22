-- PATCH B: Ingest quality reports (Data Quality Gate v1)
--
-- NOTE: Patch A/A.1 already creates md_bars and other backtest tables.
-- This migration must ONLY add md_quality_reports.

create table if not exists md_quality_reports (
  ingest_id   uuid primary key,
  created_at  timestamptz not null default now(),
  stats_json  jsonb not null
);

create index if not exists idx_md_quality_reports_created_at
  on md_quality_reports(created_at);