-- DATA-PROVIDER-MD-SOURCE-METADATA-01:
-- Add provider/source provenance metadata to canonical market-data bars.
-- Existing rows backfill safely through provider_id default 'unknown'.

alter table md_bars
  add column if not exists provider_id text not null default 'unknown',
  add column if not exists provider_source text,
  add column if not exists provider_symbol text,
  add column if not exists ingest_mode text,
  add column if not exists provider_bar_id text,
  add column if not exists provider_updated_at_utc timestamptz;
