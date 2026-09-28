-- 0083 (B6, V4-M5-M8-APPROVED-DECISIONS-IMPLEMENTATION-01-CONTINUATION):
-- durable, restart-safe, deduplicated Crypto fee-activity ingestion.
--
-- `fee_attribution::normalize_fee_activity` (mqk-broker-alpaca) and
-- `AlpacaBrokerAdapter::fetch_fee_activities_since` already existed with
-- zero production callers or durable evidence -- this migration adds the
-- two tables an ingestion pipeline needs to be deterministic, idempotent,
-- and restart-safe without either table existing before:
--
-- sys_crypto_fee_activity_ledger: one immutable row per Alpaca CFEE/FEE
-- account activity ever ingested. `activity_id` is Alpaca's own activity
-- id (the broker's natural idempotency key -- fee_attribution.rs's own doc:
-- "an activity must never be applied to the ledger twice") and is the
-- PRIMARY KEY, so a duplicate fetch (restart, overlapping poll, retried
-- request) can insert with ON CONFLICT (activity_id) DO NOTHING and the
-- caller can distinguish "newly ingested" from "already had this one" --
-- no duplicate charge is possible at the DB level, not merely by caller
-- discipline. `attribution_status` mirrors
-- fee_attribution::FeeAttributionRecord's three shapes exactly; exactly one
-- of fee_micros/qty_raw is populated depending on which shape this row is,
-- enforced by the CHECK below. This is fee-activity evidence only -- never
-- a fill, never written to any fills/oms_inbox table (fee activity is not
-- fabricated as a market fill).
--
-- sys_crypto_fee_ingestion_cursor: one row per (engine_id, mode,
-- activity_type) recording the last successfully ingested activity_id --
-- the restart-safe watermark `fetch_fee_activities_since(after_id)` resumes
-- from. Advanced only after the corresponding ledger rows are durably
-- committed (same transaction), so a crash between fetch and commit leaves
-- the cursor at its prior value and the next attempt safely re-fetches and
-- re-dedupes via the ledger's PRIMARY KEY rather than skipping or
-- double-applying anything.
--
-- No DEFAULT now() or DEFAULT gen_random_uuid() -- callers supply every
-- timestamp and identity, per this repo's DB rules.

CREATE TABLE IF NOT EXISTS sys_crypto_fee_activity_ledger (
    activity_id                 text                     NOT NULL,
    engine_id                   text                     NOT NULL,
    mode                        text                     NOT NULL,
    activity_type                text                     NOT NULL,
    symbol                      text                     NULL,
    attribution_status          text                     NOT NULL,
    fee_micros                  bigint                   NULL,
    qty_raw                     text                     NULL,
    ingested_at_utc             timestamp with time zone NOT NULL,
    CONSTRAINT sys_crypto_fee_activity_ledger_pkey PRIMARY KEY (activity_id),
    CONSTRAINT sys_crypto_fee_activity_ledger_activity_type_check
        CHECK (activity_type in ('CFEE', 'FEE')),
    CONSTRAINT sys_crypto_fee_activity_ledger_attribution_status_check
        CHECK (attribution_status in (
            'cash_fee', 'asset_denominated_fee_unsupported', 'confirmed_zero_fee'
        )),
    -- Exactly one payload field populated per shape: cash_fee carries
    -- fee_micros only; asset_denominated_fee_unsupported carries qty_raw
    -- only; confirmed_zero_fee carries neither.
    CONSTRAINT sys_crypto_fee_activity_ledger_payload_shape_check
        CHECK (
            (attribution_status = 'cash_fee'
                AND fee_micros IS NOT NULL AND qty_raw IS NULL)
            OR (attribution_status = 'asset_denominated_fee_unsupported'
                AND fee_micros IS NULL AND qty_raw IS NOT NULL)
            OR (attribution_status = 'confirmed_zero_fee'
                AND fee_micros IS NULL AND qty_raw IS NULL)
        )
);

CREATE INDEX IF NOT EXISTS idx_crypto_fee_activity_ledger_engine_mode
    ON sys_crypto_fee_activity_ledger (engine_id, mode, activity_type, ingested_at_utc);

CREATE TABLE IF NOT EXISTS sys_crypto_fee_ingestion_cursor (
    engine_id                   text                     NOT NULL,
    mode                        text                     NOT NULL,
    activity_type                text                     NOT NULL,
    last_activity_id             text                     NOT NULL,
    updated_at_utc               timestamp with time zone NOT NULL,
    CONSTRAINT sys_crypto_fee_ingestion_cursor_pkey
        PRIMARY KEY (engine_id, mode, activity_type),
    CONSTRAINT sys_crypto_fee_ingestion_cursor_activity_type_check
        CHECK (activity_type in ('CFEE', 'FEE'))
);
